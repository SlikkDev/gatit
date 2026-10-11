//! One background worker that transcribes and summarizes meetings, one at a time, so a
//! long transcription never competes with itself for the CPU.

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::library::{format_duration, Status};
use crate::state::{library_changed, state, Job, JobProgress};
use crate::{drive, settings, summary, transcribe, transcript};

pub fn report(app: &AppHandle, id: &str, label: &str, percent: f64) {
    let progress = JobProgress {
        id: id.into(),
        label: label.into(),
        percent: percent.clamp(0.0, 100.0),
    };
    *state(app).progress.lock().unwrap() = Some(progress.clone());
    let _ = app.emit("job", Some(progress));
}

fn clear_report(app: &AppHandle) {
    *state(app).progress.lock().unwrap() = None;
    let _ = app.emit("job", None::<JobProgress>);
}

pub async fn run(app: AppHandle, mut jobs: UnboundedReceiver<Job>) {
    while let Some(job) = jobs.recv().await {
        let id = job.meeting_id().to_string();
        if matches!(job, Job::Process(_)) {
            wait_for_power(&app, &id).await;
        }
        state(&app).dequeued(&job);
        let result = match &job {
            Job::Process(id) => process(&app, id).await,
            Job::Summarize(id) => summarize(&app, id).await,
        };
        if let Err(message) = result {
            let _ = state(&app).library().update(&id, |m| {
                m.status = Status::Error;
                m.error = Some(message);
            });
        }
        clear_report(&app);
        library_changed(&app, &id);
        // The transcript or summary changed, so the Drive copy is behind.
        drive::enqueue(&app, &id);
    }
}

/// With "transcribe only when plugged in", hold the job until the Mac is on AC power.
async fn wait_for_power(app: &AppHandle, id: &str) {
    while state(app).settings().transcribe_when == "on_power" && !on_ac_power() {
        report(app, id, "Waiting until the Mac is plugged in", 0.0);
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

fn on_ac_power() -> bool {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("/usr/bin/pmset")
        .args(["-g", "batt"])
        .output()
    {
        return String::from_utf8_lossy(&out.stdout).contains("'AC Power'");
    }
    true
}

async fn process(app: &AppHandle, id: &str) -> Result<(), String> {
    let st = state(app);
    let settings = st.settings();
    let (folder, language) = {
        let lib = st.library();
        (lib.folder(id)?, lib.get(id)?.language)
    };
    st.library().update(id, |m| {
        m.status = Status::Transcribing;
        m.error = None;
    })?;
    library_changed(app, id);

    let work_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("work")
        .join(id);
    let progress = |label: &str, pct: f64| report(app, id, label, pct);
    let transcript =
        transcribe::transcribe_meeting(app, &folder, &work_dir, &language, &settings, &progress)
            .await?;
    st.library().save_transcript(id, &transcript)?;
    st.library()
        .update(id, |m| m.status = Status::Transcribed)?;
    library_changed(app, id);

    if settings.auto_summarize && settings::api_key().is_some() {
        summarize(app, id).await?;
    }
    Ok(())
}

async fn summarize(app: &AppHandle, id: &str) -> Result<(), String> {
    let st = state(app);
    let key = settings::api_key().ok_or("Add your Claude API key in Settings to get summaries.")?;
    let settings = st.settings();
    let (meeting, transcript) = {
        let lib = st.library();
        (
            lib.get(id)?,
            lib.transcript(id).ok_or("Transcribe this meeting first.")?,
        )
    };
    if transcript.segments.is_empty() {
        return Err("The transcript is empty, so there is nothing to summarize.".into());
    }
    st.library().update(id, |m| {
        m.status = Status::Summarizing;
        m.error = None;
    })?;
    library_changed(app, id);
    report(app, id, "Writing the summary", 50.0);

    let context = format!(
        "Call recorded {}, {} long{}.",
        meeting.started_at.format("%A %-d %B %Y at %H:%M"),
        format_duration(meeting.duration_secs),
        if meeting.source_app.is_empty() {
            String::new()
        } else {
            format!(", on {}", meeting.source_app)
        }
    );
    let prompt = summary_prompt(app);
    let summary = summary::summarize(
        &key,
        &settings.claude_model,
        &prompt,
        &context,
        &transcript::to_text(&transcript.segments),
    )
    .await?;
    let mut lib = st.library();
    lib.save_summary(id, &summary)?;
    lib.update(id, |m| {
        if !m.title_edited && !summary.title.trim().is_empty() {
            m.title = summary.title.trim().to_string();
        }
        m.status = Status::Done;
    })?;
    lib.write_markdown(id)
}

/// The summary instructions: the user's editable copy if there is one, else the default.
pub fn summary_prompt(app: &AppHandle) -> String {
    prompt_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| summary::DEFAULT_PROMPT.to_string())
}

pub fn prompt_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("summary-prompt.md"))
}

/// Re-queues work that was interrupted when the app last quit.
pub fn resume_pending(app: &AppHandle) {
    let st = state(app);
    let settings = st.settings();
    let meetings = st.library().list().unwrap_or_default();
    for m in meetings {
        match m.status {
            Status::Recording => {
                // The app quit or crashed mid-recording. Keep whatever audio made it to disk.
                let _ = st.library().update(&m.id, |m| m.status = Status::Recorded);
                if settings.transcribe_when != "manual" {
                    st.enqueue(Job::Process(m.id));
                }
            }
            Status::Recorded if settings.transcribe_when != "manual" => {
                st.enqueue(Job::Process(m.id))
            }
            Status::Transcribing => st.enqueue(Job::Process(m.id)),
            Status::Summarizing => st.enqueue(Job::Summarize(m.id)),
            Status::Transcribed if settings.auto_summarize && settings::api_key().is_some() => {
                st.enqueue(Job::Summarize(m.id))
            }
            _ => {}
        }
    }
}

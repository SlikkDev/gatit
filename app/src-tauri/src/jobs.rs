//! One background worker that transcribes and summarizes meetings, one at a time, so a
//! long transcription never competes with itself for the CPU.

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::library::{format_duration, same_person, Meeting, Status};
use crate::state::{library_changed, state, Job, JobProgress};
use crate::transcript::Names;
use crate::{settings, summary, transcribe, transcript};

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
    }
}

/// With "transcribe only when plugged in", hold the job until the machine is on AC power.
async fn wait_for_power(app: &AppHandle, id: &str) {
    while state(app).settings().transcribe_when == "on_power" && !on_ac_power() {
        report(app, id, "Waiting until the computer is plugged in", 0.0);
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
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut status = SYSTEM_POWER_STATUS {
            ACLineStatus: 255,
            BatteryFlag: 0,
            BatteryLifePercent: 0,
            SystemStatusFlag: 0,
            BatteryLifeTime: 0,
            BatteryFullLifeTime: 0,
        };
        // SAFETY: the struct is a plain C record that the call fills in.
        if unsafe { GetSystemPowerStatus(&mut status) } != 0 {
            // 0 = on battery, 1 = plugged in, 255 = unknown (a desktop: treat as plugged in).
            return status.ACLineStatus != 0;
        }
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
    let (meeting, transcript, names) = {
        let lib = st.library();
        let meeting = lib.get(id)?;
        let names = lib.names(&meeting);
        (
            meeting,
            lib.transcript(id).ok_or("Transcribe this meeting first.")?,
            names,
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

    let context = summary_context(&meeting, &names);
    let prompt = summary_prompt(app);
    let summary = summary::summarize(
        &key,
        &settings.claude_model,
        &prompt,
        &context,
        &transcript::to_text(&transcript.segments, &names),
    )
    .await?;
    let mut lib = st.library();
    lib.save_summary(id, &summary)?;
    let your_name = settings.your_name.trim();
    lib.update(id, |m| {
        if !m.title_edited && !summary.title.trim().is_empty() {
            m.title = summary.title.trim().to_string();
        }
        // People Claude heard on the call join the list, unless the user set it by hand.
        if !m.participants_edited {
            m.add_participants(
                summary
                    .participants
                    .iter()
                    .map(String::as_str)
                    .filter(|p| your_name.is_empty() || !same_person(p, your_name)),
            );
        }
        m.status = Status::Done;
    })?;
    // The transcript labels may have changed with the people on the call.
    lib.refresh(id)
}

/// What Claude is told about the call before the transcript: when it was, how long, which
/// app, who recorded it and who else was on it, and how each of them is labelled.
pub fn summary_context(meeting: &Meeting, names: &Names) -> String {
    let mut context = format!(
        "Call recorded {}, {} long{}.",
        meeting.started_at.format("%A %-d %B %Y at %H:%M"),
        format_duration(meeting.duration_secs),
        if meeting.source_app.is_empty() {
            String::new()
        } else {
            format!(", on {}", meeting.source_app)
        }
    );
    if names.me == "Me" {
        context.push_str(" The person who recorded it is labelled \"Me\".");
    } else {
        context.push_str(&format!(
            " It was recorded by {}, labelled \"{}\".",
            names.me, names.me
        ));
    }
    let others: Vec<&str> = meeting
        .participants
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    match others.as_slice() {
        [] => context.push_str(" Everyone else on the call is labelled \"Them\"."),
        [only] => context.push_str(&format!(
            " The other person on the call is {only}, labelled \"{only}\"."
        )),
        _ => context.push_str(&format!(
            " The other people on the call are {}; they share the label \"Them\".",
            others.join(", ")
        )),
    }
    context
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn meeting(participants: &[&str]) -> Meeting {
        Meeting {
            id: "2026-10-11 14-03-22".into(),
            title: "Zoom at 14:03".into(),
            started_at: chrono::Local
                .with_ymd_and_hms(2026, 10, 11, 14, 3, 22)
                .unwrap(),
            duration_secs: 1500.0,
            source_app: "Zoom".into(),
            language: "he".into(),
            status: Status::Transcribed,
            error: None,
            title_edited: false,
            participants: participants.iter().map(|p| p.to_string()).collect(),
            participants_edited: false,
        }
    }

    #[test]
    fn context_tells_claude_who_is_who() {
        let m = meeting(&[]);
        let unknown = summary_context(&m, &Names::new("", &m.participants));
        assert!(unknown
            .starts_with("Call recorded Sunday 11 October 2026 at 14:03, 25 min long, on Zoom."));
        assert!(unknown.contains("labelled \"Me\""));
        assert!(unknown.contains("Everyone else on the call is labelled \"Them\""));

        let m = meeting(&["Dana Cohen"]);
        let one = summary_context(&m, &Names::new("Yarin", &m.participants));
        assert!(one.contains("recorded by Yarin, labelled \"Yarin\""));
        assert!(one.contains("The other person on the call is Dana Cohen, labelled \"Dana Cohen\""));

        let m = meeting(&["Dana", "Avi"]);
        let group = summary_context(&m, &Names::new("Yarin", &m.participants));
        assert!(group
            .contains("The other people on the call are Dana, Avi; they share the label \"Them\""));
    }
}

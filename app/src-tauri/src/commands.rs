//! Commands the window calls through `window.__TAURI__.core.invoke`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::library::{Meeting, Status};
use crate::settings::{self, Settings};
use crate::state::{library_changed, state, Job};
use crate::summary::Summary;
use crate::transcribe::{self, Model};
use crate::transcript::{Names, Transcript};
use crate::{jobs, recorder};

#[derive(Serialize)]
pub struct AppStatus {
    recording: bool,
    meeting_id: Option<String>,
    started_at_ms: Option<i64>,
    job: Option<crate::state::JobProgress>,
    can_record: Option<String>,
    macos: Option<String>,
    arch: &'static str,
    has_api_key: bool,
}

#[tauri::command]
pub fn app_status(app: AppHandle) -> AppStatus {
    let st = state(&app);
    let (recording, meeting_id, started_at_ms) = {
        let rec = st.recording();
        (
            rec.child.is_some(),
            rec.meeting_id.clone(),
            rec.started_at_ms,
        )
    };
    let job = st.progress.lock().unwrap().clone();
    AppStatus {
        recording,
        meeting_id,
        started_at_ms,
        job,
        can_record: recorder::recording_blocker(),
        macos: recorder::macos_version().map(|v| v.2),
        arch: std::env::consts::ARCH,
        has_api_key: settings::api_key().is_some(),
    }
}

#[tauri::command]
pub fn toggle_recording(app: AppHandle) -> Result<(), String> {
    if recorder::is_recording(&app) {
        recorder::stop(&app)
    } else {
        recorder::start(&app, None)
    }
}

#[tauri::command]
pub fn list_meetings(app: AppHandle) -> Result<Vec<Meeting>, String> {
    state(&app).library().list()
}

#[derive(Serialize)]
pub struct MeetingDetail {
    meeting: Meeting,
    /// What the two tracks are called in this meeting's transcript.
    names: Names,
    transcript: Option<Transcript>,
    summary: Option<Summary>,
    mic: Option<PathBuf>,
    system: Option<PathBuf>,
}

#[tauri::command]
pub fn get_meeting(app: AppHandle, id: String) -> Result<MeetingDetail, String> {
    let st = state(&app);
    let lib = st.library();
    let folder = lib.folder(&id)?;
    let existing = |name: &str| Some(folder.join(name)).filter(|p| p.exists());
    let meeting = lib.get(&id)?;
    Ok(MeetingDetail {
        names: lib.names(&meeting),
        meeting,
        transcript: lib.transcript(&id),
        summary: lib.summary(&id),
        mic: existing("mic.m4a"),
        system: existing("system.m4a"),
    })
}

#[derive(Deserialize)]
pub struct MeetingChanges {
    title: Option<String>,
    source_app: Option<String>,
    /// The other people on the call, as typed by the user.
    participants: Option<Vec<String>>,
}

#[tauri::command]
pub fn update_meeting(
    app: AppHandle,
    id: String,
    changes: MeetingChanges,
) -> Result<Meeting, String> {
    let st = state(&app);
    let mut lib = st.library();
    let meeting = lib.update(&id, |m| {
        if let Some(title) = changes
            .title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
        {
            m.title = title;
            m.title_edited = true;
        }
        if let Some(app_name) = changes.source_app {
            m.source_app = app_name;
        }
        if let Some(people) = changes.participants {
            m.participants.clear();
            m.add_participants(people.iter().map(String::as_str));
            m.participants_edited = true;
        }
    })?;
    let _ = lib.refresh(&id);
    drop(lib);
    library_changed(&app, &id);
    Ok(meeting)
}

/// Transcribes (or re-transcribes) a meeting, optionally in a different language.
#[tauri::command]
pub fn transcribe_meeting(
    app: AppHandle,
    id: String,
    language: Option<String>,
) -> Result<(), String> {
    let st = state(&app);
    st.library().update(&id, |m| {
        if let Some(lang) = language.filter(|l| ["he", "en", "auto"].contains(&l.as_str())) {
            m.language = lang;
        }
        m.status = Status::Recorded;
        m.error = None;
    })?;
    st.enqueue(Job::Process(id.clone()));
    library_changed(&app, &id);
    Ok(())
}

#[tauri::command]
pub fn summarize_meeting(app: AppHandle, id: String) -> Result<(), String> {
    if settings::api_key().is_none() {
        return Err("Add your Claude API key in Settings first.".into());
    }
    state(&app).enqueue(Job::Summarize(id));
    Ok(())
}

#[tauri::command]
pub fn delete_meeting(app: AppHandle, id: String) -> Result<(), String> {
    let st = state(&app);
    if st.recording().meeting_id.as_deref() == Some(id.as_str()) {
        return Err("Stop the recording before deleting it.".into());
    }
    st.library().delete(&id)?;
    library_changed(&app, &id);
    Ok(())
}

#[derive(Serialize)]
pub struct SearchHit {
    id: String,
    snippet: String,
}

#[tauri::command]
pub fn search(app: AppHandle, query: String) -> Result<Vec<SearchHit>, String> {
    Ok(state(&app)
        .library()
        .search(&query)?
        .into_iter()
        .map(|(id, snippet)| SearchHit { id, snippet })
        .collect())
}

fn open_path(path: &std::path::Path, reveal: bool) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        if reveal {
            c.arg(format!("/select,{}", path.display()));
        } else {
            c.arg(path);
        }
        c
    } else {
        let mut c = std::process::Command::new("open");
        if reveal {
            c.arg("-R");
        }
        c.arg(path);
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Shows the meeting's Markdown export in Finder.
#[tauri::command]
pub fn reveal_meeting(app: AppHandle, id: String) -> Result<(), String> {
    let st = state(&app);
    let lib = st.library();
    lib.write_markdown(&id)?;
    open_path(&lib.folder(&id)?.join("meeting.md"), true)
}

#[tauri::command]
pub fn open_library_folder(app: AppHandle) -> Result<(), String> {
    let root = state(&app).library().root().to_path_buf();
    open_path(&root, false)
}

#[tauri::command]
pub fn copy_text(text: String) -> Result<(), String> {
    use std::io::Write;
    let program = if cfg!(target_os = "windows") {
        "clip"
    } else {
        "pbcopy"
    };
    let mut child = std::process::Command::new(program)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    state(&app).settings()
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let st = state(&app);
    let old = st.settings();
    if settings.shortcut != old.shortcut {
        register_shortcut(&app, &settings.shortcut, Some(&old.shortcut))?;
    }
    settings::save(&st.settings_path, &settings)?;
    if settings.your_name.trim() != old.your_name.trim() {
        st.library().set_your_name(&settings.your_name)?;
    }
    *st.settings.lock().unwrap() = settings;
    Ok(())
}

/// Registers the start/stop shortcut, replacing the previous one. If the new one can't be
/// registered (for example another app owns it), the previous one is kept.
pub fn register_shortcut(
    app: &AppHandle,
    shortcut: &str,
    previous: Option<&str>,
) -> Result<(), String> {
    let gs = app.global_shortcut();
    if let Some(prev) = previous {
        let _ = gs.unregister(prev);
    }
    if shortcut.trim().is_empty() {
        return Ok(());
    }
    gs.register(shortcut).map_err(|e| {
        if let Some(prev) = previous {
            let _ = gs.register(prev);
        }
        format!("Couldn't use {shortcut} as the shortcut: {e}")
    })
}

#[tauri::command]
pub fn set_api_key(key: String) -> Result<(), String> {
    settings::set_api_key(&key)
}

#[tauri::command]
pub fn model_status(app: AppHandle) -> Value {
    let settings = state(&app).settings();
    let hebrew = Model::hebrew(&settings);
    json!([
        { "key": "hebrew", "label": hebrew.label(), "installed": transcribe::is_installed(&app, hebrew) },
        { "key": "english", "label": Model::English.label(), "installed": transcribe::is_installed(&app, Model::English) },
    ])
}

/// Downloads a model ahead of the first transcription, reporting progress as a job.
#[tauri::command]
pub async fn download_model(app: AppHandle, key: String) -> Result<(), String> {
    let model = match key.as_str() {
        "hebrew" => Model::hebrew(&state(&app).settings()),
        "english" => Model::English,
        _ => return Err("unknown model".into()),
    };
    let progress = |label: &str, pct: f64| jobs::report(&app, "", label, pct);
    let result = async {
        transcribe::ensure_model(&app, Model::Vad, &progress).await?;
        transcribe::ensure_model(&app, model, &progress).await
    }
    .await;
    *state(&app).progress.lock().unwrap() = None;
    let _ = app.emit("job", None::<crate::state::JobProgress>);
    result.map(|_| ())
}

/// Opens the summary instructions in a text editor, creating the editable copy first.
#[tauri::command]
pub fn open_summary_prompt(app: AppHandle) -> Result<(), String> {
    let path = jobs::prompt_path(&app)?;
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, crate::summary::DEFAULT_PROMPT).map_err(|e| e.to_string())?;
    }
    if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg("-t")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    } else {
        open_path(&path, false)
    }
}

/// The built-in Hebrew-English terms, grouped, for Settings to show.
#[tauri::command]
pub fn builtin_vocabulary() -> Vec<crate::vocabulary::BuiltinGroupInfo> {
    crate::vocabulary::builtin_groups()
}

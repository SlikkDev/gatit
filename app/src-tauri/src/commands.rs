//! Commands the window calls through `window.__TAURI__.core.invoke`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::drive::{self, SyncState};
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
    drive_job: Option<drive::Progress>,
    can_record: Option<String>,
    /// "macos" or "windows".
    platform: &'static str,
    /// What to show as the system, like "macOS 15.1" or "Windows".
    os: String,
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
    let drive_job = st.drive.progress.lock().unwrap().clone();
    AppStatus {
        recording,
        meeting_id,
        started_at_ms,
        job,
        drive_job,
        can_record: recorder::recording_blocker(),
        platform: std::env::consts::OS,
        os: match recorder::macos_version() {
            Some((_, _, version)) => format!("macOS {version}"),
            None if cfg!(target_os = "windows") => "Windows".into(),
            None => std::env::consts::OS.into(),
        },
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
    video: Option<PathBuf>,
    /// Seconds into the audio that the video starts.
    video_offset: f64,
    /// Google Drive upload state, once the meeting has been uploaded or tried.
    drive: Option<Value>,
}

#[tauri::command]
pub fn get_meeting(app: AppHandle, id: String) -> Result<MeetingDetail, String> {
    let st = state(&app);
    let lib = st.library();
    let folder = lib.folder(&id)?;
    let existing = |name: &str| Some(folder.join(name)).filter(|p| p.exists());
    let meeting = lib.get(&id)?;
    // video.mp4 is lined up with the audio; until it exists, screen.mp4 starts `offset` in.
    let (video, video_offset) = match existing("video.mp4") {
        Some(path) => (Some(path), 0.0),
        None => {
            let offset = std::fs::read(folder.join("screen.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| v["offset"].as_f64())
                .unwrap_or(0.0);
            (existing("screen.mp4"), offset)
        }
    };
    Ok(MeetingDetail {
        names: lib.names(&meeting),
        meeting,
        transcript: lib.transcript(&id),
        summary: lib.summary(&id),
        mic: transcribe::track_file(&folder, "mic"),
        system: transcribe::track_file(&folder, "system"),
        video,
        video_offset,
        drive: Some(SyncState::load(&folder))
            .filter(|s| !s.folder_id.is_empty() || s.error.is_some())
            .map(|s| s.summary()),
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
    drive::enqueue(&app, &id);
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
    let drive_folder = st
        .library()
        .folder(&id)
        .map(|f| SyncState::load(&f).folder_id)
        .unwrap_or_default();
    st.library().delete(&id)?;
    library_changed(&app, &id);
    // The Drive copy follows the local one into the (Drive) trash.
    drive::trash_folder(&app, drive_folder);
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
    recorder::quiet(&mut cmd)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Shows the meeting's Markdown export in Finder or Explorer.
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
pub fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
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
    let catch_up = settings.drive_upload && !old.drive_upload;
    *st.settings.lock().unwrap() = settings;
    if catch_up {
        drive::resume(&app);
    }
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
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-t");
        c
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("notepad")
    } else {
        std::process::Command::new("xdg-open")
    };
    recorder::quiet(cmd.arg(&path))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Opens a web page in the default browser.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("Not a web address.".into());
    }
    open::that_detached(&url).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn drive_status(app: AppHandle) -> drive::Account {
    state(&app).drive.account()
}

/// Signs in to Google in the browser and, once connected, uploads every call.
#[tauri::command]
pub async fn drive_connect(
    app: AppHandle,
    client_id: String,
    client_secret: String,
) -> Result<drive::Account, String> {
    let account = state(&app)
        .drive
        .connect(&client_id, &client_secret)
        .await?;
    drive::resume(&app);
    Ok(account)
}

#[tauri::command]
pub fn drive_disconnect(app: AppHandle) -> Result<(), String> {
    state(&app).drive.disconnect()
}

/// Uploads (or retries uploading) one meeting now.
#[tauri::command]
pub fn upload_meeting(app: AppHandle, id: String) -> Result<(), String> {
    let st = state(&app);
    if !st.drive.is_connected() {
        return Err("Connect Google Drive in Settings first.".into());
    }
    if !st.settings().drive_upload {
        return Err("Turn on Google Drive uploads in Settings first.".into());
    }
    drive::enqueue(&app, &id);
    Ok(())
}

/// The built-in Hebrew-English terms, grouped, for Settings to show.
#[tauri::command]
pub fn builtin_vocabulary() -> Vec<crate::vocabulary::BuiltinGroupInfo> {
    crate::vocabulary::builtin_groups()
}

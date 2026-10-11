use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use serde::Serialize;
use tauri::menu::MenuItem;
use tauri::{AppHandle, Emitter, Manager, Wry};
use tauri_plugin_shell::process::CommandChild;
use tokio::sync::mpsc::UnboundedSender;

use crate::drive::Drive;
use crate::library::Library;
use crate::settings::Settings;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    /// Transcribe (and then summarize, if enabled) a meeting.
    Process(String),
    /// Only (re)write the summary.
    Summarize(String),
}

impl Job {
    pub fn meeting_id(&self) -> &str {
        match self {
            Job::Process(id) | Job::Summarize(id) => id,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct JobProgress {
    pub id: String,
    pub label: String,
    pub percent: f64,
}

#[derive(Default)]
pub struct Recording {
    pub child: Option<CommandChild>,
    pub meeting_id: Option<String>,
    /// Set once the recorder reports that both tracks are being written.
    pub started: Option<Instant>,
    pub started_at_ms: Option<i64>,
    /// Quit the app once the recorder has finished writing its files.
    pub quit_when_stopped: bool,
}

pub struct AppState {
    pub library: Mutex<Library>,
    pub settings: Mutex<Settings>,
    pub settings_path: PathBuf,
    pub recording: Mutex<Recording>,
    pub toggle_item: MenuItem<Wry>,
    pub jobs: UnboundedSender<Job>,
    pub queued: Mutex<HashSet<String>>,
    pub progress: Mutex<Option<JobProgress>>,
    /// The call the "Record this call?" pop-up is asking about.
    pub pending_call: Mutex<Option<crate::calls::CallInfo>>,
    pub drive: Drive,
}

impl AppState {
    pub fn library(&self) -> MutexGuard<'_, Library> {
        self.library.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn settings(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn recording(&self) -> MutexGuard<'_, Recording> {
        self.recording.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn enqueue(&self, job: Job) {
        let key = format!("{job:?}");
        if self.queued.lock().unwrap().insert(key) {
            let _ = self.jobs.send(job);
        }
    }

    pub fn dequeued(&self, job: &Job) {
        self.queued.lock().unwrap().remove(&format!("{job:?}"));
    }
}

pub fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

/// Tells the window a meeting changed (or was added/removed).
pub fn library_changed(app: &AppHandle, id: &str) {
    let _ = app.emit("library", id);
}

//! Starts and stops the Swift recorder sidecar (`recorder-mac/`) and turns each recording
//! into a meeting in the library.

use chrono::Local;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;

use crate::library::{default_title, Status};
use crate::state::{library_changed, state, Job};

/// The process tap that records the other side of the call needs macOS 14.2.
const MIN_MACOS: (u32, u32) = (14, 2);

pub fn macos_version() -> Option<(u32, u32, String)> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let mut parts = text.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        return Some((parts.next()?, parts.next().unwrap_or(0), text));
    }
    #[allow(unreachable_code)]
    None
}

/// Returns why recording can't work on this machine, if it can't.
pub fn recording_blocker() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return Some("Recording is only available on macOS for now.".into());
    }
    match macos_version() {
        Some((major, minor, text)) if (major, minor) < MIN_MACOS => Some(format!(
            "Recording needs macOS 14.2 or later, and this Mac is on macOS {text}. \
             Update it in System Settings > General > Software Update."
        )),
        _ => None,
    }
}

/// Best guess at which call app is in use, from what's running. Meet runs in a browser and
/// can't be told apart this way, so it's left blank to be set by hand.
fn detect_call_app() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("/bin/ps")
        .args(["-axo", "comm="])
        .output()
    {
        let running = String::from_utf8_lossy(&out.stdout);
        if running
            .lines()
            .any(|l| l.ends_with("/zoom.us") || l.ends_with("/CptHost"))
        {
            return "Zoom".into();
        }
        if running.lines().any(|l| l.ends_with("/WhatsApp")) {
            return "WhatsApp".into();
        }
    }
    String::new()
}

pub fn is_recording(app: &AppHandle) -> bool {
    state(app).recording().child.is_some()
}

pub fn start(app: &AppHandle) -> Result<(), String> {
    if let Some(reason) = recording_blocker() {
        return Err(reason);
    }
    let st = state(app);
    let mut rec = st.recording();
    if rec.child.is_some() {
        return Err("Already recording.".into());
    }

    let (language, record_video) = {
        let settings = st.settings();
        (settings.default_language.clone(), settings.record_video)
    };
    let meeting = st
        .library()
        .create(Local::now(), detect_call_app(), language)?;
    let folder = st.library().folder(&meeting.id)?;

    let mut args = vec!["--out".into(), folder.into_os_string()];
    if record_video {
        args.push("--video".into());
    }
    let (mut events, child) = app
        .shell()
        .sidecar("gatit-recorder")
        .map_err(|e| e.to_string())?
        .args(args)
        .spawn()
        .map_err(|e| e.to_string())?;
    rec.child = Some(child);
    rec.meeting_id = Some(meeting.id.clone());
    rec.started = None;
    rec.started_at_ms = None;
    drop(rec);
    let _ = st.toggle_item.set_text("Stop recording");
    let _ = app.emit("recorder", json!({ "event": "starting" }));
    library_changed(app, &meeting.id);

    let app = app.clone();
    let id = meeting.id;
    tauri::async_runtime::spawn(async move {
        let mut last_error: Option<String> = None;
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stdout(line) => {
                    let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                        continue;
                    };
                    match value["event"].as_str() {
                        Some("started") => {
                            let st = state(&app);
                            let mut rec = st.recording();
                            rec.started = Some(std::time::Instant::now());
                            rec.started_at_ms = Some(Local::now().timestamp_millis());
                        }
                        Some("error") => last_error = value["message"].as_str().map(String::from),
                        Some("video") => {
                            if let Some(name) = value["app"].as_str() {
                                set_source_app(&app, &id, name);
                            }
                        }
                        _ => {}
                    }
                    let _ = app.emit("recorder", value);
                }
                CommandEvent::Stderr(line) => {
                    eprintln!(
                        "gatit-recorder: {}",
                        String::from_utf8_lossy(&line).trim_end()
                    );
                }
                CommandEvent::Terminated(_) => break,
                _ => {}
            }
        }
        finished(&app, &id, last_error);
    });
    Ok(())
}

/// Fills in the call app from the window being recorded (the only way to tell it's Meet),
/// unless it's already known.
fn set_source_app(app: &AppHandle, id: &str, name: &str) {
    let st = state(app);
    let updated = st.library().update(id, |m| {
        if m.source_app.is_empty() {
            m.source_app = name.to_string();
            if !m.title_edited {
                m.title = default_title(&m.started_at, name);
            }
        }
    });
    if updated.is_ok() {
        library_changed(app, id);
    }
}

/// Combines a recording's screen.mp4 with its audio into video.mp4 (see `muxVideo` in the
/// recorder), in the background. Does nothing when there's no separate video waiting.
pub fn combine_video(app: &AppHandle, id: &str) {
    let Ok(folder) = state(app).library().folder(id) else {
        return;
    };
    if !folder.join("screen.mp4").exists() {
        return;
    }
    let Ok(command) = app.shell().sidecar("gatit-recorder") else {
        return;
    };
    let command = command.args(["--mux".as_ref(), folder.as_os_str()]);
    let app = app.clone();
    let id = id.to_string();
    tauri::async_runtime::spawn(async move {
        match command.output().await {
            Ok(out) if out.status.success() => library_changed(&app, &id),
            Ok(out) => eprintln!(
                "gatit-recorder --mux: {}",
                String::from_utf8_lossy(&out.stdout).trim_end()
            ),
            Err(e) => eprintln!("gatit-recorder --mux: {e}"),
        }
    });
}

/// Called when the recorder process exits, however it stopped.
fn finished(app: &AppHandle, id: &str, error: Option<String>) {
    let st = state(app);
    let (started, quit) = {
        let mut rec = st.recording();
        let started = rec.started.take();
        rec.child = None;
        rec.meeting_id = None;
        rec.started_at_ms = None;
        (started, std::mem::take(&mut rec.quit_when_stopped))
    };
    let _ = st.toggle_item.set_text("Start recording");

    match started {
        Some(started) => {
            let secs = started.elapsed().as_secs_f64();
            let _ = st.library().update(id, |m| {
                m.duration_secs = secs;
                m.status = Status::Recorded;
            });
            combine_video(app, id);
            if st.settings().transcribe_when != "manual" {
                st.enqueue(Job::Process(id.to_string()));
            }
        }
        None => {
            // Never got going (for example a permission was denied): there's no audio to keep.
            if let Ok(folder) = st.library().folder(id) {
                let _ = std::fs::remove_dir_all(folder);
            }
            let _ = st.library().delete(id);
            let message = error.unwrap_or_else(|| "The recorder stopped before it started.".into());
            let _ = app.emit("recorder", json!({ "event": "error", "message": message }));
        }
    }
    library_changed(app, id);
    let _ = app.emit("recorder", json!({ "event": "idle" }));
    if quit {
        app.exit(0);
    }
}

pub fn stop(app: &AppHandle) -> Result<(), String> {
    let st = state(app);
    let mut rec = st.recording();
    match rec.child.as_mut() {
        Some(child) => child.write(b"stop\n").map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

/// Stops the recording (if any) and quits once both files are safely written.
pub fn stop_and_quit(app: &AppHandle) {
    if is_recording(app) {
        state(app).recording().quit_when_stopped = true;
        let _ = stop(app);
    } else {
        app.exit(0);
    }
}

pub fn toggle(app: &AppHandle) {
    let result = if is_recording(app) {
        stop(app)
    } else {
        start(app)
    };
    if let Err(message) = result {
        let _ = app.emit("recorder", json!({ "event": "error", "message": message }));
    }
}

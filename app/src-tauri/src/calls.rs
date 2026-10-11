//! Notices calls starting and offers to record them. The Swift sidecar, run as
//! `gatit-recorder --watch-calls`, reports when Zoom, WhatsApp or a browser (Meet) starts or
//! stops using the microphone; this module shows a small "Record this call?" pop-up in the
//! top-right corner of the screen.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::utils::config::WindowEffectsConfig;
use tauri::window::{Effect, EffectState};
use tauri::{AppHandle, Emitter, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;

use crate::recorder;
use crate::state::state;

const PROMPT_LABEL: &str = "prompt";
const PROMPT_WIDTH: f64 = 380.0;
const PROMPT_HEIGHT: f64 = 84.0;
/// Gap between the pop-up and the screen's top-right corner (below the menu bar).
const PROMPT_MARGIN: (f64, f64) = (14.0, 38.0);

#[derive(Clone, Debug, Serialize)]
pub struct CallInfo {
    /// What the pop-up shows: "Zoom", "WhatsApp", "Google Chrome", ...
    pub app: String,
    /// What the meeting is filed under: "Zoom", "WhatsApp", "Meet" or "Other".
    pub source: String,
}

/// Starts watching for calls, restarting the watcher if it ever exits.
pub fn watch(app: &AppHandle) {
    if !cfg!(target_os = "macos") || recorder::recording_blocker().is_some() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(e) = run_watcher(&app).await {
                eprintln!("call watcher: {e}");
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

async fn run_watcher(app: &AppHandle) -> Result<(), String> {
    let (mut events, _child) = app
        .shell()
        .sidecar("gatit-recorder")
        .map_err(|e| e.to_string())?
        .args(["--watch-calls"])
        .spawn()
        .map_err(|e| e.to_string())?;
    while let Some(event) = events.recv().await {
        match event {
            CommandEvent::Stdout(line) => {
                if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                    handle_event(app, &value);
                }
            }
            CommandEvent::Stderr(line) => {
                eprintln!(
                    "call watcher: {}",
                    String::from_utf8_lossy(&line).trim_end()
                );
            }
            CommandEvent::Terminated(status) => {
                return Err(format!("exited ({:?})", status.code));
            }
            _ => {}
        }
    }
    Ok(())
}

fn handle_event(app: &AppHandle, value: &Value) {
    let name = value["app"].as_str().unwrap_or_default().to_string();
    match value["event"].as_str() {
        Some("call_started") => {
            let call = CallInfo {
                source: value["source"].as_str().unwrap_or("Other").to_string(),
                app: name,
            };
            if state(app).settings().ask_on_call && !recorder::is_recording(app) {
                show_prompt(app, call);
            }
        }
        Some("call_ended") => {
            let pending = state(app).pending_call.lock().unwrap().clone();
            if pending.is_some_and(|c| c.app == name) {
                close_prompt(app);
            }
        }
        Some("error") => eprintln!("call watcher: {value}"),
        _ => {}
    }
}

fn show_prompt(app: &AppHandle, call: CallInfo) {
    *state(app).pending_call.lock().unwrap() = Some(call);
    if let Some(window) = app.get_webview_window(PROMPT_LABEL) {
        // Already showing for another call: let it pick up the new app name.
        let _ = window.eval("window.loadCall && window.loadCall()");
        return;
    }
    if let Err(e) = build_prompt(app) {
        eprintln!("couldn't show the recording prompt: {e}");
    }
}

fn build_prompt(app: &AppHandle) -> tauri::Result<()> {
    let (x, y) = match app.primary_monitor()? {
        Some(monitor) => {
            let scale = monitor.scale_factor();
            let origin = monitor.position().to_logical::<f64>(scale);
            let size = monitor.size().to_logical::<f64>(scale);
            (
                origin.x + size.width - PROMPT_WIDTH - PROMPT_MARGIN.0,
                origin.y + PROMPT_MARGIN.1,
            )
        }
        None => (PROMPT_MARGIN.0, PROMPT_MARGIN.1),
    };

    let builder =
        WebviewWindowBuilder::new(app, PROMPT_LABEL, WebviewUrl::App("prompt.html".into()))
            .title("Record this call?")
            .inner_size(PROMPT_WIDTH, PROMPT_HEIGHT)
            .position(x, y)
            .resizable(false)
            .decorations(false)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .shadow(true)
            // Don't take focus away from the call; a click still lands on the first try.
            .focused(false)
            .accept_first_mouse(true)
            // A rounded, blurred panel like macOS's own notifications.
            .transparent(true)
            .effects(WindowEffectsConfig {
                effects: vec![Effect::HudWindow],
                state: Some(EffectState::Active),
                radius: Some(14.0),
                ..Default::default()
            });

    let window = builder.build()?;
    let _ = window.set_position(LogicalPosition::new(x, y));
    Ok(())
}

pub fn close_prompt(app: &AppHandle) {
    state(app).pending_call.lock().unwrap().take();
    if let Some(window) = app.get_webview_window(PROMPT_LABEL) {
        let _ = window.close();
    }
}

/// The call the pop-up is asking about, if it's still open.
#[tauri::command]
pub fn pending_call(app: AppHandle) -> Option<CallInfo> {
    state(&app).pending_call.lock().unwrap().clone()
}

/// The pop-up's answer: start recording (filed under the call's app), or not. The pop-up is
/// gone by the time recording could fail, so a failure opens the main window to explain it.
#[tauri::command]
pub fn answer_call_prompt(app: AppHandle, record: bool) {
    let call = state(&app).pending_call.lock().unwrap().take();
    close_prompt(&app);
    let Some(call) = call.filter(|_| record && !recorder::is_recording(&app)) else {
        return;
    };
    if let Err(message) = recorder::start(&app, Some(call.source)) {
        let _ = app.emit("recorder", json!({ "event": "error", "message": message }));
        crate::show_window(&app);
    }
}

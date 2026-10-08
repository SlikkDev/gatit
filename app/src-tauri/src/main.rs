// Hide the extra console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{json, Value};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent, Wry};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

struct Recorder {
    child: Mutex<Option<CommandChild>>,
    toggle: MenuItem<Wry>,
}

fn recordings_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("recordings");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[tauri::command]
fn is_recording(state: State<Recorder>) -> bool {
    state.child.lock().unwrap().is_some()
}

#[tauri::command]
fn start_recording(app: AppHandle, state: State<Recorder>) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Recording is only available on macOS for now.".into());
    }
    let mut child_slot = state.child.lock().unwrap();
    if child_slot.is_some() {
        return Err("Already recording.".into());
    }

    let folder = recordings_dir(&app)?.join(chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string());
    let (mut events, child) = app
        .shell()
        .sidecar("gatit-recorder")
        .map_err(|e| e.to_string())?
        .args(["--out".as_ref(), folder.as_os_str()])
        .spawn()
        .map_err(|e| e.to_string())?;
    *child_slot = Some(child);
    let _ = state.toggle.set_text("Stop recording");
    let _ = app.emit("recorder", json!({ "event": "starting" }));

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stdout(line) => {
                    if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                        let _ = app.emit("recorder", value);
                    }
                }
                CommandEvent::Stderr(line) => {
                    eprintln!("gatit-recorder: {}", String::from_utf8_lossy(&line).trim_end());
                }
                CommandEvent::Terminated(_) => break,
                _ => {}
            }
        }
        let state = app.state::<Recorder>();
        *state.child.lock().unwrap() = None;
        let _ = state.toggle.set_text("Start recording");
        let _ = app.emit("recorder", json!({ "event": "idle" }));
    });
    Ok(())
}

#[tauri::command]
fn stop_recording(state: State<Recorder>) -> Result<(), String> {
    match state.child.lock().unwrap().as_mut() {
        Some(child) => child.write(b"stop\n").map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

#[tauri::command]
fn open_recordings(app: AppHandle) -> Result<(), String> {
    let dir = recordings_dir(&app)?;
    let opener = if cfg!(target_os = "windows") { "explorer" } else { "open" };
    std::process::Command::new(opener).arg(dir).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn toggle_recording(app: &AppHandle) {
    let state = app.state::<Recorder>();
    let result = if is_recording(state.clone()) {
        stop_recording(state)
    } else {
        start_recording(app.clone(), state)
    };
    if let Err(message) = result {
        let _ = app.emit("recorder", json!({ "event": "error", "message": message }));
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let toggle = MenuItem::with_id(app, "toggle", "Start recording", true, None::<&str>)?;
            let open = MenuItem::with_id(app, "open", "Open recordings folder", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Show Gatit", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toggle, &open, &show, &quit])?;

            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Gatit")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "toggle" => toggle_recording(app),
                    "open" => {
                        let _ = open_recordings(app.clone());
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        let _ = stop_recording(app.state::<Recorder>());
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            app.manage(Recorder { child: Mutex::new(None), toggle });
            Ok(())
        })
        // Closing the window keeps Gatit (and any recording) running in the menu bar.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            is_recording,
            start_recording,
            stop_recording,
            open_recordings
        ])
        .run(tauri::generate_context!())
        .expect("error while running Gatit");
}

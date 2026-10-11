// Hide the extra console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod calls;
mod commands;
mod drive;
mod hints;
mod jobs;
mod library;
#[cfg(target_os = "macos")]
mod panel;
mod recorder;
mod settings;
mod state;
mod summary;
mod transcribe;
mod transcript;
mod updater;
mod vocabulary;

use std::collections::HashSet;
use std::sync::Mutex;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WindowEvent};
use tauri_plugin_global_shortcut::ShortcutState;

use crate::library::Library;
use crate::state::{AppState, Recording};

pub(crate) fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        recorder::toggle(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = app.path().app_data_dir()?;
            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            std::fs::create_dir_all(&config_dir)?;

            let settings_path = config_dir.join("settings.json");
            let settings = settings::load(&settings_path);
            let library = Library::open(
                data_dir.join("meetings"),
                &data_dir.join("index.sqlite"),
                &settings.your_name,
            )
            .map_err(std::io::Error::other)?;

            let toggle = MenuItem::with_id(app, "toggle", "Start recording", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Open Gatit", true, None::<&str>)?;
            let folder =
                MenuItem::with_id(app, "folder", "Open meetings folder", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &toggle,
                    &PredefinedMenuItem::separator(app)?,
                    &show,
                    &folder,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?;

            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Gatit")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "toggle" => recorder::toggle(app),
                    "show" => show_window(app),
                    "folder" => {
                        let _ = commands::open_library_folder(app.clone());
                    }
                    "update" => {
                        if let Err(e) = updater::restart(app) {
                            eprintln!("{e}");
                            show_window(app);
                        }
                    }
                    "quit" => recorder::stop_and_quit(app),
                    _ => {}
                })
                .build(app)?;

            let (jobs_tx, jobs_rx) = tokio::sync::mpsc::unbounded_channel();
            let (uploads_tx, uploads_rx) = tokio::sync::mpsc::unbounded_channel();
            app.manage(AppState {
                library: Mutex::new(library),
                settings: Mutex::new(settings.clone()),
                settings_path,
                recording: Mutex::new(Recording::default()),
                toggle_item: toggle,
                jobs: jobs_tx,
                queued: Mutex::new(HashSet::new()),
                progress: Mutex::new(None),
                pending_call: Mutex::new(None),
                drive: drive::Drive::new(uploads_tx),
            });

            if let Err(e) = commands::register_shortcut(&handle, &settings.shortcut, None) {
                eprintln!("{e}");
            }
            tauri::async_runtime::spawn(jobs::run(handle.clone(), jobs_rx));
            tauri::async_runtime::spawn(drive::run(handle.clone(), uploads_rx));
            jobs::resume_pending(&handle);
            drive::resume(&handle);
            calls::watch(&handle);
            updater::start(&handle, menu);
            Ok(())
        })
        // Closing the window keeps Gatit (and any recording) running in the menu bar.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_status,
            commands::toggle_recording,
            commands::list_meetings,
            commands::get_meeting,
            commands::update_meeting,
            commands::transcribe_meeting,
            commands::summarize_meeting,
            commands::delete_meeting,
            commands::search,
            commands::reveal_meeting,
            commands::open_library_folder,
            commands::copy_text,
            commands::get_settings,
            commands::save_settings,
            commands::set_api_key,
            commands::model_status,
            commands::download_model,
            commands::open_summary_prompt,
            commands::builtin_vocabulary,
            commands::open_url,
            commands::drive_status,
            commands::drive_connect,
            commands::drive_disconnect,
            commands::upload_meeting,
            commands::restart_to_update,
            calls::pending_call,
            calls::answer_call_prompt,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Gatit");
}

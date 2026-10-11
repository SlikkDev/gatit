//! Keeps Gatit up to date from the "latest" GitHub release.
//!
//! The check runs shortly after launch and every few hours after that. When a newer build is
//! published, it is downloaded, its signature verified and the app on disk replaced quietly;
//! the window shows a "Restart" banner and the menu bar gets a "Restart to update" item, so a
//! recording in progress is never interrupted. On Windows, installing means running the new
//! installer, which closes the app, so there the download and install wait for that button.
//! Until the release carries a `latest.json` (which needs the updater key in the repository's
//! secrets), the check finds nothing and stays silent.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use tauri::menu::{Menu, MenuItem};
use tauri::{AppHandle, Emitter, Wry};
use tauri_plugin_updater::UpdaterExt;

use crate::recorder;

const FIRST_CHECK: Duration = Duration::from_secs(30);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// The version installed on disk and waiting for a restart, if any.
static READY: Mutex<Option<String>> = Mutex::new(None);

pub fn ready_version() -> Option<String> {
    READY.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Starts the periodic check. `menu` is the tray menu, which gets a "Restart to update"
/// item once an update is installed.
pub fn start(app: &AppHandle, menu: Menu<Wry>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if ready_version().is_none() {
                match check(&app).await {
                    Ok(Some(version)) => {
                        eprintln!("update: Gatit {version} installed, restart to use it");
                        *READY.lock().unwrap_or_else(|e| e.into_inner()) = Some(version.clone());
                        if let Ok(item) = MenuItem::with_id(
                            &app,
                            "update",
                            format!("Restart to update to Gatit {version}"),
                            true,
                            None::<&str>,
                        ) {
                            let _ = menu.prepend(&item);
                        }
                        let _ = app.emit("update", json!({ "version": version }));
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("update check: {e}"),
                }
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// Looks for a newer build and returns its version. On the Mac it is downloaded and installed
/// right away; on Windows that happens in `restart`.
async fn check(app: &AppHandle) -> Result<Option<String>, tauri_plugin_updater::Error> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(None);
    };
    if cfg!(target_os = "macos") {
        update.download_and_install(|_, _| {}, || {}).await?;
    }
    Ok(Some(update.version))
}

/// Downloads the newer build and installs it. On Windows this runs the installer, which
/// closes the app and opens the new one when it is done.
async fn install(app: &AppHandle) -> Result<(), tauri_plugin_updater::Error> {
    if let Some(update) = app.updater()?.check().await? {
        update.download_and_install(|_, _| {}, || {}).await?;
    }
    Ok(())
}

/// Restarts into the installed update. Refused while a call is being recorded.
pub fn restart(app: &AppHandle) -> Result<(), String> {
    if ready_version().is_none() {
        return Err("No update is waiting.".into());
    }
    if recorder::is_recording(app) {
        return Err("Finish the recording first, then restart to update.".into());
    }
    if cfg!(target_os = "macos") {
        app.restart()
    } else {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = install(&app).await {
                eprintln!("update install: {e}");
            }
        });
        Ok(())
    }
}

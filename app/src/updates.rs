//! Updates through the Tauri updater (stage 6 plan): when the setting is on, Laurel asks
//! the update channel of the public repository whether a newer release exists, a little
//! after it starts and when the user asks in About. That is Laurel's only request to the
//! internet. A release is downloaded and installed only when the user asks, and its
//! signature must match the project's public key (plugins.updater in tauri.conf.json);
//! the installer then closes Laurel and opens the new version.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::i18n::t;
use crate::log::log;

/// How long after the start the automatic check waits, so it never slows the start.
const FIRST_CHECK: Duration = Duration::from_secs(15);

/// The update the last check found, kept for the install.
#[derive(Default)]
pub struct Pending(Mutex<Option<Update>>);

/// Whether this build can check: it carries the project's public key.
pub fn configured(app: &AppHandle) -> bool {
    app.config().plugins.0.get("updater").and_then(|u| u["pubkey"].as_str()).is_some_and(|key| !key.trim().is_empty())
}

/// Asks the update channel; the answer says whether a newer version exists.
pub async fn check(app: &AppHandle) -> Result<Value, String> {
    let current = app.package_info().version.to_string();
    if !configured(app) {
        return Err(t!("updates are not set up in this build"));
    }
    let found = match app.updater() {
        Ok(updater) => updater.check().await,
        Err(e) => Err(e),
    }
    .map_err(|e| {
        log!("updates: check failed: {e}");
        e.to_string()
    })?;
    let result = match &found {
        Some(update) => json!({ "available": true, "version": update.version, "notes": update.body, "current": current }),
        None => json!({ "available": false, "current": current }),
    };
    log!("updates: {result}");
    *app.state::<Pending>().0.lock().unwrap() = found;
    Ok(result)
}

/// Downloads the update found by the last check, checks its signature and runs its
/// installer, which closes Laurel. Progress goes to the windows as "update-progress".
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let update = app.state::<Pending>().0.lock().unwrap().take().ok_or("no update to install: check again")?;
    log!("updates: installing {}", update.version);
    let emitter = app.clone();
    let mut received = 0u64;
    let on_chunk = move |chunk: usize, total: Option<u64>| {
        received += chunk as u64;
        let _ = emitter.emit("update-progress", json!({ "received": received, "total": total }));
    };
    update.download_and_install(on_chunk, || log!("updates: downloaded, checking the signature")).await.map_err(|e| {
        log!("updates: install failed: {e}");
        e.to_string()
    })
}

/// The check a little after the start, when the setting is on; a newer version is
/// announced to the windows as "update-available".
pub fn check_after_start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK);
        if !app.state::<std::sync::Arc<crate::settings::Store>>().get().check_updates {
            log!("updates: the check is off in the settings");
            return;
        }
        if !configured(&app) {
            log!("updates: not set up in this build (no public key)");
            return;
        }
        if let Ok(result) = tauri::async_runtime::block_on(check(&app)) {
            if result["available"] == true {
                let _ = app.emit("update-available", result);
            }
        }
    });
}

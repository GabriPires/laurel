//! Laurel desktop app (PoC-U1): tray icon, main window, global hotkeys and the
//! supervision of the laurel-engine process, which it drives over JSON-RPC.
//!
//! Specs: app.startup, clip.hotkeys, game.detection, game.manual-add, overlay.in-game, alerts.sound, library.*,
//! edit.trim, share.local, storage.location, storage.limits (wiki/specs/).
//! Options: `--minimized` starts in the tray; `--selftest` runs a scripted check and
//! exits (results in app.log). Environment overrides for development: LAUREL_ENGINE
//! (engine executable, inside its OBS runtime), LAUREL_WORK_DIR,
//! LAUREL_CAPTURE (a fixed target instead of game detection), LAUREL_OVERLAY_CAPTURABLE=1 (overlay visible to capture),
//! LAUREL_FFMPEG (ffmpeg.exe), LAUREL_START_VIEW (library | clips | screenshots | settings[:<page>]).
#![windows_subsystem = "windows"]

mod detect;
mod engine;
mod games;
mod hotkeys;
mod i18n;
mod library;
mod log;
mod media;
mod mp4;
mod overlay;
mod settings;
mod share;
mod sound;
mod steam;
mod storage;
mod tray_icons;
mod updates;
mod win;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::Wry;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Listener, Manager, RunEvent, WindowEvent};

use detect::{Detector, Mode};
use engine::{Engine, EngineConfig};
use hotkeys::{Combo, Hotkeys};
use i18n::t;
use library::Library;
use log::log;
use overlay::{Kind, Notifier};
use settings::{CustomGame, IgnoredGame, Settings, Store};
use storage::Storage;

const CLIP_TIMEOUT: Duration = Duration::from_secs(40);
const SHOT_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the free space of the capture drive is checked.
const DISK_CHECK: Duration = Duration::from_secs(15);
/// Name of the value under the Run key (spec app.startup).
const AUTOSTART_NAME: &str = "Laurel";

/// The tray menu items, relabeled when the hotkeys or the language change.
struct TrayItems {
    open: MenuItem<Wry>,
    toggle: CheckMenuItem<Wry>,
    clip: MenuItem<Wry>,
    shot: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

/// The hotkey actions; "toggle" (record.toggle) has no hotkey by default.
const HOTKEY_ACTIONS: [&str; 3] = ["clip", "screenshot", "toggle"];

fn hotkey_text<'a>(settings: &'a Settings, action: &str) -> &'a str {
    match action {
        "clip" => &settings.hotkey_clip,
        "screenshot" => &settings.hotkey_screenshot,
        _ => &settings.hotkey_toggle,
    }
}

fn bindings(settings: &Settings) -> Vec<(&'static str, Combo)> {
    HOTKEY_ACTIONS.into_iter().filter_map(|action| Some((action, Combo::parse(hotkey_text(settings, action))?))).collect()
}

/// The hotkey of an action as shown, with " (…)" around it, or nothing when unset.
fn hotkey_suffix(settings: &Settings, action: &str) -> String {
    Combo::parse(hotkey_text(settings, action)).map_or_else(String::new, |c| format!(" ({})", c.label()))
}

fn hotkeys_json(app: &AppHandle) -> Value {
    let Some(hotkeys) = app.try_state::<Hotkeys>() else { return json!({ "available": false }) };
    let mut v = json!({ "available": true, "capturing": hotkeys.capturing() });
    let bound = hotkeys.bindings();
    for action in HOTKEY_ACTIONS {
        v[action] = bound.iter().find(|(a, _)| *a == action).map_or(Value::Null, |(_, c)| json!({ "combo": c.name(), "label": c.label() }));
    }
    v
}

fn update_tray(app: &AppHandle) {
    if let Some(items) = app.try_state::<TrayItems>() {
        let settings = app.state::<Arc<Store>>().get();
        let _ = items.open.set_text(t!("Open Laurel"));
        let _ = items.toggle.set_text(format!("{}{}", t!("Recording"), hotkey_suffix(&settings, "toggle")));
        let _ = items.toggle.set_checked(settings.recording_enabled);
        let _ = items.clip.set_text(format!("{}{}", t!("Save clip"), hotkey_suffix(&settings, "clip")));
        let _ = items.shot.set_text(format!("{}{}", t!("Screenshot"), hotkey_suffix(&settings, "screenshot")));
        let _ = items.quit.set_text(t!("Quit"));
    }
}

/// Saves new hotkeys and applies them to the listener, the tray and the windows.
fn set_hotkeys(app: &AppHandle, edit: impl FnOnce(&mut Settings)) {
    let store = app.state::<Arc<Store>>();
    let mut settings = store.get();
    edit(&mut settings);
    if let Err(e) = store.set(settings.clone()) {
        log!("hotkeys: could not save the settings: {e}");
    }
    if let Some(hotkeys) = app.try_state::<Hotkeys>() {
        hotkeys.set_bindings(bindings(&settings));
    }
    update_tray(app);
    let _ = app.emit("hotkeys-changed", hotkeys_json(app));
}

/// Turns recording on or off (spec record.toggle), from the UI, the tray or the
/// hotkey; `notice` tells the player in game (for the hotkey).
fn set_recording_enabled(app: &AppHandle, enabled: bool, notice: bool) {
    let store = app.state::<Arc<Store>>();
    if let Err(e) = store.set(Settings { recording_enabled: enabled, ..store.get() }) {
        log!("recording: could not save the settings: {e}");
    }
    log!("recording turned {}", if enabled { "on" } else { "off" });
    app.state::<Arc<Detector>>().set_enabled(enabled);
    update_tray(app);
    if notice {
        let notifier = app.state::<Notifier>();
        if enabled {
            notifier.notify(Kind::Info, &t!("Recording on"), &t!("Laurel records your games again"));
        } else {
            notifier.notify(Kind::Info, &t!("Recording off"), &t!("Laurel stops recording until you turn it back on"));
        }
    }
}

/// Runs on the hotkey listener thread (hotkeys.rs); everything slow goes to other threads.
fn on_hotkey(app: &AppHandle, event: hotkeys::Event) {
    use hotkeys::Event;
    match event {
        Event::Pressed("clip") => save_clip(app, "the hotkey"),
        Event::Pressed("screenshot") => take_screenshot(app, "the hotkey"),
        Event::Pressed("toggle") => {
            let app = app.clone();
            std::thread::spawn(move || {
                let enabled = app.state::<Arc<Detector>>().enabled();
                set_recording_enabled(&app, !enabled, true);
            });
        }
        Event::Pressed(_) => {}
        Event::Captured(action, combo) => {
            log!("hotkeys: {action} set to {}", combo.name());
            set_hotkeys(app, |s| match action {
                "clip" => s.hotkey_clip = combo.name(),
                "screenshot" => s.hotkey_screenshot = combo.name(),
                _ => s.hotkey_toggle = combo.name(),
            });
        }
        Event::Rejected(action, combo, reason) => {
            log!("hotkeys: {} refused for {action}: {reason}", combo.name());
            let _ = app.emit("hotkey-rejected", json!({ "action": action, "label": combo.label(), "reason": reason }));
        }
        Event::Cancelled(action) => {
            let _ = app.emit("hotkey-cancelled", json!({ "action": action }));
        }
    }
}

/// laurel-engine.exe: LAUREL_ENGINE, else the bin\64bit folder of its OBS runtime
/// (tools/obs-runtime.py), bundled next to the app or in the development layout.
fn engine_exe() -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    let engine_in = |runtime: PathBuf| runtime.join("bin").join("64bit").join("laurel-engine.exe");
    std::env::var_os("LAUREL_ENGINE").map(PathBuf::from).unwrap_or_else(|| {
        let bundled = engine_in(exe_dir.join("engine"));
        // Development layout: laurel-dev\laurel-app next to laurel-dev\laurel-engine.
        if bundled.exists() { bundled } else { engine_in(exe_dir.join("..").join("laurel-engine")) }
    })
}

fn engine_config(work_dir: PathBuf, storage: Arc<Storage>, detector: Arc<Detector>, store: Arc<Store>) -> EngineConfig {
    let exe = engine_exe();
    let (w, h) = win::primary_monitor_size();
    let options = move || {
        let (clips, shots) = (storage.clips_dir(), storage.screenshots_dir());
        let mut options = detector.engine_args();
        if !store.get().show_cursor {
            options.push("--no-cursor".into());
        }
        options.extend(["--base".into(), format!("{w}x{h}"), "--clips-dir".into(), clips.display().to_string(), "--screenshots-dir".into(), shots.display().to_string()]);
        // Video and clip length (spec quality.encoding): a lower height keeps the
        // monitor's aspect; the engine makes both sides even.
        let s = store.get();
        options.extend([
            "--encoder".into(),
            s.video_encoder.clone(),
            "--codec".into(),
            s.video_codec.clone(),
            "--rate".into(),
            s.video_rate.clone(),
            "--preset".into(),
            s.video_preset.clone(),
            "--bitrate".into(),
            s.video_bitrate.to_string(),
            "--fps".into(),
            s.video_fps.to_string(),
            "--buffer".into(),
            s.clip_secs.to_string(),
            "--mix".into(),
            s.audio_mix.clone(),
        ]);
        if s.video_height > 0 && s.video_height < h {
            options.extend(["--out".into(), format!("{}x{}", w * s.video_height / h, s.video_height)]);
        }
        options
    };
    EngineConfig { exe, work_dir, options: Box::new(options) }
}

/// Saves a clip and tells the windows and the player; used by the hotkey, the tray
/// and the UI. `source` goes to the log, to tell later what asked for each clip.
fn save_clip(app: &AppHandle, source: &str) {
    save_clip_for(app, source, None);
}

/// save_clip for an automatic clip too: `achievement` (autoclip.steam-achievements)
/// names the file, marks the library item and picks the auto-clip notice and sound;
/// an automatic clip that cannot be saved is only logged.
pub fn save_clip_for(app: &AppHandle, source: &str, achievement: Option<Value>) {
    log!("clip requested by {source}");
    let app = app.clone();
    std::thread::spawn(move || {
        let engine = app.state::<Engine>();
        let notifier = app.state::<Notifier>();
        let refused = if !app.state::<Arc<Detector>>().enabled() {
            Some(t!("Recording is off"))
        } else if app.state::<Arc<Storage>>().is_paused() {
            Some(t!("Disk almost full: free up space to keep recording"))
        } else if !app.state::<Arc<Detector>>().recording() {
            Some(t!("No game is being recorded"))
        } else {
            None
        };
        if let Some(message) = refused {
            if achievement.is_some() {
                log!("automatic clip not saved: {message}");
                return;
            }
            notifier.notify(Kind::Error, &t!("Clip not saved"), &message);
            report(&app, "clip-saved", "saveClip", Err(message));
            return;
        }
        let mut result = engine.request("saveClip", Value::Null, CLIP_TIMEOUT);
        match &mut result {
            Ok(saved) => {
                attach_game(&app, saved);
                let secs = engine.state()["info"]["bufferSecs"].as_u64().unwrap_or_default();
                match &achievement {
                    Some(a) => notifier.notify(Kind::AutoClip, &t!("Achievement clip saved"), a["name"].as_str().unwrap_or_default()),
                    None => {
                        let detail = match library::game_name(&saved["game"]) {
                            Some(game) => t!("Last {secs} s · {game}", secs = secs, game = game),
                            None => t!("Last {secs} s", secs = secs),
                        };
                        notifier.notify(Kind::Clip, &t!("Clip saved"), &detail);
                    }
                }
                finish_saved_for(&app, saved, achievement.as_ref());
            }
            Err(e) if achievement.is_some() => log!("automatic clip not saved: {e}"),
            Err(e) => notifier.notify(Kind::Error, &t!("Clip not saved"), e),
        }
        report(&app, "clip-saved", "saveClip", result);
    });
}

fn take_screenshot(app: &AppHandle, source: &str) {
    log!("screenshot requested by {source}");
    let app = app.clone();
    std::thread::spawn(move || {
        let notifier = app.state::<Notifier>();
        let detector = app.state::<Arc<Detector>>();
        if !detector.enabled() || (detector.mode() == Mode::Detect && detector.current().is_none()) {
            let message = if detector.enabled() { t!("No game is being captured") } else { t!("Recording is off") };
            notifier.notify(Kind::Error, &t!("Screenshot not saved"), &message);
            report(&app, "screenshot-saved", "screenshot", Err(message));
            return;
        }
        let mut result = app.state::<Engine>().request("screenshot", Value::Null, SHOT_TIMEOUT);
        match &mut result {
            Ok(saved) => {
                attach_game(&app, saved);
                notifier.notify(Kind::Screenshot, &t!("Screenshot saved"), &t!("Saved to your screenshots"));
                finish_saved(&app, saved);
            }
            Err(e) => notifier.notify(Kind::Error, &t!("Screenshot not saved"), e),
        }
        report(&app, "screenshot-saved", "screenshot", result);
    });
}

/// Puts the detected game's name (from its store library or the user's list) on a
/// clip or screenshot of that game, for file names and the library; a screen
/// recording with no game is named "Screen" (spec capture.screen).
fn attach_game(app: &AppHandle, saved: &mut Value) {
    let detector = app.state::<Arc<Detector>>();
    let Some(game) = detector.current() else {
        if detector.snapshot()["capture"] == "screen" && saved["game"].is_null() {
            saved["game"] = json!({ "name": "Screen", "key": "screen" });
        }
        return;
    };
    let hooked = saved["game"]["exe"].as_str().unwrap_or_default();
    if hooked.is_empty() || hooked.eq_ignore_ascii_case(game.exe()) {
        let mut info = saved["game"].as_object().cloned().unwrap_or_default();
        info.insert("name".into(), json!(game.id.name));
        info.insert("key".into(), json!(game.id.key));
        saved["game"] = Value::Object(info);
    }
}

/// After the engine saved a clip or screenshot: names it after the pattern, adds
/// it to the library and applies the folder size limit. `saved["path"]` is
/// updated to the final name.
fn finish_saved(app: &AppHandle, saved: &mut Value) {
    finish_saved_for(app, saved, None);
}

fn finish_saved_for(app: &AppHandle, saved: &mut Value, achievement: Option<&Value>) {
    let Some(path) = saved["path"].as_str().map(String::from) else { return };
    let library = app.state::<Arc<Library>>();
    let storage = app.state::<Arc<Storage>>();
    let game = library::game_name(&saved["game"]);
    let extra = achievement.and_then(|a| a["name"].as_str().filter(|n| !n.is_empty()).or(a["id"].as_str()));
    let path = storage.apply_naming_with(&path, game.as_deref(), extra, library.thumbs_dir());
    saved["path"] = Value::String(path.clone());
    let added = library.add(&path, game).and_then(|item| match achievement {
        Some(a) => library.set_achievement(&path, a),
        None => Ok(item),
    });
    match added {
        Ok(item) => {
            let _ = app.emit("library-changed", item);
        }
        Err(e) => log!("library: could not index {path}: {e}"),
    }
    let removed = storage.enforce_limit(&library);
    if !removed.is_empty() {
        log!("storage limit: removed {removed:?}");
        let _ = app.emit("storage-pruned", json!({ "paths": removed }));
        let _ = app.emit("library-changed", Value::Null);
    }
}

/// Pauses recording when the capture drive is almost full and resumes it when
/// there is room again (spec storage.limits), telling the player both times.
fn watch_disk(app: AppHandle) {
    std::thread::spawn(move || loop {
        let storage = app.state::<Arc<Storage>>();
        if let Some(pause) = storage.disk_transition() {
            let free_gb = storage.free_space().0.unwrap_or_default() as f64 / (1u64 << 30) as f64;
            app.state::<Arc<Detector>>().set_disk_paused(pause);
            let notifier = app.state::<Notifier>();
            if pause {
                log!("disk almost full ({free_gb:.1} GB free): recording paused");
                notifier.notify(Kind::Error, &t!("Recording paused"), &t!("Disk almost full ({free} GB free). Free up space to keep recording.", free = i18n::number(free_gb, 1)));
            } else {
                log!("disk space back ({free_gb:.1} GB free): recording resumed");
                notifier.notify(Kind::Info, &t!("Recording resumed"), &t!("{free} GB free on the capture drive", free = i18n::number(free_gb, 1)));
            }
            let _ = app.emit("storage-state", storage.state(&app.state::<Arc<Library>>()));
        }
        std::thread::sleep(DISK_CHECK);
    });
}

/// Brings a (re)started engine to the detector's state. With a fixed target, says
/// "Recording" when the engine becomes ready (with detection, finding the game says
/// it); "Recording stopped" when the engine stops while it was ready (but not while
/// Laurel itself is closing).
fn watch_engine_state(app: &AppHandle) {
    let handle = app.clone();
    let previous = Mutex::new(String::new());
    let previous_encoder = Mutex::new(String::new());
    app.listen("engine-state", move |event| {
        let state: Value = serde_json::from_str(event.payload()).unwrap_or_default();
        let now = state["state"].as_str().unwrap_or_default().to_string();
        let mut previous = previous.lock().unwrap();
        let notifier = handle.state::<Notifier>();
        // Spec quality.encoding: the player hears about an encoder that could not start.
        if now == "ready" {
            let info = &state["info"];
            let encoder = info["encoder"].as_str().unwrap_or_default().to_string();
            let mut previous_encoder = previous_encoder.lock().unwrap();
            let fallbacks = info["encoderFallbacks"].as_array().map_or(0, Vec::len);
            if fallbacks > 0 && (*previous != "ready" || *previous_encoder != encoder) {
                notifier.notify(Kind::Error, &t!("Video encoder changed"), &t!("Recording with {encoder}", encoder = encoder_name(info, &encoder)));
            }
            *previous_encoder = encoder;
        }
        if now == "ready" && *previous != "ready" {
            let detector = handle.state::<Arc<Detector>>().inner().clone();
            // Off the event thread: the detector waits for engine replies.
            std::thread::spawn(move || detector.engine_ready());
            if handle.state::<Arc<Detector>>().mode() != Mode::Detect && !handle.state::<Arc<Storage>>().is_paused() {
                let secs = state["info"]["bufferSecs"].as_u64().unwrap_or_default();
                notifier.notify(Kind::Info, &t!("Recording|notice"), &t!("Laurel keeps the last {secs} s", secs = secs));
            }
        } else if *previous == "ready" && now != "ready" && state["closing"] != true && state["restart"] != true {
            notifier.notify(Kind::Error, &t!("Recording stopped"), &state["message"].as_str().map_or_else(|| t!("The engine stopped"), String::from));
        }
        *previous = now;
    });
}

fn report(app: &AppHandle, event: &str, action: &str, result: Result<Value, String>) {
    match result {
        Ok(value) => {
            log!("{action}: {value}");
            let _ = app.emit(event, value);
        }
        Err(message) => {
            log!("{action} failed: {message}");
            let _ = app.emit("action-failed", json!({ "action": action, "message": message }));
        }
    }
}

/// Shows the main window when another start of Laurel asks for it.
fn listen_for_show(app: &AppHandle) {
    let app = app.clone();
    win::on_show_request(move || {
        log!("another start of Laurel: showing this one");
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || show_main(&handle));
    });
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn quit(app: &AppHandle) {
    log!("quit requested");
    app.state::<Arc<steam::Achievements>>().shutdown();
    app.state::<Engine>().shutdown();
    app.exit(0);
}

#[tauri::command]
async fn engine_request(engine: tauri::State<'_, Engine>, method: String, params: Option<Value>) -> Result<Value, String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.request(&method, params.unwrap_or(Value::Null), CLIP_TIMEOUT))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn engine_state(engine: tauri::State<'_, Engine>) -> Value {
    engine.state()
}

#[tauri::command]
fn engine_restart(engine: tauri::State<'_, Engine>) {
    engine.restart();
}

/// The command registered to start Laurel with Windows: this executable, in the tray.
fn autostart_command() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    format!("\"{}\" --minimized", exe.display())
}

/// Settings plus state that does not live in settings.json: whether Laurel starts
/// with Windows (the Run key) and the storage figures.
#[tauri::command]
fn get_settings(app: AppHandle) -> Value {
    let mut v = app.state::<Arc<Store>>().get().to_json();
    v["languageResolved"] = json!(i18n::current());
    v["systemLanguage"] = json!(i18n::system());
    v["autostart"] = Value::Bool(win::autostart_command(AUTOSTART_NAME).is_some());
    v["storage"] = app.state::<Arc<Storage>>().state(&app.state::<Arc<Library>>());
    v
}

/// The display name of an encoder id, from the engine's list.
fn encoder_name(info: &Value, id: &str) -> String {
    info["encoders"].as_array().into_iter().flatten().find(|e| e["id"] == id).and_then(|e| e["name"].as_str()).unwrap_or(id).to_string()
}

/// Saves the settings; the capture folder is changed only through `change_folder`.
#[tauri::command]
fn set_settings(app: AppHandle, settings: Value) -> Result<Value, String> {
    let store = app.state::<Arc<Store>>();
    let mut new = Settings::from_json(&settings);
    // The folder, the game lists, the hotkeys and recording on/off change only through
    // their own commands.
    let current = store.get();
    let video_changed = new.video_differs(&current);
    new.folder = current.folder;
    new.custom_games = current.custom_games;
    new.ignored_games = current.ignored_games;
    new.hotkey_clip = current.hotkey_clip;
    new.hotkey_screenshot = current.hotkey_screenshot;
    new.hotkey_toggle = current.hotkey_toggle;
    new.recording_enabled = current.recording_enabled;
    let cursor_changed = new.show_cursor != current.show_cursor;
    let language_changed = new.language != current.language;
    let fewer_days = new.deleted_days < current.deleted_days;
    let mix_changed = new.audio_mix != current.audio_mix;
    let screen = detect::screen_of(&new);
    store.set(new.clone()).map_err(|e| e.to_string())?;
    log!("settings: {}", new.to_json());
    app.state::<Arc<Detector>>().set_screen(screen);
    if fewer_days {
        let removed = app.state::<Arc<Library>>().expire(new.deleted_days, new.recycle);
        if !removed.is_empty() {
            log!("recently deleted: {} item(s) past {} day(s) removed", removed.len(), new.deleted_days);
            let _ = app.emit("library-changed", Value::Null);
        }
    }
    if language_changed {
        // The tray and the next notices change now; the main window reloads itself.
        i18n::set(&new.language);
        log!("language: {} ({})", new.language, i18n::current());
        update_tray(&app);
    }
    if cursor_changed {
        // Spec capture.cursor: applies to the running capture; a restarted engine gets --no-cursor.
        if let Err(e) = app.state::<Engine>().request("setCursor", json!({ "enabled": new.show_cursor }), SHOT_TIMEOUT) {
            log!("setCursor failed: {e}");
        }
    }
    if mix_changed && !video_changed {
        // Spec audio.separate-tracks: track 1 changes without a restart; a restarted engine gets --mix.
        if let Err(e) = app.state::<Engine>().request("setAudioMix", json!({ "mix": new.audio_mix }), SHOT_TIMEOUT) {
            log!("setAudioMix failed: {e}");
        }
    }
    if video_changed {
        // Spec quality.encoding: the engine takes the video settings when it starts.
        log!("settings: video changed, restarting the engine");
        app.state::<Engine>().restart();
    }
    let removed = app.state::<Arc<Storage>>().enforce_limit(&app.state::<Arc<Library>>());
    if !removed.is_empty() {
        log!("storage limit: removed {removed:?}");
        let _ = app.emit("storage-pruned", json!({ "paths": removed }));
        let _ = app.emit("library-changed", Value::Null);
    }
    Ok(get_settings(app))
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<Value, String> {
    let command = autostart_command();
    win::set_autostart(AUTOSTART_NAME, enabled.then_some(command.as_str()))?;
    let store = app.state::<Arc<Store>>();
    let mut settings = store.get();
    settings.autostart_asked = true;
    store.set(settings).map_err(|e| e.to_string())?;
    log!("start with Windows: {enabled}");
    Ok(get_settings(app))
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().set_title(t!("Choose the capture folder")).blocking_pick_folder())
        .await
        .map_err(|e| e.to_string())?;
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.display().to_string()))
}

#[tauri::command]
async fn pick_exe(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().set_title(t!("Choose the game's program")).add_filter(t!("Programs"), &["exe"]).blocking_pick_file())
        .await
        .map_err(|e| e.to_string())?;
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.display().to_string()))
}

/// Starts Laurel again as administrator (UAC prompt), to record a game that runs as
/// administrator, and closes this instance once the new one is starting.
#[tauri::command]
async fn restart_as_admin(app: AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let visible = app.get_webview_window("main").is_some_and(|w| w.is_visible().unwrap_or(false));
    let params = format!("--wait-pid {}{}", std::process::id(), if visible { "" } else { " --minimized" });
    // The UAC prompt blocks until the user answers.
    tauri::async_runtime::spawn_blocking(move || win::run_as_admin(&exe, &params)).await.map_err(|e| e.to_string())??;
    log!("restarting as administrator");
    quit(&app);
    Ok(())
}

#[tauri::command]
fn hotkeys_get(app: AppHandle) -> Value {
    hotkeys_json(&app)
}

/// Records a new hotkey for an action (spec clip.hotkeys): the next valid
/// combination is saved; hotkeys are suspended meanwhile.
#[tauri::command]
fn hotkey_capture(app: AppHandle, action: String) -> Result<Value, String> {
    let Some(action) = HOTKEY_ACTIONS.into_iter().find(|a| *a == action) else { return Err(format!("unknown action {action}")) };
    app.try_state::<Hotkeys>().ok_or("hotkeys are unavailable")?.capture(action);
    log!("hotkeys: recording a new hotkey for {action}");
    Ok(hotkeys_json(&app))
}

#[tauri::command]
fn hotkey_cancel(app: AppHandle) {
    if let Some(hotkeys) = app.try_state::<Hotkeys>() {
        hotkeys.cancel_capture();
    }
}

#[tauri::command]
fn hotkeys_reset(app: AppHandle) -> Value {
    set_hotkeys(&app, |s| {
        s.hotkey_clip = settings::DEFAULT_HOTKEY_CLIP.into();
        s.hotkey_screenshot = settings::DEFAULT_HOTKEY_SCREENSHOT.into();
        s.hotkey_toggle = String::new();
    });
    hotkeys_json(&app)
}

/// Removes the recording on/off hotkey (the only one that may be unset).
#[tauri::command]
fn hotkey_clear(app: AppHandle) -> Value {
    set_hotkeys(&app, |s| s.hotkey_toggle = String::new());
    hotkeys_json(&app)
}

#[tauri::command]
fn set_recording(app: AppHandle, enabled: bool) -> Value {
    set_recording_enabled(&app, enabled, false);
    app.state::<Arc<Detector>>().snapshot()
}

#[tauri::command]
fn game_state(detector: tauri::State<'_, Arc<Detector>>) -> Value {
    detector.snapshot()
}

/// What Laurel knows about games (specs game.detection and game.manual-add): the
/// current game, the installed games found per store, the user's own games and the
/// games the user chose not to record.
#[tauri::command]
fn games_overview(detector: tauri::State<'_, Arc<Detector>>, store: tauri::State<'_, Arc<Store>>) -> Value {
    let settings = store.get();
    let catalog = detector.catalog();
    let ignored = |key: &str| settings.ignored_games.iter().any(|g| g.key == key);
    let mut installed: Vec<Value> = catalog
        .installed()
        .iter()
        .map(|g| json!({ "key": g.id.key, "name": g.id.name, "source": g.id.source, "dir": g.dir, "ignored": ignored(&g.id.key) }))
        .collect();
    installed.sort_by_key(|g| g["name"].as_str().unwrap_or_default().to_lowercase());
    let mut v = detector.snapshot();
    v["counts"] = catalog.counts();
    v["installed"] = Value::Array(installed);
    v["custom"] = settings.custom_games.iter().map(|g| json!({ "path": g.path, "name": g.name })).collect();
    v["ignored"] = settings.ignored_games.iter().map(|g| json!({ "key": g.key, "name": g.name })).collect();
    v
}

/// Settings → Auto-clip: the game whose achievements are watched now, and the
/// installed Steam games with whether achievement clips are left out for them.
#[tauri::command]
fn autoclip_status(achievements: tauri::State<'_, Arc<steam::Achievements>>, detector: tauri::State<'_, Arc<Detector>>, store: tauri::State<'_, Arc<Store>>) -> Value {
    let excluded = store.get().steam_excluded;
    let mut games: Vec<Value> = detector
        .catalog()
        .installed()
        .iter()
        .filter(|g| g.id.source == "steam")
        .map(|g| json!({ "key": g.id.key, "name": g.id.name, "excluded": excluded.iter().any(|e| e.key == g.id.key) }))
        .collect();
    games.sort_by_key(|g| g["name"].as_str().unwrap_or_default().to_lowercase());
    let mut v = achievements.status();
    v["steamGames"] = Value::Array(games);
    v
}

/// Running programs with a window that are not games, to add one by hand.
#[tauri::command]
fn games_running(detector: tauri::State<'_, Arc<Detector>>) -> Vec<Value> {
    detect::running_programs(&detector)
}

fn change_games(app: &AppHandle, what: &str, edit: impl FnOnce(&mut Settings)) -> Result<Value, String> {
    let store = app.state::<Arc<Store>>();
    let mut settings = store.get();
    edit(&mut settings);
    store.set(settings).map_err(|e| e.to_string())?;
    log!("games: {what}");
    app.state::<Arc<Detector>>().wake();
    Ok(games_overview(app.state(), app.state()))
}

/// Marks a program as a game (spec game.manual-add); its name defaults to the file name.
#[tauri::command]
fn games_add(app: AppHandle, path: String, name: Option<String>) -> Result<Value, String> {
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| {
        std::path::Path::new(&path).file_stem().map_or_else(|| path.clone(), |s| s.to_string_lossy().into_owned())
    });
    change_games(&app, &format!("added {name} ({path})"), |s| {
        s.custom_games.retain(|g| g.path.to_lowercase() != path.to_lowercase());
        s.custom_games.push(CustomGame { path: path.clone(), name: name.clone() });
    })
}

#[tauri::command]
fn games_remove(app: AppHandle, path: String) -> Result<Value, String> {
    change_games(&app, &format!("removed {path}"), |s| s.custom_games.retain(|g| g.path.to_lowercase() != path.to_lowercase()))
}

/// Stops or resumes recording a detected game.
#[tauri::command]
fn games_set_ignored(app: AppHandle, key: String, name: String, ignored: bool) -> Result<Value, String> {
    change_games(&app, &format!("{} {name} [{key}]", if ignored { "ignoring" } else { "recording" }), |s| {
        s.ignored_games.retain(|g| g.key != key);
        if ignored {
            s.ignored_games.push(IgnoredGame { key: key.clone(), name: name.clone() });
        }
    })
}

/// Switches the capture folder (spec storage.location): the folder must be
/// writable; existing clips and screenshots move with it when `move_items` is set.
/// The engine restarts to save into the new folder.
#[tauri::command]
async fn change_folder(app: AppHandle, folder: String, move_items: bool) -> Result<Value, String> {
    let task = app.clone();
    let summary = tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let (store, storage, library) = (task.state::<Arc<Store>>(), task.state::<Arc<Storage>>(), task.state::<Arc<Library>>());
        let new = PathBuf::from(folder.trim());
        if !new.is_absolute() {
            return Err(t!("Choose a full folder path"));
        }
        Storage::check_writable(&new)?;
        let old = storage.folder();
        let (moved, failed) = if move_items && old != new { Storage::move_items(&library, &old, &new) } else { (0, Vec::new()) };
        let mut settings = store.get();
        settings.folder = new.display().to_string();
        store.set(settings).map_err(|e| e.to_string())?;
        let (clips, shots) = (storage.clips_dir(), storage.screenshots_dir());
        for dir in [&clips, &shots] {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            task.asset_protocol_scope().allow_directory(dir, true).map_err(|e| e.to_string())?;
        }
        library.set_dirs(clips, shots);
        let trash = library.ensure_trash()?;
        task.asset_protocol_scope().allow_directory(&trash, true).map_err(|e| e.to_string())?;
        library.scan()?;
        task.state::<Engine>().restart();
        log!("capture folder: {} -> {} ({moved} moved, {} failed)", old.display(), new.display(), failed.len());
        Ok(json!({ "moved": moved, "failed": failed }))
    })
    .await
    .map_err(|e| e.to_string())??;
    let _ = app.emit("library-changed", Value::Null);
    Ok(summary)
}

/// Tells the windows that the library and the room it takes changed.
fn library_changed(app: &AppHandle) {
    let _ = app.emit("library-changed", Value::Null);
    let _ = app.emit("storage-state", app.state::<Arc<Storage>>().state(&app.state::<Arc<Library>>()));
}

/// Runs `action` on each path off the main thread: the results that worked, and
/// the paths that failed with why.
async fn for_each_item(paths: Vec<String>, action: impl Fn(&str) -> Result<Value, String> + Send + 'static) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (mut items, mut failed) = (Vec::new(), Vec::new());
        for path in paths {
            match action(&path) {
                Ok(item) => items.push(item),
                Err(e) => {
                    log!("{path}: {e}");
                    failed.push(format!("{path}: {e}"));
                }
            }
        }
        json!({ "items": items, "failed": failed })
    })
    .await
    .map_err(|e| e.to_string())
}

/// Marks items to keep, or not (spec storage.limits).
#[tauri::command]
async fn library_keep(app: AppHandle, paths: Vec<String>, kept: bool) -> Result<Value, String> {
    let library = app.state::<Arc<Library>>().inner().clone();
    let result = for_each_item(paths, move |path| library.set_kept(path, kept)).await?;
    let _ = app.emit("library-changed", Value::Null);
    Ok(result)
}

/// Shows a sample notice of the given kind, with its sound.
#[tauri::command]
fn preview_notice(app: AppHandle, kind: String) {
    let notifier = app.state::<Notifier>();
    match kind.as_str() {
        "screenshot" => notifier.notify(Kind::Screenshot, &t!("Screenshot saved"), &t!("Saved to your screenshots")),
        "error" => notifier.notify(Kind::Error, &t!("Clip not saved"), &t!("Disk almost full: free up space to keep recording")),
        _ => notifier.notify(Kind::Clip, &t!("Clip saved"), &t!("Last {secs} s", secs = app.state::<Arc<Store>>().get().clip_secs)),
    }
}

#[tauri::command]
async fn library_list(library: tauri::State<'_, Arc<Library>>, kind: Option<String>, game: Option<String>, deleted: Option<bool>) -> Result<Vec<Value>, String> {
    let library = library.inner().clone();
    tauri::async_runtime::spawn_blocking(move || if deleted == Some(true) { library.list_deleted() } else { library.list(kind.as_deref(), game.as_deref()) })
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn library_games(library: tauri::State<'_, Arc<Library>>, kind: Option<String>) -> Result<Vec<String>, String> {
    library.games(kind.as_deref())
}

#[tauri::command]
async fn library_rescan(library: tauri::State<'_, Arc<Library>>) -> Result<Value, String> {
    let library = library.inner().clone();
    let (added, removed) = tauri::async_runtime::spawn_blocking(move || library.scan()).await.map_err(|e| e.to_string())??;
    Ok(json!({ "added": added, "removed": removed }))
}

#[tauri::command]
async fn library_trim(app: AppHandle, path: String, start: f64, end: f64, exact: Option<bool>) -> Result<Value, String> {
    let library = app.state::<Arc<Library>>().inner().clone();
    let exact = exact.unwrap_or(false);
    let item = tauri::async_runtime::spawn_blocking(move || library.trim(&path, start, end, exact)).await.map_err(|e| e.to_string())??;
    log!("trimmed clip: {item}");
    let _ = app.emit("library-changed", item.clone());
    Ok(item)
}

/// What the trim needs from a clip (spec edit.trim): its frame and keyframe times
/// and the small frames of the timeline.
#[tauri::command]
async fn library_trim_info(app: AppHandle, path: String) -> Result<Value, String> {
    let library = app.state::<Arc<Library>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let video = library.video_frames(&path).unwrap_or_default();
        let frames = library.timeline(&path, 12).unwrap_or_else(|e| {
            log!("trim timeline of {path}: {e}");
            vec![]
        });
        json!({ "keyframes": video.keyframes, "frameTimes": video.times, "frames": frames })
    })
    .await
    .map_err(|e| e.to_string())
}

/// Deletes a clip or screenshot from the viewer: the file goes to the Recycle Bin.
#[tauri::command]
async fn library_delete(app: AppHandle, paths: Vec<String>) -> Result<Value, String> {
    let library = app.state::<Arc<Library>>().inner().clone();
    let result = for_each_item(paths, move |path| library.delete(path)).await?;
    log!("deleted to Recently deleted: {}", result["items"].as_array().map_or(0, Vec::len));
    library_changed(&app);
    Ok(result)
}

/// Brings items back from Recently deleted (spec library.trash).
#[tauri::command]
async fn library_restore(app: AppHandle, paths: Vec<String>) -> Result<Value, String> {
    let library = app.state::<Arc<Library>>().inner().clone();
    let result = for_each_item(paths, move |path| library.restore(path)).await?;
    log!("restored from Recently deleted: {}", result["items"].as_array().map_or(0, Vec::len));
    library_changed(&app);
    Ok(result)
}

/// Removes items from Recently deleted for good, or all of them without `paths`:
/// to the Recycle Bin when the storage setting says so.
#[tauri::command]
async fn library_purge(app: AppHandle, paths: Option<Vec<String>>) -> Result<Value, String> {
    let library = app.state::<Arc<Library>>().inner().clone();
    let recycle = app.state::<Arc<Store>>().get().recycle;
    let paths = match paths {
        Some(paths) => paths,
        None => library.list_deleted()?.iter().filter_map(|i| i["path"].as_str().map(String::from)).collect(),
    };
    let result = for_each_item(paths, move |path| library.purge(path, recycle).map(|()| json!(path))).await?;
    log!("removed for good from Recently deleted: {}", result["items"].as_array().map_or(0, Vec::len));
    library_changed(&app);
    Ok(result)
}

/// Removes for good the deleted items whose days are up (spec library.trash), now
/// and every ten minutes.
fn watch_deleted(app: AppHandle) {
    std::thread::spawn(move || loop {
        let settings = app.state::<Arc<Store>>().get();
        let removed = app.state::<Arc<Library>>().expire(settings.deleted_days, settings.recycle);
        if !removed.is_empty() {
            log!("recently deleted: {} item(s) past {} day(s) removed", removed.len(), settings.deleted_days);
            library_changed(&app);
        }
        std::thread::sleep(Duration::from_secs(600));
    });
}

#[tauri::command]
fn share_action(action: String, path: String) -> Result<(), String> {
    match action.as_str() {
        "copy-file" => share::copy_file(&path),
        "copy-path" => share::copy_text(&share::windows_path(&path)),
        "show" => share::show_in_folder(&path),
        "open" => share::open(&path),
        other => Err(format!("unknown action {other}")),
    }
}

/// Where Laurel's source code is published, with each release's GPL sources.
const SOURCE_URL: &str = "https://github.com/GabriPires/laurel";

/// The license texts the About page shows (the GPL's appropriate legal notices):
/// Laurel's license and the third-party notices built into the app, then the notices
/// that tools/obs-runtime.py and tools/ffmpeg-build.py stage in the runtime's licenses
/// folder, next to the engine.
#[tauri::command]
fn licenses() -> Value {
    let mut docs = vec![
        json!({ "title": t!("Laurel: GNU GPL, version 2"), "text": include_str!("../../LICENSE") }),
        json!({ "title": t!("Third-party notices"), "text": include_str!("../../THIRD-PARTY-NOTICES.md") }),
    ];
    let engine = engine_exe();
    let folder = engine.parent().and_then(Path::parent).and_then(Path::parent).map(|runtime| runtime.join("licenses"));
    for (file, title) in [("OBS-RUNTIME.txt", t!("OBS Studio runtime")), ("FFMPEG.txt", "FFmpeg".into()), ("gplv3.txt", t!("GNU GPL, version 3"))] {
        if let Some(text) = folder.as_ref().and_then(|f| std::fs::read_to_string(f.join(file)).ok()) {
            docs.push(json!({ "title": title, "text": text }));
        }
    }
    Value::Array(docs)
}

#[tauri::command]
fn open_source_page() -> Result<(), String> {
    share::open_url(SOURCE_URL)
}

/// Whether this build can update itself, and its version.
#[tauri::command]
fn update_info(app: AppHandle) -> Value {
    json!({ "configured": updates::configured(&app), "current": app.package_info().version.to_string() })
}

#[tauri::command]
async fn update_check(app: AppHandle) -> Result<Value, String> {
    updates::check(&app).await
}

#[tauri::command]
async fn update_install(app: AppHandle) -> Result<(), String> {
    updates::install(&app).await
}

/// Opens the logs folder, for bug reports.
#[tauri::command]
fn open_logs() -> Result<(), String> {
    share::open(&log::dir().ok_or("the logs folder is not set")?.display().to_string())
}

/// Settings → Advanced → Save logs to send: one .zip in Downloads with every log
/// and an info.txt (versions, Windows, the engine, the settings, the game being
/// recorded), the Windows user name hidden; Explorer opens on it.
#[tauri::command]
async fn logs_export(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dest = export_logs(&app, &win::downloads_folder().ok_or("no Downloads folder")?)?;
        let _ = share::show_in_folder(&dest.to_string_lossy());
        Ok(dest.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn export_logs(app: &AppHandle, dir: &std::path::Path) -> Result<PathBuf, String> {
    let (date, time) = win::date_time();
    let dest = storage::unique(dir, &format!("Laurel logs {date} {time}"), "zip");
    let windows = |name: &str| win::reg_string(win::HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", name).unwrap_or_default();
    let info = format!(
        "Laurel {}\nWindows: {} {} (build {})\nSaved: {date} {time}\nAdministrator: {}\n\nEngine:\n{}\n\nGame:\n{}\n\nSteam achievements:\n{}\n\nSettings:\n{}\n",
        app.package_info().version,
        windows("ProductName"),
        windows("DisplayVersion"),
        windows("CurrentBuild"),
        win::self_elevated(),
        serde_json::to_string_pretty(&app.state::<Engine>().state()).unwrap_or_default(),
        serde_json::to_string_pretty(&app.state::<Arc<Detector>>().snapshot()).unwrap_or_default(),
        serde_json::to_string_pretty(&app.state::<Arc<steam::Achievements>>().status()).unwrap_or_default(),
        serde_json::to_string_pretty(&app.state::<Arc<Store>>().get().to_json()).unwrap_or_default(),
    );
    let files = log::export(&dest, &[("info.txt", info)])?;
    log!("logs saved to send: {} ({files} files)", dest.display());
    Ok(dest)
}

/// The view the main window opens in (development: LAUREL_START_VIEW).
#[tauri::command]
fn start_view() -> Option<String> {
    std::env::var("LAUREL_START_VIEW").ok()
}

#[tauri::command]
fn save_clip_now(app: AppHandle) {
    save_clip(&app, "the title bar button");
}

#[tauri::command]
fn screenshot_now(app: AppHandle) {
    take_screenshot(&app, "the title bar button");
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", t!("Open Laurel"), true, None::<&str>)?;
    let settings = app.state::<Arc<Store>>().get();
    let toggle = CheckMenuItem::with_id(app, "toggle", format!("{}{}", t!("Recording"), hotkey_suffix(&settings, "toggle")), true, settings.recording_enabled, None::<&str>)?;
    let clip = MenuItem::with_id(app, "clip", format!("{}{}", t!("Save clip"), hotkey_suffix(&settings, "clip")), true, None::<&str>)?;
    let shot = MenuItem::with_id(app, "shot", format!("{}{}", t!("Screenshot"), hotkey_suffix(&settings, "screenshot")), true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", t!("Quit"), true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &toggle, &clip, &shot, &PredefinedMenuItem::separator(app)?, &quit_item])?;
    let mut tray = TrayIconBuilder::with_id("main").tooltip("Laurel").menu(&menu).show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.on_menu_event(|app, event| match event.id.as_ref() {
        "open" => show_main(app),
        "toggle" => {
            let enabled = app.state::<Arc<Detector>>().enabled();
            let app = app.clone();
            std::thread::spawn(move || set_recording_enabled(&app, !enabled, false));
        }
        "clip" => save_clip(app, "the tray menu"),
        "shot" => take_screenshot(app, "the tray menu"),
        "quit" => quit(app),
        _ => {}
    })
    .on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
            show_main(tray.app_handle());
        }
    })
    .build(app)?;
    app.manage(TrayItems { open, toggle, clip, shot, quit: quit_item });
    Ok(())
}

/// The recorder state the tray icon shows, with its tooltip: recording (jade),
/// waiting for a game or starting (gray), recording off (yellow), or something that
/// needs the player (red: engine down, disk almost full, game needs administrator).
fn tray_state(app: &AppHandle) -> (&'static str, String) {
    let engine = app.state::<Engine>().state();
    let game = app.state::<Arc<Detector>>().snapshot();
    let name = game["game"]["name"].as_str().map_or_else(|| t!("the game"), String::from);
    match engine["state"].as_str().unwrap_or_default() {
        "failed" | "stopped" => return ("attention", t!("Laurel: the recording engine stopped")),
        "starting" if game["enabled"] != false => return ("waiting", t!("Laurel: starting")),
        _ => {}
    }
    if game["enabled"] == false {
        ("off", t!("Laurel: recording is off"))
    } else if app.state::<Arc<Storage>>().is_paused() {
        ("attention", t!("Laurel: paused, the disk is almost full"))
    } else if game["needsAdmin"] == true {
        ("attention", t!("Laurel: {name} needs administrator", name = name))
    } else if game["recording"] == true && game["capture"] == "screen" {
        ("recording", t!("Laurel: recording the screen"))
    } else if game["recording"] == true && !game["game"].is_null() {
        ("recording", t!("Laurel: recording {name}", name = name))
    } else if game["recording"] == true && game["mode"] == "fixed" {
        ("recording", t!("Laurel: recording"))
    } else {
        ("waiting", t!("Laurel: waiting for a game"))
    }
}

/// Keeps the tray icon on the recorder state, in the set for the taskbar theme
/// (light or dark) and at the system's small icon size.
fn watch_tray(app: AppHandle) {
    std::thread::spawn(move || {
        let mut shown = None;
        loop {
            let (state, tooltip) = tray_state(&app);
            let theme = if win::taskbar_light() { "light" } else { "dark" };
            let want = win::small_icon_size();
            let size = tray_icons::SIZES.iter().copied().find(|s| *s >= want).unwrap_or(48);
            let now = (state, theme, size, tooltip.clone());
            if shown.as_ref() != Some(&now) {
                if let (Some(tray), Some(png)) = (app.tray_by_id("main"), tray_icons::png(theme, state, size)) {
                    match tauri::image::Image::from_bytes(png) {
                        Ok(icon) => {
                            let _ = tray.set_icon(Some(icon));
                            let _ = tray.set_tooltip(Some(&tooltip));
                            log!("tray: {state} ({theme} taskbar, {size} px)");
                        }
                        Err(e) => log!("tray icon {theme}-{state}-{size}: {e}"),
                    }
                }
                shown = Some(now);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

/// Scripted end-to-end check for development: clip, screenshot, engine crash and
/// restart, clip again. Writes "selftest: PASS" or "selftest: FAIL ..." to app.log.
fn selftest(app: AppHandle) {
    let engine = app.state::<Engine>().inner().clone();
    let wait_ready = |what: &str| {
        let deadline = Instant::now() + Duration::from_secs(60);
        while engine.state()["state"] != "ready" {
            if Instant::now() > deadline {
                return Err(format!("engine not ready {what}"));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    };
    let clip = || -> Result<Value, String> {
        let r = engine.request("saveClip", Value::Null, CLIP_TIMEOUT)?;
        for key in ["path", "thumbnail"] {
            let path = r[key].as_str().ok_or(format!("saveClip without {key}"))?;
            if !std::path::Path::new(path).exists() {
                return Err(format!("{key} missing on disk: {path}"));
            }
        }
        log!("[selftest] clip ok: {r}");
        Ok(r)
    };
    // Notices on, sounds off while the self-test runs (it kills the engine on purpose);
    // the user's settings are restored at the end.
    let notifier = app.state::<Notifier>().inner().clone();
    let user_settings = notifier.settings();
    let _ = notifier.set_settings(Settings { overlay: true, sounds: false, ..user_settings.clone() });
    // The checks read the English texts.
    i18n::set("en");
    let run = || -> Result<(), String> {
        wait_ready("at start")?;
        check_log_rotation()?;
        check_single_instance(&app)?;
        check_language()?;
        std::thread::sleep(Duration::from_secs(8)); // let the "Recording" notice expire
        let first = clip()?;
        check_library(&app, &first)?;
        check_storage(&app, &engine)?;
        check_overlay(&engine, &notifier)?;
        check_autoclip(&app)?;
        log!("[selftest] screenshot ok: {}", engine.request("screenshot", Value::Null, SHOT_TIMEOUT)?);
        check_video(&app, &engine, &wait_ready)?;
        check_audio(&app, &engine)?;
        log!("[selftest] targets: {}", engine.request("listTargets", Value::Null, SHOT_TIMEOUT)?);
        log!("[selftest] status: {}", engine.request("status", Value::Null, SHOT_TIMEOUT)?);
        log!("[selftest] killing the engine");
        engine.kill();
        std::thread::sleep(Duration::from_millis(500));
        wait_ready("after the restart")?;
        std::thread::sleep(Duration::from_secs(4));
        clip()?;
        check_games(&app, &engine)?;
        check_hotkeys(&app)?;
        check_toggle_and_cursor(&app, &engine)
    };
    let outcome = run();
    i18n::set(&user_settings.language);
    let _ = notifier.set_settings(user_settings);
    match outcome {
        Ok(()) => log!("selftest: PASS"),
        Err(e) => log!("selftest: FAIL {e}"),
    }
    quit(&app);
}

/// Self-test of the interface language: the Portuguese strings load, a count picks
/// its form, fields are filled in, and English comes back.
fn check_language() -> Result<(), String> {
    i18n::set("pt-BR");
    let saved = t!("Clip saved");
    let last = t!("Last {secs} s", secs = 30);
    let items = (i18n::tr_plural(1, "{n} item", "{n} items", &[]), i18n::tr_plural(3, "{n} item", "{n} items", &[]));
    let script = i18n::script();
    i18n::set("en");
    let english = t!("Clip saved");
    log!("[selftest] language: {saved:?}, {last:?}, {items:?}; window script {} bytes; back to English: {english:?}", script.len());
    if saved == "Clip saved" || last.contains("{secs}") || items.0 == items.1 || english != "Clip saved" || !script.contains("\"pt-BR\"") {
        return Err("the interface language did not behave".into());
    }
    Ok(())
}

/// Self-test of game detection (specs game.detection, game.manual-add): a program
/// inside an installed game's folder is that game, a crash reporter there and a
/// system program are not, and an ignored game is ignored. Then, with detection on,
/// a PowerShell window added as a game is found, recording runs, a clip carries the
/// game's name, and recording pauses after the window closes.
fn check_games(app: &AppHandle, engine: &Engine) -> Result<(), String> {
    use games::Verdict;
    use std::os::windows::process::CommandExt;
    let detector = app.state::<Arc<Detector>>().inner().clone();
    let store = app.state::<Arc<Store>>().inner().clone();
    let settings = store.get();
    {
        let catalog = detector.catalog();
        log!("[selftest] installed games: {} {}", catalog.installed().len(), catalog.counts());
        if let Some(game) = catalog.installed().first() {
            let inside = game.dir.join(r"Game\Binaries\Win64\Game-Win64-Shipping.exe").to_string_lossy().into_owned();
            let helper = game.dir.join("UnityCrashHandler64.exe").to_string_lossy().into_owned();
            let ignoring = Settings { ignored_games: vec![IgnoredGame { key: game.id.key.clone(), name: game.id.name.clone() }], ..settings.clone() };
            let verdicts = (catalog.classify(&inside, &settings), catalog.classify(&helper, &settings), catalog.classify(&inside, &ignoring));
            log!("[selftest] classify in {}: {:?}", game.id.name, verdicts);
            if verdicts != (Verdict::Game(game.id.clone()), Verdict::NotGame, Verdict::Ignored(game.id.clone())) {
                return Err("library classification failed".into());
            }
        } else {
            log!("[selftest] no installed games; library classification not exercised");
        }
        let system = catalog.classify(r"C:\Windows\explorer.exe", &settings);
        if system != Verdict::NotGame {
            return Err(format!("explorer.exe classified as {system:?}"));
        }
    }

    // With a real game open, detection would pick it: the live check is skipped.
    let open = detector.games_open();
    if !open.is_empty() {
        log!("[selftest] games open ({open:?}): live detection check skipped");
        return Ok(());
    }
    let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let shell = root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let name = "Laurel self-test game";
    let mut custom = settings.custom_games.clone();
    custom.push(CustomGame { path: shell.to_string_lossy().into_owned(), name: name.into() });
    store.set(Settings { custom_games: custom, ..settings.clone() }).map_err(|e| e.to_string())?;
    detector.set_mode(Mode::Detect);
    // A window off every monitor that does not take the focus, so the test does not
    // disturb whoever uses the PC.
    let script = "Add-Type -AssemblyName System.Windows.Forms, System.Drawing; \
        Add-Type -ReferencedAssemblies System.Windows.Forms -TypeDefinition 'public class QuietForm : System.Windows.Forms.Form { protected override bool ShowWithoutActivation { get { return true; } } }'; \
        $f = New-Object QuietForm; $f.Text = 'Laurel self-test game'; $f.StartPosition = 'Manual'; \
        $f.Location = New-Object System.Drawing.Point(-6000, -6000); $f.Size = New-Object System.Drawing.Size(960, 600); \
        $t = New-Object System.Windows.Forms.Timer; $t.Interval = 60000; $t.Add_Tick({ $f.Close() }); $t.Start(); \
        [System.Windows.Forms.Application]::Run($f)";
    let outcome = (|| -> Result<(), String> {
        let status = |key: &str| engine.request("status", Value::Null, SHOT_TIMEOUT).map(|s| s[key].clone());
        let until = |what: &str, done: &dyn Fn() -> bool| -> Result<(), String> {
            let deadline = Instant::now() + Duration::from_secs(25);
            while !done() {
                if Instant::now() > deadline {
                    return Err(format!("timed out waiting for {what}"));
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            Ok(())
        };
        // Stopping the replay buffer completes asynchronously.
        until("recording to stop without a game", &|| status("recording").is_ok_and(|r| r == false))?;
        let mut child = std::process::Command::new(&shell).args(["-NoProfile", "-Command", script]).creation_flags(0x0800_0000).spawn().map_err(|e| e.to_string())?;
        let found = until("the game", &|| detector.current().is_some_and(|g| g.id.name == name));
        let result = found.and_then(|_| {
            until("recording", &|| status("recording").is_ok_and(|r| r == true))?;
            log!("[selftest] game detected: {}", detector.snapshot());
            std::thread::sleep(Duration::from_secs(4));
            let mut saved = engine.request("saveClip", Value::Null, CLIP_TIMEOUT)?;
            attach_game(app, &mut saved);
            finish_saved(app, &mut saved);
            log!("[selftest] game clip: {saved}");
            let path = saved["path"].as_str().unwrap_or_default();
            if !path.contains(name) || !std::path::Path::new(path).exists() {
                return Err(format!("the clip is not named after the game: {path}"));
            }
            // The PowerShell window has no swap chain, so it is never hooked; it is
            // not elevated either, so no restart as administrator may be offered,
            // and after 15 s its window is recorded instead (spec capture.game-hook).
            until("the hook check", &|| detector.hook_checked())?;
            let snapshot = detector.snapshot();
            let target = status("target")?;
            log!("[selftest] hook check: {snapshot}; engine target {target}; Laurel elevated: {}, this process elevated: {:?}", win::self_elevated(), win::process_elevated(std::process::id()));
            if snapshot["needsAdmin"] != false || win::process_elevated(std::process::id()) != Some(win::self_elevated()) {
                return Err("the elevation check went wrong".into());
            }
            if snapshot["capture"] != "window" || !target.as_str().unwrap_or_default().starts_with("window:") {
                return Err("the window capture fallback did not start".into());
            }
            // No hook reports a game recorded by its window, and the game audio
            // still has to follow its process (spec audio.per-app).
            let follows = status("audio")?["follows"].clone();
            log!("[selftest] game audio follows {follows}");
            if !follows.as_str().is_some_and(|exe| exe.eq_ignore_ascii_case("powershell.exe")) {
                return Err(format!("the game audio does not follow the game's window: {follows}"));
            }
            Ok(())
        });
        let _ = child.kill();
        let _ = child.wait();
        result?;
        until("the game to close", &|| detector.current().is_none())?;
        until("recording to pause", &|| status("recording").is_ok_and(|r| r == false))?;
        log!("[selftest] game closed: {}", detector.snapshot());

        // Screen recording (spec capture.screen): with no game open, the primary
        // monitor is recorded; turned off, nothing is captured again.
        detector.set_screen(Some(detect::Screen { monitor: String::new(), switch_to_games: true }));
        until("screen recording", &|| status("recording").is_ok_and(|r| r == true) && status("target").is_ok_and(|t| t.as_str().unwrap_or_default().starts_with("monitor-id:")))?;
        std::thread::sleep(Duration::from_secs(2));
        let size = status("captureSize")?;
        log!("[selftest] screen recording: {}; capture {size}", detector.snapshot());
        // A screen clip is named "Screen …" and listed under Screen.
        let mut saved = engine.request("saveClip", Value::Null, CLIP_TIMEOUT)?;
        attach_game(app, &mut saved);
        finish_saved(app, &mut saved);
        let path = saved["path"].as_str().unwrap_or_default().to_string();
        log!("[selftest] screen clip: {path}");
        if !fileish_name(&path).starts_with("Screen ") {
            detector.set_screen(None);
            return Err(format!("the screen clip is not named Screen: {path}"));
        }
        detector.set_screen(None);
        until("screen recording to stop", &|| status("recording").is_ok_and(|r| r == false))?;
        if size[0].as_u64().unwrap_or_default() == 0 {
            return Err("screen recording captured nothing".into());
        }
        Ok(())
    })();
    detector.set_screen(detect::screen_of(&settings));
    store.set(settings).map_err(|e| e.to_string())?;
    detector.set_mode(Mode::Fixed("none".into()));
    outcome
}

/// Self-test of the hotkeys (spec clip.hotkeys): the text form round-trips; invalid
/// combinations are refused with a reason; a hotkey typed on the keyboard fires
/// through Raw Input; recording a new hotkey saves it, refuses one that another
/// action uses, and can be cancelled. The typed keys are F13 and F14, which
/// applications do not use; they go to whatever window has the focus.
fn check_hotkeys(app: &AppHandle) -> Result<(), String> {
    const VK_CONTROL: u16 = 0x11;
    const VK_F13: u16 = 0x7C;
    const VK_F14: u16 = 0x7D;
    let hotkeys = app.try_state::<Hotkeys>().ok_or("hotkeys unavailable")?;
    for text in ["F8", "Shift+F8", "Ctrl+Alt+K", "Mouse4", "Ctrl+Mouse5", "Numpad5", "NumpadEnter", "Ctrl+Oem_BA"] {
        let combo = Combo::parse(text).ok_or(format!("{text} does not parse"))?;
        if combo.name() != text {
            return Err(format!("{text} came back as {}", combo.name()));
        }
    }
    let verdict = |text: &str| Combo::parse(text).map(|c| hotkeys::reject_reason(&c).is_none());
    let refused = ["K", "Shift+K", "Space", "Up", "Win+F8", "Esc", "Alt+F4", "Alt+Tab", "Mouse1"];
    let allowed = ["F9", "Ctrl+K", "Alt+K", "Mouse4", "Ctrl+Shift+S", "Insert", "Numpad0"];
    let wrong: Vec<&str> = refused.iter().filter(|t| verdict(t) != Some(false)).chain(allowed.iter().filter(|t| verdict(t) != Some(true))).copied().collect();
    log!("[selftest] hotkey rules: {} refused, {} allowed, wrong: {wrong:?}; Ctrl+Oem_BA shows as {}", refused.len(), allowed.len(), Combo::parse("Ctrl+Oem_BA").map(|c| c.label()).unwrap_or_default());
    if !wrong.is_empty() {
        return Err(format!("hotkey rules wrong for {wrong:?}"));
    }

    let store = app.state::<Arc<Store>>().inner().clone();
    let saved = store.get();
    let wait = |what: &str, done: &dyn Fn() -> bool| -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !done() {
            if Instant::now() > deadline {
                return Err(format!("timed out waiting for {what}"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    };
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let listener = app.listen("hotkey-rejected", move |e| {
        let _ = tx.send(e.payload().to_string());
    });
    let outcome = (|| -> Result<(), String> {
        set_hotkeys(app, |s| s.hotkey_clip = "F14".into());
        let before = hotkeys.presses();
        win::send_keys(&[(VK_F14, false), (VK_F14, true)]);
        wait("F14 to fire", &|| hotkeys.presses() == before + 1)?;
        log!("[selftest] typed F14 fired the clip hotkey");

        hotkeys.capture("screenshot");
        win::send_keys(&[(VK_CONTROL, false), (VK_F13, false), (VK_F13, true), (VK_CONTROL, true)]);
        wait("Ctrl+F13 to be recorded", &|| store.get().hotkey_screenshot == "Ctrl+F13")?;
        hotkeys.capture("clip");
        win::send_keys(&[(VK_CONTROL, false), (VK_F13, false), (VK_F13, true), (VK_CONTROL, true)]);
        let rejected = rx.recv_timeout(Duration::from_secs(3)).map_err(|_| "a hotkey used by another action was not refused".to_string())?;
        hotkeys.cancel_capture();
        log!("[selftest] recorded Ctrl+F13 for screenshots; the same for clips was refused: {rejected}; capture cancelled: {}", hotkeys.capturing().is_none());
        if !rejected.contains("Already used for Screenshot") || hotkeys.capturing().is_some() || store.get().hotkey_clip != "F14" {
            return Err("recording a hotkey did not behave".into());
        }
        Ok(())
    })();
    app.unlisten(listener);
    hotkeys.cancel_capture();
    set_hotkeys(app, |s| {
        s.hotkey_clip = saved.hotkey_clip;
        s.hotkey_screenshot = saved.hotkey_screenshot;
        s.hotkey_toggle = saved.hotkey_toggle;
    });
    outcome
}

/// Self-test of recording on/off (spec record.toggle) and of the cursor option
/// (capture.cursor): a recording on/off hotkey (F16, typed) turns recording off,
/// which stops the replay buffer and drops the capture, and on again; the engine
/// hides and shows the cursor without restarting the capture.
fn check_toggle_and_cursor(app: &AppHandle, engine: &Engine) -> Result<(), String> {
    const VK_F16: u16 = 0x7F;
    let detector = app.state::<Arc<Detector>>().inner().clone();
    let store = app.state::<Arc<Store>>().inner().clone();
    let saved = store.get();
    let status = || engine.request("status", Value::Null, SHOT_TIMEOUT);
    let until = |what: &str, done: &dyn Fn() -> bool| -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            if Instant::now() > deadline {
                return Err(format!("timed out waiting for {what}"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    };
    let outcome = (|| -> Result<(), String> {
        set_hotkeys(app, |s| s.hotkey_toggle = "F16".into());
        win::send_keys(&[(VK_F16, false), (VK_F16, true)]);
        until("recording to turn off", &|| !detector.enabled())?;
        until("the replay buffer to stop", &|| status().is_ok_and(|s| s["recording"] == false))?;
        let off = status()?;
        let tray_off = tray_state(app);
        log!("[selftest] F16 turned recording off: target {}, recording {}, saved setting {}; tray {:?}", off["target"], off["recording"], store.get().recording_enabled, tray_off);
        if off["target"] != "none" || store.get().recording_enabled || tray_off.0 != "off" {
            return Err("turning recording off kept the capture".into());
        }
        win::send_keys(&[(VK_F16, false), (VK_F16, true)]);
        until("recording to turn on", &|| detector.enabled())?;
        until("the replay buffer to start", &|| status().is_ok_and(|s| s["recording"] == true))?;
        let tray_on = tray_state(app);
        log!("[selftest] F16 turned recording on: target {}; tray {:?}; taskbar light {}, small icon {} px", status()?["target"], tray_on, win::taskbar_light(), win::small_icon_size());
        if tray_on.0 == "off" {
            return Err(format!("the tray still shows recording off: {tray_on:?}"));
        }

        let before = status()?["totalFrames"].as_u64().unwrap_or_default();
        let hidden = engine.request("setCursor", json!({ "enabled": false }), SHOT_TIMEOUT)?;
        let shown = engine.request("setCursor", json!({ "enabled": true }), SHOT_TIMEOUT)?;
        std::thread::sleep(Duration::from_millis(300));
        let after = status()?;
        log!("[selftest] cursor: hidden {hidden}, shown {shown}; frames kept going: {before} -> {}", after["totalFrames"]);
        if hidden["cursor"] != false || shown["cursor"] != true || after["totalFrames"].as_u64().unwrap_or_default() <= before {
            return Err("the cursor switch failed".into());
        }
        Ok(())
    })();
    set_hotkeys(app, |s| s.hotkey_toggle = saved.hotkey_toggle.clone());
    if detector.enabled() != saved.recording_enabled {
        set_recording_enabled(app, saved.recording_enabled, false);
    }
    outcome
}

/// Self-test of the log rotation: a log over its limit moves to `.old.log` and starts
/// again, so it stays within twice the limit, and a fresh open keeps the previous one.
/// Self-test of the single instance: a second start of the program (as a probe)
/// finds this one running and asks it to show its window, which then shows.
fn check_single_instance(app: &AppHandle) -> Result<(), String> {
    let main = app.get_webview_window("main").ok_or("no main window")?;
    let _ = main.hide();
    std::thread::sleep(Duration::from_millis(300));
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let code = std::process::Command::new(exe).arg("--instance-probe").status().map_err(|e| e.to_string())?.code();
    let shown = Instant::now();
    while !main.is_visible().unwrap_or(false) && shown.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(100));
    }
    let visible = main.is_visible().unwrap_or(false);
    log!("[selftest] single instance: a second start exited with {code:?}; this window shown again: {visible}");
    if code != Some(3) || !visible {
        return Err("a second start was not stopped or did not show this window".into());
    }
    Ok(())
}

fn check_log_rotation() -> Result<(), String> {
    let dir = std::env::temp_dir().join("laurel-selftest-logs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("test.log");
    let mut log = log::Rotating::open(&path, 0, 10_000);
    for i in 0..300 {
        log.line(&format!("{i:04} {}", "x".repeat(94)));
    }
    drop(log);
    let size = |p: &std::path::Path| std::fs::metadata(p).map_or(0, |m| m.len());
    let (current, old) = (size(&path), size(&log::Rotating::old_path(&path)));
    let last = std::fs::read_to_string(&path).unwrap_or_default().lines().last().unwrap_or_default().chars().take(4).collect::<String>();
    // Three new sessions keeping two: the first one ends up in .2, the one after in .1.
    drop(log::Rotating::open(&path, 2, 10_000));
    let mut second = log::Rotating::open(&path, 2, 10_000);
    second.line("second session");
    drop(second);
    drop(log::Rotating::open(&path, 2, 10_000));
    let (fresh, one, two, three) = (size(&path), std::fs::read_to_string(log::Rotating::session_path(&path, 1)).unwrap_or_default(), size(&log::Rotating::session_path(&path, 2)), log::Rotating::session_path(&path, 3).exists());
    log!("[selftest] log rotation: 30000 bytes written with a 10000-byte limit left {current} + {old} old, last line {last}; three new sessions keeping two left {fresh} bytes, .1 {:?}, .2 {two} bytes, .3 {three}", one.trim());
    if current > 10_100 || old == 0 || old > 10_100 || last != "0299" || fresh != 0 || one.trim() != "second session" || two != 0 || three {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("the log rotation did not behave".into());
    }

    // The .zip to send: the masked user name, a log, and a readable archive.
    let user = std::env::var("USERNAME").unwrap_or_default();
    let masked = log::mask_user(&format!(r"C:\Users\{user}\Videos and C:\\Users\\{user}\\x and C:/Users/{user}/y and C:\Users\{user}x\z"), &user);
    let wanted = format!(r"C:\Users\{{user}}\Videos and C:\\Users\\{{user}}\\x and C:/Users/{{user}}/y and C:\Users\{user}x\z");
    log!("[selftest] user name masked: {}", masked == wanted || user.len() < 2);
    if user.len() >= 2 && masked != wanted {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("masking gave {masked}"));
    }
    // Left in the temporary folder, to open it with other tools after the run.
    let zip = std::env::temp_dir().join("laurel-selftest-logs.zip");
    let files = log::export(&zip, &[("info.txt", format!(r"work dir C:\Users\{user}\AppData"))])?;
    let bytes = std::fs::read(&zip).map_err(|e| e.to_string())?;
    let entries = bytes.windows(4).filter(|w| *w == [0x50, 0x4b, 0x01, 0x02]).count();
    let end_ok = bytes.len() > 22 && bytes[bytes.len() - 22..bytes.len() - 18] == [0x50, 0x4b, 0x05, 0x06];
    let _ = std::fs::remove_dir_all(&dir);
    log!("[selftest] logs .zip: {files} files, {entries} central entries, end record {end_ok}, {} bytes", bytes.len());
    if files < 2 || entries != files || !end_ok {
        return Err("the logs .zip is not as expected".into());
    }
    Ok(())
}

/// The file name of a path written with either kind of slash.
fn fileish_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Self-test of the library: the saved clip is indexed with its metadata, and a
/// 0.5 to 3.0 s trim gives a new clip of about that length with every audio track.
fn check_library(app: &AppHandle, saved: &Value) -> Result<(), String> {
    let library = app.state::<Arc<Library>>();
    let path = saved["path"].as_str().ok_or("saveClip without path")?;
    // Naming: a game name with characters Windows forbids still gives a valid name.
    // Without a game the pattern gives the name the engine already chose: it stays.
    let same = app.state::<Arc<Storage>>().apply_naming(path, None, library.thumbs_dir());
    log!("[selftest] named without a game: {same}");
    if fileish_name(&same) != fileish_name(path) {
        return Err(format!("a clip without a game was renamed to {same}"));
    }
    let path = app.state::<Arc<Storage>>().apply_naming(path, Some("Laurel self-test: <a/b>?"), library.thumbs_dir());
    log!("[selftest] named: {path}");
    if !std::path::Path::new(&path).exists() || !path.contains("Laurel self-test a b") {
        return Err(format!("naming failed: {path}"));
    }
    let path = path.as_str();
    // Files indexed without a game (found on disk) take it from their name.
    for (stem, game) in [("Overwatch® 2026-10-23 20-55-06 (trim 2)", Some("Overwatch®")), ("Laurel 2026-10-23 23-26-49 (2)", None), ("my clip", None)] {
        if library::game_from_name(stem).as_deref() != game {
            return Err(format!("game from the name {stem:?}: {:?}", library::game_from_name(stem)));
        }
    }
    let item = library.add(path, Some("Laurel self-test".into()))?;
    log!("[selftest] library item: {item}");
    let trimmed = library.trim(path, 0.5, 3.0, false)?;
    log!("[selftest] trimmed: {trimmed}");
    let duration = trimmed["durationMs"].as_i64().unwrap_or_default();
    let tracks = trimmed["audioTracks"].as_i64().unwrap_or_default();
    if !(2000..=4000).contains(&duration) || tracks != item["audioTracks"].as_i64().unwrap_or(-1) {
        return Err(format!("unexpected trim: {duration} ms, {tracks} audio tracks"));
    }
    // The new trim (spec edit.trim): keyframes read from the file, the timeline
    // frames, an exact trim of the chosen length, and a trim of a trim named
    // "(trim N)" rather than "(trim) (trim)".
    // Frame accuracy: frames 30 to 89 (exact) and from the second keyframe to frame
    // 149 (fast) must come out as exactly those frames, the way the viewer snaps
    // start to a frame's time and end to the end of the last frame kept.
    let video = library.video_frames(path)?;
    let frames = library.timeline(path, 12)?;
    let t = &video.times;
    if t.len() < 200 || video.keyframes.len() < 2 {
        return Err(format!("too few frames in the self-test clip: {} frames, {} keyframes", t.len(), video.keyframes.len()));
    }
    let frame = t[1] - t[0];
    let exact = library.trim(path, t[30], t[89] + frame, true)?;
    let key = video.keyframes[1];
    let fast = library.trim(path, key, t[149] + frame, false)?;
    let again = library.trim(trimmed["path"].as_str().unwrap_or_default(), 0.2, 1.5, false)?;
    let count = |v: &Value| library.video_frames(v["path"].as_str().unwrap_or_default()).map(|f| f.times.len()).unwrap_or(0);
    let fast_expected = 150 - t.iter().position(|x| (*x - key).abs() < 0.0001).unwrap_or(0);
    let again_path = again["path"].as_str().unwrap_or_default().to_string();
    log!(
        "[selftest] trim: {} frames, {} keyframes {:?}; {} timeline frames on disk; exact trim of 60 frames gave {} ({} audio tracks); fast trim of {fast_expected} frames gave {}; trim of a trim: {again_path}",
        t.len(),
        video.keyframes.len(),
        video.keyframes.iter().take(3).map(|k| format!("{k:.4}")).collect::<Vec<_>>(),
        frames.iter().filter(|f| f.exists()).count(),
        count(&exact),
        exact["audioTracks"],
        count(&fast)
    );
    if frames.iter().filter(|f| f.exists()).count() != 12 || count(&exact) != 60 || count(&fast) != fast_expected || exact["audioTracks"] != item["audioTracks"] || again_path.contains("(trim) (trim)") {
        return Err("the new trim did not behave".into());
    }
    // Delete (spec library.trash): the file moves to Recently deleted, comes back
    // where it was, and leaves for good, with its thumbnail.
    let exists = |p: &str| std::path::Path::new(p).exists();
    let again_thumb = again["thumbnail"].as_str().unwrap_or_default().to_string();
    let deleted = library.delete(&again_path)?;
    let trashed = deleted["path"].as_str().unwrap_or_default().to_string();
    let listed = |p: &str| -> Result<(bool, bool), String> {
        Ok((library.list(Some("clip"), None)?.iter().any(|i| i["path"] == p), library.list_deleted()?.iter().any(|i| i["path"] == p)))
    };
    let after_delete = (exists(&again_path), exists(&trashed), listed(&trashed)?, deleted["originalPath"] == again_path.as_str());
    let restored = library.restore(&trashed)?;
    let after_restore = (exists(&again_path), exists(&trashed), listed(&again_path)?, restored["deletedMs"].is_null());
    let trashed = library.delete(&again_path)?["path"].as_str().unwrap_or_default().to_string();
    library.purge(&trashed, true)?;
    let after_purge = (exists(&trashed), listed(&trashed)?, !again_thumb.is_empty() && exists(&again_thumb));
    log!("[selftest] deleted {again_path} to {trashed}: {after_delete:?}; restored: {after_restore:?}; removed for good: {after_purge:?}");
    if after_delete != (false, true, (false, true), true) || after_restore != (true, false, (true, false), true) || after_purge != (false, (false, false), false) || !trashed.contains(r"\.deleted\") {
        return Err("delete, restore or removal did not behave".into());
    }
    let listed = library.list(Some("clip"), Some("Laurel self-test"))?;
    log!("[selftest] library lists {} self-test clips", listed.len());
    Ok(())
}

/// Self-test of the video settings (spec quality.encoding): the engine lists the
/// encoders it can use and records with one; the processor encoder at 720p, 30 fps
/// and 15 s, set in the settings, reaches the engine after a restart, with the
/// all-PC clip sound (audio.separate-tracks); then the player's settings come back.
fn check_video(app: &AppHandle, engine: &Engine, wait_ready: &dyn Fn(&str) -> Result<(), String>) -> Result<(), String> {
    let list = engine.request("listEncoders", Value::Null, SHOT_TIMEOUT)?;
    let ids: Vec<&str> = list.as_array().into_iter().flatten().filter_map(|e| e["id"].as_str()).collect();
    let info = engine.state()["info"].clone();
    log!("[selftest] encoders: {ids:?}; recording with {} (fallbacks {})", info["encoder"], info["encoderFallbacks"]);
    if !ids.contains(&"obs_x264") || !ids.iter().any(|id| info["encoder"] == *id) {
        return Err(format!("unexpected encoders: {ids:?}, in use {}", info["encoder"]));
    }
    let store = app.state::<Arc<Store>>();
    let original = store.get();
    let original_mix = original.audio_mix.clone();
    let test = Settings { video_encoder: "x264".into(), video_preset: "performance".into(), video_height: 720, video_fps: 30, clip_secs: 15, audio_mix: "pc-mic".into(), ..original.clone() };
    // A restart is asynchronous: the old engine may still read as ready for a moment,
    // so wait for it to go before waiting for the new one.
    let restart = |what: &str| -> Result<(), String> {
        engine.restart();
        let deadline = Instant::now() + Duration::from_secs(10);
        while engine.state()["state"] == "ready" && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        wait_ready(what)
    };
    store.set(test).map_err(|e| e.to_string())?;
    let result = restart("after the video change").map(|_| engine.state()["info"].clone());
    store.set(original).map_err(|e| e.to_string())?;
    restart("after restoring the video")?;
    let changed = result?;
    let restored = engine.state()["info"].clone();
    log!(
        "[selftest] video settings: encoder {}, output {}, fps {}, buffer {} s, clip sound {}; restored: {}, clip sound {}",
        changed["encoder"], changed["output"], changed["fps"], changed["bufferSecs"], changed["audioMix"], restored["encoder"], restored["audioMix"]
    );
    if changed["encoder"] != "obs_x264" || changed["output"][1] != 720 || changed["fps"] != 30 || changed["bufferSecs"] != 15 || changed["audioMix"] != "pc-mic" {
        return Err("the video settings did not reach the engine".into());
    }
    if restored["audioMix"] != original_mix.as_str() {
        return Err(format!("the clip sound was not restored: {}", restored["audioMix"]));
    }
    Ok(())
}

/// Self-test of the clip sound (spec audio.separate-tracks, D12 revised): the mix
/// changes in the running engine, without a restart; a clip saved with all PC sound
/// keeps its three tracks; a bad mix is refused; then the player's mix comes back.
fn check_audio(app: &AppHandle, engine: &Engine) -> Result<(), String> {
    let original = app.state::<Arc<Store>>().get().audio_mix;
    let set = |mix: &str| engine.request("setAudioMix", json!({ "mix": mix }), SHOT_TIMEOUT);
    let outcome = (|| -> Result<(), String> {
        let frames = engine.request("status", Value::Null, SHOT_TIMEOUT)?["totalFrames"].as_u64().unwrap_or_default();
        let pc = set("pc-mic")?;
        let status = engine.request("status", Value::Null, SHOT_TIMEOUT)?;
        std::thread::sleep(Duration::from_secs(3));
        let saved = engine.request("saveClip", Value::Null, CLIP_TIMEOUT)?;
        let path = saved["path"].as_str().ok_or("saveClip without path")?;
        let tracks = mp4::probe(std::path::Path::new(path))?.audio_tracks;
        let after = engine.request("status", Value::Null, SHOT_TIMEOUT)?["totalFrames"].as_u64().unwrap_or_default();
        let game = set("game")?;
        let bad = set("loud");
        log!(
            "[selftest] clip sound: {} (status {}), clip {path} with {tracks} audio tracks; then {}; bad mix refused: {:?}; frames {frames} -> {after}",
            pc["mix"], status["audio"]["mix"], game["mix"], bad.as_ref().err()
        );
        if pc["mix"] != "pc-mic" || status["audio"]["mix"] != "pc-mic" || tracks != 3 || game["mix"] != "game" || bad.is_ok() {
            return Err("the clip sound did not change in the engine".into());
        }
        if after <= frames {
            return Err("the engine stopped while the clip sound changed".into());
        }
        Ok(())
    })();
    let restored = set(&original);
    log!("[selftest] clip sound restored: {restored:?}");
    outcome.and(restored.map(|_| ()))
}

/// Self-test of storage: an unwritable folder is refused; the size limit removes
/// the oldest item that is not kept (through the Recycle Bin), in a throwaway
/// library; a free-space minimum above the free space pauses recording and
/// restoring it resumes; start with Windows can be set and is restored.
fn check_storage(app: &AppHandle, engine: &Engine) -> Result<(), String> {
    let storage = app.state::<Arc<Storage>>();
    match Storage::check_writable(std::path::Path::new(r"C:\Windows\System32\laurel-selftest")) {
        Err(e) => log!("[selftest] unwritable folder refused: {e}"),
        Ok(()) => return Err("a folder in System32 was accepted".into()),
    }

    // Size limit in a throwaway library with three tiny screenshots.
    let tmp = std::env::temp_dir().join("laurel-selftest-storage");
    let _ = std::fs::remove_dir_all(&tmp);
    let shots = tmp.join("Screenshots");
    std::fs::create_dir_all(&shots).map_err(|e| e.to_string())?;
    let lib = Library::open(&tmp, tmp.join("Clips"), shots.clone(), None)?;
    let base = std::time::SystemTime::now() - Duration::from_secs(60);
    let mut paths = Vec::new();
    for i in 0..3u64 {
        let p = shots.join(format!("Laurel self-test {i} (safe to delete).png"));
        std::fs::write(&p, vec![0u8; 1000]).map_err(|e| e.to_string())?;
        std::fs::File::options().write(true).open(&p).and_then(|f| f.set_modified(base + Duration::from_secs(i * 10))).map_err(|e| e.to_string())?;
        paths.push(p.to_string_lossy().into_owned());
    }
    lib.scan()?;
    lib.set_kept(&paths[0], true)?;
    let removed = Storage::enforce_limit_bytes(&lib, lib.total_size() - 1, true);
    let on_disk: Vec<bool> = paths.iter().map(|p| std::path::Path::new(p).exists()).collect();
    log!("[selftest] limit removed {removed:?}; on disk: {on_disk:?}");
    if removed != [paths[1].clone()] || on_disk != [true, false, true] {
        return Err("the size limit removed the wrong item".into());
    }
    // Recently deleted goes before the oldest item, and leaves when its days are up
    // (these test files are deleted, not sent to the Recycle Bin).
    let trashed = lib.delete(&paths[2])?["path"].as_str().unwrap_or_default().to_string();
    let first = Storage::enforce_limit_bytes(&lib, lib.total_size() - 1, false);
    let trashed = lib.delete(&paths[0])?["path"].as_str().unwrap_or_default().to_string() + "|" + &trashed;
    let kept_days = lib.expire(1, false);
    let expired = lib.expire(0, false);
    log!("[selftest] the limit removed {first:?} first (deleted {trashed}); one day kept {kept_days:?}; no days removed {expired:?}");
    if first.len() != 1 || !trashed.ends_with(&first[0]) || !kept_days.is_empty() || expired.len() != 1 || !lib.list_deleted()?.is_empty() {
        return Err("Recently deleted and the size limit did not behave".into());
    }
    drop(lib);
    let _ = std::fs::remove_dir_all(&tmp);

    // Free-space guard, through the real watcher thread.
    let store = app.state::<Arc<Store>>();
    let saved_settings = store.get();
    log!("[selftest] free space: {:?}", storage.free_space());
    store.set(Settings { min_free_gb: 1000, ..saved_settings.clone() }).map_err(|e| e.to_string())?;
    let wait = |want_paused: bool| -> Result<(bool, bool), String> {
        let deadline = Instant::now() + DISK_CHECK + Duration::from_secs(10);
        while storage.is_paused() != want_paused && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
        }
        std::thread::sleep(Duration::from_millis(500));
        let status = engine.request("status", Value::Null, SHOT_TIMEOUT)?;
        Ok((storage.is_paused(), status["recording"].as_bool().unwrap_or(true)))
    };
    let paused = wait(true)?;
    store.set(saved_settings).map_err(|e| e.to_string())?;
    let resumed = wait(false)?;
    log!("[selftest] disk guard: paused (paused, recording) = {paused:?}; resumed = {resumed:?}");
    if (paused.0 && paused.1) || resumed != (false, true) {
        return Err("the free-space guard did not pause and resume".into());
    }
    if !paused.0 {
        log!("[selftest] the drive has more than 1000 GB free; the pause was not exercised");
    }

    // Start with Windows: set, read back, restore.
    let before = win::autostart_command(AUTOSTART_NAME);
    win::set_autostart(AUTOSTART_NAME, Some(&autostart_command()))?;
    let set = win::autostart_command(AUTOSTART_NAME);
    win::set_autostart(AUTOSTART_NAME, before.as_deref())?;
    let after = win::autostart_command(AUTOSTART_NAME);
    log!("[selftest] autostart set to {set:?}, restored to {after:?}");
    if set.as_deref() != Some(autostart_command().as_str()) || after != before {
        return Err("start with Windows could not be set and restored".into());
    }
    Ok(())
}

/// Self-test of the Steam achievement auto-clips (autoclip.steam-achievements),
/// without Steam: the rules that pick which unlocks are clipped, then two unlocks
/// given to the module a moment apart, which must make one clip named after the
/// first and marked with both in the library.
fn check_autoclip(app: &AppHandle) -> Result<(), String> {
    let base = Settings { steam_any: false, steam_rare: true, steam_ultra: true, ..Settings::default() };
    let cases = [
        (base.clone(), Some(2.0), Some("ultra-rare")),
        (base.clone(), Some(7.5), Some("rare")),
        (base.clone(), Some(50.0), None),
        (base.clone(), None, None),
        (Settings { steam_ultra: false, ..base.clone() }, Some(2.0), Some("rare")),
        (Settings { steam_any: true, ..base.clone() }, Some(50.0), Some("any")),
        (Settings { steam_any: true, ..base.clone() }, None, Some("any")),
    ];
    let wrong: Vec<String> = cases
        .iter()
        .filter(|(s, p, want)| steam::clipped_as(s, *p) != *want)
        .map(|(s, p, want)| format!("{p:?} any={} rare={} ultra={} wanted {want:?}", s.steam_any, s.steam_rare, s.steam_ultra))
        .collect();
    log!("[selftest] auto-clip rules: {} cases, wrong: {wrong:?}", cases.len());
    if !wrong.is_empty() {
        return Err("auto-clip rules".into());
    }

    let store = app.state::<Arc<Store>>();
    let before = store.get();
    let _ = store.set(Settings { auto_clip: true, steam_achievements: true, steam_any: false, steam_rare: true, steam_ultra: true, steam_excluded: vec![], ..before.clone() });
    let achievements = app.state::<Arc<steam::Achievements>>();
    achievements.set_delay(Duration::from_secs(1));
    achievements.seed_percents(480, [("LAUREL_SELFTEST_1".to_string(), 2.0), ("LAUREL_SELFTEST_2".to_string(), 7.5)].into_iter().collect());
    let library = app.state::<Arc<Library>>();
    // Only a clip of this run counts: the self-test's library keeps earlier ones.
    let earlier: Vec<Value> = library.list(Some("clip"), None)?.into_iter().filter(|i| i["achievement"]["id"] == "LAUREL_SELFTEST_1").map(|i| i["path"].clone()).collect();
    let started = Instant::now();
    achievements.on_unlock(480, "Laurel self-test", &json!({ "id": "LAUREL_SELFTEST_1", "name": "Self-test trophy", "description": "Checks the auto-clip" }));
    std::thread::sleep(Duration::from_millis(300));
    achievements.on_unlock(480, "Laurel self-test", &json!({ "id": "LAUREL_SELFTEST_2", "name": "Second trophy", "description": "" }));
    let mut found = None;
    while found.is_none() && started.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(250));
        found = library.list(Some("clip"), None)?.into_iter().find(|i| i["achievement"]["id"] == "LAUREL_SELFTEST_1" && !earlier.contains(&i["path"]));
    }
    achievements.set_delay(steam::AUTO_CLIP_DELAY);
    let _ = store.set(before);
    let item = found.ok_or("no clip marked with the self-test achievement")?;
    let a = &item["achievement"];
    let path = item["path"].as_str().unwrap_or_default();
    log!("[selftest] auto-clip after {} ms: {path}; achievement {a}", started.elapsed().as_millis());
    if !path.contains("Self-test trophy") || a["tier"] != "ultra-rare" || a["percent"] != 2.0 || a["others"] != json!(["Second trophy"]) {
        return Err("the auto-clip is not named or marked as expected".into());
    }
    Ok(())
}

/// Self-test of the overlay: a notice shows without taking the focus, with the
/// overlay window styles, and hides again; a screenshot of the monitor under it is
/// taken while it is visible, for the capture-exclusion check done outside the app
/// (compare a run with LAUREL_OVERLAY_CAPTURABLE=1).
fn check_overlay(engine: &Engine, notifier: &Notifier) -> Result<(), String> {
    let hwnd = notifier.hwnd();
    if hwnd == 0 {
        return Err("overlay window not initialized".into());
    }
    let foreground = win::foreground_window();
    notifier.notify(Kind::Info, "Laurel self-test", "Checking the overlay");
    std::thread::sleep(Duration::from_millis(400));
    let ex = win::ex_style(hwnd);
    let wanted = win::WS_EX_NOACTIVATE | win::WS_EX_TOOLWINDOW | win::WS_EX_TOPMOST | win::WS_EX_TRANSPARENT | win::WS_EX_LAYERED;
    let r = win::window_rect(hwnd);
    log!(
        "[selftest] overlay visible={} focus_kept={} ex_style=0x{ex:08X} styles_ok={} display_affinity=0x{:X} rect={},{},{},{}",
        win::is_visible(hwnd),
        win::foreground_window() == foreground,
        ex & wanted == wanted && ex & win::WS_EX_APPWINDOW == 0,
        win::display_affinity(hwnd),
        r.left,
        r.top,
        r.right,
        r.bottom
    );
    if !win::is_visible(hwnd) || win::foreground_window() != foreground || ex & wanted != wanted || ex & win::WS_EX_APPWINDOW != 0 {
        return Err("overlay check failed (see the line above)".into());
    }

    // Screenshot of the monitor that holds the overlay, while the notice is visible.
    let original = engine.request("status", Value::Null, SHOT_TIMEOUT)?["target"].as_str().unwrap_or("none").to_string();
    let targets = engine.request("listTargets", Value::Null, SHOT_TIMEOUT)?;
    let monitor = targets.as_array().into_iter().flatten().find_map(|t| {
        let (target, label) = (t["target"].as_str()?, t["label"].as_str()?);
        let (size, origin) = label.split_once(" @ ")?;
        let (w, h) = size.rsplit(' ').next()?.split_once('x')?;
        let (x, y) = origin.split(' ').next()?.split_once(',')?;
        let (x, y, w, h): (i32, i32, i32, i32) = (x.parse().ok()?, y.parse().ok()?, w.parse().ok()?, h.parse().ok()?);
        (target.starts_with("monitor:") && r.left >= x && r.left < x + w && r.top >= y && r.top < y + h).then(|| (target.to_string(), x, y))
    });
    if let Some((target, x, y)) = monitor {
        engine.request("setCapture", json!({ "target": target }), SHOT_TIMEOUT)?;
        std::thread::sleep(Duration::from_millis(1500));
        let shot = engine.request("screenshot", Value::Null, SHOT_TIMEOUT)?;
        log!("[selftest] overlay screenshot: target={target} origin={x},{y} crop={},{},{},{} path={}", r.left - x, r.top - y, r.right - r.left, r.bottom - r.top, shot["path"]);
        engine.request("setCapture", json!({ "target": original }), SHOT_TIMEOUT)?;
    } else {
        log!("[selftest] no monitor target found for the overlay rect");
    }

    let shown = Instant::now();
    while win::is_visible(hwnd) && shown.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(100));
    }
    log!("[selftest] overlay hidden={} after {} ms more", !win::is_visible(hwnd), shown.elapsed().as_millis());
    if win::is_visible(hwnd) {
        return Err("overlay did not hide".into());
    }

    // A clip notice keeps the overlay up about 1.5 s: the game leaves independent
    // flip while it shows.
    notifier.notify(Kind::Clip, "Laurel self-test", "Checking the clip notice");
    let shown = Instant::now();
    std::thread::sleep(Duration::from_millis(100));
    let was_visible = win::is_visible(hwnd);
    while win::is_visible(hwnd) && shown.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ms = shown.elapsed().as_millis();
    log!("[selftest] clip notice visible={was_visible} hidden after {ms} ms");
    if !was_visible || !(1300..=1900).contains(&ms) {
        return Err("the clip notice did not last about 1.5 s".into());
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let minimized = args.iter().any(|a| a == "--minimized");
    let selftest_mode = args.iter().any(|a| a == "--selftest");
    // After "restart as administrator": let the previous instance close first.
    if let Some(pid) = args.iter().position(|a| a == "--wait-pid").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()) {
        win::wait_for_exit(pid, 15_000);
    }
    // One Laurel at a time: a second start (the Start menu while Laurel waits in the
    // tray) shows the running one's window instead of recording the game twice.
    // --instance-probe (self-test) only reports it: exit code 3 when another runs.
    let first = win::single_instance();
    if args.iter().any(|a| a == "--instance-probe") {
        std::process::exit(if first { 0 } else { 3 });
    }
    if !first {
        return;
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates::Pending::default())
        // Tauri's own Raw Input registration would replace the hotkeys' one (one
        // registration per device type and process); the app needs no device events.
        .device_event_filter(tauri::DeviceEventFilter::Always)
        // The main window's first script (index.html): the interface language and its
        // strings (i18n.rs), read on every load, so a reload shows a new language.
        // Windows serves the scheme as http://laurel.localhost/.
        .register_uri_scheme_protocol("laurel", |_ctx, request| {
            let (status, body) = if request.uri().path() == "/i18n.js" { (200, i18n::script()) } else { (404, String::new()) };
            tauri::http::Response::builder()
                .status(status)
                .header("Content-Type", "text/javascript; charset=utf-8")
                .header("Cache-Control", "no-store")
                .body(body.into_bytes())
                .unwrap_or_default()
        })
        .invoke_handler(tauri::generate_handler![
            autoclip_status,
            engine_request,
            engine_state,
            engine_restart,
            get_settings,
            set_settings,
            preview_notice,
            library_list,
            library_games,
            library_rescan,
            library_trim,
            library_trim_info,
            library_keep,
            library_delete,
            library_restore,
            library_purge,
            set_autostart,
            pick_folder,
            pick_exe,
            restart_as_admin,
            hotkeys_get,
            hotkey_capture,
            hotkey_cancel,
            hotkeys_reset,
            hotkey_clear,
            set_recording,
            change_folder,
            game_state,
            games_overview,
            games_running,
            games_add,
            games_remove,
            games_set_ignored,
            share_action,
            licenses,
            open_source_page,
            open_logs,
            logs_export,
            update_info,
            update_check,
            update_install,
            start_view,
            save_clip_now,
            screenshot_now
        ])
        .setup(move |app| {
            let work_dir = match std::env::var_os("LAUREL_WORK_DIR") {
                Some(dir) => PathBuf::from(dir),
                None => app.path().app_local_data_dir()?,
            };
            log::init(&work_dir.join("logs"))?;
            log!("Laurel {} starting; work dir {}; administrator: {}", app.package_info().version, work_dir.display(), win::self_elevated());
            listen_for_show(app.handle());
            let store = Arc::new(Store::open(&work_dir.join("settings.json")));
            app.manage(store.clone());
            i18n::set(&store.get().language);
            log!("language: {} ({})", store.get().language, i18n::current());
            // Game detection, unless a fixed target is asked for. The self-test records
            // no capture at all (a fullscreen game capture would hook whatever game the
            // player has open) and turns detection on for its own check.
            let mode = match std::env::var("LAUREL_CAPTURE") {
                Ok(target) => Mode::Fixed(target),
                Err(_) if selftest_mode => Mode::Fixed("none".into()),
                Err(_) => Mode::Detect,
            };
            log!("capture mode: {mode:?}");
            let detector = Detector::new(app.handle().clone(), store.clone(), mode);
            app.manage(detector.clone());
            let selftest_folder = selftest_mode.then(|| std::env::temp_dir().join("laurel-selftest-capture"));
            let storage = Arc::new(Storage::new(store.clone(), selftest_folder));
            if let Err(e) = Storage::check_writable(&storage.folder()) {
                log!("capture folder not writable: {e}");
            }
            for dir in [storage.clips_dir(), storage.screenshots_dir()] {
                let _ = std::fs::create_dir_all(dir);
            }
            log!("capture folder: {}", storage.folder().display());
            app.manage(storage.clone());
            app.manage(Notifier::new(app.handle().clone(), store.clone()));
            watch_engine_state(app.handle());
            let ffmpeg = media::find_ffmpeg(&engine_exe());
            log!("ffmpeg: {}", ffmpeg.as_ref().map_or("not found".into(), |p| p.display().to_string()));
            // The self-test keeps its own library index: scanning its temporary folder
            // would otherwise drop the player's items, with their games and Keep marks.
            let library_dir = if selftest_mode { std::env::temp_dir().join("laurel-selftest-library") } else { work_dir.clone() };
            std::fs::create_dir_all(&library_dir)?;
            let library = Arc::new(Library::open(&library_dir, storage.clips_dir(), storage.screenshots_dir(), ffmpeg)?);
            for dir in library.dirs() {
                let _ = std::fs::create_dir_all(&dir);
                app.asset_protocol_scope().allow_directory(&dir, true)?;
            }
            match library.ensure_trash() {
                Ok(trash) => app.asset_protocol_scope().allow_directory(&trash, true)?,
                Err(e) => log!("recently deleted unavailable: {e}"),
            }
            app.manage(library.clone());
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                match library.scan() {
                    Ok((added, removed)) => log!("library scan: {added} added, {removed} removed"),
                    Err(e) => log!("library scan failed: {e}"),
                }
                library_changed(&handle);
                watch_deleted(handle);
            });
            // The windows are created here, after every state they call into exists
            // (windows created from the config would load before setup runs).
            for label in ["overlay", "main"] {
                if let Some(config) = app.config().app.windows.iter().find(|w| w.label == label) {
                    tauri::WebviewWindowBuilder::from_config(app.handle(), config)?.build()?;
                }
            }
            if let Err(e) = app.state::<Notifier>().init_window() {
                log!("overlay unavailable: {e}");
            }
            let achievements = steam::Achievements::new(app.handle().clone(), store.clone(), &work_dir);
            app.manage(achievements.clone());
            let cfg = engine_config(work_dir, storage, detector.clone(), store.clone());
            app.manage(Engine::start(app.handle().clone(), cfg));
            detector.start();
            achievements.start();
            watch_disk(app.handle().clone());
            setup_tray(app.handle())?;
            watch_tray(app.handle().clone());
            let handle = app.handle().clone();
            match Hotkeys::start(bindings(&store.get()), move |event| on_hotkey(&handle, event)) {
                Ok(hotkeys) => {
                    app.manage(hotkeys);
                }
                Err(e) => log!("hotkeys unavailable: {e}"),
            }
            if !minimized && !selftest_mode {
                show_main(app.handle());
            }
            if selftest_mode {
                let handle = app.handle().clone();
                std::thread::spawn(move || selftest(handle));
            } else {
                // The self-test makes no network request.
                updates::check_after_start(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window keeps Laurel in the tray (spec app.startup).
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building the Laurel app");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            app.state::<Engine>().shutdown();
        }
    });
}

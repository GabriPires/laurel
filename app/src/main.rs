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
// The settings JSON (settings.rs) has more fields than json! expands by default.
#![recursion_limit = "256"]

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
            "--mic-device".into(),
            s.mic_device.clone(),
            "--mic-noise".into(),
            s.mic_noise.clone(),
        ]);
        if !s.mic_enabled {
            options.push("--mic-off".into());
        }
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
                notifier.notify_critical(&t!("Recording paused"), &t!("Disk almost full ({free} GB free). Free up space to keep recording.", free = i18n::number(free_gb, 1)));
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
        // A15: the supervisor gave up on the engine; nothing records until the player
        // restarts it, so this shows over the game even with notices off.
        if now == "failed" && *previous != "failed" {
            notifier.notify_critical(&t!("Laurel stopped recording"), &t!("The recording engine failed. Open Laurel to restart it."));
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

/// The microphone's state (A16 of the stability tests) with the mix the player
/// chose: the window warns about a denied microphone only when track 1 has it.
#[tauri::command]
fn microphone_state(app: AppHandle) -> Value {
    let mut state = app.state::<Engine>().microphone();
    let settings = app.state::<Arc<Store>>().get();
    state["mix"] = json!(settings.audio_mix);
    state["chosen"] = json!({ "enabled": settings.mic_enabled, "device": settings.mic_device, "noise": settings.mic_noise });
    state
}

/// Whether this PC keeps the overlay out of screen capture; without it only
/// critical notices show (A10 and A15 of the stability tests).
#[tauri::command]
fn overlay_status(notifier: tauri::State<'_, Notifier>) -> Value {
    json!({ "excluded": notifier.excluded() })
}

/// Opens Windows' microphone privacy page, where desktop apps get the microphone.
#[tauri::command]
fn open_microphone_privacy() -> Result<(), String> {
    share::open_url("ms-settings:privacy-microphone")
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
    let mic_changed = (new.mic_enabled, &new.mic_device, &new.mic_noise) != (current.mic_enabled, &current.mic_device, &current.mic_noise);
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
    if mic_changed && !video_changed {
        // Spec audio.mic: the switch and the device apply without a restart, and so
        // does the noise suppression (audio.mic-processing); a restarted engine gets
        // --mic-device, --mic-off and --mic-noise.
        if let Err(e) = app.state::<Engine>().request("setMicrophone", json!({ "enabled": new.mic_enabled, "device": new.mic_device, "noise": new.mic_noise }), SHOT_TIMEOUT) {
            log!("setMicrophone failed: {e}");
        }
    }
    if mix_changed || mic_changed {
        // A16: the window's microphone warning depends on whether track 1 has it.
        let _ = app.emit("microphone", Value::Null);
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

/// Programs that run others (Java, Python, script hosts): their version resource
/// names the runtime, not the game.
const HOST_PROGRAMS: &[&str] = &["java.exe", "javaw.exe", "python.exe", "pythonw.exe", "node.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "wscript.exe", "cscript.exe", "rundll32.exe"];

/// The name a program added by hand starts with (spec game.manual-add): the one in
/// its version resource ("Spotify"), not the window title, which a music player or
/// a browser keeps changing (A27 of the stability tests); for a program that runs
/// others, or one without that name, the window title, else the file name.
fn default_game_name(path: &str, title: Option<&str>) -> String {
    let file = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
    let host = HOST_PROGRAMS.contains(&file.as_str());
    let described = if host { None } else { win::file_description(path) };
    described
        .or_else(|| title.map(str::trim).filter(|t| !t.is_empty()).map(String::from))
        .unwrap_or_else(|| std::path::Path::new(path).file_stem().map_or_else(|| path.to_string(), |s| s.to_string_lossy().into_owned()))
}

/// Marks a program as a game (spec game.manual-add): with the name given, else the
/// default name (the program's own, or the window title of the open program).
#[tauri::command]
fn games_add(app: AppHandle, path: String, name: Option<String>, title: Option<String>) -> Result<Value, String> {
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| default_game_name(&path, title.as_deref()));
    change_games(&app, &format!("added {name} ({path})"), |s| {
        s.custom_games.retain(|g| g.path.to_lowercase() != path.to_lowercase());
        s.custom_games.push(CustomGame { path: path.clone(), name: name.clone() });
    })
}

/// Renames a game the player added (A27): its clips are named after it from then on.
#[tauri::command]
fn games_rename(app: AppHandle, path: String, name: String) -> Result<Value, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(t!("The name cannot be empty"));
    }
    if !store_has_custom(&app, &path) {
        return Err(t!("This game is not in your games"));
    }
    change_games(&app, &format!("renamed {path} to {name}"), |s| {
        for game in s.custom_games.iter_mut().filter(|g| g.path.to_lowercase() == path.to_lowercase()) {
            game.name = name.clone();
        }
    })
}

fn store_has_custom(app: &AppHandle, path: &str) -> bool {
    app.state::<Arc<Store>>().get().custom_games.iter().any(|g| g.path.to_lowercase() == path.to_lowercase())
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
                    // Some errors already start with the path; the player sees the file name.
                    let full = share::windows_path(&path);
                    let reason = e.strip_prefix(&format!("{full}: ")).unwrap_or(&e);
                    log!("{full}: {reason}");
                    let name = std::path::Path::new(&full).file_name().map_or(full.clone(), |n| n.to_string_lossy().into_owned());
                    failed.push(format!("{name}: {reason}"));
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

/// "Save and delete original" (spec edit.trim): after library_trim saved the copy,
/// the original goes to Recently deleted and the copy takes its place.
#[tauri::command]
async fn library_replace_with_trim(app: AppHandle, original: String, copy: String) -> Result<Value, String> {
    let library = app.state::<Arc<Library>>().inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || library.replace_with_trim(&original, &copy)).await.map_err(|e| e.to_string());
    let result = result.and_then(|r| r);
    match &result {
        Ok(r) => log!("trim took its original's place: {} (original to {})", r["item"]["path"], r["deleted"]["path"]),
        Err(e) => log!("trim could not take its original's place: {e}"),
    }
    library_changed(&app);
    result
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

/// The registry's ProductName still says Windows 10 on Windows 11, whose builds
/// start at 22000 (stability tests, finding A6).
fn windows_name(product: &str, build: &str) -> String {
    match product.strip_prefix("Windows 10") {
        Some(edition) if build.parse::<u32>().is_ok_and(|b| b >= 22000) => format!("Windows 11{edition}"),
        _ => product.to_string(),
    }
}

fn export_logs(app: &AppHandle, dir: &std::path::Path) -> Result<PathBuf, String> {
    let (date, time) = win::date_time();
    let dest = storage::unique(dir, &format!("Laurel logs {date} {time}"), "zip");
    let windows = |name: &str| win::reg_string(win::HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", name).unwrap_or_default();
    let build = windows("CurrentBuild");
    let info = format!(
        "Laurel {}\nWindows: {} {} (build {})\nSaved: {date} {time}\nAdministrator: {}\n\nEngine:\n{}\n\nMicrophone:\n{}\n\nGame:\n{}\n\nSteam achievements:\n{}\n\nSettings:\n{}\n",
        app.package_info().version,
        windows_name(&windows("ProductName"), &build),
        windows("DisplayVersion"),
        build,
        win::self_elevated(),
        serde_json::to_string_pretty(&app.state::<Engine>().state()).unwrap_or_default(),
        serde_json::to_string_pretty(&app.state::<Engine>().microphone()).unwrap_or_default(),
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

/// Player diagnostics (A30: a friend's clips looked slow motion after he moved the
/// window to his 60 Hz second monitor). The viewer reports each stretch of playback
/// (library.js); this keeps the monitor the main window was on when it began.
#[derive(Default)]
struct PlayerStretch(Mutex<Option<win::Display>>);

/// `start` notes the main window's monitor; the end of a stretch logs one line with
/// that monitor, the one the window is on now when it changed, and the viewer's
/// numbers. A few calls per playback, none per frame.
#[tauri::command]
fn player_stats(app: AppHandle, start: bool, stats: Option<Value>) {
    let display = app.get_webview_window("main").and_then(|w| w.hwnd().ok()).and_then(|h| win::window_display(h.0 as isize));
    let state = app.state::<PlayerStretch>();
    let began = std::mem::replace(&mut *state.0.lock().unwrap(), if start { display.clone() } else { None });
    if start {
        return;
    }
    if let (Some(stats), Some(began)) = (stats, began) {
        log!("{}", player_line(&stats, &began, display.as_ref()));
    }
}

/// "player: <file> <size> <fps>, <s> s on <monitor>: media seconds per wall second,
/// decoded and dropped frames, how often the page was drawn; ended: <why>".
fn player_line(s: &Value, began: &win::Display, now: Option<&win::Display>) -> String {
    let num = |key: &str| s[key].as_f64().unwrap_or(0.0);
    let wall = num("wall").max(0.001);
    let fps = s["fps"].as_f64().map_or_else(|| "? fps".to_string(), |f| format!("{} fps", (f * 100.0).round() / 100.0));
    let full = if s["full"].as_bool() == Some(true) { " in full screen" } else { "" };
    let mut line = format!(
        "player: \"{}\" {}x{} {fps}, {wall:.1} s on {began}{full}: {:.3} s of video per second, {:.1} frames decoded/s, {} dropped, page drawn {:.1} times/s",
        s["file"].as_str().unwrap_or("?"),
        num("width") as u32,
        num("height") as u32,
        num("media") / wall,
        num("decoded") / wall,
        num("dropped") as u64,
        num("frames") / wall,
    );
    let rate = num("rate");
    if rate > 0.0 && rate != 1.0 {
        line += &format!(", playback rate {rate}");
    }
    line += &format!("; ended: {}", s["ended"].as_str().unwrap_or("?"));
    if let Some(now) = now.filter(|now| *now != began) {
        line += &format!("; now on {now}");
    }
    line
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

/// A5: what Windows says about the end of the session goes to app.log; the engine
/// supervisor reads `win::session_ending`.
fn on_session(event: win::SessionEvent) {
    match event {
        win::SessionEvent::Asked(why) => log!("Windows asks to end the session ({why})"),
        win::SessionEvent::Ending => log!("Windows is ending the session"),
        win::SessionEvent::Canceled => log!("Windows did not end the session"),
    }
}

/// Scripted end-to-end check for development: clip, screenshot, engine crash and
/// restart, clip again. Writes "selftest: PASS" or "selftest: FAIL ..." to app.log.
/// The copy of settings.json made before the self-test (see setup).
struct SettingsBackup(PathBuf);

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
        check_player_stats(&app)?;
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
        check_microphone(&app, &engine)?;
        log!("[selftest] targets: {}", engine.request("listTargets", Value::Null, SHOT_TIMEOUT)?);
        log!("[selftest] status: {}", engine.request("status", Value::Null, SHOT_TIMEOUT)?);
        let log_start = engine_log_len();
        log!("[selftest] killing the engine");
        let killed = Instant::now();
        engine.kill();
        // A26: an engine that ended on its own is started again only after OBS's
        // hook in a game could see it go (HOOK_RELEASE_WAIT in engine.rs).
        let deadline = Instant::now() + Duration::from_secs(5);
        while engine.state()["state"] != "stopped" && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        while engine.state()["state"] == "stopped" && killed.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let gap = killed.elapsed();
        log!("[selftest] killed engine started again after {:.1} s", gap.as_secs_f64());
        if gap < Duration::from_secs(6) {
            return Err(format!("the engine started again {:.1} s after it was killed", gap.as_secs_f64()));
        }
        wait_ready("after the restart")?;
        check_crash_report(&engine, &wait_ready, log_start)?;
        check_session_end(&engine, &wait_ready)?;
        check_engine_quit(&engine, &wait_ready)?;
        check_health(&engine, &wait_ready)?;
        std::thread::sleep(Duration::from_secs(4));
        clip()?;
        check_games(&app, &engine)?;
        check_hotkeys(&app)?;
        check_toggle_and_cursor(&app, &engine)
    };
    let outcome = run();
    i18n::set(&user_settings.language);
    let _ = notifier.set_settings(user_settings);
    // Back to the settings copied before the self-test, also after a check that
    // failed before putting its own changes back.
    if let Some(backup) = app.try_state::<SettingsBackup>() {
        let saved = Store::open(&backup.0).get();
        match app.state::<Arc<Store>>().set(saved) {
            Ok(()) => {
                let _ = std::fs::remove_file(&backup.0);
            }
            Err(e) => log!("selftest: settings not put back: {e}"),
        }
    }
    match outcome {
        Ok(()) => log!("selftest: PASS"),
        Err(e) => log!("selftest: FAIL {e}"),
    }
    quit(&app);
}

/// Size of engine.log now, to read what is written after.
fn engine_log_len() -> u64 {
    log::dir().and_then(|d| std::fs::metadata(d.join("engine.log")).ok()).map_or(0, |m| m.len())
}

/// What engine.log got since it was `start` bytes long, with the part in
/// engine.old.log if it rotated meanwhile.
fn engine_log_since(start: u64) -> String {
    let Some(dir) = log::dir() else {
        return String::new();
    };
    let current = std::fs::read(dir.join("engine.log")).unwrap_or_default();
    let bytes = match current.get(start as usize..) {
        Some(tail) => tail.to_vec(),
        None => {
            let old = std::fs::read(dir.join("engine.old.log")).unwrap_or_default();
            [old.get(start as usize..).unwrap_or_default(), &current].concat()
        }
    };
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Self-test of A19: engine.log tells how each engine ended (the one the self-test
/// just killed, exit code 1), and an engine that crashes leaves there the exception,
/// where it happened and its stack before the supervisor restarts it. The crash is
/// a deliberate access violation (the "crash" request), which ends without a Windows
/// Error Reporting report.
/// Self-test of A21: an engine asked to quit (Laurel closing, or an update about to
/// run its installer) has told engine.log how it ended, after its own
/// "[laurel] shutdown", by the time Engine::shutdown returns; and after an update
/// whose installer did not start, the engine runs again.
fn check_engine_quit(engine: &Engine, wait_ready: &dyn Fn(&str) -> Result<(), String>) -> Result<(), String> {
    let start = engine_log_len();
    let asked = Instant::now();
    engine.shutdown();
    let took = asked.elapsed();
    let text = engine_log_since(start);
    let shutdown = text.lines().position(|l| l.contains("[laurel] shutdown"));
    let exited = text.lines().position(|l| l.ends_with("==== engine exited: exit code 0"));
    engine.resume();
    wait_ready("after the engine quit and resumed")?;
    log!("[selftest] engine quit in {} ms: shutdown line {shutdown:?}, exit line {exited:?}; resumed", took.as_millis());
    match (shutdown, exited) {
        (Some(s), Some(e)) if s < e => Ok(()),
        _ => Err("engine.log does not tell how the engine ended when Engine::shutdown returns".into()),
    }
}

/// Self-test of A24 and A25: when the health watch finds the graphics device lost
/// or the running replay buffer without video for 20 s (both faked with the
/// "simulateHealth" request), the engine shuts down in order and ends with its own
/// exit code, and the supervisor starts a new one; each new engine checks its real
/// device at start.
fn check_health(engine: &Engine, wait_ready: &dyn Fn(&str) -> Result<(), String>) -> Result<(), String> {
    let start = engine_log_len();
    let simulate = |problem: &str, secs: u64| -> Result<u128, String> {
        let asked = Instant::now();
        engine.request("simulateHealth", json!({ "problem": problem }), SHOT_TIMEOUT)?;
        let deadline = asked + Duration::from_secs(secs);
        while engine.state()["state"] == "ready" {
            if Instant::now() > deadline {
                return Err(format!("the engine did not end for the simulated {problem}"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let ended = asked.elapsed().as_millis();
        wait_ready(&format!("after the simulated {problem}"))?;
        Ok(ended)
    };
    let lost = simulate("device", 15)?;
    // The stall check needs the replay buffer running.
    let deadline = Instant::now() + Duration::from_secs(15);
    while !engine.request("status", Value::Null, SHOT_TIMEOUT).is_ok_and(|s| s["recording"] == true) {
        if Instant::now() > deadline {
            return Err("the replay buffer did not run again after the simulated device loss".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let stalled = simulate("stall", 40)?;
    let text = engine_log_since(start);
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("[laurel] health:") || l.contains("[laurel] shutdown") || l.contains("==== engine")).collect();
    log!("[selftest] health: device loss ended the engine in {lost} ms, the stall in {stalled} ms; {lines:?}");
    for needed in [
        "[laurel] health: graphics device ok",
        "[laurel] health: the graphics device was lost: 0x887A0005 (DXGI_ERROR_DEVICE_REMOVED, simulated)",
        "==== engine exited: exit code 60 (the engine's graphics device was lost)",
        "[laurel] health: the replay buffer got no video frame for 20 s",
        "==== engine exited: exit code 61 (the replay buffer got no video)",
    ] {
        if !lines.iter().any(|l| l.contains(needed)) {
            return Err(format!("engine.log has no \"{needed}\""));
        }
    }
    // In order: each of the two engines logged its shutdown before its exit line.
    let exits: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.contains("exit code 60") || l.contains("exit code 61")).map(|(i, _)| i).collect();
    if exits.iter().any(|&i| i == 0 || !lines[i - 1].contains("[laurel] shutdown")) {
        return Err("an engine ended for its health without shutting down in order".into());
    }
    Ok(())
}

fn check_crash_report(engine: &Engine, wait_ready: &dyn Fn(&str) -> Result<(), String>, log_start: u64) -> Result<(), String> {
    if engine.request("crash", Value::Null, SHOT_TIMEOUT).is_ok() {
        return Err("the engine answered the crash request".into());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.state()["state"] == "ready" && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    wait_ready("after the crash")?;
    let text = engine_log_since(log_start);
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("==== engine exited") || l.contains("[laurel] crash:")).collect();
    let frames = lines.iter().filter(|l| l.contains("crash: called from")).count();
    let first = lines.iter().find(|l| l.contains("unhandled exception")).copied().unwrap_or_default();
    let exits: Vec<&str> = lines.iter().filter_map(|l| l.split_once("==== engine exited: ").map(|(_, e)| e)).collect();
    log!("[selftest] crash report: {first:?}, {frames} return addresses; engines ended: {exits:?}");
    for needed in [
        "==== engine exited: exit code 1",
        "crash: unhandled exception 0xC0000005 (EXCEPTION_ACCESS_VIOLATION)",
        " at laurel-engine.exe+0x",
        ", writing 0x0000000000000008",
        "crash: called from laurel-engine.exe+0x",
        "crash: thread name: main",
        "crash: end of report",
        "==== engine exited: exit code 0xC0000005 (STATUS_ACCESS_VIOLATION)",
    ] {
        if !lines.iter().any(|l| l.contains(needed)) {
            return Err(format!("engine.log has no \"{needed}\""));
        }
    }
    Ok(())
}

/// Self-test of A5: while Windows ends the session (the WM_QUERYENDSESSION it sends
/// at a restart, sent here to the app's session window), an engine that ends is not
/// started again; once the end is stopped (WM_ENDSESSION with FALSE), it is.
fn check_session_end(engine: &Engine, wait_ready: &dyn Fn(&str) -> Result<(), String>) -> Result<(), String> {
    let before = win::session_ending();
    win::simulate_session_end(true)?;
    let asked = win::session_ending();
    engine.kill();
    let waiting = |secs: u64| {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let state = engine.state();
            if state["state"] == "stopped" && state["message"] == "Windows is ending the session" {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let stopped = waiting(5);
    std::thread::sleep(Duration::from_secs(3));
    let still = waiting(0);
    win::simulate_session_end(false)?;
    let after = win::session_ending();
    wait_ready("after the end of the session was stopped")?;
    log!("[selftest] session end: ending before {before}, asked {asked}; engine waiting {stopped}, still after 3 s {still}; end stopped {}, engine ready", !after);
    if before || !asked || !stopped || !still || after {
        return Err("the engine did not wait for the end of the session".into());
    }
    Ok(())
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
            let ignoring = Settings { ignored_games: vec![IgnoredGame { key: game.id.key.clone(), name: game.id.name.clone() }], ..settings.clone() };
            let verdicts = (catalog.classify(&inside, &settings), catalog.classify(&inside, &ignoring));
            log!("[selftest] classify in {}: {:?}", game.id.name, verdicts);
            if verdicts != (Verdict::Game(game.id.clone()), Verdict::Ignored(game.id.clone())) {
                return Err("library classification failed".into());
            }
            // Helpers in the game's folder are not the game: a crash handler, a
            // launcher such as HoYoPlay's next to Genshin Impact (A17 of the
            // stability tests), and the anti-cheat bootstrappers that start the game
            // and show a splash window (A22: Fortnite's Easy Anti-Cheat one was
            // recorded instead of the game).
            let helpers = [
                "UnityCrashHandler64.exe",
                "HYP.exe",
                r"Game\Binaries\Win64\Game-Win64-Shipping_EAC_EOS.exe",
                r"Game\Binaries\Win64\Game-Win64-Shipping_EAC.exe",
                r"Game\Binaries\Win64\Game-Win64-Shipping_BE.exe",
                "start_protected_game.exe",
            ];
            let counted: Vec<&str> = helpers.into_iter().filter(|h| catalog.classify(&game.dir.join(h).to_string_lossy(), &settings) != Verdict::NotGame).collect();
            log!("[selftest] helpers in {} not counted as the game: {} of {}", game.id.name, helpers.len() - counted.len(), helpers.len());
            if !counted.is_empty() {
                return Err(format!("helpers counted as the game: {counted:?}"));
            }
        } else {
            log!("[selftest] no installed games; library classification not exercised");
        }
        // Minecraft (Java) is found by its window and command line, from any Java
        // runtime: vanilla (an SDL window since 26.x, GLFW before), a Prism instance (whose main class and game arguments go
        // through stdin), an old LWJGL 2 version by its title when the command line
        // cannot be read; not a Swing installer nor another Java game.
        let java = r"C:\Laurel self-test\runtime\bin\javaw.exe";
        let minecraft = |class: &str, title: &str, line: Option<&str>, settings: &Settings| catalog.classify_window(java, class, title, || line.map(String::from), settings);
        let ignoring = Settings { ignored_games: vec![IgnoredGame { key: "minecraft:java".into(), name: "Minecraft".into() }], ..settings.clone() };
        let cases = [
            ("vanilla sdl", minecraft("SDL_app", "Minecraft 26.3", Some(r"C:\runtime\bin\javaw.exe -XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump --gameDir C:\.minecraft"), &settings), true),
            ("vanilla", minecraft("GLFW30", "Minecraft* 26.2", Some(r#""C:\runtime\bin\javaw.exe" -Xmx2G -cp C:\libraries\client.jar net.minecraft.client.main.Main --version 26.2"#), &settings), true),
            ("prism", minecraft("GLFW30", "Homestead", Some(r"javaw.exe -Djava.library.path=C:\instances\Homestead\natives -cp C:\libraries\NewLaunch.jar;C:\libraries\net\fabricmc\loader.jar org.prismlauncher.EntryPoint"), &settings), true),
            ("lwjgl2 title", minecraft("LWJGL", "Minecraft 1.12.2", None, &settings), true),
            ("swing installer", minecraft("SunAwtFrame", "Forge Installer", Some("java -jar forge-installer.jar net.minecraftforge.installer.SimpleInstaller"), &settings), false),
            ("other java game", minecraft("GLFW30", "Mindustry", Some("javaw.exe -jar Mindustry.jar"), &settings), false),
        ];
        let wrong: Vec<&str> = cases.iter().filter(|(_, verdict, game)| matches!(verdict, Verdict::Game(id) if id.key == "minecraft:java") != *game).map(|(name, _, _)| *name).collect();
        let ignored = minecraft("GLFW30", "Minecraft* 26.2", Some("net.minecraft.client.main.Main"), &ignoring);
        log!("[selftest] minecraft windows: {} cases, wrong: {wrong:?}; ignored {:?}", cases.len(), matches!(ignored, Verdict::Ignored(_)));
        if !wrong.is_empty() || !matches!(ignored, Verdict::Ignored(_)) {
            return Err(format!("minecraft classification failed: {wrong:?}"));
        }
        let system = catalog.classify(r"C:\Windows\explorer.exe", &settings);
        if system != Verdict::NotGame {
            return Err(format!("explorer.exe classified as {system:?}"));
        }
    }

    // A27: a program added by hand is named after its version resource, not its
    // window title (a music player's is the song); a program that runs others keeps
    // the title. A hooked game keeps the capture longer when another one has focus.
    {
        let own = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let shell = root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe").to_string_lossy().into_owned();
        let title = "A song title";
        let described = win::file_description(&own);
        let own_name = default_game_name(&own, Some(title));
        let shell_name = default_game_name(&shell, Some(title));
        let focus = (detect::focus_scans_needed(false), detect::focus_scans_needed(true));
        log!("[selftest] added-game names: this program's resource {described:?}, named {own_name:?}; powershell.exe named {shell_name:?}; focus scans to take over: {focus:?}");
        if described.is_none() || own_name == title || shell_name != title || focus != (2, 15) {
            return Err("added-game names or focus scans went wrong".into());
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
    let form = |title: &str| "Add-Type -AssemblyName System.Windows.Forms, System.Drawing; \
        Add-Type -ReferencedAssemblies System.Windows.Forms -TypeDefinition 'public class QuietForm : System.Windows.Forms.Form { protected override bool ShowWithoutActivation { get { return true; } } }'; \
        $f = New-Object QuietForm; $f.Text = 'TITLE'; $f.StartPosition = 'Manual'; \
        $f.Location = New-Object System.Drawing.Point(-6000, -6000); $f.Size = New-Object System.Drawing.Size(960, 600); \
        $t = New-Object System.Windows.Forms.Timer; $t.Interval = 120000; $t.Add_Tick({ $f.Close() }); $t.Start(); \
        [System.Windows.Forms.Application]::Run($f)"
        .replace("TITLE", title);
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
        let spawn = |title: &str| std::process::Command::new(&shell).args(["-NoProfile", "-Command", &form(title)]).creation_flags(0x0800_0000).spawn().map_err(|e| e.to_string());
        let mut child = spawn(name)?;
        let found = until("the game", &|| detector.current().is_some_and(|g| g.id.name == name));
        let result = found.and_then(|_| {
            until("recording", &|| status("recording").is_ok_and(|r| r == true))?;
            log!("[selftest] game detected: {}", detector.snapshot());
            // A27: a game added by hand can be renamed, and the detected game follows.
            let path = shell.to_string_lossy().into_owned();
            let renamed = "Laurel renamed test game";
            games_rename(app.clone(), path.clone(), format!("  {renamed} "))?;
            until("the renamed game", &|| detector.current().is_some_and(|g| g.id.name == renamed))?;
            let empty = games_rename(app.clone(), path.clone(), "   ".into());
            games_rename(app.clone(), path.clone(), name.into())?;
            until("the game's name back", &|| detector.current().is_some_and(|g| g.id.name == name))?;
            log!("[selftest] renamed the game to {renamed:?} and back; an empty name: {empty:?}");
            if empty.is_ok() {
                return Err("an empty game name was accepted".into());
            }
            std::thread::sleep(Duration::from_secs(4));
            let mut saved = engine.request("saveClip", Value::Null, CLIP_TIMEOUT)?;
            attach_game(app, &mut saved);
            finish_saved(app, &mut saved);
            log!("[selftest] game clip: {saved}");
            let path = saved["path"].as_str().unwrap_or_default();
            if !path.contains(name) || !std::path::Path::new(path).exists() {
                return Err(format!("the clip is not named after the game: {path}"));
            }
            // The fallback deadline counts from the start of the capture, not from
            // when the game was found: after an engine restart it starts over.
            let log_start = engine_log_len();
            let asked = Instant::now();
            engine.restart();
            let deadline = Instant::now() + Duration::from_secs(10);
            while engine.state()["state"] == "ready" && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(25));
            }
            until("the engine to restart", &|| engine.state()["state"] == "ready")?;
            let restarted = Instant::now();
            // A26: a restart for new settings stops the engine in order (its game
            // capture stops, and the hook in the game is free), not by a kill.
            let text = engine_log_since(log_start);
            let shutdown = text.lines().position(|l| l.contains("[laurel] shutdown"));
            let exited = text.lines().position(|l| l.contains("==== engine exited:"));
            let exit = exited.and_then(|i| text.lines().nth(i)).and_then(|l| l.split_once("==== engine exited: ")).map(|(_, e)| e.to_string());
            log!("[selftest] restart with the game: in order {:?}, {exit:?}, ready again after {} ms", shutdown.zip(exited).map(|(s, e)| s < e), asked.elapsed().as_millis());
            if !matches!((shutdown, exited), (Some(s), Some(e)) if s < e) || exit.as_deref() != Some("exit code 0") {
                return Err("the restart for new settings did not stop the engine in order".into());
            }
            // The PowerShell window has no swap chain, so it is never hooked; it is
            // not elevated either, so no restart as administrator may be offered,
            // and after 15 s its window is recorded instead (spec capture.game-hook).
            until("the hook check", &|| detector.hook_checked())?;
            let waited = restarted.elapsed();
            log!("[selftest] hook check {:.1} s after the engine restart", waited.as_secs_f64());
            if waited < Duration::from_secs(14) {
                return Err(format!("the window fallback came {:.1} s after the capture started again", waited.as_secs_f64()));
            }
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
            // A minimized game is still open (a game in exclusive fullscreen
            // minimizes on Alt+Tab): the capture and the replay buffer stay.
            let hwnd = detector.current().map_or(0, |g| g.hwnd);
            win::minimize_no_activate(hwnd);
            std::thread::sleep(Duration::from_secs(8));
            let still = detector.current().is_some_and(|g| g.id.name == name);
            let recording = status("recording")?;
            win::restore_no_activate(hwnd);
            log!("[selftest] minimized game: still the game {still}, recording {recording}");
            if !still || recording != true {
                return Err("a minimized game counted as closed".into());
            }
            // While the game is recorded by its window, its hook is tried again: a
            // game capture goes over the window capture for a while, then comes off
            // when it does not hook (the PowerShell window never is).
            detector.retry_hook_now();
            until("a hook try", &|| status("probing").is_ok_and(|p| p == true))?;
            let during = status("target")?;
            until("the hook try to end", &|| status("probing").is_ok_and(|p| p == false))?;
            let snapshot = detector.snapshot();
            log!("[selftest] hook try over {during}, then {snapshot}");
            if !during.as_str().unwrap_or_default().starts_with("window:") || snapshot["capture"] != "window" {
                return Err("the hook try changed the capture".into());
            }
            // A23: when the game's window moves to another of its processes (as
            // Fortnite's from its anti-cheat bootstrapper to the game), the capture
            // follows it. The first window is minimized, so a second PowerShell
            // window becomes the game's; then the first comes back and the second
            // closes, and the capture goes back to the first.
            let first = format!("window:{name}:");
            let other = "Laurel self-test game window 2";
            let second = format!("window:{other}:");
            win::minimize_no_activate(hwnd);
            let mut child2 = spawn(other)?;
            let pid2 = child2.id();
            let moved = until("the capture to follow the second window", &|| {
                detector.current().is_some_and(|g| g.pid == pid2) && status("target").is_ok_and(|t| t.as_str().unwrap_or_default().starts_with(&second))
            });
            let after = (detector.snapshot(), status("probing"));
            win::restore_no_activate(hwnd);
            let _ = child2.kill();
            let _ = child2.wait();
            moved?;
            log!("[selftest] game moved to pid {pid2}: {}; probing {:?}", after.0, after.1);
            if after.0["capture"] != "window" || after.1 != Ok(Value::Bool(false)) {
                return Err("the capture did not stay on the moved window".into());
            }
            until("the capture to go back to the first window", &|| status("target").is_ok_and(|t| t.as_str().unwrap_or_default().starts_with(&first)))?;
            log!("[selftest] game back to its first window: {}", detector.snapshot());
            Ok(())
        });
        let _ = child.kill();
        let _ = child.wait();
        result?;
        until("the game to close", &|| detector.current().is_none())?;
        until("recording to pause", &|| status("recording").is_ok_and(|r| r == false))?;
        // With no game, the game audio follows nothing, so it stops trying again.
        let follows = status("audio")?["follows"].clone();
        log!("[selftest] game closed: {}; game audio follows {follows}", detector.snapshot());
        if !follows.is_null() {
            return Err(format!("the game audio still follows {follows} with no game"));
        }

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

/// Self-test of the player diagnostics (A30): every monitor has a refresh rate, the
/// main window's monitor is found, and a stretch makes the expected line.
fn check_player_stats(app: &AppHandle) -> Result<(), String> {
    let displays = win::displays();
    let window = app.get_webview_window("main").and_then(|w| w.hwnd().ok()).and_then(|h| win::window_display(h.0 as isize));
    log!("[selftest] displays {displays:?}; main window on {window:?}");
    if displays.is_empty() || displays.iter().any(|d| d.hz == 0 || d.width <= 0) || !displays[0].primary {
        return Err("the monitors or their refresh rates were not read".into());
    }
    let Some(window) = window else {
        return Err("the main window's monitor was not found".into());
    };
    let stats = json!({ "file": "Test.mp4", "width": 1920, "height": 1080, "fps": 120.0, "wall": 4.0, "media": 4.0, "decoded": 480, "dropped": 2, "frames": 576, "full": false, "rate": 1.0, "ended": "paused" });
    let moved = win::Display { hz: window.hz + 1, ..window.clone() };
    let line = player_line(&stats, &window, Some(&moved));
    let want = format!("player: \"Test.mp4\" 1920x1080 120 fps, 4.0 s on {window}: 1.000 s of video per second, 120.0 frames decoded/s, 2 dropped, page drawn 144.0 times/s; ended: paused; now on {moved}");
    log!("[selftest] player line: {line}");
    if line != want {
        return Err(format!("the player line is {line:?}, not {want:?}"));
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
    // info.txt names Windows 11 by its build, whatever the registry's ProductName says.
    let windows = |name: &str| win::reg_string(win::HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", name).unwrap_or_default();
    let here = windows_name(&windows("ProductName"), &windows("CurrentBuild"));
    log!("[selftest] Windows in info.txt: {here} (registry: {}, build {})", windows("ProductName"), windows("CurrentBuild"));
    if windows_name("Windows 10 Pro", "26200") != "Windows 11 Pro" || windows_name("Windows 10 Pro", "19045") != "Windows 10 Pro" {
        return Err("the Windows name in info.txt is wrong".into());
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
    // The exact trim's video rate is capped at the original's (stability tests, A4),
    // read from the clip's sample sizes, which never add up to more than the file.
    let source = mp4::probe(std::path::Path::new(path))?;
    let file_rate = std::fs::metadata(path).map_err(|e| e.to_string())?.len() * 8 * 1000 / source.duration_ms.max(1);
    let exact_rate = mp4::probe(std::path::Path::new(exact["path"].as_str().unwrap_or_default()))?.video_bitrate;
    log!("[selftest] video rate: original {} bit/s (whole file {file_rate}), exact trim {exact_rate}", source.video_bitrate);
    if source.video_bitrate == 0 || source.video_bitrate > file_rate || exact_rate == 0 {
        return Err("the video rate of the clips could not be read".into());
    }
    // A clip cannot go to Recently deleted while ffmpeg reads it for a trim: the
    // delete is refused with a reason, and the clip stays (stability tests, A3).
    let trimming = {
        let (library, path) = (library.inner().clone(), path.to_string());
        let last = t[t.len() - 1];
        std::thread::spawn(move || library.trim(&path, 0.0, last, true))
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while !library.is_reading(path) && !trimming.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    let during = library.is_reading(path).then(|| library.delete(path));
    let long_trim = trimming.join().map_err(|_| "the trim thread panicked".to_string())??;
    log!("[selftest] delete during a trim: {during:?}; the clip is still there: {}; the trim gave {}", std::path::Path::new(path).exists(), long_trim["path"]);
    if !matches!(during, Some(Err(_))) || !std::path::Path::new(path).exists() {
        return Err("a clip being trimmed was deleted, or the trim was too quick to check".into());
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
    // "Save and delete original" (edit.trim, 2026-11-07): the trimmed copy takes the
    // original's name, date, Keep mark and achievement, with a thumbnail of its own;
    // the original waits in Recently deleted and comes back under a free name.
    let target = long_trim["path"].as_str().unwrap_or_default().to_string();
    library.set_kept(&target, true)?;
    let trophy = json!({ "name": "Laurel self-test achievement", "percent": 4.5 });
    let before = library.set_achievement(&target, &trophy)?;
    let copy = library.trim(&target, 0.5, 2.5, false)?;
    let copy_path = copy["path"].as_str().unwrap_or_default().to_string();
    let replaced = library.replace_with_trim(&target, &copy_path)?;
    let (item, gone) = (&replaced["item"], &replaced["deleted"]);
    let gone_path = gone["path"].as_str().unwrap_or_default().to_string();
    let thumbs = (item["thumbnail"].as_str().unwrap_or_default(), before["thumbnail"].as_str().unwrap_or_default());
    let checks = [
        ("in the original's place", item["path"] == target.as_str() && exists(&target) && !exists(&copy_path)),
        ("the trimmed length", (1500..=3500).contains(&item["durationMs"].as_i64().unwrap_or_default())),
        ("the Keep mark", item["kept"] == true),
        ("the achievement", item["achievement"] == trophy),
        ("the original's date", item["createdMs"] == before["createdMs"]),
        ("its own thumbnail", thumbs.0 != thumbs.1 && exists(thumbs.0) && exists(thumbs.1)),
        ("the original in Recently deleted", gone["originalPath"] == target.as_str() && exists(&gone_path) && listed(&gone_path)? == (false, true)),
        ("the copy's entry gone", !library.list(Some("clip"), None)?.iter().any(|i| i["path"] == copy_path.as_str())),
    ];
    let back = library.restore(&gone_path)?;
    let back_path = back["path"].as_str().unwrap_or_default();
    log!(
        "[selftest] trim in place of {target}: {} ms, failed checks {:?}; original restored as {back_path}",
        item["durationMs"],
        checks.iter().filter(|(_, ok)| !ok).map(|(what, _)| *what).collect::<Vec<_>>()
    );
    if checks.iter().any(|(_, ok)| !ok) || back_path == target || !exists(back_path) || !exists(&target) {
        return Err("Save and delete original did not behave".into());
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
    log!("[selftest] encoders: {ids:?}; recording with {} (fallbacks {}, copy through memory {})", info["encoder"], info["encoderFallbacks"], info["encoderCopy"]);
    if !ids.contains(&"obs_x264") || !ids.iter().any(|id| info["encoder"] == *id) {
        return Err(format!("unexpected encoders: {ids:?}, in use {}", info["encoder"]));
    }
    // A28: the engine tells when OBS reroutes NVENC's texture encoder (null here,
    // where the encoder takes the frames on the GPU it renders on).
    if info.get("encoderCopy").is_none() {
        return Err("the engine state has no encoderCopy".into());
    }
    // A13: the engine says which GPU it renders on and why (high performance on a
    // PC with two GPUs), so log packages show it.
    let adapter = &info["adapter"];
    log!("[selftest] video adapter: {adapter}");
    if !adapter["index"].is_u64() || !adapter["name"].is_string() || !["only GPU", "high performance", "default", "asked"].iter().any(|r| adapter["reason"] == *r) {
        return Err(format!("the engine did not report its video adapter: {adapter}"));
    }
    let store = app.state::<Arc<Store>>();
    let original = store.get();
    let original_mix = original.audio_mix.clone();
    let original_mic = (original.mic_enabled, original.mic_device.clone(), original.mic_noise.clone());
    let test = Settings { video_encoder: "x264".into(), video_preset: "performance".into(), video_height: 720, video_fps: 30, clip_secs: 15, audio_mix: "pc-mic".into(), mic_enabled: false, mic_device: "laurel-selftest-mic".into(), mic_noise: "speex".into(), ..original.clone() };
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
        "[selftest] video settings: encoder {}, output {}, fps {}, buffer {} s, clip sound {}, microphone {}; restored: {}, clip sound {}, microphone {}",
        changed["encoder"], changed["output"], changed["fps"], changed["bufferSecs"], changed["audioMix"], changed["microphone"], restored["encoder"], restored["audioMix"], restored["microphone"]
    );
    if changed["encoder"] != "obs_x264" || changed["output"][1] != 720 || changed["fps"] != 30 || changed["bufferSecs"] != 15 || changed["audioMix"] != "pc-mic" {
        return Err("the video settings did not reach the engine".into());
    }
    // Spec audio.mic: a restarted engine takes the microphone switch and device, and
    // the noise suppression (audio.mic-processing).
    if changed["microphone"] != json!({ "enabled": false, "device": "laurel-selftest-mic", "noise": "speex" }) {
        return Err(format!("the microphone choice did not reach the engine: {}", changed["microphone"]));
    }
    if restored["microphone"] != json!({ "enabled": original_mic.0, "device": original_mic.1, "noise": original_mic.2 }) {
        return Err(format!("the microphone choice was not restored: {}", restored["microphone"]));
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

/// Self-test of the microphone (spec audio.mic; A16 and A20 of the stability tests):
/// closed ("idle") while nothing is recorded, as in the self-test, open while a
/// capture records (the screen here) and closed again without it; the switch and
/// the device apply without a restart; a device that is not connected is reported
/// instead of tried every 3 s; a request for another try is taken. Then the
/// player's capture and microphone come back.
fn check_microphone(app: &AppHandle, engine: &Engine) -> Result<(), String> {
    let chosen = app.state::<Arc<Store>>().get();
    let target = engine.request("status", Value::Null, SHOT_TIMEOUT)?["target"].as_str().unwrap_or("none").to_string();
    let wait_for = |states: &[&str], secs: u64| -> Result<String, String> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let now = engine.microphone()["state"].as_str().unwrap_or_default().to_string();
            if states.contains(&now.as_str()) {
                return Ok(now);
            }
            if Instant::now() > deadline {
                return Err(format!("the microphone is {now}, expected {states:?}: {}", engine.microphone()));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let set = |enabled: bool, device: &str| engine.request("setMicrophone", json!({ "enabled": enabled, "device": device }), SHOT_TIMEOUT);
    const OPEN: &[&str] = &["ok", "failed", "denied", "missing"];
    let outcome = (|| -> Result<(), String> {
        set(true, "default")?;
        let idle = wait_for(&["idle"], 5)?;
        let mics = engine.request("listMicrophones", Value::Null, SHOT_TIMEOUT)?;
        engine.request("setCapture", json!({ "target": "monitor:0" }), SHOT_TIMEOUT)?;
        let open = wait_for(OPEN, 10)?;
        let retried = if open == "missing" {
            open.clone()
        } else {
            engine.request("retryMicrophone", Value::Null, SHOT_TIMEOUT)?;
            std::thread::sleep(Duration::from_millis(300));
            wait_for(&["ok", "failed", "denied"], 10)?
        };
        set(false, "default")?;
        let off = wait_for(&["off"], 5)?;
        set(true, "laurel-selftest-no-such-microphone")?;
        let missing = wait_for(&["missing"], 5)?;
        set(true, "default")?;
        // The state read before the engine takes the change is the old one.
        let deadline = Instant::now() + Duration::from_secs(5);
        while engine.microphone()["device"] != "default" && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let back = wait_for(OPEN, 10)?;
        // Noise suppression (audio.mic-processing, 2026-11-07): OBS's filter goes on
        // the open microphone with each method and comes off when switched off; a
        // clip saved with RNNoise keeps its three audio tracks.
        let noise = |method: &str| -> Result<Value, String> {
            engine.request("setMicrophone", json!({ "noise": method }), SHOT_TIMEOUT)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while engine.microphone()["noise"] != method && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(engine.microphone()["noiseFilters"].clone())
        };
        let filters = [noise("rnnoise")?, noise("speex")?, noise("off")?, noise("rnnoise")?];
        std::thread::sleep(Duration::from_secs(2));
        let saved = engine.request("saveClip", Value::Null, CLIP_TIMEOUT)?;
        let clip = saved["path"].as_str().ok_or("saveClip without path")?;
        let tracks = mp4::probe(std::path::Path::new(clip))?.audio_tracks;
        noise("off")?;
        let open_now = engine.microphone()["state"] == "ok";
        engine.request("setCapture", json!({ "target": target }), SHOT_TIMEOUT)?;
        let closed = wait_for(&["idle"], 5)?;
        log!(
            "[selftest] microphone: {} listed; no recording {idle}; recording the screen {open}, after another try {retried}; switched off {off}; a device not connected {missing}; back on {back}; noise filters on the source with rnnoise, speex, off, rnnoise: {filters:?}, clip with rnnoise {tracks} audio tracks; capture {target} again {closed}",
            mics.as_array().map_or(0, Vec::len)
        );
        // Without an open microphone (none connected, or denied) there is no source
        // to filter; the choice still reaches the engine.
        let expected = if back == "ok" && open_now { [1, 1, 0, 1] } else { [filters[0].as_u64().unwrap_or(9), filters[1].as_u64().unwrap_or(9), 0, filters[3].as_u64().unwrap_or(9)] };
        if filters.iter().map(|f| f.as_u64().unwrap_or(9)).collect::<Vec<_>>() != expected || tracks != 3 {
            return Err(format!("noise suppression did not behave: filters {filters:?}, {tracks} audio tracks"));
        }
        Ok(())
    })();
    let capture = engine.request("setCapture", json!({ "target": target }), SHOT_TIMEOUT);
    let restored = engine.request("setMicrophone", json!({ "enabled": chosen.mic_enabled, "device": chosen.mic_device, "noise": chosen.mic_noise }), SHOT_TIMEOUT);
    log!("[selftest] microphone restored: {restored:?}");
    outcome.and(capture.map(|_| ())).and(restored.map(|_| ()))
}

/// Self-test of storage: an unwritable folder is refused; the size limit removes
/// the oldest item that is not kept (through the Recycle Bin), in a throwaway
/// library; a free-space minimum above the free space pauses recording and
/// restoring it resumes; start with Windows can be set and is restored.
fn check_storage(app: &AppHandle, engine: &Engine) -> Result<(), String> {
    let storage = app.state::<Arc<Storage>>();
    // An elevated Laurel may write anywhere, System32 included (the self-test also
    // runs elevated, for the hook check of A14), so only one that is not elevated
    // checks the refusal.
    if win::self_elevated() {
        log!("[selftest] Laurel elevated: unwritable folder check skipped");
    } else {
        match Storage::check_writable(std::path::Path::new(r"C:\Windows\System32\laurel-selftest")) {
            Err(e) => log!("[selftest] unwritable folder refused: {e}"),
            Ok(()) => return Err("a folder in System32 was accepted".into()),
        }
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

    // A15: with notices over games off, an ordinary notice stays hidden and a
    // critical one (Laurel is not recording the game) shows. A10: on a PC whose
    // overlay Windows cannot keep out of capture, only critical notices show.
    let wait_hidden = || {
        let start = Instant::now();
        while win::is_visible(hwnd) && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let shows = |critical: bool, what: &str| {
        if critical {
            notifier.notify_critical("Laurel self-test", what);
        } else {
            notifier.notify(Kind::Info, "Laurel self-test", what);
        }
        std::thread::sleep(Duration::from_millis(400));
        let visible = win::is_visible(hwnd);
        wait_hidden();
        visible
    };
    let settings = notifier.settings();
    let excluded = notifier.excluded();
    let _ = notifier.set_settings(Settings { overlay: false, ..settings.clone() });
    let off = (shows(false, "An ordinary notice, notices off"), shows(true, "A critical notice, notices off"));
    let _ = notifier.set_settings(settings);
    notifier.set_excluded(false);
    let unexcluded = (shows(false, "An ordinary notice, overlay in captures"), shows(true, "A critical notice, overlay in captures"));
    notifier.set_excluded(excluded);
    log!("[selftest] notices off: ordinary shown={}, critical shown={}; overlay in captures: ordinary shown={}, critical shown={}", off.0, off.1, unexcluded.0, unexcluded.1);
    if off != (false, true) || unexcluded != (false, true) {
        return Err("critical notices did not show alone (see the line above)".into());
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
        .manage(PlayerStretch::default())
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
            microphone_state,
            open_microphone_privacy,
            overlay_status,
            get_settings,
            set_settings,
            preview_notice,
            library_list,
            library_games,
            library_rescan,
            library_trim,
            library_replace_with_trim,
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
            games_rename,
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
            screenshot_now,
            player_stats
        ])
        .setup(move |app| {
            let work_dir = match std::env::var_os("LAUREL_WORK_DIR") {
                Some(dir) => PathBuf::from(dir),
                None => app.path().app_local_data_dir()?,
            };
            log::init(&work_dir.join("logs"))?;
            log!("Laurel {} starting; work dir {}; administrator: {}", app.package_info().version, work_dir.display(), win::self_elevated());
            listen_for_show(app.handle());
            if let Err(e) = win::watch_session(on_session) {
                log!("session end not watched: {e}");
            }
            // The self-test changes settings and puts them back at its end. A copy made
            // at its start puts them back after a self-test that was stopped: one once
            // left its test game and its notice settings among the user's settings.
            let settings_path = work_dir.join("settings.json");
            let backup = work_dir.join("settings.selftest-backup.json");
            if backup.exists() {
                match std::fs::rename(&backup, &settings_path) {
                    Ok(()) => log!("settings: restored the copy made before a self-test that did not finish"),
                    Err(e) => log!("settings: could not restore the copy made before a self-test: {e}"),
                }
            }
            if selftest_mode && settings_path.exists() {
                match std::fs::copy(&settings_path, &backup) {
                    Ok(_) => {
                        app.manage(SettingsBackup(backup.clone()));
                    }
                    Err(e) => log!("settings: no copy before the self-test: {e}"),
                }
            }
            let store = Arc::new(Store::open(&settings_path));
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
            // Recording starts before the windows: building them waits for WebView2,
            // which took up to 25 s on a slower PC (stability tests, finding A9).
            // Without the overlay window yet, a notice only plays its sound.
            let achievements = steam::Achievements::new(app.handle().clone(), store.clone(), &work_dir);
            app.manage(achievements.clone());
            let cfg = engine_config(work_dir, storage, detector.clone(), store.clone());
            app.manage(Engine::start(app.handle().clone(), cfg));
            detector.start();
            achievements.start();
            watch_disk(app.handle().clone());
            let handle = app.handle().clone();
            match Hotkeys::start(bindings(&store.get()), move |event| on_hotkey(&handle, event)) {
                Ok(hotkeys) => {
                    app.manage(hotkeys);
                }
                Err(e) => log!("hotkeys unavailable: {e}"),
            }
            // The windows are created here, after every state they call into exists
            // (windows created from the config would load before setup runs).
            let building = Instant::now();
            for label in ["overlay", "main"] {
                if let Some(config) = app.config().app.windows.iter().find(|w| w.label == label) {
                    tauri::WebviewWindowBuilder::from_config(app.handle(), config)?.build()?;
                }
            }
            log!("windows built in {} ms", building.elapsed().as_millis());
            // For the player diagnostics (A30): the WebView2 that plays the clips and the
            // monitors, with their refresh rates.
            let displays: Vec<String> = win::displays().iter().map(ToString::to_string).collect();
            log!("webview2 {}; displays: {}", tauri::webview_version().unwrap_or_else(|e| format!("unknown ({e})")), displays.join(", "));
            if let Err(e) = app.state::<Notifier>().init_window() {
                log!("overlay unavailable: {e}");
            }
            setup_tray(app.handle())?;
            watch_tray(app.handle().clone());
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

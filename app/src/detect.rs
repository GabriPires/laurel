//! Game detection loop and capture control (spec game.detection). Every scan looks
//! at the visible windows and classifies their programs (games.rs); the game being
//! recorded stays while its window lives, and another game takes over when it stays
//! in focus. This module alone tells the engine what to capture and whether the
//! replay buffer runs: recording turned on (spec record.toggle), a game and enough
//! disk space are all needed; with recording off nothing is captured or hooked.
//! A game is captured by its hook, or by its window when the hook does not start
//! (spec capture.game-hook). With the screen recording option (capture.screen, off
//! by default per D13), a monitor is recorded while no game is open, or always when
//! the switch to games is off.
//!
//! With a fixed target (LAUREL_CAPTURE, or the self-test), detection is off and the
//! engine records that target all the time, as in the Phase 3 PoC.

use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::engine::Engine;
use crate::games::{Catalog, GameId, Verdict};
use crate::log::log;
use crate::overlay::{Kind, Notifier};
use crate::settings::Store;
use crate::win::{self, AppWindow};

const SCAN_EVERY: Duration = Duration::from_secs(2);
const CATALOG_EVERY: Duration = Duration::from_secs(60);
/// Smaller windows are launchers, splash screens or dialogs, not the game itself.
const MIN_WIDTH: i32 = 640;
const MIN_HEIGHT: i32 = 360;
/// Scans without the game's window before it counts as closed (a game recreates
/// its window when it changes display mode).
const MISSING_SCANS: u32 = 3;
/// Scans another game must stay in focus before the capture moves to it.
const FOCUS_SCANS: u32 = 2;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// A game not hooked this long after it was found is checked for elevation.
const HOOK_GRACE: Duration = Duration::from_secs(8);
/// A game that is not elevated and still not hooked this long after it was found
/// is recorded through its window instead.
const FALLBACK_GRACE: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    Detect,
    Fixed(String),
}

/// The screen recording option (spec capture.screen).
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    /// monitor_capture device id; empty for the primary monitor.
    pub monitor: String,
    /// Games take over the capture while they are open.
    pub switch_to_games: bool,
}

impl Screen {
    fn target(&self) -> String {
        format!("monitor-id:{}", self.monitor)
    }
}

/// The game being recorded.
#[derive(Clone, Debug)]
pub struct Active {
    pub id: GameId,
    pub path: String,
    pub pid: u32,
    pub hwnd: isize,
    pub title: String,
    pub class: String,
    /// When the game was found.
    pub since: Instant,
}

impl Active {
    pub fn exe(&self) -> &str {
        self.path.rsplit('\\').next().unwrap_or(&self.path)
    }

    /// The libobs window spec `title:class:exe`, escaped as libobs does; the engine
    /// matches the executable when the title changes.
    fn spec(&self) -> String {
        let enc = |s: &str| s.replace('#', "#22").replace(':', "#3A");
        format!("{}:{}:{}", enc(&self.title), enc(&self.class), enc(self.exe()))
    }

    pub fn to_json(&self) -> Value {
        json!({ "key": self.id.key, "name": self.id.name, "source": self.id.source, "exe": self.exe(), "path": self.path, "pid": self.pid })
    }
}

struct State {
    mode: Mode,
    /// Recording turned on by the user (spec record.toggle).
    enabled: bool,
    current: Option<Active>,
    missing: u32,
    focus: Option<(String, u32)>,
    disk_paused: bool,
    /// What the engine was last told: (capture target, recording).
    applied: Option<(String, bool)>,
    /// The game whose hook was already checked (see check_hook).
    hook_checked: Option<String>,
    /// The game that runs as administrator while Laurel does not.
    needs_admin: Option<String>,
    /// The game recorded through its window because its hook did not start.
    fallback: Option<String>,
    screen: Option<Screen>,
}

pub struct Detector {
    app: AppHandle,
    store: Arc<Store>,
    catalog: RwLock<Catalog>,
    state: Mutex<State>,
    /// Serializes the requests that bring the engine to the wanted state.
    apply_lock: Mutex<()>,
    wake: (Mutex<bool>, Condvar),
    /// Whether Laurel runs as administrator.
    elevated: bool,
}

impl Detector {
    pub fn new(app: AppHandle, store: Arc<Store>, mode: Mode) -> Arc<Detector> {
        let catalog = Catalog::scan();
        let settings = store.get();
        let enabled = settings.recording_enabled;
        let screen = screen_of(&settings);
        log!("games: {} installed games found {}", catalog.installed().len(), catalog.counts());
        Arc::new(Detector {
            app,
            store,
            catalog: RwLock::new(catalog),
            state: Mutex::new(State { mode, enabled, current: None, missing: 0, focus: None, disk_paused: false, applied: None, hook_checked: None, needs_admin: None, fallback: None, screen }),
            apply_lock: Mutex::new(()),
            wake: (Mutex::new(false), Condvar::new()),
            elevated: win::self_elevated(),
        })
    }

    /// The engine's start options for the current mode: no capture and a paused
    /// replay buffer until a game shows up.
    pub fn engine_args(&self) -> Vec<String> {
        match initial(&self.state.lock().unwrap()) {
            (target, true) => vec!["--capture".into(), target],
            (target, false) => vec!["--capture".into(), target, "--paused".into()],
        }
    }

    pub fn start(self: &Arc<Detector>) {
        let me = self.clone();
        std::thread::spawn(move || {
            let mut catalog_at = Instant::now();
            loop {
                if catalog_at.elapsed() > CATALOG_EVERY {
                    me.rescan();
                    catalog_at = Instant::now();
                }
                me.scan();
                let (lock, cvar) = &me.wake;
                let mut woken = cvar.wait_timeout_while(lock.lock().unwrap(), SCAN_EVERY, |w| !*w).unwrap().0;
                *woken = false;
            }
        });
    }

    /// Scans again now (after the user changes the game lists).
    pub fn wake(&self) {
        *self.wake.0.lock().unwrap() = true;
        self.wake.1.notify_all();
    }

    pub fn rescan(&self) {
        let catalog = Catalog::scan();
        let mut current = self.catalog.write().unwrap();
        if catalog.installed().len() != current.installed().len() {
            log!("games: {} installed games found {}", catalog.installed().len(), catalog.counts());
        }
        *current = catalog;
    }

    pub fn catalog(&self) -> std::sync::RwLockReadGuard<'_, Catalog> {
        self.catalog.read().unwrap()
    }

    pub fn mode(&self) -> Mode {
        self.state.lock().unwrap().mode.clone()
    }

    pub fn set_mode(&self, mode: Mode) {
        {
            let mut s = self.state.lock().unwrap();
            s.mode = mode;
            s.current = None;
            s.missing = 0;
        }
        self.apply();
        self.wake();
    }

    /// Whether the current game's hook was checked (see check_hook).
    pub fn hook_checked(&self) -> bool {
        let s = self.state.lock().unwrap();
        s.current.as_ref().is_some_and(|g| s.hook_checked.as_deref() == Some(g.id.key.as_str()))
    }

    pub fn enabled(&self) -> bool {
        self.state.lock().unwrap().enabled
    }

    /// Turns recording on or off (spec record.toggle). Off releases the capture and
    /// the replay buffer; on starts them again for the current game, if any.
    pub fn set_enabled(&self, enabled: bool) {
        {
            let mut s = self.state.lock().unwrap();
            s.enabled = enabled;
            // The hook is checked again from now on.
            s.hook_checked = None;
            s.needs_admin = None;
            s.fallback = None;
            if let Some(game) = s.current.as_mut() {
                game.since = Instant::now();
            }
        }
        self.apply();
        let _ = self.app.emit("game-state", self.snapshot());
    }

    pub fn current(&self) -> Option<Active> {
        self.state.lock().unwrap().current.clone()
    }

    /// Applies a change of the screen recording option.
    pub fn set_screen(&self, screen: Option<Screen>) {
        let changed = {
            let mut s = self.state.lock().unwrap();
            let changed = s.screen != screen;
            s.screen = screen;
            changed
        };
        if changed {
            self.apply();
            let _ = self.app.emit("game-state", self.snapshot());
        }
    }

    /// Whether the replay buffer should be running.
    pub fn recording(&self) -> bool {
        let s = self.state.lock().unwrap();
        wanted(&s).1
    }

    pub fn snapshot(&self) -> Value {
        let s = self.state.lock().unwrap();
        json!({
            "mode": if s.mode == Mode::Detect { "detect" } else { "fixed" },
            "enabled": s.enabled,
            "game": s.current.as_ref().map(Active::to_json),
            "recording": wanted(&s).1,
            "needsAdmin": s.needs_admin.is_some(),
            "capture": capture_kind(&wanted(&s).0),
            "screen": s.screen.is_some(),
            "elevated": self.elevated,
        })
    }

    /// Called by the disk guard (spec storage.limits).
    pub fn set_disk_paused(&self, paused: bool) {
        self.state.lock().unwrap().disk_paused = paused;
        self.apply();
    }

    /// The engine (re)started with the options of `engine_args`.
    pub fn engine_ready(&self) {
        self.state.lock().unwrap().applied = None;
        self.apply();
    }

    /// Names of the games open now (the self-test leaves them alone).
    pub fn games_open(&self) -> Vec<String> {
        self.game_windows().into_iter().map(|g| g.id.name).collect()
    }

    /// The classified windows of games, one per game (the focused window, else the
    /// first in Z order).
    fn game_windows(&self) -> Vec<Active> {
        let settings = self.store.get();
        let catalog = self.catalog();
        let me = std::process::id();
        let foreground = win::foreground_window();
        let mut found: Vec<Active> = Vec::new();
        let mut paths = std::collections::HashMap::new();
        for w in win::app_windows() {
            let AppWindow { hwnd, pid, title, class, rect } = w;
            if pid == me || rect.right - rect.left < MIN_WIDTH || rect.bottom - rect.top < MIN_HEIGHT {
                continue;
            }
            let Some(path) = paths.entry(pid).or_insert_with(|| win::process_path(pid)).clone() else { continue };
            let Verdict::Game(id) = catalog.classify(&path, &settings) else { continue };
            let active = Active { id, path, pid, hwnd, title, class, since: Instant::now() };
            match found.iter_mut().find(|a| a.id.key == active.id.key) {
                Some(existing) if hwnd == foreground => *existing = active,
                Some(_) => {}
                None => found.push(active),
            }
        }
        found
    }

    fn scan(&self) {
        if self.mode() != Mode::Detect {
            return;
        }
        let found = self.game_windows();
        let foreground = win::foreground_window();
        let (started, ended) = {
            let mut s = self.state.lock().unwrap();
            let mut ended = None;
            if let Some(current) = s.current.clone() {
                match found.iter().find(|a| a.id.key == current.id.key) {
                    Some(same) => {
                        s.missing = 0;
                        s.current = Some(Active { since: current.since, ..same.clone() });
                    }
                    None => {
                        s.missing += 1;
                        if s.missing >= MISSING_SCANS {
                            ended = s.current.take();
                            s.missing = 0;
                        }
                    }
                }
            }
            // Another game in focus takes over after a few scans.
            let focused = found.iter().find(|a| a.hwnd == foreground && s.current.as_ref().is_some_and(|c| c.id.key != a.id.key)).cloned();
            let mut started = None;
            match focused {
                Some(other) => {
                    let scans = match &s.focus {
                        Some((key, n)) if *key == other.id.key => n + 1,
                        _ => 1,
                    };
                    s.focus = Some((other.id.key.clone(), scans));
                    if scans >= FOCUS_SCANS {
                        ended = s.current.replace(other.clone());
                        started = Some(other);
                        s.focus = None;
                    }
                }
                None => s.focus = None,
            }
            if s.current.is_none() {
                let next = found.iter().find(|a| a.hwnd == foreground).or(found.first()).cloned();
                if let Some(next) = next {
                    s.current = Some(next.clone());
                    started = Some(next);
                }
            }
            if ended.is_some() || started.is_some() {
                s.hook_checked = None;
                s.needs_admin = None;
                s.fallback = None;
            }
            (started, ended)
        };
        if let Some(game) = &ended {
            log!("game closed or left: {} ({})", game.id.name, game.path);
        }
        if let Some(game) = &started {
            log!("game found: {} [{}] {} pid {} window {:?}", game.id.name, game.id.key, game.path, game.pid, game.title);
        }
        if started.is_some() || ended.is_some() {
            self.apply();
            let _ = self.app.emit("game-state", self.snapshot());
            if let Some(game) = started.filter(|_| self.recording()) {
                let secs = self.app.state::<Engine>().state()["info"]["bufferSecs"].as_u64().unwrap_or_default();
                self.app.state::<Notifier>().notify(Kind::Info, "Recording", &format!("{} · Laurel keeps the last {secs} s", game.id.name));
            }
        }
        self.check_hook();
    }

    /// A game that runs as administrator cannot be recorded by a Laurel that does
    /// not: the engine cannot open its process to hook it, and its keys do not reach
    /// the hotkeys. When the game is still not hooked HOOK_GRACE after it was found
    /// and its process is elevated (or its token cannot be read), the player is
    /// offered a restart of Laurel as administrator.
    fn check_hook(&self) {
        let game = {
            let s = self.state.lock().unwrap();
            let hooking = wanted(&s).0.starts_with("game:");
            match &s.current {
                Some(g) if hooking && s.hook_checked.as_deref() != Some(g.id.key.as_str()) && g.since.elapsed() >= HOOK_GRACE => g.clone(),
                _ => return,
            }
        };
        let hooked = if self.elevated {
            Ok(true)
        } else {
            self.app.state::<Engine>().request("status", Value::Null, REQUEST_TIMEOUT).map(|status| !status["game"].is_null())
        };
        let Ok(hooked) = hooked else { return };
        let elevated = if hooked { Some(false) } else { win::process_elevated(game.pid) };
        let needs_admin = !hooked && elevated != Some(false);
        // A game that is not elevated gets until FALLBACK_GRACE before its window is
        // recorded instead.
        let fallback = !hooked && !needs_admin && game.since.elapsed() >= FALLBACK_GRACE;
        if !hooked && !needs_admin && !fallback {
            return;
        }
        {
            let mut s = self.state.lock().unwrap();
            if s.current.as_ref().map(|g| &g.id.key) != Some(&game.id.key) {
                return;
            }
            s.hook_checked = Some(game.id.key.clone());
            if needs_admin {
                s.needs_admin = Some(game.id.key.clone());
            }
            if fallback {
                s.fallback = Some(game.id.key.clone());
            }
        }
        if hooked {
            return;
        }
        log!("capture: {} not hooked after {} s; process elevated: {elevated:?}", game.id.name, game.since.elapsed().as_secs());
        let notifier = self.app.state::<Notifier>();
        if needs_admin {
            let _ = self.app.emit("game-state", self.snapshot());
            notifier.notify(Kind::Error, &format!("Can't record {}", game.id.name), "It runs as administrator. Open Laurel and restart it as administrator.");
        } else {
            self.apply();
            let _ = self.app.emit("game-state", self.snapshot());
            notifier.notify(Kind::Info, &format!("Recording {} by its window", game.id.name), "Game capture did not start, so Laurel records the game's window");
        }
    }

    /// Brings the engine to the wanted capture target and recording state.
    fn apply(&self) {
        let _serial = self.apply_lock.lock().unwrap();
        let (target, recording, applied) = {
            let s = self.state.lock().unwrap();
            let (t, r) = wanted(&s);
            (t, r, s.applied.clone())
        };
        if applied.as_ref() == Some(&(target.clone(), recording)) {
            return;
        }
        let engine = self.app.state::<Engine>();
        if engine.state()["state"] != "ready" {
            return; // engine_ready applies it
        }
        let (have_target, have_recording) = applied.unwrap_or_else(|| initial(&self.state.lock().unwrap()));
        if have_target != target {
            if let Err(e) = engine.request("setCapture", json!({ "target": target }), REQUEST_TIMEOUT) {
                log!("capture: setCapture {target} failed: {e}");
                return;
            }
        }
        if have_recording != recording {
            if let Err(e) = engine.request("setRecording", json!({ "enabled": recording }), REQUEST_TIMEOUT) {
                log!("capture: setRecording {recording} failed: {e}");
                return;
            }
        }
        log!("capture: target {target}, recording {recording}");
        let screen_started = target.starts_with("monitor") && !have_target.starts_with("monitor") && recording;
        self.state.lock().unwrap().applied = Some((target, recording));
        if screen_started {
            let secs = engine.state()["info"]["bufferSecs"].as_u64().unwrap_or_default();
            self.app.state::<Notifier>().notify(Kind::Info, "Recording the screen", &format!("Laurel keeps the last {secs} s of your screen"));
        }
    }
}

/// What the engine starts with (see engine_args).
fn initial(s: &State) -> (String, bool) {
    match &s.mode {
        Mode::Fixed(target) if s.enabled => (target.clone(), true),
        _ => ("none".into(), false),
    }
}

/// The capture target and recording state the engine should have.
fn wanted(s: &State) -> (String, bool) {
    if !s.enabled {
        return ("none".into(), false);
    }
    match &s.mode {
        Mode::Fixed(target) => (target.clone(), !s.disk_paused),
        Mode::Detect => {
            let game = s.current.as_ref().filter(|_| s.screen.as_ref().is_none_or(|screen| screen.switch_to_games));
            match (game, &s.screen) {
                (Some(game), _) if s.fallback.as_deref() == Some(game.id.key.as_str()) => (format!("window:{}", game.spec()), !s.disk_paused),
                (Some(game), _) => (format!("game:{}", game.spec()), !s.disk_paused),
                (None, Some(screen)) => (screen.target(), !s.disk_paused),
                (None, None) => ("none".into(), false),
            }
        }
    }
}

/// game, window, screen or none, for the UI.
fn capture_kind(target: &str) -> &'static str {
    match target.split(':').next().unwrap_or_default() {
        "game" => "game",
        "window" => "window",
        t if t.starts_with("monitor") => "screen",
        _ => "none",
    }
}

/// The screen recording option from the settings.
pub fn screen_of(settings: &crate::settings::Settings) -> Option<Screen> {
    settings.screen_recording.then(|| Screen { monitor: settings.screen_monitor.clone(), switch_to_games: settings.screen_game_switch })
}

/// Windows of running programs that are not games, for "add a running app".
pub fn running_programs(detector: &Detector) -> Vec<Value> {
    let settings = detector.store.get();
    let catalog = detector.catalog();
    let me = std::process::id();
    let mut seen = std::collections::HashSet::new();
    let mut list = Vec::new();
    for w in win::app_windows() {
        if w.pid == me || w.title.is_empty() {
            continue;
        }
        let Some(path) = win::process_path(w.pid) else { continue };
        if !seen.insert(path.to_lowercase()) || catalog.classify(&path, &settings) != Verdict::NotGame {
            continue;
        }
        let lower = path.to_lowercase();
        if lower.starts_with(r"c:\windows\") || lower.ends_with(r"\explorer.exe") {
            continue;
        }
        list.push(json!({ "path": path, "title": w.title }));
    }
    list
}

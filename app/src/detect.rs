//! The app's side of game detection (spec game.detection): the thread that scans
//! the windows every 2 s, the catalog of games read from the store libraries, and
//! the Windows, engine and interface behind the traits of
//! laurel_app_core::detect, where the rules and the requests to the engine live.
//!
//! With a fixed target (LAUREL_CAPTURE, or the self-test), detection is off and the
//! engine records that target all the time, as in the Phase 3 PoC.

use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

use laurel_app_core::detect::{self as core, Config, EngineLink, Notice, Reporter};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::engine::Engine;
use crate::games::{self, Catalog, Curated, Verdict};
use laurel_app_core::stores::{self, Report};
use crate::i18n::t;
use crate::log::log;
use crate::overlay::{Kind, Notifier};
use crate::settings::Store;
use crate::win::{self, Win32};

pub use laurel_app_core::detect::{Active, Mode, Screen};

const CATALOG_EVERY: Duration = Duration::from_secs(60);

/// The engine the app supervises (engine.rs), looked up when needed: the detector
/// exists before the engine starts.
pub struct EngineRpc(AppHandle);

impl EngineLink for EngineRpc {
    fn ready(&self) -> bool {
        self.0.state::<Engine>().state()["state"] == "ready"
    }

    fn buffer_secs(&self) -> u64 {
        self.0.state::<Engine>().state()["info"]["bufferSecs"].as_u64().unwrap_or_default()
    }

    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.0.state::<Engine>().request(method, params, timeout)
    }
}

/// app.log, the overlay and the "game-state" event of the interface.
pub struct AppReporter(AppHandle);

impl Reporter for AppReporter {
    fn log(&self, line: &str) {
        log!("{line}");
    }

    fn notice(&self, notice: Notice) {
        let notifier = self.0.state::<Notifier>();
        match notice {
            Notice::Recording { game, secs } => notifier.notify(Kind::Info, &t!("Recording|notice"), &t!("{game} · Laurel keeps the last {secs} s", game = game, secs = secs)),
            Notice::RecordingScreen { secs } => notifier.notify(Kind::Info, &t!("Recording the screen"), &t!("Laurel keeps the last {secs} s of your screen", secs = secs)),
            Notice::RecordingByWindow { game } => notifier.notify(Kind::Info, &t!("Recording {game} by its window", game = game), &t!("Game capture did not start, so Laurel records the game's window")),
            // A15: shown over the game even with notices over games off.
            Notice::NeedsAdmin { game } => notifier.notify_critical(&t!("Can't record {game}", game = game), &t!("It runs as administrator. Open Laurel and restart it as administrator.")),
        }
    }

    fn game_state(&self, snapshot: Value) {
        let _ = self.0.emit("game-state", snapshot);
    }
}

pub struct Detector {
    core: core::Detector<Win32, EngineRpc, AppReporter>,
    store: Arc<Store>,
    /// The curated list the catalog is built from: the shipped one, except while
    /// the self-test uses its own.
    curated: Mutex<Curated>,
    /// Replaced whole at each scan of the stores, so that a scan of the windows
    /// holds no lock while it waits for the engine.
    catalog: RwLock<Arc<Catalog>>,
    /// What the last scan of each store saw; a store's line is logged again only
    /// when it changes (A32).
    store_reports: Mutex<Vec<Report>>,
    wake: (Mutex<bool>, Condvar),
}

impl Detector {
    pub fn new(app: AppHandle, store: Arc<Store>, mode: Mode) -> Arc<Detector> {
        let curated = Curated::shipped();
        let (catalog, reports) = games::scan(curated.clone());
        let settings = store.get();
        log!("games: {} installed games found {}", catalog.store_games().len(), catalog.counts());
        for report in &reports {
            log!("games: {report}");
        }
        let config = Config {
            elevated: win::self_elevated(),
            own_pid: std::process::id(),
            test_fallback: std::env::var("LAUREL_TEST_FALLBACK").is_ok_and(|v| v == "1"),
        };
        Arc::new(Detector {
            core: core::Detector::new(Win32, EngineRpc(app.clone()), AppReporter(app), mode, settings.recording_enabled, screen_of(&settings), config),
            store,
            curated: Mutex::new(curated),
            catalog: RwLock::new(Arc::new(catalog)),
            store_reports: Mutex::new(reports),
            wake: (Mutex::new(false), Condvar::new()),
        })
    }

    /// The engine's start options for the current mode: no capture and a paused
    /// replay buffer until a game shows up.
    pub fn engine_args(&self) -> Vec<String> {
        self.core.engine_args()
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
                me.tick();
                let (lock, cvar) = &me.wake;
                let mut woken = cvar.wait_timeout_while(lock.lock().unwrap(), core::SCAN_EVERY, |w| !*w).unwrap().0;
                *woken = false;
            }
        });
    }

    fn tick(&self) {
        let settings = self.store.get();
        self.core.tick(&self.catalog(), &settings.user_games());
    }

    /// Scans again now (after the user changes the game lists).
    pub fn wake(&self) {
        *self.wake.0.lock().unwrap() = true;
        self.wake.1.notify_all();
    }

    pub fn rescan(&self) {
        let curated = self.curated.lock().unwrap().clone();
        let (catalog, reports) = games::scan(curated);
        {
            let mut before = self.store_reports.lock().unwrap();
            for report in stores::changed(&before, &reports) {
                log!("games: {report}");
            }
            *before = reports;
        }
        let mut current = self.catalog.write().unwrap();
        if catalog.store_games().len() != current.store_games().len() {
            log!("games: {} installed games found {}", catalog.store_games().len(), catalog.counts());
        }
        *current = Arc::new(catalog);
    }

    pub fn catalog(&self) -> Arc<Catalog> {
        self.catalog.read().unwrap().clone()
    }

    /// Builds the catalog from another curated list (self-test: a window-only game
    /// it can open), or from the shipped one again.
    pub fn set_curated(&self, curated: Curated) {
        *self.curated.lock().unwrap() = curated;
        self.rescan();
    }

    pub fn mode(&self) -> Mode {
        self.core.mode()
    }

    pub fn set_mode(&self, mode: Mode) {
        self.core.set_mode(mode);
        self.wake();
    }

    /// Whether the current game's hook was checked.
    pub fn hook_checked(&self) -> bool {
        self.core.hook_checked()
    }

    pub fn enabled(&self) -> bool {
        self.core.enabled()
    }

    /// Turns recording on or off (spec record.toggle).
    pub fn set_enabled(&self, enabled: bool) {
        self.core.set_enabled(enabled);
    }

    pub fn current(&self) -> Option<Active> {
        self.core.current()
    }

    /// Applies a change of the screen recording option.
    pub fn set_screen(&self, screen: Option<Screen>) {
        self.core.set_screen(screen);
    }

    /// The name of the current game when it runs as administrator and Laurel does
    /// not, so that it is not recorded.
    pub fn needs_admin_game(&self) -> Option<String> {
        self.core.needs_admin_game()
    }

    /// Whether the replay buffer should be running.
    pub fn recording(&self) -> bool {
        self.core.recording()
    }

    pub fn snapshot(&self) -> Value {
        self.core.snapshot()
    }

    /// Called by the disk guard (spec storage.limits).
    pub fn set_disk_paused(&self, paused: bool) {
        self.core.set_disk_paused(paused);
    }

    /// The engine (re)started with the options of `engine_args`.
    pub fn engine_ready(&self) {
        self.core.engine_ready();
    }

    /// Starts the next try of the fallback game's hook now (self-test).
    pub fn retry_hook_now(&self) {
        self.core.retry_hook_now();
    }

    /// Names of the games open now (the self-test leaves them alone).
    pub fn games_open(&self) -> Vec<String> {
        let settings = self.store.get();
        self.core.games_open(&self.catalog(), &settings.user_games())
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
        if !seen.insert(path.to_lowercase()) || catalog.classify_window(&path, &w.class, &w.title, || win::process_command_line(w.pid), &settings.user_games()) != Verdict::NotGame {
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

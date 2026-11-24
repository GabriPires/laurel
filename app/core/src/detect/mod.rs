//! Game detection and capture control (spec game.detection). Every scan looks at
//! the visible windows and classifies their programs (games.rs); the game being
//! recorded stays while its window lives, minimized or not, and another game takes
//! over when it stays in focus. This module alone tells the engine what to capture
//! and whether the replay buffer runs: recording turned on (spec record.toggle), a
//! game and enough disk space are all needed; with recording off nothing is
//! captured or hooked.
//!
//! A game is captured by its hook, or by its window when the hook does not start
//! (spec capture.game-hook); a window-only game of the curated list (A39: Roblox)
//! by its window from the start, with no hook check and no hook retries. With the
//! screen recording option (capture.screen, off by default per D13), a monitor is
//! recorded while no game is open, or always when the switch to games is off.
//!
//! With a fixed target (LAUREL_CAPTURE, or the self-test), detection is off and the
//! engine records that target all the time, as in the Phase 3 PoC.
//!
//! The rules live in [`rules`], with no input or output. [`Detector`] is the
//! orchestration: it reaches Windows through [`Platform`], the engine through
//! [`EngineLink`] and the log, the overlay and the interface through [`Reporter`],
//! feeds the rules what it sees and carries out what they ask, in order. The app
//! implements the traits, runs [`Detector::tick`] on its own thread every
//! [`SCAN_EVERY`], and calls the other methods from wherever the change comes.
//! Every instant comes from [`Platform::now`].

pub mod rules;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::games::{Catalog, UserGames, Verdict};
use rules::{Command, Detection, HookCheck, RetryStep, Scan};

pub use rules::{Active, AppWindow, Config, Mode, Screen, SCAN_EVERY};

/// How long each request to the engine waits for its answer.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Windows as the detector sees it.
pub trait Platform {
    /// The visible, unowned, not minimized, not cloaked top-level windows that are
    /// not tool windows, in Z order.
    fn app_windows(&self) -> Vec<AppWindow>;
    /// The window in focus, or 0.
    fn foreground_window(&self) -> isize;
    /// Whether `hwnd` still exists and belongs to process `pid`, minimized, hidden
    /// or cloaked included (app_windows leaves those out).
    fn window_of(&self, hwnd: isize, pid: u32) -> bool;
    /// The full path of a process's program, as Windows writes it.
    fn process_path(&self, pid: u32) -> Option<String>;
    /// The command line a process was started with, when it can be read.
    fn process_command_line(&self, pid: u32) -> Option<String>;
    /// Whether a process runs as administrator; None when its token cannot be read.
    fn process_elevated(&self, pid: u32) -> Option<bool>;
    /// The current instant of a monotonic clock.
    fn now(&self) -> Instant;
}

/// The engine as the detector sees it.
pub trait EngineLink {
    /// Whether the engine runs and has said it is ready for requests.
    fn ready(&self) -> bool;
    /// The replay buffer's length the ready engine reported, in seconds.
    fn buffer_secs(&self) -> u64;
    /// Sends a request and waits up to `timeout` for its answer; the error is the
    /// text the log shows.
    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String>;
}

/// A notice over the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// A game was found and is recorded: Laurel keeps its last `secs` seconds.
    Recording { game: String, secs: u64 },
    /// The screen is recorded: Laurel keeps its last `secs` seconds.
    RecordingScreen { secs: u64 },
    /// The game's hook did not start, so its window is recorded.
    RecordingByWindow { game: String },
    /// The game runs as administrator and Laurel does not (shown over the game even
    /// with notices over games off: A15).
    NeedsAdmin { game: String },
}

/// Where the detector tells what happened.
pub trait Reporter {
    /// A line of app.log.
    fn log(&self, line: &str);
    fn notice(&self, notice: Notice);
    /// What the interface shows changed; `snapshot` is [`Detector::snapshot`].
    fn game_state(&self, snapshot: Value);
}

pub struct Detector<P, E, R> {
    platform: P,
    engine: E,
    reporter: R,
    detection: Mutex<Detection>,
    /// Serializes the requests that bring the engine to the wanted state.
    apply_lock: Mutex<()>,
    own_pid: u32,
}

impl<P: Platform, E: EngineLink, R: Reporter> Detector<P, E, R> {
    /// A detector in `mode`, with recording on or off and the screen option as the
    /// settings have them.
    pub fn new(platform: P, engine: E, reporter: R, mode: Mode, enabled: bool, screen: Option<Screen>, config: Config) -> Self {
        Detector { platform, engine, reporter, detection: Mutex::new(Detection::new(mode, enabled, screen, config)), apply_lock: Mutex::new(()), own_pid: config.own_pid }
    }

    fn detection(&self) -> std::sync::MutexGuard<'_, Detection> {
        self.detection.lock().unwrap()
    }

    /// The engine's start options for the current mode: no capture and a paused
    /// replay buffer until a game shows up.
    pub fn engine_args(&self) -> Vec<String> {
        self.detection().engine_args()
    }

    pub fn mode(&self) -> Mode {
        self.detection().mode().clone()
    }

    /// Changes the mode; the app scans again right after.
    pub fn set_mode(&self, mode: Mode) {
        self.detection().set_mode(mode);
        self.apply();
    }

    /// Whether the current game's hook was checked.
    pub fn hook_checked(&self) -> bool {
        self.detection().hook_checked()
    }

    pub fn enabled(&self) -> bool {
        self.detection().enabled()
    }

    /// Turns recording on or off (spec record.toggle). Off releases the capture and
    /// the replay buffer; on starts them again for the current game, if any.
    pub fn set_enabled(&self, enabled: bool) {
        self.detection().set_enabled(enabled);
        self.apply();
        self.reporter.game_state(self.snapshot());
    }

    pub fn current(&self) -> Option<Active> {
        self.detection().current().cloned()
    }

    /// Applies a change of the screen recording option.
    pub fn set_screen(&self, screen: Option<Screen>) {
        if self.detection().set_screen(screen) {
            self.apply();
            self.reporter.game_state(self.snapshot());
        }
    }

    /// The name of the current game when it runs as administrator and Laurel does
    /// not, so that it is not recorded.
    pub fn needs_admin_game(&self) -> Option<String> {
        self.detection().needs_admin_game()
    }

    /// Whether the replay buffer should be running.
    pub fn recording(&self) -> bool {
        self.detection().recording()
    }

    pub fn snapshot(&self) -> Value {
        self.detection().snapshot()
    }

    /// Called by the disk guard (spec storage.limits).
    pub fn set_disk_paused(&self, paused: bool) {
        self.detection().set_disk_paused(paused);
        self.apply();
    }

    /// The engine (re)started with the options of `engine_args`.
    pub fn engine_ready(&self) {
        let now = self.platform.now();
        self.detection().engine_ready(now);
        self.apply();
    }

    /// Starts the next try of the fallback game's hook now (self-test).
    pub fn retry_hook_now(&self) {
        let now = self.platform.now();
        self.detection().retry_now(now);
    }

    /// Names of the games open now (the self-test leaves them alone).
    pub fn games_open(&self, catalog: &Catalog, user: &UserGames) -> Vec<String> {
        self.game_windows(catalog, user).into_iter().map(|g| g.id.name).collect()
    }

    /// The classified windows of games, one per game (see rules::one_per_game).
    fn game_windows(&self, catalog: &Catalog, user: &UserGames) -> Vec<Active> {
        let foreground = self.platform.foreground_window();
        let mut games = Vec::new();
        let mut paths = std::collections::HashMap::new();
        for window in self.platform.app_windows().into_iter().filter(|w| rules::may_be_game(w, self.own_pid)) {
            let AppWindow { hwnd, pid, title, class, .. } = window;
            let Some(path) = paths.entry(pid).or_insert_with(|| self.platform.process_path(pid)).clone() else { continue };
            let Verdict::Game(id) = catalog.classify_window(&path, &class, &title, || self.platform.process_command_line(pid), user) else { continue };
            let capture = catalog.capture(&path);
            games.push(Active { id, path, pid, hwnd, title, class, capture });
        }
        rules::one_per_game(games, foreground)
    }

    /// One scan of the windows, then the hook check and the hook retries; the app
    /// calls it every SCAN_EVERY, and right away after a change of the game lists.
    pub fn tick(&self, catalog: &Catalog, user: &UserGames) {
        if self.mode() != Mode::Detect {
            return;
        }
        let games = self.game_windows(catalog, user);
        let foreground = self.platform.foreground_window();
        let commands = {
            let mut rules = self.detection();
            let current_window = rules.current().is_some_and(|c| self.platform.window_of(c.hwnd, c.pid));
            rules.scan(Scan { games, foreground, current_window }, self.platform.now())
        };
        self.run(commands);
        self.check_hook();
        self.retry_hook();
    }

    /// Carries out the rules' commands, in order.
    fn run(&self, commands: Vec<Command>) {
        for command in commands {
            match command {
                Command::Log(line) => self.reporter.log(&line),
                Command::GameState => self.reporter.game_state(self.snapshot()),
                Command::Apply => self.apply(),
                Command::AnnounceGame { game } => self.reporter.notice(Notice::Recording { game, secs: self.engine.buffer_secs() }),
                Command::AnnounceScreen => self.reporter.notice(Notice::RecordingScreen { secs: self.engine.buffer_secs() }),
                Command::AnnounceByWindow { game } => self.reporter.notice(Notice::RecordingByWindow { game }),
                Command::AnnounceNeedsAdmin { game } => self.reporter.notice(Notice::NeedsAdmin { game }),
                Command::CheckHookAfterMove(game) => {
                    let Some(hooked) = self.hooked() else { continue };
                    let next = self.detection().answer_moved(&game, hooked, self.platform.now());
                    self.run(next);
                }
                Command::EndHookRetry => {
                    if let Err(e) = self.engine.request("probeHook", json!({ "enabled": false }), REQUEST_TIMEOUT) {
                        self.reporter.log(&format!("capture: probeHook failed: {e}"));
                    }
                }
            }
        }
    }

    /// Whether the engine's game capture has the hook; None when the engine does
    /// not answer, and the rules ask again later.
    fn hooked(&self) -> Option<bool> {
        self.engine.request("status", Value::Null, REQUEST_TIMEOUT).ok().map(|status| !status["game"].is_null())
    }

    /// The hook check, when one is due: the engine's answer and, for a game it has
    /// not hooked, whether the game runs as administrator.
    fn check_hook(&self) {
        let Some(check): Option<HookCheck> = self.detection().hook_check(self.platform.now()) else { return };
        let Some(hooked) = self.hooked() else { return };
        let elevated = if hooked { None } else { self.platform.process_elevated(check.game.pid) };
        let commands = self.detection().answer_hook_check(&check, hooked, elevated, self.platform.now());
        self.run(commands);
    }

    /// The step of the hook retries, when one is due: start a try, or ask the
    /// engine whether the running one has the hook.
    fn retry_hook(&self) {
        let now = self.platform.now();
        let Some(step): Option<RetryStep> = self.detection().hook_retry(now) else { return };
        let commands = match step {
            RetryStep::Start { game, tries } => {
                if let Err(e) = self.engine.request("probeHook", json!({ "enabled": true }), REQUEST_TIMEOUT) {
                    self.reporter.log(&format!("capture: probeHook failed: {e}"));
                    return;
                }
                self.detection().hook_retry_started(&game, tries, now)
            }
            RetryStep::Check { game, tries, until } => {
                let Some(hooked) = self.hooked() else { return };
                self.detection().answer_hook_retry(&game, tries, until, hooked, now)
            }
        };
        self.run(commands);
    }

    /// Brings the engine to the wanted capture target and recording state.
    fn apply(&self) {
        let _serial = self.apply_lock.lock().unwrap();
        let Some(apply) = self.detection().apply() else { return };
        if !self.engine.ready() {
            return; // engine_ready applies it
        }
        if apply.changes_target() {
            if let Err(e) = self.engine.request("setCapture", json!({ "target": apply.target }), REQUEST_TIMEOUT) {
                self.reporter.log(&format!("capture: setCapture {} failed: {e}", apply.target));
                return;
            }
        }
        if apply.changes_recording() {
            if let Err(e) = self.engine.request("setRecording", json!({ "enabled": apply.recording }), REQUEST_TIMEOUT) {
                self.reporter.log(&format!("capture: setRecording {} failed: {e}", apply.recording));
                return;
            }
        }
        let commands = self.detection().applied(&apply, self.platform.now());
        self.run(commands);
    }
}

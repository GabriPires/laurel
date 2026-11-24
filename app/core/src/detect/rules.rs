//! The rules of game detection, with no input or output of their own (sans-IO):
//! which game is the current one, when it counts as closed, when another game takes
//! the capture, the target the engine should have, when the hook is checked, when
//! the game's window is recorded instead, the hook retries and the game that runs
//! as administrator. What Windows and the engine show comes in as observations,
//! with the instant; what should happen next goes out as [`Command`]s, which the
//! orchestration ([`super::Detector`]) carries out, giving the engine's answers
//! back here.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::games::{Capture, GameId};

/// How often the app scans the windows.
pub const SCAN_EVERY: Duration = Duration::from_secs(2);
/// Smaller windows are launchers, splash screens or dialogs, not the game itself.
const MIN_WIDTH: i32 = 640;
const MIN_HEIGHT: i32 = 360;
/// Scans without the game's window before it counts as closed (a game recreates
/// its window when it changes display mode). A minimized window still counts: a
/// game in exclusive fullscreen minimizes on Alt+Tab.
const MISSING_SCANS: u32 = 3;
/// Scans another game must stay in focus before the capture moves to it.
const FOCUS_SCANS: u32 = 2;
/// The same when the current game's capture is hooked (A27 of the stability tests):
/// 30 s, so that a quick switch to another program the player added, such as a
/// music player, does not take the capture from the game being played, while a
/// game started and played next to an idle one still gets it.
const HOOKED_FOCUS_SCANS: u32 = 15;
/// A game not hooked this long after the engine started capturing it is checked
/// for elevation.
const HOOK_GRACE: Duration = Duration::from_secs(8);
/// A game that is not elevated and still not hooked this long after the engine
/// started capturing it is recorded through its window instead.
const FALLBACK_GRACE: Duration = Duration::from_secs(15);
/// While a game is recorded by its window, its hook is tried again over the window
/// capture: the first try starts RETRY_FIRST after the fallback, each lasts
/// RETRY_FOR, and the wait doubles after each, up to RETRY_MAX. A waiting game
/// capture logs a few lines every 2 s, so the tries stay short and sparse.
const RETRY_FIRST: Duration = Duration::from_secs(30);
const RETRY_FOR: Duration = Duration::from_secs(10);
const RETRY_MAX: Duration = Duration::from_secs(300);

/// Scans in focus another game needs before it takes the capture from the current
/// one, which is or is not hooked.
fn focus_scans_needed(current_hooked: bool) -> u32 {
    if current_hooked { HOOKED_FOCUS_SCANS } else { FOCUS_SCANS }
}

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

/// A top-level window a player could be looking at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppWindow {
    pub hwnd: isize,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub width: i32,
    pub height: i32,
}

/// Whether a window can be a game's, before its program is classified: Laurel's
/// own windows never are, and windows smaller than 640x360 are launchers, splash
/// screens or dialogs, not the game itself.
pub fn may_be_game(window: &AppWindow, own_pid: u32) -> bool {
    window.pid != own_pid && window.width >= MIN_WIDTH && window.height >= MIN_HEIGHT
}

/// The games' windows, given in Z order, one per game: the window in focus, else
/// the first in Z order.
pub fn one_per_game(windows: Vec<Active>, foreground: isize) -> Vec<Active> {
    let mut found: Vec<Active> = Vec::new();
    for window in windows {
        match found.iter_mut().find(|a| a.id.key == window.id.key) {
            Some(existing) if window.hwnd == foreground => *existing = window,
            Some(_) => {}
            None => found.push(window),
        }
    }
    found
}

/// A game's window: a game the scan found, or the game being recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct Active {
    pub id: GameId,
    pub path: String,
    pub pid: u32,
    pub hwnd: isize,
    pub title: String,
    pub class: String,
    /// By its hook (with the window fallback) or, for a window-only game, by its
    /// window.
    pub capture: Capture,
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

/// What the detector knows of the app it runs in.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Whether Laurel runs as administrator.
    pub elevated: bool,
    /// Laurel's own process, whose windows are never games.
    pub own_pid: u32,
    /// LAUREL_TEST_FALLBACK=1 (tests): a game is recorded by its window from the
    /// start, as if its hook had not started, so the tries of its hook can be seen.
    pub test_fallback: bool,
}

/// What one scan of the windows saw.
#[derive(Clone, Debug)]
pub struct Scan {
    /// The games' windows, one per game (the focused window, else the first in Z
    /// order), in Z order.
    pub games: Vec<Active>,
    /// The window in focus.
    pub foreground: isize,
    /// Whether the current game's window still exists, minimized, hidden or
    /// cloaked included (not asked without a current game).
    pub current_window: bool,
}

/// What should happen next, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// A line of app.log.
    Log(String),
    /// Tell the interface: what it shows changed.
    GameState,
    /// Bring the engine to the wanted target ([`Detection::apply`]).
    Apply,
    /// Tell the player the game is recorded, with the length of the replay buffer.
    AnnounceGame { game: String },
    /// Tell the player the screen is recorded, with the length of the replay buffer.
    AnnounceScreen,
    /// The game's hook did not start, so its window is recorded.
    AnnounceByWindow { game: String },
    /// The game runs as administrator and Laurel does not.
    AnnounceNeedsAdmin { game: String },
    /// Ask the engine whether the game, whose window moved to another of its
    /// processes, is hooked, and give the answer to [`Detection::answer_moved`].
    CheckHookAfterMove(Active),
    /// End the hook retry running over the window capture.
    EndHookRetry,
}

/// The hook check that is due ([`Detection::hook_check`]).
#[derive(Clone, Debug, PartialEq)]
pub struct HookCheck {
    pub game: Active,
    /// How long the engine has been capturing the game.
    pub capturing: Duration,
}

/// The tries of the hook of a game recorded by its window.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Retry {
    /// When the next try starts.
    next: Instant,
    /// When the current try ends, during a try.
    until: Option<Instant>,
    /// Tries already made.
    tries: u32,
}

impl Retry {
    fn after_fallback(now: Instant) -> Retry {
        Retry { next: now + RETRY_FIRST, until: None, tries: 0 }
    }
}

/// The step of the hook retries that is due ([`Detection::hook_retry`]), with the
/// tries already made.
#[derive(Clone, Debug, PartialEq)]
pub enum RetryStep {
    /// A try starts: the engine is asked for a game capture over the window
    /// capture, and then told [`Detection::hook_retry_started`].
    Start { game: Active, tries: u32 },
    /// A try runs until `until`: the engine is asked whether it has the hook, and
    /// its answer goes to [`Detection::answer_hook_retry`].
    Check { game: Active, tries: u32, until: Instant },
}

/// What [`Detection::apply`] asks of the engine: the target and recording state
/// wanted, and those the engine has.
#[derive(Clone, Debug, PartialEq)]
pub struct Apply {
    pub target: String,
    pub recording: bool,
    pub have_target: String,
    pub have_recording: bool,
    /// The process of the game the target names.
    pid: Option<u32>,
}

impl Apply {
    pub fn changes_target(&self) -> bool {
        self.target != self.have_target
    }

    pub fn changes_recording(&self) -> bool {
        self.recording != self.have_recording
    }
}

/// The detection's state and its rules.
pub struct Detection {
    config: Config,
    mode: Mode,
    /// Recording turned on by the user (spec record.toggle).
    enabled: bool,
    current: Option<Active>,
    missing: u32,
    focus: Option<(String, u32)>,
    disk_paused: bool,
    /// What the engine was last told: (capture target, recording).
    applied: Option<(String, bool)>,
    /// The process of the game window the applied target names (see answer_moved).
    applied_pid: Option<u32>,
    /// Another process of the current game whose window took over, and the scans
    /// it has been the current one (see scan).
    moved: Option<(u32, u32)>,
    /// A process the capture did not follow because the game was already hooked.
    kept: Option<u32>,
    /// When the engine started capturing the applied target (see hook_check).
    capture_started: Option<Instant>,
    /// The game whose hook was already checked (see hook_check).
    hook_checked: Option<String>,
    /// The game that runs as administrator while Laurel does not.
    needs_admin: Option<String>,
    /// The process of a game confirmed to run as administrator while Laurel does
    /// not (A29 of the stability tests): its capture waits, instead of OBS trying
    /// to open the process every 2 s, until Laurel runs as administrator, the game
    /// closes or its window moves to another process.
    parked: Option<u32>,
    /// The game recorded through its window because its hook did not start (a
    /// window-only game never is: see Active::capture).
    fallback: Option<String>,
    /// The tries of the fallback game's hook.
    retry: Option<Retry>,
    screen: Option<Screen>,
}

impl Detection {
    /// In `mode`, with recording on or off and the screen option as the settings
    /// have them, and an engine started with [`Detection::engine_args`].
    pub fn new(mode: Mode, enabled: bool, screen: Option<Screen>, config: Config) -> Detection {
        Detection { config, mode, enabled, current: None, missing: 0, focus: None, disk_paused: false, applied: None, applied_pid: None, moved: None, kept: None, capture_started: None, hook_checked: None, needs_admin: None, parked: None, fallback: None, retry: None, screen }
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn current(&self) -> Option<&Active> {
        self.current.as_ref()
    }

    /// The engine's start options: no capture and a paused replay buffer while
    /// detecting, until a game shows up.
    pub fn engine_args(&self) -> Vec<String> {
        match self.initial() {
            (target, true) => vec!["--capture".into(), target],
            (target, false) => vec!["--capture".into(), target, "--paused".into()],
        }
    }

    /// What the engine starts with (see engine_args).
    fn initial(&self) -> (String, bool) {
        match &self.mode {
            Mode::Fixed(target) if self.enabled => (target.clone(), true),
            _ => ("none".into(), false),
        }
    }

    /// The capture target and recording state the engine should have.
    pub fn wanted(&self) -> (String, bool) {
        if !self.enabled {
            return ("none".into(), false);
        }
        let paused = self.disk_paused;
        match &self.mode {
            Mode::Fixed(target) => (target.clone(), !paused),
            Mode::Detect => {
                let game = self.current.as_ref().filter(|_| self.screen.as_ref().is_none_or(|screen| screen.switch_to_games));
                match (game, &self.screen) {
                    // A29: nothing can be recorded of a parked game; the screen, when the
                    // player records it without a game.
                    (Some(game), Some(screen)) if self.parked == Some(game.pid) => (screen.target(), !paused),
                    (Some(game), None) if self.parked == Some(game.pid) => ("none".into(), false),
                    (Some(game), _) if game.capture == Capture::Window || self.fallback.as_deref() == Some(game.id.key.as_str()) => (format!("window:{}", game.spec()), !paused),
                    (Some(game), _) => (format!("game:{}", game.spec()), !paused),
                    (None, Some(screen)) => (screen.target(), !paused),
                    (None, None) => ("none".into(), false),
                }
            }
        }
    }

    /// Whether the replay buffer should be running.
    pub fn recording(&self) -> bool {
        self.wanted().1
    }

    /// Whether the current game's hook was checked (see hook_check).
    pub fn hook_checked(&self) -> bool {
        self.current.as_ref().is_some_and(|g| self.hook_checked.as_deref() == Some(g.id.key.as_str()))
    }

    /// The name of the current game when it runs as administrator and Laurel does
    /// not, so that it is not recorded.
    pub fn needs_admin_game(&self) -> Option<String> {
        self.current.as_ref().filter(|g| self.needs_admin.as_deref() == Some(g.id.key.as_str())).map(|g| g.id.name.clone())
    }

    /// What the interface shows.
    pub fn snapshot(&self) -> Value {
        let (target, recording) = self.wanted();
        json!({
            "mode": if self.mode == Mode::Detect { "detect" } else { "fixed" },
            "enabled": self.enabled,
            "game": self.current.as_ref().map(Active::to_json),
            "recording": recording,
            "needsAdmin": self.needs_admin.is_some(),
            "capture": capture_kind(&target),
            "screen": self.screen.is_some(),
            "elevated": self.config.elevated,
        })
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.current = None;
        self.missing = 0;
    }

    /// Turns recording on or off (spec record.toggle). Off releases the capture and
    /// the replay buffer; on starts them again for the current game, if any.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        // The hook is checked again once the capture starts again.
        self.hook_checked = None;
        self.needs_admin = None;
        self.parked = None;
        self.fallback = None;
        self.retry = None;
    }

    /// The screen recording option; whether it changed.
    pub fn set_screen(&mut self, screen: Option<Screen>) -> bool {
        let changed = self.screen != screen;
        self.screen = screen;
        changed
    }

    /// The capture drive is almost full, or no longer (spec storage.limits).
    pub fn set_disk_paused(&mut self, paused: bool) {
        self.disk_paused = paused;
    }

    /// The engine (re)started with the options of `engine_args`.
    pub fn engine_ready(&mut self, now: Instant) {
        self.applied = None;
        // A26: the new engine hooks the game anew, so its hook is checked again
        // (a game hooked before a restart could stay without image); the
        // administrator notice is not shown a second time (see answer_hook_check).
        self.hook_checked = None;
        // A new engine has no hook retry running: the next one starts now.
        if let Some(retry) = self.retry.as_mut() {
            *retry = Retry { next: now, until: None, ..*retry };
        }
    }

    /// Starts the next try of the fallback game's hook now (self-test).
    pub fn retry_now(&mut self, now: Instant) {
        if let Some(retry) = self.retry.as_mut() {
            retry.next = now;
        }
    }

    /// One scan of the windows: which game is the current one, and what follows.
    pub fn scan(&mut self, scan: Scan, now: Instant) -> Vec<Command> {
        if self.mode != Mode::Detect {
            return Vec::new();
        }
        let Scan { games: found, foreground, current_window } = scan;
        let mut renamed = false;
        let mut ended = None;
        let mut held = None;
        if let Some(current) = self.current.clone() {
            match found.iter().find(|a| a.id.key == current.id.key) {
                Some(same) => {
                    self.missing = 0;
                    // The player renamed the game (A27): the app shows the new name.
                    renamed = same.id.name != current.id.name;
                    self.current = Some(same.clone());
                }
                // Minimized (or hidden) is not closed: the window is still there,
                // and so are the capture and the replay buffer.
                None if current_window => self.missing = 0,
                None => {
                    self.missing += 1;
                    if self.missing >= MISSING_SCANS {
                        ended = self.current.take();
                        self.missing = 0;
                    }
                }
            }
        }
        // Another game in focus takes over after a few scans, or after more of
        // them when the current game's capture is hooked (A27).
        let focused = found.iter().find(|a| a.hwnd == foreground && self.current.as_ref().is_some_and(|c| c.id.key != a.id.key)).cloned();
        let hook_confirmed = self.current.as_ref().is_some_and(|c| self.hook_checked.as_deref() == Some(c.id.key.as_str())) && self.fallback.is_none() && self.needs_admin.is_none();
        let mut started = None;
        match focused {
            Some(other) => {
                let scans = match &self.focus {
                    Some((key, n)) if *key == other.id.key => n + 1,
                    _ => 1,
                };
                self.focus = Some((other.id.key.clone(), scans));
                if scans >= focus_scans_needed(hook_confirmed) {
                    ended = self.current.replace(other.clone());
                    started = Some(other);
                    self.focus = None;
                } else if hook_confirmed && scans == FOCUS_SCANS {
                    held = self.current.as_ref().map(|c| (other.id.name.clone(), c.id.name.clone()));
                }
            }
            None => self.focus = None,
        }
        if self.current.is_none() {
            let next = found.iter().find(|a| a.hwnd == foreground).or(found.first()).cloned();
            if let Some(next) = next {
                self.current = Some(next.clone());
                started = Some(next);
            }
        }
        // A29: a parked game whose window is now another process's (a launcher
        // running as administrator, then the game itself) is checked again.
        let mut unparked = None;
        let moved_on = self.current.clone().filter(|c| self.parked.is_some_and(|pid| pid != c.pid));
        if let Some(c) = moved_on.filter(|_| started.is_none() && ended.is_none()) {
            self.parked = None;
            self.hook_checked = None;
            unparked = Some(c);
        }
        // A23: the game's window moved to another of its processes, as Fortnite's
        // from its anti-cheat bootstrapper's splash screen to the game itself,
        // and the target the engine has names the old one. A blip of focus does
        // not count: the new window must stay the game's for FOCUS_SCANS scans.
        let mut moved = None;
        let follow = match (&self.current, &self.applied) {
            (Some(c), Some((target, _)))
                if started.is_none()
                    && ended.is_none()
                    && (target.starts_with("game:") || target.starts_with("window:"))
                    && self.applied_pid.is_some_and(|pid| pid != c.pid)
                    && self.kept != Some(c.pid)
                    && self.wanted().0 != *target =>
            {
                Some(c.clone())
            }
            _ => None,
        };
        match follow {
            Some(game) => {
                let scans = match self.moved {
                    Some((pid, n)) if pid == game.pid => n + 1,
                    _ => 1,
                };
                self.moved = Some((game.pid, scans));
                if scans >= FOCUS_SCANS {
                    self.moved = None;
                    moved = Some(game);
                }
            }
            None => self.moved = None,
        }
        if ended.is_some() || started.is_some() {
            self.hook_checked = None;
            self.needs_admin = None;
            self.parked = None;
            self.fallback = None;
            self.retry = None;
            self.moved = None;
            self.kept = None;
        }
        if let Some(game) = started.as_ref().filter(|g| self.config.test_fallback && g.capture == Capture::Hook) {
            self.hook_checked = Some(game.id.key.clone());
            self.fallback = Some(game.id.key.clone());
            self.retry = Some(Retry::after_fallback(now));
        }

        let mut commands = Vec::new();
        if let Some((other, current)) = held {
            commands.push(Command::Log(format!("{other} in focus: {current} keeps the capture while it is hooked, unless {other} stays in focus for {} s", u64::from(HOOKED_FOCUS_SCANS) * SCAN_EVERY.as_secs())));
        }
        if let Some(game) = &ended {
            commands.push(Command::Log(format!("game closed or left: {} ({})", game.id.name, game.path)));
        }
        if let Some(game) = &started {
            commands.push(Command::Log(format!("game found: {} [{}] {} pid {} window {:?}", game.id.name, game.id.key, game.path, game.pid, game.title)));
            if game.capture == Capture::Window {
                commands.push(Command::Log(format!("capture: {} is a window-only game: recorded by its window, with no game hook", game.id.name)));
            }
        }
        if renamed && started.is_none() && ended.is_none() {
            commands.push(Command::GameState);
        }
        if started.is_some() || ended.is_some() {
            commands.extend([Command::Apply, Command::GameState]);
            if let Some(game) = started.filter(|_| self.recording()) {
                commands.push(Command::AnnounceGame { game: game.id.name });
            }
        }
        if let Some(game) = moved {
            commands.push(Command::CheckHookAfterMove(game));
        }
        if let Some(game) = unparked {
            commands.push(Command::Log(format!("capture: {} moved to {} (pid {}); its capture starts again", game.id.name, game.exe(), game.pid)));
            commands.extend([Command::Apply, Command::GameState]);
        }
        commands
    }

    /// The engine's answer about a game whose window moved to another of its
    /// processes (A23 of the stability tests: Fortnite shows its anti-cheat
    /// bootstrapper's splash screen first, then its own window). Before the hook,
    /// the game is captured in the new window: the game capture starts over there,
    /// with its grace, or, for a game recorded by its window, the window capture and
    /// the tries of the hook move to it. A hooked game keeps its capture, so a
    /// second window of the game does not drop the hook.
    pub fn answer_moved(&mut self, game: &Active, hooked: bool, now: Instant) -> Vec<Command> {
        if self.current.as_ref().map(|c| c.pid) != Some(game.pid) {
            return Vec::new();
        }
        if hooked {
            self.kept = Some(game.pid);
            return vec![Command::Log(format!("capture: {} moved to {} (pid {}) after its hook started; capture kept", game.id.name, game.exe(), game.pid))];
        }
        // The hook of the new process is checked again; the notice that the game
        // runs as administrator still comes once (see answer_hook_check).
        self.hook_checked = None;
        if self.fallback.is_some() {
            self.retry = Some(Retry::after_fallback(now));
        }
        let line = match game.capture {
            Capture::Hook => format!("capture: {} moved to {} (pid {}) before its hook started; the capture follows it", game.id.name, game.exe(), game.pid),
            Capture::Window => format!("capture: {} moved to {} (pid {}); its window capture follows it", game.id.name, game.exe(), game.pid),
        };
        vec![Command::Log(line), Command::Apply, Command::GameState]
    }

    /// The hook check that is due, if one is: a game captured by its hook, not
    /// checked yet, HOOK_GRACE or more after the engine started capturing it. Both
    /// deadlines count from the start of the capture, not from when the game was
    /// found: on a slow PC, starting Laurel and the engine used up the time before
    /// the hook was even tried (A8 of the stability tests).
    pub fn hook_check(&self, now: Instant) -> Option<HookCheck> {
        let hooking = self.applied.as_ref().is_some_and(|(target, _)| target.starts_with("game:"));
        match (&self.current, self.capture_started) {
            (Some(g), Some(started)) if hooking && self.hook_checked.as_deref() != Some(g.id.key.as_str()) && now.saturating_duration_since(started) >= HOOK_GRACE => {
                Some(HookCheck { game: g.clone(), capturing: now.saturating_duration_since(started) })
            }
            _ => None,
        }
    }

    /// The answer to a hook check: whether the engine has the hook and, when it
    /// does not, whether the game's process runs as administrator (None when its
    /// token cannot be read).
    ///
    /// A game that runs as administrator cannot be recorded by a Laurel that does
    /// not: the engine cannot open its process to hook it, and its keys do not reach
    /// the hotkeys, so the player is offered a restart of Laurel as administrator.
    /// An elevated Laurel asks the engine too (A14 of the stability tests): its hook
    /// can still fail, for example with the game on another GPU (A13), and then the
    /// game's window is recorded as for any other game after FALLBACK_GRACE.
    pub fn answer_hook_check(&mut self, check: &HookCheck, hooked: bool, elevated: Option<bool>, now: Instant) -> Vec<Command> {
        let HookCheck { game, capturing } = check;
        let elevated = if hooked { Some(false) } else { elevated };
        let needs_admin = !hooked && !self.config.elevated && elevated != Some(false);
        // A game this Laurel can hook (not elevated, or Laurel elevated too) gets
        // until FALLBACK_GRACE before its window is recorded instead.
        let fallback = !hooked && !needs_admin && *capturing >= FALLBACK_GRACE;
        if !hooked && !needs_admin && !fallback {
            return Vec::new();
        }
        if self.current.as_ref().map(|g| &g.id.key) != Some(&game.id.key) {
            return Vec::new();
        }
        self.hook_checked = Some(game.id.key.clone());
        // Checked again after the game moved to another process (see answer_moved): that
        // process decides, and the notice is not shown a second time.
        let notified = self.needs_admin.is_some();
        self.needs_admin = needs_admin.then(|| game.id.key.clone());
        // A29: a process known to run as administrator cannot be opened by this
        // Laurel; OBS would try every 2 s for as long as the game runs.
        let parked = needs_admin && elevated == Some(true);
        if parked {
            self.parked = Some(game.pid);
        }
        if fallback {
            self.fallback = Some(game.id.key.clone());
            self.retry = Some(Retry::after_fallback(now));
        }
        if hooked {
            return vec![Command::Log(format!("capture: {} hooked (checked {} s after the capture started)", game.id.name, capturing.as_secs()))];
        }
        let mut commands = vec![Command::Log(format!("capture: {} not hooked after {} s of capture; process elevated: {elevated:?}; Laurel elevated: {}", game.id.name, capturing.as_secs(), self.config.elevated))];
        if needs_admin {
            if parked {
                commands.push(Command::Log(format!("capture: {} runs as administrator: its capture waits until Laurel runs as administrator, the game closes or its window moves to another process", game.id.name)));
                commands.push(Command::Apply);
            }
            commands.push(Command::GameState);
            if !notified {
                commands.push(Command::AnnounceNeedsAdmin { game: game.id.name.clone() });
            }
        } else {
            commands.extend([Command::Apply, Command::GameState, Command::AnnounceByWindow { game: game.id.name.clone() }]);
        }
        commands
    }

    /// The step of the hook retries that is due, if one is (A7 of the stability
    /// tests): while a game is recorded by its window, the engine puts a game
    /// capture of the same window over the window capture for RETRY_FOR. Once it
    /// hooks the game, the game capture takes over without a second hook, and the
    /// window capture goes, with the border Windows 10 draws around a captured
    /// window.
    pub fn hook_retry(&self, now: Instant) -> Option<RetryStep> {
        let windowed = self.applied.as_ref().is_some_and(|(target, _)| target.starts_with("window:"));
        match (&self.current, self.retry) {
            (Some(g), Some(retry)) if windowed && self.fallback.as_deref() == Some(g.id.key.as_str()) => match retry.until {
                None if now < retry.next => None,
                None => Some(RetryStep::Start { game: g.clone(), tries: retry.tries }),
                Some(until) => Some(RetryStep::Check { game: g.clone(), tries: retry.tries, until }),
            },
            _ => None,
        }
    }

    /// The engine started, at `now`, the try of `game` that follows `tries` tries
    /// ([`RetryStep::Start`]).
    pub fn hook_retry_started(&mut self, game: &Active, tries: u32, now: Instant) -> Vec<Command> {
        let line = Command::Log(format!("capture: trying the hook of {} again (try {})", game.id.name, tries + 1));
        if self.retrying(game) {
            self.retry = Some(Retry { next: now, until: Some(now + RETRY_FOR), tries });
        }
        vec![line]
    }

    /// Whether the engine has the hook at `now`, during the try of `game` that
    /// follows `tries` tries and runs until `until` ([`RetryStep::Check`]).
    pub fn answer_hook_retry(&mut self, game: &Active, tries: u32, until: Instant, hooked: bool, now: Instant) -> Vec<Command> {
        if hooked {
            let line = Command::Log(format!("capture: {} hooked on try {}; back to game capture", game.id.name, tries + 1));
            if !self.retrying(game) {
                return vec![line];
            }
            // The game capture already hooked the game: setCapture for it makes
            // that capture the one recorded (Recorder::switch_capture). The hook is
            // checked again, in case the game capture lost it meanwhile.
            self.retry = None;
            self.fallback = None;
            self.hook_checked = None;
            return vec![line, Command::Apply, Command::GameState];
        }
        if now < until {
            return Vec::new();
        }
        let wait = (RETRY_FIRST * 2u32.saturating_pow(tries + 1)).min(RETRY_MAX);
        if self.retrying(game) {
            self.retry = Some(Retry { next: now + wait, until: None, tries: tries + 1 });
        }
        vec![Command::EndHookRetry, Command::Log(format!("capture: {} still not hooked; next try in {} s", game.id.name, wait.as_secs()))]
    }

    /// Whether `game` is still the current game, recorded by its window.
    fn retrying(&self, game: &Active) -> bool {
        self.current.as_ref().map(|g| &g.id.key) == Some(&game.id.key) && self.fallback.as_deref() == Some(game.id.key.as_str())
    }

    /// What the engine needs to be told to reach the wanted target and recording
    /// state, if anything: the engine has what it was last told or, after it
    /// started, what it started with.
    pub fn apply(&self) -> Option<Apply> {
        let (target, recording) = self.wanted();
        if self.applied.as_ref() == Some(&(target.clone(), recording)) {
            return None;
        }
        let (have_target, have_recording) = self.applied.clone().unwrap_or_else(|| self.initial());
        Some(Apply { target, recording, have_target, have_recording, pid: self.current.as_ref().map(|g| g.pid) })
    }

    /// The engine took what `apply` asked, at `now`: a new capture target starts
    /// the hook deadlines.
    pub fn applied(&mut self, apply: &Apply, now: Instant) -> Vec<Command> {
        if apply.changes_target() {
            self.capture_started = Some(now);
        }
        self.applied = Some((apply.target.clone(), apply.recording));
        self.applied_pid = apply.pid;
        let mut commands = vec![Command::Log(format!("capture: target {}, recording {}", apply.target, apply.recording))];
        if apply.target.starts_with("monitor") && !apply.have_target.starts_with("monitor") && apply.recording {
            commands.push(Command::AnnounceScreen);
        }
        commands
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

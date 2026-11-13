//! A clip request (spec record.replay-buffer): what asked for it, and the clip
//! cooldown that turns a repeated press into one clip (A33 of the stability tests).

use std::time::{Duration, Instant};

use crate::games::{Catalog, UserGames, Verdict};

/// What asked for a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Hotkey,
    TitleBar,
    Tray,
    /// Laurel itself, for a Steam achievement (autoclip.steam-achievements).
    Automatic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    GoAhead,
    /// Within the cooldown of the last request that went ahead, this long ago.
    Ignored { since: Duration },
}

/// The clip cooldown (decision of 2026-11-10, value in the spec's table of default
/// values): the user's clip requests within this time of the last one that went
/// ahead are ignored, so that a double press gives one result.
pub const CLIP_COOLDOWN: Duration = Duration::from_secs(1);

/// One cooldown for the user's requests from the hotkey, the title bar button and
/// the tray menu. It counts from the last request that went ahead, whether that one
/// then saved a clip, failed or was refused. Automatic clips are outside it: never
/// held, and they do not open it.
#[derive(Debug, Default)]
pub struct ClipCooldown {
    last: Option<Instant>,
}

impl ClipCooldown {
    /// Whether a request made at `now` goes ahead; one that does opens the cooldown.
    pub fn admit(&mut self, source: Source, now: Instant) -> Admission {
        if source == Source::Automatic {
            return Admission::GoAhead;
        }
        if let Some(last) = self.last {
            let since = now.saturating_duration_since(last);
            if since < CLIP_COOLDOWN {
                return Admission::Ignored { since };
            }
        }
        self.last = Some(now);
        Admission::GoAhead
    }
}

/// Why a clip request was refused before it reached the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    RecordingOff,
    DiskAlmostFull,
    /// The game runs as administrator and Laurel does not, so it is not recorded.
    NeedsAdmin { game: String },
    /// No game is being recorded: none is open, or its program is not recognized.
    NoGame,
}

/// What the app knows about recording when a clip is requested.
#[derive(Clone, Debug, Default)]
pub struct Recording {
    /// Recording turned on by the user (spec record.toggle).
    pub enabled: bool,
    /// Recording paused because the capture drive is almost full.
    pub disk_paused: bool,
    /// Whether the replay buffer runs.
    pub buffer_runs: bool,
    /// The game that runs as administrator while Laurel does not.
    pub needs_admin: Option<String>,
}

/// Why a clip request is refused, if it is: recording off first, then the disk,
/// then why no game is recorded.
pub fn refusal(r: &Recording) -> Option<Refusal> {
    if !r.enabled {
        Some(Refusal::RecordingOff)
    } else if r.disk_paused {
        Some(Refusal::DiskAlmostFull)
    } else if r.buffer_runs {
        None
    } else if let Some(game) = &r.needs_admin {
        Some(Refusal::NeedsAdmin { game: game.clone() })
    } else {
        Some(Refusal::NoGame)
    }
}

/// A rectangle in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// The window in focus: its program, its client area (without frame and title
/// bar) and its monitor, in screen pixels.
#[derive(Clone, Copy, Debug)]
pub struct Foreground<'a> {
    pub exe_path: &'a str,
    pub client: Rect,
    pub monitor: Rect,
}

impl Rect {
    fn covers(&self, other: &Rect) -> bool {
        self.left <= other.left && self.top <= other.top && self.right >= other.right && self.bottom >= other.bottom
    }
}

/// Whether a refused clip's reason shows over the screen even with notices over
/// games off (A32, decision of 2026-11-10): only for a request from the hotkey,
/// when the window in focus fills its monitor, as a game in fullscreen or in a
/// borderless window does (its client area covers the monitor, so a maximized
/// window with a title bar does not count), and its program is not on the
/// blocklist. From the title bar button or the tray, Laurel's window shows why.
/// A game the user chose not to record gets no "add it as a game".
pub fn refusal_over_screen(source: Source, refusal: &Refusal, foreground: Option<&Foreground>, catalog: &Catalog, user: &UserGames) -> bool {
    let Some(f) = foreground.filter(|_| source == Source::Hotkey) else { return false };
    let ignored = *refusal == Refusal::NoGame && matches!(catalog.classify(f.exe_path, user), Verdict::Ignored(_));
    f.client.covers(&f.monitor) && !catalog.blocks(f.exe_path) && !ignored
}

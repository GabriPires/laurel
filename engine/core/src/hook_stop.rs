//! A34 of the stability tests: the engine clears the game hook's stop signal that it
//! left itself, so that a game that draws its first frame late still gets the hook.
//! This is the rule; engine/src/hook_stop.rs opens the processes and the signal.
//!
//! In OBS 32.1.2 (plugins/win-capture/game-capture.c), a game capture that stops
//! (stop_capture) sets the hook's stop signal, an auto-reset event named
//! "CaptureHook_Stop<pid>" that the hook creates inside the game, and then logs
//! "capture stopped". Only a hook that captures reads the signal. When the inject
//! helper of the compatibility hook ends (after 4 s) before the game drew a frame,
//! the game capture stops and the signal stays set: every later OBS hook attempt on
//! that process finds it on its first tick ("hook stop signal received") and stops,
//! setting it again, and the game stays on the window fallback.
//!
//! The rule (the owner's decisions of 2026-11-11, issues #2 and #6 of the private
//! repository, and of 2026-11-17 for A36, issue #12): after a "capture stopped" that
//! did not come from a signal OBS found ("hook stop signal received" just before
//! it), the engine clears the signal, on a game process that is not hooked and whose
//! first stop line in this engine was such a lone "capture stopped". A process whose
//! first stop line was "hook stop signal received" has a signal the engine found
//! already set: after an orderly restart of the engine it is what makes a hook that
//! captured for the old engine let go of that capture (A26), so it is never cleared.
//! The same holds when the engine itself removes a game capture (the window
//! fallback, the end of a hook retry), which logs the same line, but not at the
//! engine's orderly end: the shell stops reading the lines before the recorder lets
//! its sources go.
//!
//! A process is hooked from OBS seeing the hook's ready signal ("capture
//! initializing!") in a source until the hook's "capture freed" line in the same
//! source: meanwhile the hook captures and reads the stop signal itself, also when OBS
//! fails to open its texture (A13) or the device is lost (A24) and the attempt ends
//! with a lone "capture stopped". The hook writes "capture freed" (graphics-hook,
//! one line per graphics API) whenever it lets go of its capture: when it reads the
//! stop signal or OBS's keepalive is gone, but then OBS has already closed the hook's
//! pipe and the line is lost; and by itself, with the source's capture still on, when
//! the game destroys, resizes or replaces its swap chain (DXGI, Vulkan), resets its
//! device (Direct3D 9), resizes its window (OpenGL) or the capture fails to start.
//! So a "capture freed" in OBS's log comes from a hook that no longer captures and
//! reads no signal. When the engine then removes that capture, nothing reads the
//! signal it sets, and OBS finds it on every later attempt in that process (A36: 49 s
//! without the game's image in package 10). A hook that lets go of its capture after
//! OBS stopped it leaves no line, so a source's own "capture stopped" ends its hook
//! too, once the rule has decided on that line with the process still hooked: in the
//! A13 case the hook still captures and reads that signal, and so does a paused
//! game's hook when it draws again (meanwhile every OBS attempt finds the signal set,
//! so none ends with a lone "capture stopped").
//!
//! Left alone, with one note each: a Microsoft Store game (OBS keeps its signals in
//! the app's own namespace), a process whose signal the engine cannot open (a game
//! running as administrator), and a process the engine cannot open at all.
//!
//! The watch also notes a hook that is in the game while OBS waits for its first
//! frame (A34; issue #5 of the private repository): from OBS's "map id:
//! CaptureHook_HookInfo<pid>", which comes only after OBS opened the texture mutexes
//! the hook creates when it loads in the game, until "capture initializing!" (the
//! hook's ready signal, its first frame) or the attempt's end. A stuck stop signal
//! ends each attempt on its first tick ("hook stop signal received"), so the note
//! tells a hook that sees no frame of the game apart from a stuck signal. The first
//! note comes after FIRST_NOTE, longer than the inject helper's 4 s and shorter than
//! the app's 10 s hook retries; the next ones each three times as far, at most
//! NOTE_GAP_MAX apart. An attempt lasts at most one of those retries in the app, so
//! the note also says how long ago OBS first found the hook in that process, while
//! no attempt there has had a first frame (a hook that freed its capture had one).
//!
//! The names and lines read here are OBS 32.1.2's (wiki/projeto/runtime-obs.md).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

/// The hook's wait for the game's first frame is noted first after this long...
const FIRST_NOTE: Duration = Duration::from_secs(5);
/// ...then each time it has gone three times as long, at most this far apart.
const NOTE_GAP_MAX: Duration = Duration::from_secs(300);

/// What the watch asks Windows about a game process, and the clearing of its stop
/// signal. The engine implements it with the Windows calls.
pub trait GameProcesses {
    /// When the process started, to tell it from a later process that gets the same
    /// id; None when the engine cannot open it.
    fn started(&mut self, pid: u32) -> Option<u64>;
    /// Whether the process is a Microsoft Store app.
    fn store_app(&mut self, pid: u32) -> bool;
    /// Clears the stop signal of the hook in the process, and logs it; false, logged,
    /// when the engine cannot, which leaves the process alone from then on.
    fn clear_stop_signal(&mut self, pid: u32, exe: &str) -> bool;
}

/// A line the watch has for the engine's log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// The engine could not open the game's process, and leaves its signal alone.
    Unopened { exe: String, pid: u32 },
    /// The game is a Microsoft Store app, whose signal the engine leaves alone.
    StoreApp { exe: String, pid: u32 },
    /// The hook is in the game and OBS has waited this long for its first frame;
    /// `found`, while no attempt in that process has had a first frame, is how long
    /// ago OBS first found the hook there.
    Waiting { exe: String, pid: u32, source: String, waited: Duration, found: Option<Duration> },
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Note::Unopened { exe, pid } => write!(f, "[laurel] game hook: could not open {exe} (pid {pid}); the engine leaves its stop signal alone"),
            Note::StoreApp { exe, pid } => write!(
                f,
                "[laurel] game hook: {exe} (pid {pid}) is a Microsoft Store app; OBS keeps its stop signal in the app's own namespace, which the engine does not clear"
            ),
            Note::Waiting { exe, pid, source, waited, found } => {
                write!(f, "[laurel] game hook: the hook is in {exe} (pid {pid}) and has waited {} s for the game's first frame (source '{source}')", waited.as_secs())?;
                if let Some(found) = found {
                    write!(f, "; no first frame since OBS found it there {} s ago", found.as_secs())?;
                }
                Ok(())
            }
        }
    }
}

/// A game process: its id and its start time, since Windows reuses ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ProcessKey {
    pid: u32,
    started: u64,
}

/// What OBS logged last about one game capture source.
#[derive(Debug, Default)]
struct GameSource {
    /// The executable of the current hook attempt.
    exe: String,
    /// The process of the current hook attempt, once OBS opened its hook info.
    process: Option<ProcessKey>,
    /// "hook stop signal received": the "capture stopped" that follows is OBS
    /// setting again the signal it found.
    signal_found: bool,
    /// The hook is in the game and OBS waits for its first frame.
    waiting: Option<Wait>,
}

/// OBS waiting for the first frame of the hook in a game process.
#[derive(Debug)]
struct Wait {
    pid: u32,
    since: Instant,
    /// How long the wait has to go before the next note.
    next_note: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum FirstStop {
    /// A lone "capture stopped": the signal is the engine's own.
    Own,
    /// "hook stop signal received": the engine found the signal already set.
    Found,
}

#[derive(Debug, Default)]
struct Process {
    /// When OBS first opened the hook in this process ("map id").
    hook_found: Option<Instant>,
    first_stop: Option<FirstStop>,
    /// The sources for which the hook started a capture in this process ("capture
    /// initializing!") and has not freed it since ("capture freed" in the same
    /// source), and whose capture has not stopped: the process is hooked while there
    /// is one.
    hooked_in: BTreeSet<String>,
    /// The hook delivered a first frame in this process for this engine, once.
    first_frame: bool,
    /// A Microsoft Store game, or a signal the engine could not open.
    left_alone: bool,
}

/// The watch on the game capture lines, which clears the engine's own stop signals
/// and notes the hook's waits, from the engine's start until its orderly end.
#[derive(Debug, Default)]
pub struct StopSignalWatch {
    sources: BTreeMap<String, GameSource>,
    processes: HashMap<ProcessKey, Process>,
    /// Game processes the engine could not open, already noted once.
    unopened: HashSet<u32>,
}

impl StopSignalWatch {
    /// Reads a game capture line (laurel_engine_core::obs_lines::game_capture_line):
    /// the source's name and OBS's message, at `now`. Clears a stop signal through
    /// `processes` when the rule says so.
    pub fn line(&mut self, name: &str, message: &str, now: Instant, processes: &mut impl GameProcesses) -> Option<Note> {
        let attempt = message.strip_prefix("attempting to hook process: ").or_else(|| message.strip_prefix("attempting to hook fullscreen process: "));
        if let Some(exe) = attempt {
            self.sources.insert(name.to_string(), GameSource { exe: exe.to_string(), ..GameSource::default() });
            return None;
        }
        if let Some(pid) = message.strip_prefix("map id: CaptureHook_HookInfo").and_then(|id| id.parse().ok()) {
            let source = self.sources.entry(name.to_string()).or_default();
            source.waiting = Some(Wait { pid, since: now, next_note: FIRST_NOTE });
            source.process = processes.started(pid).map(|started| ProcessKey { pid, started });
            match source.process {
                None if self.unopened.insert(pid) => return Some(Note::Unopened { exe: source.exe.clone(), pid }),
                Some(key) if !self.processes.contains_key(&key) => {
                    let process = self.processes.entry(key).or_default();
                    process.hook_found = Some(now);
                    if processes.store_app(pid) {
                        process.left_alone = true;
                        return Some(Note::StoreApp { exe: source.exe.clone(), pid });
                    }
                }
                _ => {}
            }
            return None;
        }
        let source = self.sources.get_mut(name)?;
        if ends_wait(message) {
            source.waiting = None;
        }
        let key = source.process?;
        match message {
            "hook stop signal received" => {
                source.signal_found = true;
                self.processes.entry(key).or_default().first_stop.get_or_insert(FirstStop::Found);
            }
            "capture stopped" => {
                let found = std::mem::take(&mut source.signal_found);
                let process = self.processes.entry(key).or_default();
                if !found {
                    let own = *process.first_stop.get_or_insert(FirstStop::Own) == FirstStop::Own;
                    if own && process.hooked_in.is_empty() && !process.left_alone && !processes.clear_stop_signal(key.pid, &source.exe) {
                        process.left_alone = true;
                    }
                }
                // This source's capture is over: its hook reads the signal or has
                // freed the capture, with no line since OBS closed its pipe first.
                process.hooked_in.remove(name);
            }
            // OBS saw the hook's ready signal: the hook captures for this engine from
            // now on, also when OBS then fails to open its texture (A13) and stops.
            "capture initializing!" => {
                let process = self.processes.entry(key).or_default();
                process.hooked_in.insert(name.to_string());
                process.first_frame = true;
            }
            // A36: the hook freed this source's capture, and reads the stop signal no more.
            _ if capture_freed(message) => {
                self.processes.entry(key).or_default().hooked_in.remove(name);
            }
            _ => {}
        }
        None
    }

    /// The notes of each hook in a game that has waited for its first frame past its
    /// next note, at `now`. The engine looks every second.
    pub fn waits(&mut self, now: Instant) -> Vec<Note> {
        let mut notes = Vec::new();
        for (name, source) in &mut self.sources {
            let Some(wait) = source.waiting.as_mut() else { continue };
            let waited = now.saturating_duration_since(wait.since);
            if waited < wait.next_note {
                continue;
            }
            let process = source.process.and_then(|key| self.processes.get(&key)).filter(|process| !process.first_frame);
            let found = process.and_then(|process| process.hook_found).map(|found| now.saturating_duration_since(found));
            notes.push(Note::Waiting { exe: source.exe.clone(), pid: wait.pid, source: name.clone(), waited, found });
            while wait.next_note <= waited {
                wait.next_note += wait.next_note.saturating_mul(2).min(NOTE_GAP_MAX);
            }
        }
        notes
    }
}

/// The hook's line when it frees its capture, which OBS logs under the source's name
/// ("----------------- d3d11 capture freed ----------------"), for each graphics API
/// the hook of OBS 32.1.2 captures.
fn capture_freed(message: &str) -> bool {
    let words = message.trim_matches(|c| c == '-' || c == ' ');
    words.strip_suffix(" capture freed").is_some_and(|api| matches!(api, "d3d8" | "d3d9" | "d3d10" | "d3d11" | "d3d12" | "gl" | "vulkan"))
}

/// The lines after "map id" that end OBS's wait for the hook's first frame: the
/// first frame ("capture initializing!"), the attempt's end ("capture stopped", which
/// also follows "hook stop signal received"), and the failures of the rest of
/// init_hook, which end the attempt before it is active and so without "capture
/// stopped".
fn ends_wait(message: &str) -> bool {
    matches!(message, "capture initializing!" | "capture stopped")
        || message.starts_with("init_hook_info: get_hook_info failed")
        || message.starts_with("init_hook_info: failed to map data view")
        || message.starts_with("init_events: ")
}

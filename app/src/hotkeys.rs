//! Global hotkeys (spec clip.hotkeys) through Raw Input with RIDEV_INPUTSINK on a
//! hidden message-only window. Raw Input gets a copy of every key and mouse button
//! without taking it away from the game (RegisterHotKey, used in the PoC, swallows
//! the key) and without sitting in the input path, as a low-level hook would, so
//! it adds no input latency and also works with games in exclusive fullscreen.
//!
//! A hotkey's rules and text form live in laurel_app_core::hotkeys; this module
//! reads the keyboard and the mouse and shows the hotkeys in the current keyboard
//! layout and language. While the user records a new hotkey, the hotkeys are
//! suspended.

use std::collections::HashSet;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub use laurel_app_core::hotkeys::{Combo, Key, Refused, Replaced, Replacement, ALT, CTRL, SHIFT, WIN};

use crate::i18n::{t, t_later};
use crate::log::log;
use crate::win;

/// How long recording a new hotkey waits for keys.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);
/// A modifier pressed longer ago than this counts only if the system still sees it
/// held: its key-up may have gone to another desktop (Ctrl+Alt+Del) or an elevated
/// window, which Raw Input does not report.
const STALE_MODIFIER: Duration = Duration::from_secs(1);

const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
const VK_MENU: u16 = 0x12;
const VK_LWIN: u16 = 0x5B;
const VK_RWIN: u16 = 0x5C;
const VK_ESCAPE: u16 = 0x1B;
/// Not a real virtual key; Windows leaves 0x0E unassigned, so the numpad Enter
/// (Enter with the E0 prefix) gets it here to tell it from the main Enter.
const VK_NUMPAD_ENTER: u16 = 0x0E;

/// The modifier bit of a modifier key, if it is one.
fn modifier_bit(vk: u16) -> Option<u8> {
    match vk {
        VK_CONTROL | 0xA2 | 0xA3 => Some(CTRL),
        VK_SHIFT | 0xA0 | 0xA1 => Some(SHIFT),
        VK_MENU | 0xA4 | 0xA5 => Some(ALT),
        VK_LWIN | VK_RWIN => Some(WIN),
        _ => None,
    }
}

/// The system's virtual key for a modifier bit, for the stale check.
fn modifier_vk(bit: u8) -> u16 {
    match bit {
        CTRL => VK_CONTROL,
        SHIFT => VK_SHIFT,
        ALT => VK_MENU,
        _ => VK_LWIN,
    }
}

/// The text shown to the user: punctuation keys with the character of the current
/// keyboard layout.
pub fn label(c: &Combo) -> String {
    c.label(win::key_char)
}

/// Why a combination cannot be a hotkey, in English (translated where it is shown).
pub fn refusal_text(r: Refused) -> &'static str {
    match r {
        Refused::MouseButton => t_later!("The left and right mouse buttons are needed in games"),
        Refused::WindowsKey => t_later!("Shortcuts with the Windows key belong to Windows"),
        Refused::Escape => t_later!("Esc is used to cancel; choose another key"),
        Refused::AltF4 => t_later!("Alt+F4 closes the game"),
        Refused::WindowsShortcut => t_later!("Windows uses this shortcut"),
        Refused::AltEnter => t_later!("Alt+Enter switches fullscreen in many games"),
        Refused::TypingKey => t_later!("Letters, digits, arrows and typing keys need Ctrl or Alt, or they would fire while you play or chat"),
        Refused::LockKey => t_later!("Lock keys change how you type"),
        Refused::Tab => t_later!("Tab is used in games and to move between fields"),
        Refused::SystemKey => t_later!("Media, volume and other special keys belong to Windows"),
        Refused::MenuKey => t_later!("The menu key opens menus in games and programs"),
        Refused::EditingShortcut => t_later!("Ctrl+C, Ctrl+V and the other editing shortcuts are used by every program"),
        Refused::NoName => t_later!("Laurel can't keep this key as a shortcut"),
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    Pressed(&'static str),
    Captured(&'static str, Combo),
    Rejected(&'static str, Combo, String),
    Cancelled(&'static str),
}

struct Capture {
    action: &'static str,
    id: u64,
}

struct Shared {
    bindings: Mutex<Vec<(&'static str, Combo)>>,
    capture: Mutex<Option<Capture>>,
    /// Non-modifier keys held down, to ignore auto-repeat.
    held: Mutex<HashSet<(u16, bool)>>,
    /// Modifier bits held down, with when they went down (from the Raw Input events,
    /// which keep the order of fast presses).
    mods: Mutex<Vec<(u8, Instant)>>,
    handler: Box<dyn Fn(Event) + Send + Sync>,
    hwnd: AtomicIsize,
    next_capture: AtomicU64,
    presses: AtomicU64,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

pub struct Hotkeys(Arc<Shared>);

impl Hotkeys {
    /// Starts the listener thread. `handler` runs on that thread and must return quickly.
    pub fn start(bindings: Vec<(&'static str, Combo)>, handler: impl Fn(Event) + Send + Sync + 'static) -> Result<Hotkeys, String> {
        let shared = Arc::new(Shared {
            bindings: Mutex::new(bindings),
            capture: Mutex::new(None),
            held: Mutex::new(HashSet::new()),
            mods: Mutex::new(Vec::new()),
            handler: Box::new(handler),
            hwnd: AtomicIsize::new(0),
            next_capture: AtomicU64::new(1),
            presses: AtomicU64::new(0),
        });
        SHARED.set(shared.clone()).map_err(|_| "hotkeys already started".to_string())?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = win::raw_input_window(on_raw_input);
            let _ = tx.send(result.map(|hwnd| {
                SHARED.get().unwrap().hwnd.store(hwnd, Ordering::SeqCst);
                hwnd
            }));
            win::run_message_loop();
        });
        let hwnd = rx.recv().map_err(|e| e.to_string())??;
        let me = Hotkeys(shared);
        me.register(hwnd);
        log!("hotkeys: listening through Raw Input: {}", me.describe());
        Ok(me)
    }

    fn describe(&self) -> String {
        self.0.bindings.lock().unwrap().iter().map(|(a, c)| format!("{a}={}", c.name())).collect::<Vec<_>>().join(", ")
    }

    /// The mouse is only listened to while a binding or a capture needs it: a gaming
    /// mouse sends up to thousands of reports per second.
    fn register(&self, hwnd: isize) {
        let mouse = self.0.capture.lock().unwrap().is_some() || self.0.bindings.lock().unwrap().iter().any(|(_, c)| matches!(c.key, Key::Mouse(_)));
        win::request_raw_input(hwnd, mouse);
    }

    fn reregister(&self) {
        let hwnd = self.0.hwnd.load(Ordering::SeqCst);
        if hwnd != 0 {
            self.register(hwnd);
        }
    }

    pub fn bindings(&self) -> Vec<(&'static str, Combo)> {
        self.0.bindings.lock().unwrap().clone()
    }

    pub fn set_bindings(&self, bindings: Vec<(&'static str, Combo)>) {
        *self.0.bindings.lock().unwrap() = bindings;
        self.reregister();
        log!("hotkeys: {}", self.describe());
    }

    /// Records the next valid combination for `action`; hotkeys are suspended
    /// meanwhile. Ends with a Captured or Cancelled event (Rejected may come before).
    pub fn capture(&self, action: &'static str) {
        let id = self.0.next_capture.fetch_add(1, Ordering::SeqCst);
        *self.0.capture.lock().unwrap() = Some(Capture { action, id });
        self.reregister();
        let shared = self.0.clone();
        std::thread::spawn(move || {
            std::thread::sleep(CAPTURE_TIMEOUT);
            let mut capture = shared.capture.lock().unwrap();
            if capture.as_ref().is_some_and(|c| c.id == id) {
                *capture = None;
                drop(capture);
                (shared.handler)(Event::Cancelled(action));
            }
        });
    }

    pub fn cancel_capture(&self) {
        let ended = self.0.capture.lock().unwrap().take();
        if let Some(c) = ended {
            self.reregister();
            (self.0.handler)(Event::Cancelled(c.action));
        }
    }

    pub fn capturing(&self) -> Option<&'static str> {
        self.0.capture.lock().unwrap().as_ref().map(|c| c.action)
    }

    /// Hotkeys that fired since start (for the self-test).
    pub fn presses(&self) -> u64 {
        self.0.presses.load(Ordering::SeqCst)
    }
}

/// The modifiers held now: recent presses as reported, older ones only if the
/// system still sees them held (see STALE_MODIFIER).
fn current_mods(shared: &Shared) -> u8 {
    let mut held = shared.mods.lock().unwrap();
    held.retain(|(bit, since)| since.elapsed() < STALE_MODIFIER || win::key_down(modifier_vk(*bit)) || (*bit == WIN && win::key_down(VK_RWIN)));
    held.iter().fold(0, |mods, (bit, _)| mods | bit)
}

/// Numpad keys with NumLock off arrive as navigation keys without the E0 prefix.
fn numpad_vk(vk: u16, e0: bool) -> u16 {
    if e0 {
        return if vk == 0x0D { VK_NUMPAD_ENTER } else { vk };
    }
    match vk {
        0x2D => 0x60,
        0x23 => 0x61,
        0x28 => 0x62,
        0x22 => 0x63,
        0x25 => 0x64,
        0x0C => 0x65,
        0x27 => 0x66,
        0x24 => 0x67,
        0x26 => 0x68,
        0x21 => 0x69,
        0x2E => 0x6E,
        _ => vk,
    }
}

/// Where a key press came from, for the log: a program can inject keys that look
/// like the hotkey (SendInput arrives with no device).
fn input_source(device: isize) -> String {
    if device == 0 {
        return "injected by a program (no device)".into();
    }
    match win::raw_input_device_id(device) {
        Some(id) => format!("from device {id}"),
        None => "from an unknown device".into(),
    }
}

fn on_raw_input(input: win::RawInput) {
    let Some(shared) = SHARED.get() else { return };
    let (key, device) = match input {
        win::RawInput::Key { vk, down, e0, device } => {
            if let Some(bit) = modifier_bit(vk) {
                let mut mods = shared.mods.lock().unwrap();
                match (down, mods.iter().any(|(b, _)| *b == bit)) {
                    (true, false) => mods.push((bit, Instant::now())),
                    (false, _) => mods.retain(|(b, _)| *b != bit),
                    (true, true) => {}
                }
                return;
            }
            if vk == 0xFF {
                return;
            }
            let vk = numpad_vk(vk, e0);
            let mut held = shared.held.lock().unwrap();
            if !down {
                held.remove(&(vk, e0));
                return;
            }
            if !held.insert((vk, e0)) {
                return; // auto-repeat
            }
            (Key::Vk(vk), device)
        }
        win::RawInput::MouseDown { button, device } => (Key::Mouse(button), device),
    };
    let combo = Combo { mods: current_mods(shared), key };

    let capturing = shared.capture.lock().unwrap().as_ref().map(|c| c.action);
    if let Some(action) = capturing {
        if matches!(key, Key::Mouse(1 | 2)) {
            return; // clicks on the app itself, for example its Cancel button
        }
        if combo == (Combo { mods: 0, key: Key::Vk(VK_ESCAPE) }) {
            *shared.capture.lock().unwrap() = None;
            Hotkeys(shared.clone()).reregister();
            (shared.handler)(Event::Cancelled(action));
            return;
        }
        let taken = shared.bindings.lock().unwrap().iter().find(|(a, c)| *a != action && *c == combo).map(|(a, _)| *a);
        let reason = laurel_app_core::hotkeys::refusal(&combo).map(|r| crate::i18n::tr(refusal_text(r))).or_else(|| taken.map(|a| t!("Already used for {action}", action = action_label(a))));
        match reason {
            Some(reason) => (shared.handler)(Event::Rejected(action, combo, reason)),
            None => {
                *shared.capture.lock().unwrap() = None;
                (shared.handler)(Event::Captured(action, combo));
            }
        }
        return;
    }
    let action = shared.bindings.lock().unwrap().iter().find(|(_, c)| *c == combo).map(|(a, _)| *a);
    if let Some(action) = action {
        shared.presses.fetch_add(1, Ordering::SeqCst);
        log!("hotkeys: {} pressed for {action}, {}", combo.name(), input_source(device));
        (shared.handler)(Event::Pressed(action));
    }
}

/// What became of a stored hotkey that is refused since an update (idea 14), for
/// the notice at the first start and the note on the Shortcuts page.
pub fn replaced_text(r: &Replaced) -> String {
    let (old, action) = (label(&r.old), action_label(r.action));
    match r.now {
        Replacement::Default(new) => t!("{old} can't be a shortcut anymore. {action} now uses {new}; you can change it in Settings, Shortcuts.", old = old, action = action, new = label(&new)),
        Replacement::DefaultTaken { default, by } => t!(
            "{old} can't be a shortcut anymore, and {default} is already used for {other}, so {action} has no shortcut now. Set one in Settings, Shortcuts.",
            old = old,
            default = label(&default),
            other = action_label(by),
            action = action
        ),
        Replacement::NoDefault => t!("{old} can't be a shortcut anymore, so {action} has no shortcut now. Set one in Settings, Shortcuts.", old = old, action = action),
    }
}

pub fn action_label(action: &str) -> String {
    match action {
        "clip" => t!("Save clip"),
        "screenshot" => t!("Screenshot"),
        "toggle" => t!("Recording on/off"),
        _ => t!("another action"),
    }
}

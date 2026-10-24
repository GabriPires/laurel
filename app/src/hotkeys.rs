//! Global hotkeys (spec clip.hotkeys) through Raw Input with RIDEV_INPUTSINK on a
//! hidden message-only window. Raw Input gets a copy of every key and mouse button
//! without taking it away from the game (RegisterHotKey, used in the PoC, swallows
//! the key) and without sitting in the input path, as a low-level hook would, so
//! it adds no input latency and also works with games in exclusive fullscreen.
//!
//! A hotkey is modifiers plus one key or mouse button, stored as text such as
//! "Shift+F8", "Ctrl+Alt+K", "Mouse4" or "Ctrl+Oem_BA" (punctuation keys by their
//! virtual-key code, shown with the character of the current keyboard layout).
//! While the user records a new hotkey, the hotkeys are suspended.

use std::collections::HashSet;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::log::log;
use crate::win;

pub const CTRL: u8 = 1;
pub const SHIFT: u8 = 2;
pub const ALT: u8 = 4;
pub const WIN: u8 = 8;

/// How long recording a new hotkey waits for keys.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);
/// A modifier pressed longer ago than this counts only if the system still sees it
/// held: its key-up may have gone to another desktop (Ctrl+Alt+Del) or an elevated
/// window, which Raw Input does not report.
const STALE_MODIFIER: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A Windows virtual-key code (numpad keys keep their numpad codes).
    Vk(u16),
    /// Mouse button 1 to 5 (left, right, middle, back, forward).
    Mouse(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Combo {
    pub mods: u8,
    pub key: Key,
}

const NAMED: &[(u16, &str)] = &[
    (0x08, "Backspace"), (0x09, "Tab"), (0x0D, "Enter"), (0x13, "Pause"), (0x14, "CapsLock"), (0x1B, "Esc"),
    (0x20, "Space"), (0x21, "PageUp"), (0x22, "PageDown"), (0x23, "End"), (0x24, "Home"), (0x25, "Left"),
    (0x26, "Up"), (0x27, "Right"), (0x28, "Down"), (0x2C, "PrintScreen"), (0x2D, "Insert"), (0x2E, "Delete"),
    (0x5D, "Menu"), (0x6A, "NumpadMultiply"), (0x6B, "NumpadAdd"), (0x6D, "NumpadSubtract"), (0x6E, "NumpadDecimal"),
    (0x6F, "NumpadDivide"), (0x90, "NumLock"), (0x91, "ScrollLock"), (0x0E, "NumpadEnter"),
];

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

fn vk_name(vk: u16) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some((vk as u8 as char).to_string()),
        0x60..=0x69 => Some(format!("Numpad{}", vk - 0x60)),
        0x70..=0x87 => Some(format!("F{}", vk - 0x6F)),
        _ => NAMED.iter().find(|(v, _)| *v == vk).map(|(_, n)| n.to_string()).or_else(|| (0xBA..=0xE2).contains(&vk).then(|| format!("Oem_{vk:02X}"))),
    }
}

fn vk_from_name(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    if upper.len() == 1 {
        let c = upper.as_bytes()[0];
        return (c.is_ascii_digit() || c.is_ascii_uppercase()).then_some(c as u16);
    }
    if let Some(n) = upper.strip_prefix("NUMPAD").and_then(|d| d.parse::<u16>().ok()).filter(|n| *n <= 9) {
        return Some(0x60 + n);
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|d| d.parse::<u16>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(0x6F + n);
    }
    if let Some(hex) = upper.strip_prefix("OEM_") {
        return u16::from_str_radix(hex, 16).ok().filter(|v| (0xBA..=0xE2).contains(v));
    }
    NAMED.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(v, _)| *v)
}

impl Combo {
    pub fn parse(text: &str) -> Option<Combo> {
        let mut mods = 0;
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" => mods |= CTRL,
                "shift" => mods |= SHIFT,
                "alt" => mods |= ALT,
                "win" => mods |= WIN,
                p if p.starts_with("mouse") => key = Some(Key::Mouse(p[5..].parse().ok().filter(|n| (1..=5).contains(n))?)),
                _ => key = Some(Key::Vk(vk_from_name(part)?)),
            }
        }
        key.map(|key| Combo { mods, key })
    }

    fn mod_prefix(&self) -> String {
        let mut s = String::new();
        for (bit, name) in [(CTRL, "Ctrl+"), (SHIFT, "Shift+"), (ALT, "Alt+"), (WIN, "Win+")] {
            if self.mods & bit != 0 {
                s.push_str(name);
            }
        }
        s
    }

    /// The stored text, for settings.json.
    pub fn name(&self) -> String {
        let key = match self.key {
            Key::Vk(vk) => vk_name(vk).unwrap_or_else(|| format!("Oem_{vk:02X}")),
            Key::Mouse(n) => format!("Mouse{n}"),
        };
        format!("{}{key}", self.mod_prefix())
    }

    /// The text shown to the user: punctuation keys with the character of the
    /// current keyboard layout, mouse buttons spaced.
    pub fn label(&self) -> String {
        let key = match self.key {
            Key::Vk(vk @ 0xBA..=0xE2) => win::key_char(vk).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| format!("Key {vk:02X}")),
            Key::Mouse(n) => format!("Mouse {n}"),
            Key::Vk(_) => self.name().rsplit('+').next().unwrap_or_default().to_string(),
        };
        format!("{}{key}", self.mod_prefix())
    }
}

/// Why a combination cannot be a hotkey (spec clip.hotkeys), if it cannot.
pub fn reject_reason(c: &Combo) -> Option<&'static str> {
    let typing = |vk: u16| matches!(vk, 0x08 | 0x09 | 0x0D | 0x20 | 0x25..=0x28 | 0x30..=0x39 | 0x41..=0x5A | 0xBA..=0xE2);
    match c.key {
        Key::Mouse(1 | 2) => Some("The left and right mouse buttons are needed in games"),
        _ if c.mods & WIN != 0 => Some("Shortcuts with the Windows key belong to Windows"),
        Key::Vk(VK_ESCAPE) => Some("Esc is used to cancel; choose another key"),
        Key::Vk(0x73) if c.mods == ALT => Some("Alt+F4 closes the game"),
        Key::Vk(0x09 | 0x20) if c.mods == ALT => Some("Windows uses this shortcut"),
        Key::Vk(0x0D) if c.mods == ALT => Some("Alt+Enter switches fullscreen in many games"),
        Key::Vk(vk) if typing(vk) && c.mods & (CTRL | ALT) == 0 => {
            Some("Letters, digits, arrows and typing keys need Ctrl or Alt, or they would fire while you play or chat")
        }
        _ => None,
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

fn on_raw_input(input: win::RawInput) {
    let Some(shared) = SHARED.get() else { return };
    let key = match input {
        win::RawInput::Key { vk, down, e0 } => {
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
            Key::Vk(vk)
        }
        win::RawInput::MouseDown(n) => Key::Mouse(n),
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
        let reason = reject_reason(&combo).map(String::from).or_else(|| taken.map(|a| format!("Already used for {}", action_label(a))));
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
        (shared.handler)(Event::Pressed(action));
    }
}

pub fn action_label(action: &str) -> &'static str {
    match action {
        "clip" => "Save clip",
        "screenshot" => "Screenshot",
        "toggle" => "Recording on/off",
        _ => "another action",
    }
}

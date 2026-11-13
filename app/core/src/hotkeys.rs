//! The rules of the hotkeys (spec clip.hotkeys): a hotkey's text form, as stored in
//! settings.json, the combinations that cannot be hotkeys and why, and the hotkeys
//! read back from the settings. Reading the keyboard and the keyboard layout stays
//! in the app (app/src/hotkeys.rs, app/src/win.rs).
//!
//! A hotkey is modifiers plus one key or mouse button, stored as text such as
//! "Shift+F8", "Ctrl+Alt+K", "Mouse4" or "Ctrl+Oem_BA" (punctuation keys by their
//! virtual-key code, shown with the character of the current keyboard layout).

pub const CTRL: u8 = 1;
pub const SHIFT: u8 = 2;
pub const ALT: u8 = 4;
pub const WIN: u8 = 8;

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

const VK_ESCAPE: u16 = 0x1B;

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

    /// The text shown to the user: punctuation keys with the character
    /// `key_char` gives for them in the current keyboard layout, mouse buttons
    /// spaced, and a key without a name (always refused) by its code.
    pub fn label(&self, key_char: impl Fn(u16) -> Option<char>) -> String {
        let key = match self.key {
            Key::Vk(vk @ 0xBA..=0xE2) => key_char(vk).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| format!("Key {vk:02X}")),
            Key::Mouse(n) => format!("Mouse {n}"),
            Key::Vk(vk) => vk_name(vk).unwrap_or_else(|| format!("Key {vk:02X}")),
        };
        format!("{}{key}", self.mod_prefix())
    }
}

/// Why a combination cannot be a hotkey (spec clip.hotkeys); the app shows each
/// reason in the user's language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The left and right mouse buttons.
    MouseButton,
    /// Any combination with the Windows key.
    WindowsKey,
    /// Esc, which cancels the recording of a hotkey.
    Escape,
    AltF4,
    /// A shortcut of Windows itself: Alt+Tab, Alt+Space.
    WindowsShortcut,
    AltEnter,
    /// A key used to type or move without Ctrl or Alt.
    TypingKey,
    /// Caps Lock, Num Lock or Scroll Lock, with anything.
    LockKey,
    /// Tab with anything but Alt alone (Alt+Tab is a Windows shortcut).
    Tab,
    /// A volume, media, browser, launch or sleep key, with anything.
    SystemKey,
    /// The context menu key, with anything.
    MenuKey,
    /// Ctrl+C, Ctrl+V, Ctrl+X, Ctrl+Z, Ctrl+Y, Ctrl+A or Ctrl+S.
    EditingShortcut,
    /// A key without a name, whose stored text would not be read back.
    NoName,
}

/// Why a combination cannot be a hotkey, if it cannot. The lock keys, Tab, the
/// system keys, the menu key, the editing shortcuts and Ctrl+Alt+Delete were added
/// at the owner's request (decision of 2026-11-10, idea 14), and the keys without a
/// name after the round-trip test found them (decision of 2026-11-13).
pub fn refusal(c: &Combo) -> Option<Refused> {
    let typing = |vk: u16| matches!(vk, 0x08 | 0x09 | 0x0D | 0x20 | 0x25..=0x28 | 0x30..=0x39 | 0x41..=0x5A | 0xBA..=0xE2);
    let ctrl_alt = c.mods & (CTRL | ALT) == CTRL | ALT;
    match c.key {
        Key::Mouse(1 | 2) => Some(Refused::MouseButton),
        _ if c.mods & WIN != 0 => Some(Refused::WindowsKey),
        Key::Vk(VK_ESCAPE) => Some(Refused::Escape),
        Key::Vk(0x73) if c.mods == ALT => Some(Refused::AltF4),
        Key::Vk(0x09 | 0x20) if c.mods == ALT => Some(Refused::WindowsShortcut),
        // Windows takes Ctrl+Alt+Delete, with Shift or the numpad's Delete too.
        Key::Vk(0x2E | 0x6E) if ctrl_alt => Some(Refused::WindowsShortcut),
        Key::Vk(0x0D) if c.mods == ALT => Some(Refused::AltEnter),
        Key::Vk(0x14 | 0x90 | 0x91) => Some(Refused::LockKey),
        Key::Vk(0x09) => Some(Refused::Tab),
        // Browser back to home, volume, media, launch mail to app 2, and sleep.
        Key::Vk(0xA6..=0xB7 | 0x5F) => Some(Refused::SystemKey),
        Key::Vk(0x5D) => Some(Refused::MenuKey),
        Key::Vk(0x43 | 0x56 | 0x58 | 0x5A | 0x59 | 0x41 | 0x53) if c.mods == CTRL => Some(Refused::EditingShortcut),
        Key::Vk(vk) if typing(vk) && c.mods & (CTRL | ALT) == 0 => Some(Refused::TypingKey),
        Key::Vk(vk) if vk_name(vk).is_none() => Some(Refused::NoName),
        _ => None,
    }
}

/// A hotkey as stored in the settings: the action, the stored text (None when the
/// settings have none) and the action's default ("" for none).
#[derive(Clone, Copy, Debug)]
pub struct Stored<'a> {
    pub action: &'static str,
    pub text: Option<&'a str>,
    pub default: &'a str,
}

/// What became of a stored hotkey that is refused since an update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replacement {
    /// The action's default.
    Default(Combo),
    /// No hotkey: another action, `by`, uses the action's default.
    DefaultTaken { default: Combo, by: &'static str },
    /// No hotkey: the action has no default.
    NoDefault,
}

/// A stored hotkey that is refused since an update, for the notice that tells the
/// user once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Replaced {
    pub action: &'static str,
    pub old: Combo,
    pub now: Replacement,
}

/// The hotkeys read back from the settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    /// One per stored action, in the order given; None for no hotkey.
    pub hotkeys: Vec<Option<Combo>>,
    pub replaced: Vec<Replaced>,
}

/// The hotkeys of the actions read back from the settings, in the order given: a
/// stored hotkey that parses and is allowed; none for an empty one; the action's
/// default when the settings have none or it does not parse. A stored hotkey that
/// parses but is refused, since an update changed the rules (idea 14), goes back to
/// the default too, unless another action uses that default, which leaves the
/// action without a hotkey; each is reported in `replaced`.
pub fn load(stored: &[Stored]) -> Loaded {
    let own: Vec<Result<Option<Combo>, Combo>> = stored
        .iter()
        .map(|s| match s.text.map(str::trim) {
            Some("") => Ok(None),
            text => match text.and_then(Combo::parse) {
                Some(c) if refusal(&c).is_some() => Err(c),
                Some(c) => Ok(Some(c)),
                None => Ok(Combo::parse(s.default)),
            },
        })
        .collect();
    let mut hotkeys: Vec<Option<Combo>> = own.iter().map(|o| o.ok().flatten()).collect();
    let mut replaced = Vec::new();
    for (i, old) in own.iter().enumerate().filter_map(|(i, o)| o.err().map(|c| (i, c))) {
        let now = match Combo::parse(stored[i].default) {
            None => Replacement::NoDefault,
            Some(default) => match hotkeys.iter().position(|h| *h == Some(default)) {
                Some(j) => Replacement::DefaultTaken { default, by: stored[j].action },
                None => {
                    hotkeys[i] = Some(default);
                    Replacement::Default(default)
                }
            },
        };
        replaced.push(Replaced { action: stored[i].action, old, now });
    }
    Loaded { hotkeys, replaced }
}

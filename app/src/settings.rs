//! User settings kept in `<work-dir>/settings.json`: in-game notices and sounds
//! (overlay.in-game, alerts.sound), storage (storage.location, storage.limits), the
//! first-run startup question (app.startup), the games the user added or ignored
//! (game.manual-add), the hotkeys (clip.hotkeys), recording on or off
//! (record.toggle), the mouse cursor (capture.cursor) and screen recording
//! (capture.screen, off by default per D13). Missing or invalid fields fall back to
//! the defaults.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

/// A program the user marked as a game, by full path.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomGame {
    pub path: String,
    pub name: String,
}

/// A detected game the user does not want recorded, by game key (see games.rs).
#[derive(Clone, Debug, PartialEq)]
pub struct IgnoredGame {
    pub key: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub overlay: bool,
    pub sounds: bool,
    /// 0 to 100.
    pub volume: u8,
    /// Capture folder; empty means the default, Videos\Laurel.
    pub folder: String,
    /// File name pattern with the fields {game}, {date} and {time}.
    pub naming: String,
    pub limit_enabled: bool,
    pub limit_gb: u32,
    /// Removed files go to the Recycle Bin instead of being deleted.
    pub recycle: bool,
    /// Recording pauses below this much free space on the capture drive.
    pub min_free_gb: u32,
    /// Whether the user already answered "start with Windows?".
    pub autostart_asked: bool,
    pub custom_games: Vec<CustomGame>,
    pub ignored_games: Vec<IgnoredGame>,
    /// Hotkeys as text (see hotkeys.rs), checked when loaded.
    pub hotkey_clip: String,
    pub hotkey_screenshot: String,
    /// Empty when not set.
    pub hotkey_toggle: String,
    pub recording_enabled: bool,
    pub show_cursor: bool,
    pub screen_recording: bool,
    /// monitor_capture device id; empty for the primary monitor.
    pub screen_monitor: String,
    pub screen_game_switch: bool,
}

pub const DEFAULT_NAMING: &str = "{game} {date} {time}";
pub const DEFAULT_HOTKEY_CLIP: &str = "F8";
pub const DEFAULT_HOTKEY_SCREENSHOT: &str = "Shift+F8";

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            overlay: true,
            sounds: true,
            volume: 70,
            folder: String::new(),
            naming: DEFAULT_NAMING.into(),
            limit_enabled: true,
            limit_gb: 50,
            recycle: true,
            min_free_gb: 2,
            autostart_asked: false,
            custom_games: vec![],
            ignored_games: vec![],
            hotkey_clip: DEFAULT_HOTKEY_CLIP.into(),
            hotkey_screenshot: DEFAULT_HOTKEY_SCREENSHOT.into(),
            hotkey_toggle: String::new(),
            recording_enabled: true,
            show_cursor: true,
            screen_recording: false,
            screen_monitor: String::new(),
            screen_game_switch: true,
        }
    }
}

impl Settings {
    pub fn from_json(v: &Value) -> Settings {
        let d = Settings::default();
        let flag = |key: &str, default: bool| v[key].as_bool().unwrap_or(default);
        let number = |key: &str, default: u32, max: u64| v[key].as_u64().map_or(default, |n| n.min(max) as u32);
        let text = |key: &str, default: &str| v[key].as_str().map_or(default.to_string(), |s| s.trim().to_string());
        let pairs = |key: &str, a: &str, b: &str| -> Vec<(String, String)> {
            v[key].as_array().into_iter().flatten().filter_map(|e| Some((e[a].as_str()?.to_string(), e[b].as_str()?.to_string()))).collect()
        };
        Settings {
            overlay: flag("overlay", d.overlay),
            sounds: flag("sounds", d.sounds),
            volume: number("volume", d.volume as u32, 100) as u8,
            folder: text("folder", &d.folder),
            naming: Some(text("naming", &d.naming)).filter(|s| !s.is_empty()).unwrap_or(d.naming),
            limit_enabled: flag("limitEnabled", d.limit_enabled),
            limit_gb: number("limitGb", d.limit_gb, 100_000).max(1),
            recycle: flag("recycle", d.recycle),
            min_free_gb: number("minFreeGb", d.min_free_gb, 1000),
            autostart_asked: flag("autostartAsked", d.autostart_asked),
            custom_games: pairs("customGames", "path", "name").into_iter().map(|(path, name)| CustomGame { path, name }).collect(),
            ignored_games: pairs("ignoredGames", "key", "name").into_iter().map(|(key, name)| IgnoredGame { key, name }).collect(),
            hotkey_clip: hotkey(v, "hotkeyClip", &d.hotkey_clip),
            hotkey_screenshot: hotkey(v, "hotkeyScreenshot", &d.hotkey_screenshot),
            hotkey_toggle: hotkey(v, "hotkeyToggle", &d.hotkey_toggle),
            recording_enabled: flag("recordingEnabled", d.recording_enabled),
            show_cursor: flag("showCursor", d.show_cursor),
            screen_recording: flag("screenRecording", d.screen_recording),
            screen_monitor: text("screenMonitor", &d.screen_monitor),
            screen_game_switch: flag("screenGameSwitch", d.screen_game_switch),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "overlay": self.overlay,
            "sounds": self.sounds,
            "volume": self.volume,
            "folder": self.folder,
            "naming": self.naming,
            "limitEnabled": self.limit_enabled,
            "limitGb": self.limit_gb,
            "recycle": self.recycle,
            "minFreeGb": self.min_free_gb,
            "autostartAsked": self.autostart_asked,
            "customGames": self.custom_games.iter().map(|g| json!({ "path": g.path, "name": g.name })).collect::<Vec<_>>(),
            "ignoredGames": self.ignored_games.iter().map(|g| json!({ "key": g.key, "name": g.name })).collect::<Vec<_>>(),
            "hotkeyClip": self.hotkey_clip,
            "hotkeyScreenshot": self.hotkey_screenshot,
            "hotkeyToggle": self.hotkey_toggle,
            "recordingEnabled": self.recording_enabled,
            "showCursor": self.show_cursor,
            "screenRecording": self.screen_recording,
            "screenMonitor": self.screen_monitor,
            "screenGameSwitch": self.screen_game_switch,
        })
    }
}

/// A stored hotkey that parses and is allowed, else the default.
fn hotkey(v: &Value, key: &str, default: &str) -> String {
    v[key]
        .as_str()
        .and_then(crate::hotkeys::Combo::parse)
        .filter(|c| crate::hotkeys::reject_reason(c).is_none())
        .map_or(default.to_string(), |c| c.name())
}

/// The settings file and its current content, shared by the app's parts.
pub struct Store {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl Store {
    pub fn open(path: &Path) -> Store {
        let current = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|v| Settings::from_json(&v))
            .unwrap_or_default();
        Store { path: path.to_path_buf(), current: Mutex::new(current) }
    }

    pub fn get(&self) -> Settings {
        self.current.lock().unwrap().clone()
    }

    pub fn set(&self, settings: Settings) -> std::io::Result<()> {
        let mut current = self.current.lock().unwrap();
        *current = settings;
        std::fs::write(&self.path, serde_json::to_string_pretty(&current.to_json()).unwrap_or_default())
    }
}

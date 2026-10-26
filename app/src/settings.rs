//! User settings kept in `<work-dir>/settings.json`: in-game notices and sounds
//! (overlay.in-game, alerts.sound), storage (storage.location, storage.limits), the
//! first-run startup question (app.startup), the games the user added or ignored
//! (game.manual-add), the hotkeys (clip.hotkeys), recording on or off
//! (record.toggle), the mouse cursor (capture.cursor), screen recording
//! (capture.screen, off by default per D13), the video and the clip length
//! (quality.encoding, record.replay-buffer), and what the clips' main audio track
//! mixes (audio.separate-tracks), whether Laurel checks for updates (stage 6
//! plan), the interface language, how long deleted items wait in Recently
//! deleted (library.trash), and the automatic clips of Steam achievements
//! (autoclip.steam-achievements). Missing or invalid fields fall back to the
//! defaults.

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
    /// Days a deleted item waits in Recently deleted (library.trash): 1, 7, 15 or 30.
    pub deleted_days: u32,
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
    /// Video encoder: auto, or a family (nvenc, amf, qsv, x264).
    pub video_encoder: String,
    /// h264, hevc or av1; H.264 is the fallback.
    pub video_codec: String,
    /// cbr or vbr.
    pub video_rate: String,
    /// quality, balanced or performance, mapped to each encoder's presets.
    pub video_preset: String,
    pub video_bitrate: u32,
    /// Output height; 0 records at the monitor's size.
    pub video_height: u32,
    pub video_fps: u32,
    /// Replay buffer length in seconds.
    pub clip_secs: u32,
    /// Track 1: game, game-mic or pc-mic (D12 revised); tracks 2 and 3 stay the game
    /// and the microphone.
    pub audio_mix: String,
    /// Ask the update channel for a new version a little after each start.
    pub check_updates: bool,
    /// system, en or pt-BR (see i18n.rs).
    pub language: String,
    /// Automatic clips (autoclip.steam-achievements): the main switch, the Steam
    /// achievements group and its three events (any, under 10%, under 5% of the
    /// players), the Steam games left out, and the auto-clip sound.
    pub auto_clip: bool,
    pub steam_achievements: bool,
    pub steam_any: bool,
    pub steam_rare: bool,
    pub steam_ultra: bool,
    pub steam_excluded: Vec<IgnoredGame>,
    pub auto_clip_sound: bool,
}

pub const DEFAULT_NAMING: &str = "{game} {date} {time}";
pub const DEFAULT_HOTKEY_CLIP: &str = "F8";
pub const DEFAULT_HOTKEY_SCREENSHOT: &str = "Shift+F8";
/// The choices for `deleted_days`, decided by the owner on 2026-10-25.
pub const DELETED_DAYS: [u32; 4] = [1, 7, 15, 30];

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            // Off since 0.1.0-rc.3: a notice over the game takes it out of
            // independent flip while it shows (owner's decision of 2026-10-26).
            overlay: false,
            sounds: true,
            volume: 70,
            folder: String::new(),
            naming: DEFAULT_NAMING.into(),
            limit_enabled: true,
            limit_gb: 50,
            recycle: true,
            min_free_gb: 2,
            deleted_days: 7,
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
            video_encoder: "auto".into(),
            video_codec: "h264".into(),
            video_rate: "cbr".into(),
            video_preset: "balanced".into(),
            video_bitrate: 20000,
            video_height: 0,
            video_fps: 60,
            clip_secs: 30,
            audio_mix: "game-mic".into(),
            check_updates: true,
            language: "system".into(),
            auto_clip: true,
            steam_achievements: true,
            steam_any: false,
            steam_rare: true,
            steam_ultra: true,
            steam_excluded: vec![],
            auto_clip_sound: true,
        }
    }
}

impl Settings {
    pub fn from_json(v: &Value) -> Settings {
        let d = Settings::default();
        let flag = |key: &str, default: bool| v[key].as_bool().unwrap_or(default);
        let number = |key: &str, default: u32, max: u64| v[key].as_u64().map_or(default, |n| n.min(max) as u32);
        let text = |key: &str, default: &str| v[key].as_str().map_or(default.to_string(), |s| s.trim().to_string());
        let choice = |key: &str, allowed: &[&str], default: &str| v[key].as_str().filter(|s| allowed.contains(s)).unwrap_or(default).to_string();
        let pairs = |key: &str, a: &str, b: &str| -> Vec<(String, String)> {
            v[key].as_array().into_iter().flatten().filter_map(|e| Some((e[a].as_str()?.to_string(), e[b].as_str()?.to_string()))).collect()
        };
        Settings {
            // Stored as "overlayNotices" since 0.1.0-rc.3; the old "overlay" key is
            // ignored, so the new default reaches the earlier installs once.
            overlay: flag("overlayNotices", d.overlay),
            sounds: flag("sounds", d.sounds),
            volume: number("volume", d.volume as u32, 100) as u8,
            folder: text("folder", &d.folder),
            naming: Some(text("naming", &d.naming)).filter(|s| !s.is_empty()).unwrap_or(d.naming),
            limit_enabled: flag("limitEnabled", d.limit_enabled),
            limit_gb: number("limitGb", d.limit_gb, 100_000).max(1),
            recycle: flag("recycle", d.recycle),
            min_free_gb: number("minFreeGb", d.min_free_gb, 1000),
            deleted_days: Some(number("deletedDays", d.deleted_days, 30)).filter(|n| DELETED_DAYS.contains(n)).unwrap_or(d.deleted_days),
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
            video_encoder: choice("videoEncoder", &["auto", "nvenc", "amf", "qsv", "x264"], &d.video_encoder),
            video_codec: choice("videoCodec", &["h264", "hevc", "av1"], &d.video_codec),
            video_rate: choice("videoRate", &["cbr", "vbr"], &d.video_rate),
            video_preset: choice("videoPreset", &["quality", "balanced", "performance"], &d.video_preset),
            video_bitrate: number("videoBitrate", d.video_bitrate, 200_000).max(1000),
            video_height: number("videoHeight", d.video_height, 4320),
            video_fps: number("videoFps", d.video_fps, 360).max(10),
            clip_secs: number("clipSecs", d.clip_secs, 600).max(5),
            audio_mix: choice("audioMix", &["game", "game-mic", "pc-mic"], &d.audio_mix),
            check_updates: flag("checkUpdates", d.check_updates),
            language: choice("language", crate::i18n::LANGUAGES, &d.language),
            auto_clip: flag("autoClip", d.auto_clip),
            steam_achievements: flag("steamAchievements", d.steam_achievements),
            steam_any: flag("steamAny", d.steam_any),
            steam_rare: flag("steamRare", d.steam_rare),
            steam_ultra: flag("steamUltra", d.steam_ultra),
            steam_excluded: pairs("steamExcluded", "key", "name").into_iter().map(|(key, name)| IgnoredGame { key, name }).collect(),
            auto_clip_sound: flag("autoClipSound", d.auto_clip_sound),
        }
    }

    /// Whether the video or the clip length differ: the engine restarts with them.
    pub fn video_differs(&self, other: &Settings) -> bool {
        (&self.video_encoder, &self.video_codec, &self.video_rate, &self.video_preset, self.video_bitrate, self.video_height, self.video_fps, self.clip_secs)
            != (&other.video_encoder, &other.video_codec, &other.video_rate, &other.video_preset, other.video_bitrate, other.video_height, other.video_fps, other.clip_secs)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "overlayNotices": self.overlay,
            "sounds": self.sounds,
            "volume": self.volume,
            "folder": self.folder,
            "naming": self.naming,
            "limitEnabled": self.limit_enabled,
            "limitGb": self.limit_gb,
            "recycle": self.recycle,
            "minFreeGb": self.min_free_gb,
            "deletedDays": self.deleted_days,
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
            "videoEncoder": self.video_encoder,
            "videoCodec": self.video_codec,
            "videoRate": self.video_rate,
            "videoPreset": self.video_preset,
            "videoBitrate": self.video_bitrate,
            "videoHeight": self.video_height,
            "videoFps": self.video_fps,
            "clipSecs": self.clip_secs,
            "audioMix": self.audio_mix,
            "checkUpdates": self.check_updates,
            "language": self.language,
            "autoClip": self.auto_clip,
            "steamAchievements": self.steam_achievements,
            "steamAny": self.steam_any,
            "steamRare": self.steam_rare,
            "steamUltra": self.steam_ultra,
            "steamExcluded": self.steam_excluded.iter().map(|g| json!({ "key": g.key, "name": g.name })).collect::<Vec<_>>(),
            "autoClipSound": self.auto_clip_sound,
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

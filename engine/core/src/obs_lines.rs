//! What the engine reads in the lines libobs and its plugins log, as OBS 32.1.2
//! writes them (wiki/projeto/runtime-obs.md). Each reader takes one line, without
//! the level the engine puts before it in engine.log, and never fails: a line it
//! does not know gives nothing. A new OBS version is checked against these lines by
//! the crate's tests.

/// The level of a libobs log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Info,
    Debug,
}

impl Level {
    /// The level of libobs' number (LOG_ERROR 100, LOG_WARNING 200, LOG_INFO 300,
    /// LOG_DEBUG 400); any other number is debug.
    pub fn from_obs(level: i32) -> Level {
        match level {
            100 => Level::Error,
            200 => Level::Warning,
            300 => Level::Info,
            _ => Level::Debug,
        }
    }

    /// The word engine.log puts before the line.
    pub fn name(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Info => "info",
            Level::Debug => "debug",
        }
    }
}

/// The source's name and OBS's message in a game capture line such as
/// "[game-capture: 'capture'] init_shtex_capture: failed to open shared handle".
pub fn game_capture_line(line: &str) -> Option<(&str, &str)> {
    line.strip_prefix("[game-capture: '")?.split_once("'] ")
}

/// A13: the source in a game capture line that says OBS cannot open the game's
/// shared texture (the game renders on another GPU).
pub fn shared_texture_failure(line: &str) -> Option<&str> {
    let (name, message) = game_capture_line(line)?;
    message.starts_with("init_shtex_capture: failed to open shared handle").then_some(name)
}

/// A28: the reason in an obs-nvenc line about its texture encoder, if it is one: an
/// error ("init_session: ... failed: 1 (NV_ENC_ERR_NO_ENCODE_DEVICE)" gives the
/// error's name) or a fallback it chose ("..., falling back to non-texture encoder").
pub fn nvenc_texture_failure(level: Level, line: &str) -> Option<String> {
    let message = line.strip_prefix("[obs-nvenc] ")?;
    if message.contains("falling back to") {
        return Some(message.to_string());
    }
    if level != Level::Error {
        return None;
    }
    let named = message.rsplit_once('(').and_then(|(_, rest)| rest.strip_suffix(')')).filter(|name| name.starts_with("NV_ENC_ERR_"));
    Some(named.unwrap_or(message).to_string())
}

/// A24: the line libobs logs after a Direct3D 11 call fails on a lost device
/// ("  Device Removed Reason: 887A0005").
pub fn device_removed(line: &str) -> bool {
    line.contains("Device Removed Reason")
}

/// What a WASAPI audio source logged about its device (A16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioLine {
    /// The source could not start its device, or tries it again; with the Windows
    /// error when OBS logged one (only the first failure in a row has it).
    Failed { source: String, code: Option<String> },
    /// The source started its device.
    Started { source: String },
}

/// The reader of win-wasapi's lines. One thread logs a device's error and then
/// which source failed to start, so the engine keeps one reader per thread.
#[derive(Debug, Default)]
pub struct AudioLines {
    /// The error of the last "[WASAPISource::TryInitialize]" line.
    init_error: Option<String>,
}

impl AudioLines {
    /// The audio device event in a libobs line, if any:
    /// "[WASAPISource::TryInitialize]:[<device>] Failed to initialize audio client: 80070005",
    /// "WASAPI: Device 'default' failed to start (source: microphone)",
    /// "Device '<device>' invalidated.  Retrying (source: microphone)",
    /// "WASAPI: Device '<device>' [48000 Hz] initialized (source: microphone)".
    pub fn read(&mut self, line: &str) -> Option<AudioLine> {
        if let Some(rest) = line.strip_prefix("[WASAPISource::TryInitialize]:[") {
            let code = rest.rsplit(": ").next().filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_hexdigit()));
            self.init_error = code.map(str::to_ascii_uppercase);
            return None;
        }
        let source = line.strip_suffix(')')?.rsplit_once("(source: ")?.1.to_string();
        if line.starts_with("WASAPI: Device '") && line.contains("' failed to start (source: ") {
            Some(AudioLine::Failed { source, code: self.init_error.take() })
        } else if line.starts_with("Device '") && line.contains("' invalidated.  Retrying (source: ") {
            Some(AudioLine::Failed { source, code: None })
        } else if line.starts_with("WASAPI: Device '") && line.contains("] initialized (source: ") {
            Some(AudioLine::Started { source })
        } else {
            None
        }
    }
}

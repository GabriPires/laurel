//! Recording pipeline for PoC-E1: one capture source, desktop and microphone audio
//! on separate tracks, one video encoder and a libobs replay buffer.
//!
//! Specs: capture.*, record.replay-buffer, record.buffer-storage, record.screenshots,
//! quality.encoding, quality.container, audio.* (wiki/specs/).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::obs::{clear_output_channel, cstr, ffi, opt_string, Data, Encoder, Obs, Output, Scene, SharedSource, Source};

const VIDEO_CHANNEL: u32 = 0;
/// Game audio, or all PC audio in `AudioMode::All`.
const DESKTOP_AUDIO_CHANNEL: u32 = 1;
const MIC_CHANNEL: u32 = 3;

/// Audio mixer bits: track 1 is the full mix, track 2 the game (or all PC audio),
/// track 3 the microphone (decision D12).
const MIX_TRACK: u32 = 1 << 0;
const DESKTOP_TRACK: u32 = 1 << 1;
const MIC_TRACK: u32 = 1 << 2;

/// The process the game capture source is hooked into, from its "hooked" signal.
/// Feeds the library (game name) and, later, the game audio capture (D12).
#[derive(Clone, Debug, Default)]
pub struct GameInfo {
    pub title: String,
    pub class: String,
    pub exe: String,
}

/// What the game capture's signal callbacks share with the recorder and with the
/// thread that points the game audio source at the hooked process.
#[derive(Default)]
struct GameWatch {
    game: Mutex<Option<GameInfo>>,
    /// Feeds the game audio binder; None when there is no game audio source.
    binder: Mutex<Option<Sender<GameInfo>>>,
    /// Executable (or test window spec) the game audio follows.
    audio_bound: Mutex<Option<String>>,
}

/// Where track 2 comes from (spec audio.per-app, decision D12).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioMode {
    /// The audio of the hooked game's process only (Windows process loopback).
    Game,
    /// Everything the default output device plays (Parity option).
    All,
}

/// A libobs window spec `title:class:exe`, with ':' and '#' escaped as libobs does.
fn window_spec(game: &GameInfo) -> String {
    let enc = |s: &str| s.replace('#', "#22").replace(':', "#3A");
    format!("{}:{}:{}", enc(&game.title), enc(&game.class), enc(&game.exe))
}

/// Match the window's title, otherwise any window of the same executable. The app
/// finds games by executable (spec game.detection); titles can change.
const MATCH_TITLE_THEN_EXE: i64 = 2;

#[derive(Clone, Debug)]
pub enum Capture {
    /// Monitor by index in the `monitor_capture` device list, with the capture
    /// method value of that source (0 automatic, 1 DXGI duplication, 2 WGC).
    Monitor { index: usize, method: i64 },
    /// Monitor by its `monitor_capture` device id (spec capture.screen), which stays
    /// the same when the monitor order changes.
    MonitorId(String),
    /// Window by libobs window spec `title:class:exe`.
    Window(String),
    /// Game capture of any fullscreen application, or of a specific window.
    Game(Option<String>),
    /// No capture source: the app found no game (spec game.detection).
    None,
}

#[derive(Clone, Debug)]
pub struct RecorderConfig {
    pub capture: Capture,
    pub cursor: bool,
    pub video_encoder: String,
    pub bitrate_kbps: i64,
    pub buffer_secs: i64,
    /// Game (or PC) audio on track 2.
    pub desktop_audio: bool,
    pub audio: AudioMode,
    /// Tests: bind the game audio to this window spec from the start.
    pub game_audio: Option<String>,
    pub mic: bool,
    pub clips_dir: PathBuf,
    pub output_size: (u32, u32),
    /// The canvas the capture is fitted into.
    pub canvas: (u32, u32),
    /// When false only the capture source is created (no audio, encoders or
    /// replay buffer); used to measure capture cost in isolation.
    pub record: bool,
    /// Build the replay buffer without starting it; the app starts it when it finds
    /// a game (spec game.detection).
    pub start_paused: bool,
}

pub struct Recorder {
    // Field order matters for drop: output first, then encoders, then sources.
    output: Option<Output>,
    _encoders: Vec<Encoder>,
    _sources: Vec<Source>,
    capture: Option<Source>,
    /// Fits the capture into the canvas; it is what the video channel shows.
    scene: Scene,
    /// Shared with the signal callbacks (stable pointer) and the audio binder.
    game: Arc<GameWatch>,
    watching_game: bool,
    binder_thread: Option<JoinHandle<()>>,
    saved_tx: *mut Sender<()>,
    saved_rx: Receiver<()>,
    output_size: (u32, u32),
    /// Whether captures draw the mouse cursor (spec capture.cursor).
    cursor: bool,
}

unsafe extern "C" fn on_replay_saved(data: *mut c_void, _cd: *mut ffi::calldata_t) {
    let tx = &*(data as *const Sender<()>);
    let _ = tx.send(());
}

impl Recorder {
    pub fn start(obs: &Obs, cfg: &RecorderConfig) -> Result<Recorder, String> {
        // Watch the game capture before anything else: with the hook already in the
        // game, "hooked" can fire before the rest of the pipeline exists.
        let game = Arc::new(GameWatch::default());
        let capture = create_capture(obs, &cfg.capture, cfg.cursor)?;
        let watching_game = capture.as_ref().is_some_and(|c| connect_game_signals(c, &cfg.capture, &game));
        let mut scene = Scene::new("laurel-canvas", cfg.canvas)?;
        scene.set(capture.as_ref());
        scene.set_output_channel(VIDEO_CHANNEL);
        let (tx, saved_rx) = channel();
        let saved_tx = Box::into_raw(Box::new(tx));
        if !cfg.record {
            return Ok(Recorder { output: None, _encoders: vec![], _sources: vec![], capture, scene, game, watching_game, binder_thread: None, saved_tx, saved_rx, output_size: cfg.output_size, cursor: cfg.cursor });
        }

        let mut sources = Vec::new();
        let mut track_masks = vec![MIX_TRACK];
        let mut binder_thread = None;
        if cfg.desktop_audio {
            let audio = match cfg.audio {
                AudioMode::All => Source::new("wasapi_output_capture", "desktop-audio", &Data::new().str("device_id", "default"))?,
                AudioMode::Game => {
                    let window = cfg.game_audio.clone().unwrap_or_default();
                    let source = Source::new("wasapi_process_output_capture", "game-audio", &Data::new().str("window", &window).int("priority", MATCH_TITLE_THEN_EXE))?;
                    if !window.is_empty() {
                        *game.audio_bound.lock().unwrap() = Some(window);
                    }
                    binder_thread = Some(spawn_audio_binder(SharedSource::of(&source), game.clone()));
                    source
                }
            };
            audio.set_audio_mixers(MIX_TRACK | DESKTOP_TRACK);
            audio.set_output_channel(DESKTOP_AUDIO_CHANNEL);
            sources.push(audio);
            track_masks.push(DESKTOP_TRACK);
        }
        if cfg.mic {
            let mic = Source::new("wasapi_input_capture", "microphone", &Data::new().str("device_id", "default"))?;
            mic.set_audio_mixers(MIX_TRACK | MIC_TRACK);
            mic.set_output_channel(MIC_CHANNEL);
            sources.push(mic);
            track_masks.push(MIC_TRACK);
        }

        let video_settings = Data::new()
            .str("rate_control", "CBR")
            .int("bitrate", cfg.bitrate_kbps)
            .int("keyint_sec", 1);
        let venc = Encoder::video(&cfg.video_encoder, "video", &video_settings)?;

        let mut encoders = vec![];
        let out_settings = Data::new()
            .str("directory", &cfg.clips_dir.display().to_string())
            .str("format", "Laurel %CCYY-%MM-%DD %hh-%mm-%ss")
            .str("extension", "mp4")
            .int("max_time_sec", cfg.buffer_secs)
            // Size cap with headroom over bitrate * duration (video plus audio).
            .int("max_size_mb", (cfg.bitrate_kbps + 640) * cfg.buffer_secs * 2 / 8 / 1000 + 64);
        let output = Output::new("replay_buffer", "replay-buffer", &out_settings)?;
        output.set_video_encoder(&venc);
        for (track, mask) in track_masks.iter().enumerate() {
            let mixer = mask.trailing_zeros() as usize;
            let aenc = Encoder::audio("ffmpeg_aac", &format!("audio-track-{}", track + 1), &Data::new().int("bitrate", 160), mixer)?;
            output.set_audio_encoder(&aenc, track);
            encoders.push(aenc);
        }
        encoders.push(venc);

        output.connect("saved", on_replay_saved, saved_tx.cast());
        if !cfg.start_paused {
            output.start()?;
        }

        Ok(Recorder { output: Some(output), _encoders: encoders, _sources: sources, capture, scene, game, watching_game, binder_thread, saved_tx, saved_rx, output_size: cfg.output_size, cursor: cfg.cursor })
    }

    fn game_ptr(&self) -> *mut c_void {
        Arc::as_ptr(&self.game).cast_mut().cast()
    }

    fn unwatch_game(&mut self) {
        if let (true, Some(capture)) = (self.watching_game, &self.capture) {
            capture.disconnect("hooked", on_game_hooked, self.game_ptr());
            capture.disconnect("unhooked", on_game_unhooked, self.game_ptr());
        }
        self.watching_game = false;
        *self.game.game.lock().unwrap() = None;
    }

    /// Starts or stops the replay buffer; the capture keeps running.
    pub fn set_recording(&self, enabled: bool) -> Result<(), String> {
        let output = self.output()?;
        match (enabled, output.active()) {
            (true, false) => output.start()?,
            (false, true) => output.stop(),
            _ => {}
        }
        log!("[laurel] recording {}", if enabled { "resumed" } else { "paused" });
        Ok(())
    }

    /// The game being captured, if the game capture source is hooked.
    pub fn game(&self) -> Option<GameInfo> {
        self.game.game.lock().unwrap().clone()
    }

    /// The process (or test window) the game audio follows, if any.
    pub fn game_audio(&self) -> Option<String> {
        self.game.audio_bound.lock().unwrap().clone()
    }

    /// Replaces the capture source while recording continues.
    pub fn switch_capture(&mut self, obs: &Obs, capture: &Capture) -> Result<(), String> {
        let next = create_capture(obs, capture, self.cursor)?;
        self.unwatch_game();
        if let Some(source) = &next {
            self.watching_game = connect_game_signals(source, capture, &self.game);
        }
        self.scene.set(next.as_ref());
        self.capture = next; // drops our reference to the previous source
        Ok(())
    }

    /// Shows or hides the mouse cursor in the current capture and the next ones,
    /// without restarting it. Each capture kind names the setting differently;
    /// sources ignore settings they do not have.
    pub fn set_cursor(&mut self, cursor: bool) {
        self.cursor = cursor;
        if let Some(source) = &self.capture {
            source.update(&Data::new().bool("capture_cursor", cursor).bool("cursor", cursor));
        }
        log!("[laurel] cursor {}", if cursor { "shown" } else { "hidden" });
    }

    pub fn cursor(&self) -> bool {
        self.cursor
    }

    fn output(&self) -> Result<&Output, String> {
        self.output.as_ref().ok_or_else(|| "not recording (--no-output)".to_string())
    }

    pub fn capture_size(&self) -> (u32, u32) {
        self.capture.as_ref().map_or((0, 0), Source::size)
    }

    /// Saves the replay buffer to a file and returns its path (spec record.replay-buffer).
    pub fn save_clip(&self, timeout: Duration) -> Result<PathBuf, String> {
        let output = self.output()?;
        if !output.active() {
            return Err("not recording: no game is being captured".into());
        }
        while self.saved_rx.try_recv().is_ok() {}
        output.call("save", None)?;
        self.saved_rx.recv_timeout(timeout).map_err(|_| "timed out waiting for the replay to be saved".to_string())?;
        output
            .call("get_last_replay", Some("path"))?
            .map(PathBuf::from)
            .ok_or_else(|| "replay saved but no path reported".to_string())
    }

    pub fn output_size(&self) -> (u32, u32) {
        self.output_size
    }

    /// Writes the next composited frame as PNG, scaled to `size` (spec
    /// record.screenshots; also used for clip thumbnails).
    pub fn screenshot(&self, path: &Path, size: (u32, u32), timeout: Duration) -> Result<(u32, u32), String> {
        let (width, height) = size;
        let shot = Box::new(Shot { width, height, frame: Mutex::new(None), ready: Condvar::new() });
        let param: *mut c_void = (&*shot as *const Shot).cast_mut().cast();
        let conversion = ffi::video_scale_info {
            format: ffi::VIDEO_FORMAT_RGBA,
            width,
            height,
            range: ffi::VIDEO_RANGE_FULL,
            colorspace: ffi::VIDEO_CS_709,
        };
        unsafe { ffi::obs_add_raw_video_callback(&conversion, on_raw_frame, param) };
        let frame = {
            let guard = shot.frame.lock().unwrap();
            let (mut guard, _) = shot.ready.wait_timeout_while(guard, timeout, |f| f.is_none()).unwrap();
            guard.take()
        };
        unsafe { ffi::obs_remove_raw_video_callback(on_raw_frame, param) };
        let rgba = frame.ok_or("timed out waiting for a frame")?;
        write_png(path, width, height, &rgba)?;
        Ok((width, height))
    }

    pub fn frames_dropped(&self) -> i32 {
        self.output.as_ref().map_or(0, Output::frames_dropped)
    }

    pub fn active(&self) -> bool {
        self.output.as_ref().is_some_and(Output::active)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.unwatch_game();
        // Closing the channel ends the binder, which releases its source reference.
        *self.game.binder.lock().unwrap() = None;
        if let Some(thread) = self.binder_thread.take() {
            let _ = thread.join();
        }
        if let Some(output) = &self.output {
            output.stop();
            output.disconnect("saved", on_replay_saved, self.saved_tx.cast());
        }
        for ch in [VIDEO_CHANNEL, DESKTOP_AUDIO_CHANNEL, MIC_CHANNEL] {
            clear_output_channel(ch);
        }
        unsafe { drop(Box::from_raw(self.saved_tx)) };
    }
}

unsafe extern "C" fn on_game_hooked(data: *mut c_void, cd: *mut ffi::calldata_t) {
    let get = |key: &str| {
        let k = cstr(key);
        let mut s = std::ptr::null();
        if ffi::calldata_get_string(cd, k.as_ptr(), &mut s) { opt_string(s).unwrap_or_default() } else { String::new() }
    };
    let game = GameInfo { title: get("title"), class: get("class"), exe: get("executable") };
    log!("[laurel] game hooked: {} ({})", game.exe, game.title);
    let watch = &*(data as *const GameWatch);
    // Store first, then notify; the binder registers first, then reads. Either way
    // the game reaches the binder (at worst twice, which is harmless).
    *watch.game.lock().unwrap() = Some(game.clone());
    if let Some(binder) = watch.binder.lock().unwrap().as_ref() {
        let _ = binder.send(game);
    }
}

unsafe extern "C" fn on_game_unhooked(data: *mut c_void, _cd: *mut ffi::calldata_t) {
    log!("[laurel] game unhooked");
    *(*(data as *const GameWatch)).game.lock().unwrap() = None;
}

/// Connects the game capture's "hooked" and "unhooked" signals; other capture
/// kinds have none. Returns whether it connected.
fn connect_game_signals(source: &Source, capture: &Capture, watch: &Arc<GameWatch>) -> bool {
    if !matches!(capture, Capture::Game(_)) {
        return false;
    }
    let data = Arc::as_ptr(watch).cast_mut().cast();
    source.connect("hooked", on_game_hooked, data);
    source.connect("unhooked", on_game_unhooked, data);
    true
}

/// Points the game audio source at each newly hooked game. Runs on its own thread
/// because the "hooked" signal comes from the graphics thread.
fn spawn_audio_binder(source: SharedSource, watch: Arc<GameWatch>) -> JoinHandle<()> {
    let (tx, rx) = channel::<GameInfo>();
    *watch.binder.lock().unwrap() = Some(tx.clone());
    // A game hooked before the binder existed is bound right away.
    if let Some(game) = watch.game.lock().unwrap().clone() {
        let _ = tx.send(game);
    }
    drop(tx);
    std::thread::spawn(move || {
        for game in rx {
            source.update(&Data::new().str("window", &window_spec(&game)).int("priority", MATCH_TITLE_THEN_EXE));
            log!("[laurel] game audio follows {}", game.exe);
            *watch.audio_bound.lock().unwrap() = Some(game.exe);
        }
    })
}

struct Shot {
    width: u32,
    height: u32,
    frame: Mutex<Option<Vec<u8>>>,
    ready: Condvar,
}

unsafe extern "C" fn on_raw_frame(param: *mut c_void, frame: *mut ffi::video_data) {
    let shot = &*(param as *const Shot);
    let mut slot = shot.frame.lock().unwrap();
    if slot.is_some() {
        return;
    }
    let frame = &*frame;
    let row = shot.width as usize * 4;
    let stride = frame.linesize[0] as usize;
    let mut rgba = Vec::with_capacity(row * shot.height as usize);
    for y in 0..shot.height as usize {
        let line = std::slice::from_raw_parts(frame.data[0].add(y * stride), row);
        rgba.extend_from_slice(line);
    }
    *slot = Some(rgba);
    shot.ready.notify_all();
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())
}

pub fn format_stats(obs: &Obs, rec: &Recorder) -> String {
    let s = obs.stats();
    let (cw, ch) = rec.capture_size();
    format!(
        "capture={cw}x{ch} active_fps={:.2} avg_frame_ms={:.3} lagged={} skipped={}/{} output_dropped={} output_active={}",
        s.active_fps,
        s.average_frame_time_ms,
        s.lagged_frames,
        s.skipped_frames,
        s.total_frames,
        rec.frames_dropped(),
        rec.active()
    )
}

fn create_capture(obs: &Obs, capture: &Capture, cursor: bool) -> Result<Option<Source>, String> {
    let source = match capture {
        Capture::None => return Ok(None),
        Capture::Monitor { index, method } => {
            let monitors = obs.source_list_property("monitor_capture", "monitor_id");
            let (label, id) = monitors
                .get(*index)
                .ok_or_else(|| format!("monitor {index} not found ({} available)", monitors.len()))?;
            log!("[laurel] capturing monitor {index}: {label}");
            Source::new(
                "monitor_capture",
                "capture",
                &Data::new().str("monitor_id", id).int("method", *method).bool("capture_cursor", cursor),
            )?
        }
        Capture::MonitorId(id) => {
            // An empty id means the primary monitor.
            let monitors = obs.source_list_property("monitor_capture", "monitor_id");
            let (label, id) = monitors
                .iter()
                .find(|(label, value)| if id.is_empty() { label.contains("Primary Monitor") } else { value == id })
                .or(monitors.first().filter(|_| id.is_empty()))
                .cloned()
                .ok_or_else(|| format!("monitor {id} not found ({} available)", monitors.len()))?;
            log!("[laurel] capturing monitor {label}");
            Source::new("monitor_capture", "capture", &Data::new().str("monitor_id", &id).int("method", 0).bool("capture_cursor", cursor))?
        }
        // Windows Graphics Capture (method 2) also captures Direct3D windows, which
        // the BitBlt method shows black; the window is found by executable when its
        // title changes.
        Capture::Window(spec) => Source::new(
            "window_capture",
            "capture",
            &Data::new().str("window", spec).int("priority", MATCH_TITLE_THEN_EXE).bool("cursor", cursor).int("method", 2).bool("client_area", true),
        )?,
        Capture::Game(window) => {
            let settings = match window {
                Some(spec) => Data::new().str("capture_mode", "window").str("window", spec).int("priority", MATCH_TITLE_THEN_EXE),
                None => Data::new().str("capture_mode", "any_fullscreen"),
            };
            Source::new("game_capture", "capture", &settings.bool("capture_cursor", cursor).bool("anti_cheat_hook", true))?
        }
    };
    Ok(Some(source))
}

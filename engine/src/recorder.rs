//! Recording pipeline for PoC-E1: one capture source, game and microphone audio
//! on separate tracks plus the mix the user chose, one video encoder and a libobs
//! replay buffer.
//!
//! Specs: capture.*, record.replay-buffer, record.buffer-storage, record.screenshots,
//! quality.encoding, quality.container, audio.* (wiki/specs/).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::obs::{
    clear_output_channel, cstr, ffi, opt_string, watch_audio_events, watch_shared_texture_failures, AudioEvent, Data, Encoder, Obs, Output, Scene, SharedSource,
    Source,
};

const VIDEO_CHANNEL: u32 = 0;
const GAME_AUDIO_CHANNEL: u32 = 1;
/// All PC audio, only while the mix asks for it.
const PC_AUDIO_CHANNEL: u32 = 2;
const MIC_CHANNEL: u32 = 3;

/// Audio mixer bits: track 1 is the mix the user chose, track 2 the game, track 3
/// the microphone (decision D12, revised 2026-10-24).
const MIX_TRACK: u32 = 1 << 0;
const GAME_TRACK: u32 = 1 << 1;
const MIC_TRACK: u32 = 1 << 2;

/// E_ACCESSDENIED: Windows' privacy settings keep the microphone from desktop apps.
const ACCESS_DENIED: &str = "80070005";
/// A denied microphone leaves its channel, which ends OBS's tries every 3 s, and is
/// tried again after this long, the wait doubling up to MIC_RETRY_MAX.
const MIC_RETRY_FIRST: Duration = Duration::from_secs(60);
const MIC_RETRY_MAX: Duration = Duration::from_secs(300);

/// Seconds between keyframes, for every video encoder.
pub const KEYINT_SECS: i64 = 1;

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

/// Game capture sources by name ("capture", "game-try"), for the thread that moves
/// one to shared-memory capture when OBS cannot open its shared texture: the game
/// renders on another GPU than the engine (A13 of the stability tests). The hook
/// then copies each frame through system memory (OBS's multi-adapter
/// compatibility); Direct3D 12 has no such path in OBS, and the app falls back to
/// the game's window as before.
#[derive(Default)]
struct TextureWatch {
    /// Name, source, and whether it was already moved to shared memory.
    sources: Mutex<Vec<(String, SharedSource, bool)>>,
}

impl TextureWatch {
    fn register(&self, name: &str, source: &Source) {
        let mut sources = self.sources.lock().unwrap();
        sources.retain(|(n, _, _)| n != name);
        sources.push((name.to_string(), SharedSource::of(source), false));
    }

    fn unregister(&self, name: &str) {
        self.sources.lock().unwrap().retain(|(n, _, _)| n != name);
    }

    fn clear(&self) {
        self.sources.lock().unwrap().clear();
    }
}

/// Follows the microphone's device (A16 of the stability tests): reports its state to
/// the app ("starting", "ok", "failed" or "denied", with the Windows error) and,
/// while Windows denies it, keeps it off its channel between tries. Runs on its own
/// thread: OBS logs from the source's thread, which the channel change waits for.
fn spawn_mic_watch(mic: SharedSource, events: Receiver<AudioEvent>, state: Arc<Mutex<Value>>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        // The last Windows error: OBS logs it only on the first failure in a row.
        let mut code: Option<String> = None;
        let mut wait = MIC_RETRY_FIRST;
        let mut retry_at: Option<Instant> = None;
        let report = |now: &str, code: &Option<String>| {
            let value = json!({ "state": now, "code": code });
            let mut current = state.lock().unwrap();
            if *current != value {
                log!("[laurel] microphone {now}{}", code.as_ref().map_or(String::new(), |c| format!(" ({c})")));
                *current = value.clone();
                crate::rpc::notify("microphone", value);
            }
        };
        let try_again = |why: &str| {
            log!("[laurel] microphone: trying again ({why})");
            mic.set_output_channel(MIC_CHANNEL);
        };
        loop {
            let event = match retry_at {
                Some(at) => events.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => events.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match event {
                Ok(AudioEvent::Failed { source, code: error }) if source == "microphone" => {
                    if error.is_some() {
                        code = error;
                    }
                    if code.as_deref() != Some(ACCESS_DENIED) {
                        report("failed", &code);
                        continue;
                    }
                    // A try that was already under way may still log once: the
                    // microphone is off its channel already.
                    if retry_at.is_none() {
                        clear_output_channel(MIC_CHANNEL);
                        log!("[laurel] microphone off its channel: Windows denies it; next try in {} s", wait.as_secs());
                        retry_at = Some(Instant::now() + wait);
                        wait = (wait * 2).min(MIC_RETRY_MAX);
                    }
                    report("denied", &code);
                }
                Ok(AudioEvent::Started { source }) if source == "microphone" => {
                    code = None;
                    wait = MIC_RETRY_FIRST;
                    retry_at = None;
                    report("ok", &code);
                }
                Ok(AudioEvent::Retry) => {
                    wait = MIC_RETRY_FIRST;
                    retry_at = None;
                    try_again("asked");
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    retry_at = None;
                    try_again("timer");
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    })
}

/// Moves a game capture whose shared texture OBS cannot open to shared-memory
/// capture, once per source. Runs on its own thread: OBS logs the failure from
/// the graphics thread, inside the source's tick.
fn spawn_texture_watch(watch: Arc<TextureWatch>) -> JoinHandle<()> {
    let (tx, rx) = channel::<String>();
    watch_shared_texture_failures(Some(tx));
    std::thread::spawn(move || {
        for name in rx {
            let source = {
                let mut sources = watch.sources.lock().unwrap();
                sources.iter_mut().find(|(n, _, moved)| *n == name && !*moved).map(|(_, source, moved)| {
                    *moved = true;
                    source.share()
                })
            };
            if let Some(source) = source {
                log!("[laurel] game capture '{name}': OBS cannot open the game's shared texture (the game runs on another GPU); trying shared-memory capture");
                source.update(&Data::new().bool("sli_compatibility", true));
            }
        }
    })
}

/// What track 1, the one players play, mixes (spec audio.separate-tracks, D12
/// revised 2026-10-24). Tracks 2 and 3 stay the game and the microphone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioMix {
    Game,
    GameMic,
    /// Everything the default output device plays, which already holds the game,
    /// and the microphone (spec audio.all-pc).
    PcMic,
}

impl AudioMix {
    pub fn parse(s: &str) -> Option<AudioMix> {
        match s {
            "game" => Some(AudioMix::Game),
            "game-mic" => Some(AudioMix::GameMic),
            "pc-mic" => Some(AudioMix::PcMic),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AudioMix::Game => "game",
            AudioMix::GameMic => "game-mic",
            AudioMix::PcMic => "pc-mic",
        }
    }
}

/// A libobs window spec `title:class:exe`, with ':' and '#' escaped as libobs does.
fn window_spec(game: &GameInfo) -> String {
    let enc = |s: &str| s.replace('#', "#22").replace(':', "#3A");
    format!("{}:{}:{}", enc(&game.title), enc(&game.class), enc(&game.exe))
}

/// Match the window's title, otherwise any window of the same executable. The app
/// finds games by executable (spec game.detection); titles can change.
const MATCH_TITLE_THEN_EXE: i64 = 2;

/// The game window a capture names: the app names the detected game's window both
/// for game capture and for the window capture it falls back to.
fn named_window(capture: &Capture) -> Option<&str> {
    match capture {
        Capture::Window(spec) | Capture::Game(Some(spec)) => Some(spec),
        _ => None,
    }
}

/// Whether a capture records a game, whose audio the game audio source follows.
/// On its output channel the source is active, and an active process audio source
/// without its window tries again every 3 s, logging each try (OBS win-wasapi): it
/// stays off the channel while there is no game.
fn expects_game(capture: &Capture) -> bool {
    matches!(capture, Capture::Window(_) | Capture::Game(_))
}

/// The executable of a window spec, unescaped.
fn spec_exe(spec: &str) -> String {
    spec.rsplit(':').next().unwrap_or_default().replace("#3A", ":").replace("#22", "#")
}

/// Game audio source settings that capture a window's process (spec audio.per-app).
fn follow_settings(spec: &str) -> Data {
    Data::new().str("window", spec).int("priority", MATCH_TITLE_THEN_EXE)
}

/// Records that the game audio now follows this window's process.
fn followed(spec: &str, watch: &GameWatch) {
    let exe = spec_exe(spec);
    log!("[laurel] game audio follows {exe}");
    *watch.audio_bound.lock().unwrap() = Some(exe);
}

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

/// A video encoder the recorder may use, with its settings; the recorder tries
/// them in order (spec quality.encoding).
pub struct VideoChoice {
    pub id: String,
    pub settings: Data,
}

#[derive(Clone, Debug)]
pub struct RecorderConfig {
    pub capture: Capture,
    pub cursor: bool,
    pub bitrate_kbps: i64,
    pub buffer_secs: i64,
    /// Game audio on track 2 and, with `AudioMix::PcMic`, PC audio on track 1.
    pub desktop_audio: bool,
    pub mix: AudioMix,
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
    video: Option<Encoder>,
    _encoders: Vec<Encoder>,
    game_audio: Option<Source>,
    mic: Option<Source>,
    /// Exists only while the mix is `AudioMix::PcMic`.
    pc_audio: Option<Source>,
    mix: AudioMix,
    desktop_audio: bool,
    capture: Option<Source>,
    /// What `capture` records.
    target: Capture,
    /// While a game is recorded by its window (the fallback), a game capture of the
    /// same window tried on top of it, with that window's spec (see set_probe).
    probe: Option<(String, Source)>,
    /// Fits the capture into the canvas; it is what the video channel shows.
    scene: Scene,
    /// Shared with the signal callbacks (stable pointer) and the audio binder.
    game: Arc<GameWatch>,
    watching_game: bool,
    binder_thread: Option<JoinHandle<()>>,
    /// Game captures that may move to shared memory (A13), and the thread doing it.
    textures: Arc<TextureWatch>,
    texture_thread: Option<JoinHandle<()>>,
    /// The microphone's state for the app (A16), the way to ask its thread for
    /// another try, and that thread.
    mic_state: Arc<Mutex<Value>>,
    mic_tx: Option<Sender<AudioEvent>>,
    mic_thread: Option<JoinHandle<()>>,
    saved_tx: *mut Sender<()>,
    saved_rx: Receiver<()>,
    output_size: (u32, u32),
    /// Whether captures draw the mouse cursor (spec capture.cursor).
    cursor: bool,
    /// The video encoders to try, the one in use, and the ones that failed with why.
    video_choices: Vec<VideoChoice>,
    video_index: usize,
    fallbacks: Vec<(String, String)>,
}

unsafe extern "C" fn on_replay_saved(data: *mut c_void, _cd: *mut ffi::calldata_t) {
    let tx = &*(data as *const Sender<()>);
    let _ = tx.send(());
}

impl Recorder {
    pub fn start(obs: &Obs, cfg: &RecorderConfig, video_choices: Vec<VideoChoice>) -> Result<Recorder, String> {
        // Watch the game capture before anything else: with the hook already in the
        // game, "hooked" can fire before the rest of the pipeline exists.
        let game = Arc::new(GameWatch::default());
        let textures = Arc::new(TextureWatch::default());
        let texture_thread = Some(spawn_texture_watch(textures.clone()));
        let capture = create_capture(obs, &cfg.capture, cfg.cursor)?;
        let watching_game = capture.as_ref().is_some_and(|c| connect_game_signals(c, &cfg.capture, &game));
        if let (Capture::Game(_), Some(source)) = (&cfg.capture, &capture) {
            textures.register("capture", source);
        }
        let mut scene = Scene::new("laurel-canvas", cfg.canvas)?;
        scene.set(capture.as_ref());
        scene.set_output_channel(VIDEO_CHANNEL);
        let (tx, saved_rx) = channel();
        let saved_tx = Box::into_raw(Box::new(tx));
        if !cfg.record {
            return Ok(Recorder {
                output: None,
                video: None,
                _encoders: vec![],
                game_audio: None,
                mic: None,
                pc_audio: None,
                mix: cfg.mix,
                desktop_audio: false,
                capture,
                target: cfg.capture.clone(),
                probe: None,
                scene,
                game,
                watching_game,
                binder_thread: None,
                textures,
                texture_thread,
                mic_state: Arc::new(Mutex::new(json!({ "state": "off" }))),
                mic_tx: None,
                mic_thread: None,
                saved_tx,
                saved_rx,
                output_size: cfg.output_size,
                cursor: cfg.cursor,
                video_choices,
                video_index: 0,
                fallbacks: vec![],
            });
        }

        let mut track_masks = vec![MIX_TRACK];
        let mut binder_thread = None;
        let mut game_audio = None;
        if cfg.desktop_audio {
            // Tests name the window; otherwise the game the app named is followed
            // from the start, without waiting for a hook that may never come.
            let window = cfg.game_audio.as_deref().or(named_window(&cfg.capture)).unwrap_or_default();
            let source = Source::new("wasapi_process_output_capture", "game-audio", &follow_settings(window))?;
            if !window.is_empty() {
                followed(window, &game);
            }
            binder_thread = Some(spawn_audio_binder(SharedSource::of(&source), game.clone()));
            if !window.is_empty() || expects_game(&cfg.capture) {
                source.set_output_channel(GAME_AUDIO_CHANNEL);
            }
            game_audio = Some(source);
            track_masks.push(GAME_TRACK);
        }
        let mut mic = None;
        let mic_state = Arc::new(Mutex::new(json!({ "state": if cfg.mic { "starting" } else { "off" } })));
        let (mut mic_tx, mut mic_thread) = (None, None);
        if cfg.mic {
            // Watch the log first: the device starts on its own thread right away.
            let (tx, rx) = channel();
            watch_audio_events(Some(tx.clone()));
            let source = Source::new("wasapi_input_capture", "microphone", &Data::new().str("device_id", "default"))?;
            mic_thread = Some(spawn_mic_watch(SharedSource::of(&source), rx, mic_state.clone()));
            mic_tx = Some(tx);
            source.set_output_channel(MIC_CHANNEL);
            mic = Some(source);
            track_masks.push(MIC_TRACK);
        }

        let mut encoders = vec![];
        let out_settings = Data::new()
            .str("directory", &cfg.clips_dir.display().to_string())
            .str("format", "Laurel %CCYY-%MM-%DD %hh-%mm-%ss")
            .str("extension", "mp4")
            // OBS keeps the buffer starting on a keyframe and drops a whole keyframe
            // interval once it grows past max_time_sec, so a clip asked for N
            // seconds would last between N - 1 and N; one interval more keeps every
            // clip at least as long as asked (spec record.replay-buffer).
            .int("max_time_sec", cfg.buffer_secs + KEYINT_SECS)
            // Size cap with headroom over bitrate * duration (video plus audio).
            .int("max_size_mb", (cfg.bitrate_kbps + 640) * (cfg.buffer_secs + KEYINT_SECS) * 2 / 8 / 1000 + 64);
        let output = Output::new("replay_buffer", "replay-buffer", &out_settings)?;
        for (track, mask) in track_masks.iter().enumerate() {
            let mixer = mask.trailing_zeros() as usize;
            let aenc = Encoder::audio("ffmpeg_aac", &format!("audio-track-{}", track + 1), &Data::new().int("bitrate", 160), mixer)?;
            output.set_audio_encoder(&aenc, track);
            encoders.push(aenc);
        }
        output.connect("saved", on_replay_saved, saved_tx.cast());

        let mut rec = Recorder {
            output: Some(output),
            video: None,
            _encoders: encoders,
            game_audio,
            mic,
            pc_audio: None,
            mix: cfg.mix,
            desktop_audio: cfg.desktop_audio,
            capture,
            target: cfg.capture.clone(),
            probe: None,
            scene,
            game,
            watching_game,
            binder_thread,
            textures,
            texture_thread,
            mic_state,
            mic_tx,
            mic_thread,
            saved_tx,
            saved_rx,
            output_size: cfg.output_size,
            cursor: cfg.cursor,
            video_choices,
            video_index: 0,
            fallbacks: vec![],
        };
        rec.set_mix(cfg.mix)?;
        rec.first_video_encoder()?;
        if !cfg.start_paused {
            rec.start_output()?;
        }
        Ok(rec)
    }

    /// Puts the first video encoder that can be created on the output.
    fn first_video_encoder(&mut self) -> Result<(), String> {
        let first = self.video_choices.first().map(|c| Encoder::video(&c.id, "video", &c.settings));
        match first {
            Some(Ok(enc)) => {
                self.output()?.set_video_encoder(&enc);
                self.video = Some(enc);
                Ok(())
            }
            Some(Err(e)) => {
                self.fallbacks.push((self.video_choices[0].id.clone(), e));
                if self.next_video_encoder()? { Ok(()) } else { Err("no video encoder could be created".into()) }
            }
            None => Err("no video encoder is available".into()),
        }
    }

    /// Moves to the next video encoder that can be created; false when none is left.
    fn next_video_encoder(&mut self) -> Result<bool, String> {
        while self.video_index + 1 < self.video_choices.len() {
            self.video_index += 1;
            let id = self.video_choices[self.video_index].id.clone();
            match Encoder::video(&id, "video", &self.video_choices[self.video_index].settings) {
                Ok(enc) => {
                    self.output()?.set_video_encoder(&enc);
                    self.video = Some(enc);
                    log!("[laurel] video encoder now {id}");
                    return Ok(true);
                }
                Err(e) => self.fallbacks.push((id, e)),
            }
        }
        Ok(false)
    }

    /// Starts the replay buffer. A video encoder that cannot start (no such GPU, a
    /// codec the GPU lacks, frames on another GPU) gives way to the next choice
    /// (spec quality.encoding); the reasons are kept for the app.
    fn start_output(&mut self) -> Result<(), String> {
        loop {
            let error = match self.output()?.start() {
                Ok(()) => return Ok(()),
                Err(e) => e,
            };
            let failed = self.encoder().to_string();
            log!("[laurel] video encoder {failed} could not start: {error}");
            self.fallbacks.push((failed, error.clone()));
            if !self.next_video_encoder()? {
                return Err(format!("no video encoder could start: {error}"));
            }
        }
    }

    /// The video encoder in use.
    pub fn encoder(&self) -> &str {
        self.video_choices.get(self.video_index).map_or("", |c| c.id.as_str())
    }

    /// The video encoders that could not be used, with the reason.
    pub fn encoder_fallbacks(&self) -> &[(String, String)] {
        &self.fallbacks
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
    pub fn set_recording(&mut self, enabled: bool) -> Result<(), String> {
        match (enabled, self.output()?.active()) {
            (true, false) => self.start_output()?,
            (false, true) => self.output()?.stop(),
            _ => {}
        }
        log!("[laurel] recording {}", if enabled { "resumed" } else { "paused" });
        Ok(())
    }

    /// The game being captured, if the game capture source is hooked.
    pub fn game(&self) -> Option<GameInfo> {
        self.game.game.lock().unwrap().clone()
    }

    /// The microphone's state: `{ "state": "off" | "starting" | "ok" | "failed" |
    /// "denied", "code": <Windows error> }` (A16).
    pub fn microphone(&self) -> Value {
        self.mic_state.lock().unwrap().clone()
    }

    /// Tries the microphone again now (the player allowed it in Windows' settings).
    pub fn retry_microphone(&self) -> Result<(), String> {
        let tx = self.mic_tx.as_ref().ok_or("no microphone")?;
        tx.send(AudioEvent::Retry).map_err(|_| "the microphone watch stopped".to_string())
    }

    /// The process (or test window) the game audio follows, if any.
    pub fn game_audio(&self) -> Option<String> {
        self.game.audio_bound.lock().unwrap().clone()
    }

    /// Replaces the capture source while recording continues. The game audio follows
    /// the game the app names right away: a game recorded by its window never sends
    /// the hook's "hooked" signal. Without a game, the game audio leaves its channel.
    pub fn switch_capture(&mut self, obs: &Obs, capture: &Capture) -> Result<(), String> {
        if self.promote_probe(capture) {
            return Ok(());
        }
        let next = create_capture(obs, capture, self.cursor)?;
        self.drop_probe();
        self.unwatch_game();
        self.textures.clear();
        if let Some(source) = &next {
            self.watching_game = connect_game_signals(source, capture, &self.game);
            if matches!(capture, Capture::Game(_)) {
                self.textures.register("capture", source);
            }
        }
        self.scene.set(next.as_ref());
        self.capture = next; // drops our reference to the previous source
        self.target = capture.clone();
        if let Some(audio) = &self.game_audio {
            if let Some(spec) = named_window(capture) {
                audio.update(&follow_settings(spec));
                followed(spec, &self.game);
            }
            if expects_game(capture) {
                audio.set_output_channel(GAME_AUDIO_CHANNEL);
            } else {
                clear_output_channel(GAME_AUDIO_CHANNEL);
                if self.game.audio_bound.lock().unwrap().take().is_some() {
                    log!("[laurel] game audio off: no game");
                }
            }
        }
        Ok(())
    }

    /// While a game is recorded by its window (the app's fallback when its hook did
    /// not start), tries a game capture of the same window on top of it, or stops
    /// trying. The game capture draws nothing until it hooks the game; then it covers
    /// the window, and switch_capture to that game makes it the capture. The app
    /// keeps the tries short: a waiting game capture logs a few lines every 2 s.
    pub fn set_probe(&mut self, enabled: bool) -> Result<bool, String> {
        if !enabled {
            self.drop_probe();
            return Ok(false);
        }
        if self.probe.is_some() {
            return Ok(true);
        }
        let Capture::Window(spec) = &self.target else {
            return Err("not recording a game by its window".into());
        };
        let spec = spec.clone();
        let source = game_capture("game-try", Some(&spec), self.cursor)?;
        connect_game_signals(&source, &Capture::Game(Some(spec.clone())), &self.game);
        self.textures.register("game-try", &source);
        self.scene.set_top(Some(&source));
        log!("[laurel] trying the game hook over the window capture");
        self.probe = Some((spec, source));
        Ok(true)
    }

    /// Whether a game capture is being tried over the window capture.
    pub fn probing(&self) -> bool {
        self.probe.is_some()
    }

    /// Makes the tried game capture the capture once it hooked the game the app now
    /// names: no second hook, and the window capture (with the border Windows 10
    /// draws around it) goes away.
    fn promote_probe(&mut self, capture: &Capture) -> bool {
        let Capture::Game(Some(spec)) = capture else { return false };
        if !self.probe.as_ref().is_some_and(|(window, _)| window == spec) || self.game().is_none() {
            return false;
        }
        let Some((_, probe)) = self.probe.take() else { return false };
        self.scene.promote_top(&probe);
        self.capture = Some(probe); // drops the window capture
        self.watching_game = true; // set_probe connected its signals
        self.target = capture.clone();
        log!("[laurel] the game capture took over from the window capture");
        true
    }

    /// Stops trying the game capture over the window capture.
    fn drop_probe(&mut self) {
        let Some((_, probe)) = self.probe.take() else { return };
        self.textures.unregister("game-try");
        probe.disconnect("hooked", on_game_hooked, self.game_ptr());
        probe.disconnect("unhooked", on_game_unhooked, self.game_ptr());
        self.scene.set_top(None);
        if !self.watching_game {
            *self.game.game.lock().unwrap() = None;
        }
        log!("[laurel] stopped trying the game hook");
    }

    /// Shows or hides the mouse cursor in the current capture and the next ones,
    /// without restarting it. Each capture kind names the setting differently;
    /// sources ignore settings they do not have.
    pub fn set_cursor(&mut self, cursor: bool) {
        self.cursor = cursor;
        for source in self.capture.iter().chain(self.probe.as_ref().map(|(_, probe)| probe)) {
            source.update(&Data::new().bool("capture_cursor", cursor).bool("cursor", cursor));
        }
        log!("[laurel] cursor {}", if cursor { "shown" } else { "hidden" });
    }

    pub fn cursor(&self) -> bool {
        self.cursor
    }

    /// Chooses what track 1 mixes, while recording continues (the next clips have
    /// it). The PC audio already holds the game, so with it the game source stays on
    /// track 2 only; the PC capture exists only while this mix is chosen.
    pub fn set_mix(&mut self, mix: AudioMix) -> Result<(), String> {
        self.output()?;
        let pc = mix == AudioMix::PcMic && self.desktop_audio;
        if pc && self.pc_audio.is_none() {
            let source = Source::new("wasapi_output_capture", "pc-audio", &Data::new().str("device_id", "default"))?;
            source.set_audio_mixers(MIX_TRACK);
            source.set_output_channel(PC_AUDIO_CHANNEL);
            self.pc_audio = Some(source);
        } else if !pc {
            if let Some(source) = self.pc_audio.take() {
                clear_output_channel(PC_AUDIO_CHANNEL);
                drop(source);
            }
        }
        if let Some(game) = &self.game_audio {
            game.set_audio_mixers(GAME_TRACK | if mix == AudioMix::PcMic { 0 } else { MIX_TRACK });
        }
        if let Some(mic) = &self.mic {
            mic.set_audio_mixers(MIC_TRACK | if mix == AudioMix::Game { 0 } else { MIX_TRACK });
        }
        self.mix = mix;
        log!("[laurel] audio mix {} (track 1)", mix.name());
        Ok(())
    }

    pub fn audio_mix(&self) -> AudioMix {
        self.mix
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
        self.drop_probe();
        self.unwatch_game();
        // Closing the channel ends the binder, which releases its source reference.
        *self.game.binder.lock().unwrap() = None;
        if let Some(thread) = self.binder_thread.take() {
            let _ = thread.join();
        }
        // Closing both senders ends the microphone thread and its source reference.
        watch_audio_events(None);
        self.mic_tx = None;
        if let Some(thread) = self.mic_thread.take() {
            let _ = thread.join();
        }
        // Closing the log watch ends the texture thread; the sources it held go too.
        self.textures.clear();
        watch_shared_texture_failures(None);
        if let Some(thread) = self.texture_thread.take() {
            let _ = thread.join();
        }
        if let Some(output) = &self.output {
            output.stop();
            output.disconnect("saved", on_replay_saved, self.saved_tx.cast());
        }
        for ch in [VIDEO_CHANNEL, GAME_AUDIO_CHANNEL, PC_AUDIO_CHANNEL, MIC_CHANNEL] {
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
            let spec = window_spec(&game);
            source.update(&follow_settings(&spec));
            followed(&spec, &watch);
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
        Capture::Game(window) => game_capture("capture", window.as_deref(), cursor)?,
    };
    Ok(Some(source))
}

/// A game capture of `window` (a libobs window spec), or of any fullscreen game.
fn game_capture(name: &str, window: Option<&str>, cursor: bool) -> Result<Source, String> {
    let settings = match window {
        Some(spec) => Data::new().str("capture_mode", "window").str("window", spec).int("priority", MATCH_TITLE_THEN_EXE),
        None => Data::new().str("capture_mode", "any_fullscreen"),
    };
    Source::new("game_capture", name, &settings.bool("capture_cursor", cursor).bool("anti_cheat_hook", true))
}

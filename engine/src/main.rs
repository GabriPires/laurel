//! laurel-engine — headless recording engine (PoC-E1, driven by the app since PoC-U1).
//!
//! Usage:
//!   laurel-engine <work-dir> [options]
//!
//! Options:
//!   --list                      list encoders, monitors, audio devices and game windows, then exit
//!   --capture monitor[:N[:auto|dxgi|wgc]] | monitor-id:<id> | window:<title:class:exe> | game[:<title:class:exe>] | none
//!   --base WxH                  canvas size (default 1920x1080)
//!   --out WxH                   encoded size (default = base)
//!   --fps N                     (default 60)
//!   --encoder auto|nvenc|amf|qsv|x264|ID  video encoder (default auto: the first that starts)
//!   --codec h264|hevc|av1       for auto (default h264); H.264 is the fallback
//!   --rate cbr|vbr              rate control (default cbr)
//!   --preset quality|balanced|performance  mapped to each encoder's presets (default balanced)
//!   --bitrate KBPS              (default 20000)
//!   --buffer SECS               replay buffer length (default 30)
//!   --mix game|game-mic|pc-mic  track 1: game only, game and microphone (default), or all
//!                               PC audio and microphone; track 2 is the game, track 3 the mic
//!   --game-audio SPEC           tests: game audio from this window spec from the start
//!   --no-desktop-audio          no game or PC audio
//!   --no-mic, --no-cursor
//!   --no-output                 capture only: no audio, encoders or replay buffer
//!   --paused                    build the replay buffer but do not start it (the app
//!                               starts it with setRecording when it finds a game)
//!   --dpi-unaware               skip per-monitor DPI awareness (for comparison tests)
//!   --rpc                       serve the app over JSON-RPC on stdin/stdout (see rpc.rs)
//!   --clips-dir DIR, --screenshots-dir DIR  (default <work-dir>\clips and \screenshots)
//!   --script "cmd; cmd; ..."    commands: wait N | save | screenshot | stats | beep N |
//!                               capture <monitor[:N[:method]]|window:..|game[:..]> |
//!                               mix <game|game-mic|pc-mic> | quit
//!                               (without --script or --rpc, commands are read from stdin)
//! Logs (libobs and the engine's own) always go to stderr.
//!
//! libobs and its plugins come from Laurel's own OBS runtime, staged by
//! tools/obs-runtime.py: the engine runs from `<runtime>\bin\64bit`, next to obs.dll.
//! Module config, clips and screenshots go under `<work-dir>`.

/// Writes a line to stderr and ignores failures: when the app dies, stderr is a
/// broken pipe, and a panicking `eprintln!` inside a libobs callback would abort the
/// engine instead of letting it shut down on stdin EOF.
macro_rules! log {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

mod obs;
mod recorder;
mod rpc;

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use obs::{Data, EncoderType, Obs, VideoConfig};
use recorder::{AudioMix, Capture, GameInfo, Recorder, RecorderConfig, VideoChoice};
use serde_json::{json, Value};

/// Plugins required by the MVP specs. Frontend-only plugins of an OBS install must
/// not be loaded by a headless host (see wiki/projeto/fase-3-pocs.md). The runtime
/// ships only these (MODULES in tools/obs-runtime.py).
const MODULES: &[&str] = &["win-capture", "win-wasapi", "obs-ffmpeg", "obs-x264", "obs-nvenc", "obs-qsv11"];

struct Args {
    obs_root: PathBuf,
    work_dir: PathBuf,
    list: bool,
    capture: Capture,
    capture_spec: String,
    base: (u32, u32),
    out: Option<(u32, u32)>,
    fps: u32,
    encoder: String,
    codec: String,
    rate: String,
    preset: String,
    bitrate: i64,
    buffer: i64,
    desktop_audio: bool,
    mix: AudioMix,
    game_audio: Option<String>,
    mic: bool,
    cursor: bool,
    record: bool,
    paused: bool,
    dpi_aware: bool,
    rpc: bool,
    clips_dir: Option<PathBuf>,
    screenshots_dir: Option<PathBuf>,
    script: Option<String>,
}

#[link(name = "user32", kind = "raw-dylib")]
extern "system" {
    fn SetProcessDpiAwarenessContext(value: isize) -> i32;
}

/// DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2; desktop duplication (DuplicateOutput1)
/// fails with DXGI_ERROR_UNSUPPORTED in DPI-unaware processes.
const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;

#[repr(C)]
#[derive(Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
}

#[link(name = "kernel32", kind = "raw-dylib")]
extern "system" {
    fn Beep(freq: u32, duration_ms: u32) -> i32;
    fn GetLocalTime(time: *mut SystemTime);
    fn GetModuleHandleW(name: *const u16) -> isize;
    fn GetModuleFileNameW(module: isize, file: *mut u16, size: u32) -> u32;
}

/// Local time as `YYYY-MM-DD hh-mm-ss`, the same shape as the replay file names.
fn local_timestamp() -> String {
    let mut t = SystemTime::default();
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02} {:02}-{:02}-{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// Local time as `hh:mm:ss.mmm`, the app log's clock, to line script events up with
/// PresentMon's `--date_time` frames.
fn clock() -> String {
    let mut t = SystemTime::default();
    unsafe { GetLocalTime(&mut t) };
    format!("{:02}:{:02}:{:02}.{:03}", t.hour, t.minute, t.second, t.millis)
}

/// `dir/Laurel <timestamp>.png`, with a counter if that name is taken.
fn screenshot_path(dir: &Path) -> PathBuf {
    let stem = format!("Laurel {}", local_timestamp());
    let mut path = dir.join(format!("{stem}.png"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).png"));
        n += 1;
    }
    path
}

/// Thumbnails are 384 pixels wide with the output aspect ratio (even height).
fn thumbnail_size((w, h): (u32, u32)) -> (u32, u32) {
    (384, ((384 * h / w.max(1)) & !1).max(2))
}

/// The OBS runtime folder: the engine lives in its `bin\64bit` (tools/obs-runtime.py).
fn runtime_root() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let root = exe.ancestors().nth(3).ok_or("engine path too short")?.to_path_buf();
    if root.join("obs-plugins").join("64bit").is_dir() {
        Ok(root)
    } else {
        Err(format!("no OBS runtime around {}: the engine must run from <runtime>\\bin\\64bit (tools/obs-runtime.py)", exe.display()))
    }
}

/// Full path of a module loaded in this process, to log which obs.dll is in use.
fn module_path(name: &str) -> Option<PathBuf> {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let handle = unsafe { GetModuleHandleW(wide.as_ptr()) };
    if handle == 0 {
        return None;
    }
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(handle, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

/// win-capture refreshes its game compatibility list from obsproject.com through
/// libcurl when it loads. Laurel makes no network requests (local-first, no telemetry),
/// so libcurl in this process gets a proxy with an unsupported scheme: the request
/// fails before any connection, and win-capture keeps the list shipped in the runtime.
/// Windows variable names ignore case, so this also covers HTTPS_PROXY and NO_PROXY.
fn block_libcurl_requests() {
    std::env::remove_var("no_proxy");
    std::env::set_var("https_proxy", "offline://laurel");
}

/// Audible phase marker for in-game tests (the player cannot see the console).
fn beep(count: u32) {
    for _ in 0..count {
        unsafe { Beep(988, 300) };
        std::thread::sleep(Duration::from_millis(120));
    }
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once('x').ok_or_else(|| format!("bad size {s}, expected WxH"))?;
    Ok((w.parse().map_err(|_| format!("bad width in {s}"))?, h.parse().map_err(|_| format!("bad height in {s}"))?))
}

fn parse_capture(s: &str) -> Result<Capture, String> {
    match s.split_once(':') {
        None if s == "monitor" => Ok(Capture::Monitor { index: 0, method: 0 }),
        None if s == "game" => Ok(Capture::Game(None)),
        None if s == "none" => Ok(Capture::None),
        Some(("monitor", rest)) => {
            let (n, method) = rest.split_once(':').unwrap_or((rest, "auto"));
            let method = match method {
                "auto" => 0,
                "dxgi" => 1,
                "wgc" => 2,
                _ => return Err(format!("bad monitor capture method {method}")),
            };
            Ok(Capture::Monitor { index: n.parse().map_err(|_| format!("bad monitor index {n}"))?, method })
        }
        Some(("monitor-id", id)) => Ok(Capture::MonitorId(id.to_string())),
        Some(("window", spec)) => Ok(Capture::Window(spec.to_string())),
        Some(("game", spec)) => Ok(Capture::Game(Some(spec.to_string()))),
        _ => Err(format!("bad capture {s}")),
    }
}

/// Paths from the command line are made absolute before the engine changes its
/// working directory.
fn absolute(path: String) -> Result<PathBuf, String> {
    std::path::absolute(&path).map_err(|e| format!("{path}: {e}"))
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let work_dir = absolute(it.next().ok_or("missing <work-dir>")?)?;
    let mut a = Args {
        obs_root: runtime_root()?,
        work_dir,
        list: false,
        capture: Capture::Monitor { index: 0, method: 0 },
        capture_spec: "monitor".into(),
        base: (1920, 1080),
        out: None,
        fps: 60,
        encoder: "auto".into(),
        codec: "h264".into(),
        rate: "cbr".into(),
        preset: "balanced".into(),
        bitrate: 20000,
        buffer: 30,
        desktop_audio: true,
        mix: AudioMix::GameMic,
        game_audio: None,
        mic: true,
        cursor: true,
        record: true,
        paused: false,
        dpi_aware: true,
        rpc: false,
        clips_dir: None,
        screenshots_dir: None,
        script: None,
    };
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--list" => a.list = true,
            "--capture" => {
                a.capture_spec = value()?;
                a.capture = parse_capture(&a.capture_spec)?;
            }
            "--base" => a.base = parse_size(&value()?)?,
            "--out" => a.out = Some(parse_size(&value()?)?),
            "--fps" => a.fps = value()?.parse().map_err(|_| "bad --fps")?,
            "--encoder" => a.encoder = value()?,
            "--codec" => a.codec = one_of(value()?, &["h264", "hevc", "av1"], "--codec")?,
            "--rate" => a.rate = one_of(value()?, &["cbr", "vbr"], "--rate")?,
            "--preset" => a.preset = one_of(value()?, &["quality", "balanced", "performance"], "--preset")?,
            "--bitrate" => a.bitrate = value()?.parse().map_err(|_| "bad --bitrate")?,
            "--buffer" => a.buffer = value()?.parse().map_err(|_| "bad --buffer")?,
            "--no-desktop-audio" => a.desktop_audio = false,
            "--mix" => {
                let mix = value()?;
                a.mix = AudioMix::parse(&mix).ok_or_else(|| format!("bad --mix {mix}, expected game, game-mic or pc-mic"))?;
            }
            "--game-audio" => a.game_audio = Some(value()?),
            "--no-mic" => a.mic = false,
            "--no-cursor" => a.cursor = false,
            "--no-output" => a.record = false,
            "--paused" => a.paused = true,
            "--dpi-unaware" => a.dpi_aware = false,
            "--rpc" => a.rpc = true,
            "--clips-dir" => a.clips_dir = Some(absolute(value()?)?),
            "--screenshots-dir" => a.screenshots_dir = Some(absolute(value()?)?),
            "--script" => a.script = Some(value()?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(a)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            log!("[laurel] {e}");
            std::process::exit(2);
        }
    };
    // win-capture copies its hook to ProgramData from ..\..\data\obs-plugins\win-capture,
    // relative to the working directory, so it must be bin\64bit.
    if let Err(e) = std::env::set_current_dir(args.obs_root.join("bin").join("64bit")) {
        log!("[laurel] could not enter the runtime folder: {e}");
        std::process::exit(2);
    }
    block_libcurl_requests();
    if args.dpi_aware && unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } == 0 {
        log!("[laurel] could not set per-monitor DPI awareness");
    }
    let rpc = args.rpc;
    if let Err(e) = run(args) {
        log!("[laurel] error: {e}");
        if rpc {
            rpc::notify("fatal", json!({ "message": e }));
        }
        std::process::exit(1);
    }
}

fn one_of(value: String, allowed: &[&str], flag: &str) -> Result<String, String> {
    if allowed.contains(&value.as_str()) { Ok(value) } else { Err(format!("bad {flag} {value}, expected {}", allowed.join(" or "))) }
}

/// Video encoders in the order auto tries them (spec quality.encoding): the texture
/// encoders of NVIDIA, AMD and Intel GPUs, which take the frames on the GPU, then
/// NVENC's fallback that copies them, then the CPU. HEVC and AV1 come before H.264,
/// which is the fallback for every codec.
const H264_ENCODERS: &[&str] = &["obs_nvenc_h264_tex", "h264_texture_amf", "obs_qsv11_v2", "obs_nvenc_h264_soft", "obs_x264"];
const HEVC_ENCODERS: &[&str] = &["obs_nvenc_hevc_tex", "h265_texture_amf", "obs_qsv11_hevc", "obs_nvenc_hevc_soft"];
const AV1_ENCODERS: &[&str] = &["obs_nvenc_av1_tex", "av1_texture_amf", "obs_qsv11_av1", "obs_nvenc_av1_soft"];

/// The encoder of a family (the choice the app offers) for a codec.
fn family_encoder(family: &str, codec: &str) -> Option<&'static str> {
    Some(match (family, codec) {
        ("nvenc", "hevc") => "obs_nvenc_hevc_tex",
        ("nvenc", "av1") => "obs_nvenc_av1_tex",
        ("nvenc", _) => "obs_nvenc_h264_tex",
        ("amf", "hevc") => "h265_texture_amf",
        ("amf", "av1") => "av1_texture_amf",
        ("amf", _) => "h264_texture_amf",
        ("qsv", "hevc") => "obs_qsv11_hevc",
        ("qsv", "av1") => "obs_qsv11_av1",
        ("qsv", _) => "obs_qsv11_v2",
        ("x264", _) => "obs_x264",
        _ => return None,
    })
}

/// The family of an encoder id, for the app.
fn encoder_family(id: &str) -> &'static str {
    if id.starts_with("obs_nvenc") {
        "nvenc"
    } else if id.ends_with("_amf") {
        "amf"
    } else if id.starts_with("obs_qsv") {
        "qsv"
    } else {
        "x264"
    }
}

/// The encoders to try: the one asked for (an id, or a family with the codec),
/// then the codec's list, then H.264's, keeping only those OBS registered on this
/// PC (it probes the GPUs at load).
fn encoder_candidates(requested: &str, codec: &str, available: &[EncoderType]) -> Vec<String> {
    let mut order: Vec<&str> = Vec::new();
    match family_encoder(requested, codec) {
        Some(id) => order.push(id),
        None if requested != "auto" => order.push(requested),
        None => {}
    }
    match codec {
        "hevc" => order.extend(HEVC_ENCODERS),
        "av1" => order.extend(AV1_ENCODERS),
        _ => {}
    }
    order.extend(H264_ENCODERS);
    let mut out: Vec<String> = Vec::new();
    for id in order {
        if available.iter().any(|e| e.video && e.id == id) && !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    }
    out
}

/// The encoders the app can offer, in the order auto tries them.
fn video_encoder_list(available: &[EncoderType]) -> Value {
    let known = H264_ENCODERS.iter().chain(HEVC_ENCODERS).chain(AV1_ENCODERS);
    Value::Array(known.filter_map(|id| available.iter().find(|e| e.video && e.id == *id)).map(|e| json!({ "id": e.id, "family": encoder_family(&e.id), "codec": e.codec, "name": e.name })).collect())
}

/// Preset levels on each encoder's own list: NVENC, x264, AMF and QSV values.
const PRESETS: [(&str, &[&str]); 3] = [
    ("quality", &["p6", "faster", "quality", "TU2"]),
    ("balanced", &["p5", "veryfast", "balanced", "TU4"]),
    ("performance", &["p3", "superfast", "speed", "TU6"]),
];

/// Settings for a video encoder: the bitrate and the keyframe every second that all
/// of them take, the rate control and the preset level as the encoder names them.
fn video_settings(obs: &Obs, id: &str, args: &Args) -> Data {
    let props = obs.encoder_list_properties(id);
    let mut data = Data::new().int("bitrate", args.bitrate).int("keyint_sec", recorder::KEYINT_SECS);
    let rate = props.iter().find(|p| p.name == "rate_control").and_then(|p| p.items.iter().find(|(_, v)| v.eq_ignore_ascii_case(&args.rate))).map(|(_, v)| v.clone());
    data = data.str("rate_control", &rate.unwrap_or_else(|| args.rate.to_uppercase()));
    let wanted = PRESETS.iter().find(|(level, _)| *level == args.preset).map_or(&[][..], |(_, values)| *values);
    for p in props.iter().filter(|p| matches!(p.name.as_str(), "preset" | "preset2" | "target_usage")) {
        if let Some(value) = wanted.iter().find(|w| p.items.iter().any(|(_, v)| v.eq_ignore_ascii_case(w))) {
            data = data.str(&p.name, value);
        }
    }
    data
}

fn fallbacks_json(rec: &Recorder) -> Value {
    Value::Array(rec.encoder_fallbacks().iter().map(|(id, error)| json!({ "encoder": id, "error": error })).collect())
}

fn run(args: Args) -> Result<(), String> {
    let config_dir = args.work_dir.join("obs-config");
    let clips_dir = args.clips_dir.clone().unwrap_or_else(|| args.work_dir.join("clips"));
    let shots_dir = args.screenshots_dir.clone().unwrap_or_else(|| args.work_dir.join("screenshots"));
    let thumbs_dir = args.work_dir.join("thumbnails");
    for d in [&config_dir, &clips_dir, &shots_dir, &thumbs_dir] {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }

    obs::log_to_stderr();
    let obs = Obs::startup("en-US", &config_dir)?;
    let obs_dll = module_path("obs.dll").map(|p| p.display().to_string()).unwrap_or_default();
    log!("[laurel] libobs {} ({obs_dll})", obs.version());
    obs.add_data_path(&args.obs_root.join("data/libobs"));
    // Encoders need even dimensions (spec quality.encoding).
    let out = args.out.unwrap_or(args.base);
    let out = (out.0.max(2) & !1, out.1.max(2) & !1);
    obs.reset_video(VideoConfig { base_width: args.base.0, base_height: args.base.1, output_width: out.0, output_height: out.1, fps: args.fps })?;
    obs.reset_audio(48000)?;
    for m in MODULES {
        if let Err(e) = obs.load_module(&args.obs_root, m) {
            log!("[laurel] module {m} not loaded: {e}");
        }
    }
    obs.post_load_modules();

    let encoders = obs.encoder_types();
    if args.list {
        for e in encoders.iter().filter(|e| e.video) {
            log!("[laurel] video encoder {} ({}, {})", e.id, e.codec, e.name);
            for p in obs.encoder_list_properties(&e.id) {
                let items: Vec<String> = p.items.iter().map(|(label, value)| format!("{value}={label}")).collect();
                log!("[laurel]   {} (default {}): {}", p.name, p.default, items.join(" | "));
            }
        }
        for (label, id) in obs.source_list_property("monitor_capture", "monitor_id") {
            log!("[laurel] monitor {label} = {id}");
        }
        for (label, value) in obs.source_list_property("monitor_capture", "method") {
            log!("[laurel] monitor capture method {label} = {value}");
        }
        for (label, id) in obs.source_list_property("wasapi_output_capture", "device_id") {
            log!("[laurel] output device {label} = {id}");
        }
        for (label, id) in obs.source_list_property("wasapi_input_capture", "device_id") {
            log!("[laurel] input device {label} = {id}");
        }
        for (label, spec) in obs.source_list_property("game_capture", "window") {
            log!("[laurel] game window {label} = {spec}");
        }
        for (label, value) in obs.source_list_property("wasapi_process_output_capture", "priority") {
            log!("[laurel] app audio match {label} = {value}");
        }
        for (label, spec) in obs.source_list_property("wasapi_process_output_capture", "window") {
            log!("[laurel] app audio window {label} = {spec}");
        }
        return Ok(());
    }

    let choices = encoder_candidates(&args.encoder, &args.codec, &encoders);
    log!("[laurel] video encoders to try: {}", choices.join(", "));
    let video_choices = choices.iter().map(|id| VideoChoice { id: id.clone(), settings: video_settings(&obs, id, &args) }).collect();
    let cfg = RecorderConfig {
        capture: args.capture.clone(),
        cursor: args.cursor,
        bitrate_kbps: args.bitrate,
        buffer_secs: args.buffer,
        desktop_audio: args.desktop_audio,
        mix: args.mix,
        game_audio: args.game_audio.clone(),
        mic: args.mic,
        clips_dir: clips_dir.clone(),
        output_size: out,
        canvas: args.base,
        record: args.record,
        start_paused: args.paused,
    };
    let started = Instant::now();
    let mut rec = Recorder::start(&obs, &cfg, video_choices)?;
    log!(
        "[laurel] recording: capture={:?} base={}x{} out={}x{} fps={} encoder={} rate={} preset={} bitrate={}kbps buffer={}s game_audio={} mic={} mix={} record={} paused={}",
        cfg.capture, args.base.0, args.base.1, out.0, out.1, args.fps, rec.encoder(), args.rate, args.preset, args.bitrate, args.buffer, args.desktop_audio, args.mic, rec.audio_mix().name(), args.record, args.paused
    );

    if args.rpc {
        let dirs = Dirs { clips: clips_dir, screenshots: shots_dir, thumbnails: thumbs_dir };
        rpc::notify(
            "ready",
            json!({
                "version": obs.version(),
                "encoder": rec.encoder(),
                "encoderRequested": args.encoder,
                "codec": args.codec,
                "encoderFallbacks": fallbacks_json(&rec),
                "encoders": video_encoder_list(&encoders),
                "target": args.capture_spec,
                "output": [out.0, out.1],
                "base": [args.base.0, args.base.1],
                "fps": args.fps,
                "bufferSecs": args.buffer,
                "audioMix": rec.audio_mix().name(),
                "recording": rec.active(),
                "clipsDir": dirs.clips,
                "screenshotsDir": dirs.screenshots,
            }),
        );
        serve_app(&obs, &mut rec, &args, &dirs);
        drop(rec);
        log!("[laurel] shutdown");
        return Ok(());
    }

    let commands: Box<dyn Iterator<Item = String>> = match &args.script {
        Some(s) => Box::new(s.split(';').map(|c| c.trim().to_string()).collect::<Vec<_>>().into_iter()),
        None => Box::new(std::io::stdin().lock().lines().map_while(Result::ok).collect::<Vec<_>>().into_iter()),
    };
    for cmd in commands {
        let mut parts = cmd.split_whitespace();
        match parts.next() {
            None => {}
            Some("wait") => {
                let secs: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
                std::thread::sleep(Duration::from_secs_f64(secs));
            }
            Some("save") => {
                log!("[laurel] save at {}", clock());
                let t = Instant::now();
                match rec.save_clip(Duration::from_secs(30)) {
                    Ok(p) => log!("[laurel] clip saved in {} ms: {}", t.elapsed().as_millis(), p.display()),
                    Err(e) => log!("[laurel] clip failed: {e}"),
                }
            }
            Some("screenshot") => {
                log!("[laurel] screenshot at {}", clock());
                let path = screenshot_path(&shots_dir);
                let t = Instant::now();
                match rec.screenshot(&path, rec.output_size(), Duration::from_secs(5)) {
                    Ok((w, h)) => log!("[laurel] screenshot {w}x{h} in {} ms: {}", t.elapsed().as_millis(), path.display()),
                    Err(e) => log!("[laurel] screenshot failed: {e}"),
                }
            }
            Some("beep") => beep(parts.next().and_then(|s| s.parse().ok()).unwrap_or(1)),
            Some("capture") => {
                let spec = cmd.trim_start()["capture".len()..].trim();
                match parse_capture(spec).and_then(|c| rec.switch_capture(&obs, &c).map(|_| c)) {
                    Ok(c) => log!("[laurel] capture switched to {c:?} at {:.1}s", started.elapsed().as_secs_f64()),
                    Err(e) => log!("[laurel] capture switch failed: {e}"),
                }
            }
            Some("mix") => match AudioMix::parse(parts.next().unwrap_or_default()).ok_or("bad mix".to_string()).and_then(|m| rec.set_mix(m)) {
                Ok(()) => log!("[laurel] mix switched at {:.1}s", started.elapsed().as_secs_f64()),
                Err(e) => log!("[laurel] mix switch failed: {e}"),
            },
            Some("stats") => log!("[laurel] stats after {:.1}s: {}", started.elapsed().as_secs_f64(), recorder::format_stats(&obs, &rec)),
            Some("quit") => break,
            Some(other) => log!("[laurel] unknown command {other}"),
        }
    }
    drop(rec);
    log!("[laurel] shutdown");
    Ok(())
}

fn game_json(game: Option<GameInfo>) -> Value {
    game.map_or(Value::Null, |g| json!({ "title": g.title, "class": g.class, "exe": g.exe }))
}

struct Dirs {
    clips: PathBuf,
    screenshots: PathBuf,
    thumbnails: PathBuf,
}

/// Answers the app's requests (see rpc.rs) until it disconnects or asks to quit.
fn serve_app(obs: &Obs, rec: &mut Recorder, args: &Args, dirs: &Dirs) {
    let mut target = args.capture_spec.clone();
    rpc::serve(|method, params| {
        let result = match method {
            "status" => {
                let s = obs.stats();
                let (cw, ch) = rec.capture_size();
                Ok(json!({
                    "target": target,
                    "captureSize": [cw, ch],
                    "activeFps": s.active_fps,
                    "laggedFrames": s.lagged_frames,
                    "skippedFrames": s.skipped_frames,
                    "totalFrames": s.total_frames,
                    "outputDropped": rec.frames_dropped(),
                    "recording": rec.active(),
                    "encoder": rec.encoder(),
                    "cursor": rec.cursor(),
                    "game": game_json(rec.game()),
                    "audio": { "mode": if args.desktop_audio { "game" } else { "off" }, "mix": rec.audio_mix().name(), "follows": rec.game_audio() },
                }))
            }
            "listTargets" => {
                let mut targets = vec![json!({ "target": "game", "label": "Any fullscreen game" })];
                for (i, (label, id)) in obs.source_list_property("monitor_capture", "monitor_id").iter().enumerate() {
                    targets.push(json!({ "target": format!("monitor:{i}"), "label": format!("Monitor: {label}"), "id": id }));
                }
                // Ordinary windows use window capture: the game hook only works on
                // Direct3D/OpenGL/Vulkan swap chains and would inject its DLL into any
                // app (see wiki/projeto/poc-u1-resultados.md). Specific games are hooked
                // once game detection exists.
                for (label, spec) in obs.source_list_property("window_capture", "window") {
                    targets.push(json!({ "target": format!("window:{spec}"), "label": format!("Window: {label}") }));
                }
                Ok(Value::Array(targets))
            }
            "setCapture" => {
                let spec = params.get("target").and_then(Value::as_str).unwrap_or_default();
                parse_capture(spec).and_then(|c| rec.switch_capture(obs, &c)).map(|_| {
                    target = spec.to_string();
                    json!({ "target": target })
                })
            }
            "saveClip" => {
                let t = Instant::now();
                let game = rec.game();
                rec.save_clip(Duration::from_secs(30)).map(|path| {
                    let save_ms = t.elapsed().as_millis() as u64;
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    let thumb = dirs.thumbnails.join(format!("{stem}.png"));
                    let thumbnail = match rec.screenshot(&thumb, thumbnail_size(rec.output_size()), Duration::from_secs(5)) {
                        Ok(_) => Some(thumb),
                        Err(e) => {
                            log!("[laurel] thumbnail failed: {e}");
                            None
                        }
                    };
                    log!("[laurel] clip saved in {save_ms} ms: {}", path.display());
                    json!({ "path": path, "saveMs": save_ms, "thumbnail": thumbnail, "game": game_json(game) })
                })
            }
            "screenshot" => {
                let path = screenshot_path(&dirs.screenshots);
                let t = Instant::now();
                rec.screenshot(&path, rec.output_size(), Duration::from_secs(5))
                    .map(|(w, h)| json!({ "path": path, "size": [w, h], "ms": t.elapsed().as_millis() as u64, "game": game_json(rec.game()) }))
            }
            "listEncoders" => Ok(video_encoder_list(&obs.encoder_types())),
            "setCursor" => {
                // Spec capture.cursor: applies to the running capture and the next ones.
                let enabled = params.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                rec.set_cursor(enabled);
                Ok(json!({ "cursor": rec.cursor() }))
            }
            "setAudioMix" => {
                // Spec audio.separate-tracks: track 1 changes without a restart.
                let mix = params.get("mix").and_then(Value::as_str).unwrap_or_default();
                match AudioMix::parse(mix) {
                    Some(mix) => rec.set_mix(mix).map(|_| json!({ "mix": rec.audio_mix().name() })),
                    None => Err(format!("bad mix {mix}, expected game, game-mic or pc-mic")),
                }
            }
            "setRecording" => {
                // Pauses or resumes the replay buffer (low disk space, record.toggle).
                let enabled = params.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                let before = rec.encoder().to_string();
                let result = rec.set_recording(enabled).map(|_| json!({ "recording": rec.active(), "encoder": rec.encoder() }));
                if rec.encoder() != before {
                    // A fallback while resuming (spec quality.encoding): the app tells the player.
                    rpc::notify("encoder", json!({ "encoder": rec.encoder(), "encoderFallbacks": fallbacks_json(rec) }));
                }
                result
            }
            "shutdown" => return (Ok(Value::Null), rpc::Flow::Quit),
            other => Err(format!("method not found: {other}")),
        };
        (result, rpc::Flow::Continue)
    });
}

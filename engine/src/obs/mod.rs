//! Thin RAII wrappers over the libobs C API (see `ffi`).
//!
//! libobs objects are reference counted and not thread safe from Rust's point of
//! view; the wrappers are used from the engine's main thread only.

pub mod ffi;

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, SyncSender};
use std::sync::{Mutex, OnceLock};

pub fn cstr(s: &str) -> CString {
    CString::new(s).expect("string without NUL")
}

pub fn opt_string(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// Gets the name of each game capture source whose shared texture OBS could not
/// open (the game renders on another GPU; A13 of the stability tests).
static SHARED_TEXTURE_FAILURES: Mutex<Option<Sender<String>>> = Mutex::new(None);

/// Starts (or, with None, stops) sending the names of game capture sources that
/// log "failed to open shared handle" to `tx`.
pub fn watch_shared_texture_failures(tx: Option<Sender<String>>) {
    if let Ok(mut slot) = SHARED_TEXTURE_FAILURES.lock() {
        *slot = tx;
    }
}

/// The source in a game capture line such as
/// "[game-capture: 'capture'] init_shtex_capture: failed to open shared handle".
fn shared_texture_failure(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("[game-capture: '")?;
    let (name, message) = rest.split_once("'] ")?;
    message.starts_with("init_shtex_capture: failed to open shared handle").then_some(name)
}

/// What a WASAPI audio source logged about its device (A16 of the stability tests).
#[derive(Debug, PartialEq)]
pub enum AudioEvent {
    /// The source could not start its device, or tries it again; with the Windows
    /// error when OBS logged one (only the first failure in a row has it).
    Failed { source: String, code: Option<String> },
    /// The source started its device.
    Started { source: String },
    /// Not from the log: the app asked for another try of the microphone.
    Retry,
    /// Not from the log: whether the recorder records a game or the screen now; the
    /// microphone is open only then (A20 of the stability tests).
    Recording(bool),
    /// Not from the log: the player's microphone switch and device (spec audio.mic).
    Choice { enabled: bool, device: String },
    /// Not from the log: the audio tracks the microphone goes to (the mix).
    Mixers(u32),
    /// Not from the log: the player's noise suppression (spec audio.mic-processing).
    Noise(crate::recorder::MicNoise),
}

static AUDIO_EVENTS: Mutex<Option<Sender<AudioEvent>>> = Mutex::new(None);

/// A28 of the stability tests: why OBS's NVENC texture encoder could not take the
/// frames on the GPU, from obs-nvenc's own lines. The encoder then reroutes to the
/// NVENC encoder that copies each frame through system memory, which libobs does
/// not report (obs_encoder_get_id keeps the original id). On a laptop whose NVIDIA
/// GPU is not DXGI adapter 0 that is every recording: obs-nvenc's texture encoder
/// opens its own Direct3D 11 device on adapter 0.
static NVENC_TEXTURE_FAILURE: Mutex<Option<String>> = Mutex::new(None);

/// Takes (and clears) what obs-nvenc logged about its texture encoder failing.
pub fn take_nvenc_texture_failure() -> Option<String> {
    NVENC_TEXTURE_FAILURE.lock().ok().and_then(|mut slot| slot.take())
}

/// The reason in an obs-nvenc line about its texture encoder, if it is one: an
/// error ("init_session: ... failed: 1 (NV_ENC_ERR_NO_ENCODE_DEVICE)" gives the
/// error's name) or a fallback it chose ("..., falling back to non-texture encoder").
fn nvenc_texture_failure(level: i32, line: &str) -> Option<String> {
    let message = line.strip_prefix("[obs-nvenc] ")?;
    if message.contains("falling back to") {
        return Some(message.to_string());
    }
    if level != ffi::LOG_ERROR {
        return None;
    }
    let named = message.rsplit_once('(').and_then(|(_, rest)| rest.strip_suffix(')')).filter(|name| name.starts_with("NV_ENC_ERR_"));
    Some(named.unwrap_or(message).to_string())
}

thread_local! {
    /// The error of the last "[WASAPISource::TryInitialize]" line on this thread;
    /// the same thread then logs which source failed to start.
    static INIT_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Starts (or, with None, stops) sending audio device events to `tx`.
pub fn watch_audio_events(tx: Option<Sender<AudioEvent>>) {
    if let Ok(mut slot) = AUDIO_EVENTS.lock() {
        *slot = tx;
    }
}

/// The audio device event in a libobs line (win-wasapi), if any:
/// "[WASAPISource::TryInitialize]:[<device>] Failed to initialize audio client: 80070005",
/// "WASAPI: Device 'default' failed to start (source: microphone)",
/// "Device '<device>' invalidated.  Retrying (source: microphone)",
/// "WASAPI: Device '<device>' [48000 Hz] initialized (source: microphone)".
fn audio_event(line: &str) -> Option<AudioEvent> {
    if let Some(rest) = line.strip_prefix("[WASAPISource::TryInitialize]:[") {
        let code = rest.rsplit(": ").next().filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_hexdigit()));
        INIT_ERROR.with(|e| *e.borrow_mut() = code.map(str::to_ascii_uppercase));
        return None;
    }
    let source = line.strip_suffix(')')?.rsplit_once("(source: ")?.1.to_string();
    if line.starts_with("WASAPI: Device '") && line.contains("' failed to start (source: ") {
        Some(AudioEvent::Failed { source, code: INIT_ERROR.with(|e| e.borrow_mut().take()) })
    } else if line.starts_with("Device '") && line.contains("' invalidated.  Retrying (source: ") {
        Some(AudioEvent::Failed { source, code: None })
    } else if line.starts_with("WASAPI: Device '") && line.contains("] initialized (source: ") {
        Some(AudioEvent::Started { source })
    } else {
        None
    }
}

/// Sends libobs log messages to stderr (the default handler prints to stdout, which
/// the app protocol needs). Call before `Obs::startup`.
pub fn log_to_stderr() {
    unsafe { ffi::base_set_log_handler(on_log, std::ptr::null_mut()) }
}

unsafe extern "C" fn on_log(level: i32, msg: *const c_char, args: *mut c_void, _param: *mut c_void) {
    let mut text = ffi::dstr { array: std::ptr::null_mut(), len: 0, capacity: 0 };
    ffi::dstr_vprintf(&mut text, msg, args);
    let line = opt_string(text.array).unwrap_or_default();
    ffi::bfree(text.array.cast());
    let prefix = match level {
        ffi::LOG_ERROR => "error",
        ffi::LOG_WARNING => "warning",
        ffi::LOG_INFO => "info",
        _ => "debug",
    };
    log!("{prefix}: {line}");
    if let Some(reason) = nvenc_texture_failure(level, &line) {
        if let Ok(mut slot) = NVENC_TEXTURE_FAILURE.lock() {
            *slot = Some(reason);
        }
    }
    if let Some(event) = audio_event(&line) {
        if let Some(tx) = AUDIO_EVENTS.lock().ok().as_ref().and_then(|slot| slot.as_ref()) {
            let _ = tx.send(event);
        }
    }
    // A24: libobs logs this after a Direct3D 11 call fails on a lost device.
    if line.contains("Device Removed Reason") {
        crate::health::wake();
    }
    if let Some(name) = shared_texture_failure(&line) {
        if let Some(tx) = SHARED_TEXTURE_FAILURES.lock().ok().as_ref().and_then(|slot| slot.as_ref()) {
            let _ = tx.send(name.to_string());
        }
    }
}

/// A18: libobs hands its "UI" tasks to the host. The only one Laurel's plugins queue
/// is win-wasapi's reset of the audio monitoring when the default output device
/// changes, once per device role; Laurel monitors no audio, so the task does
/// nothing, but without a handler libobs logs an error for each one. The engine runs
/// them in order on a thread of its own, as OBS's frontend does on its UI thread.
struct UiTask {
    task: unsafe extern "C" fn(param: *mut c_void),
    param: usize,
    done: Option<SyncSender<()>>,
}

static UI_TASKS: OnceLock<Sender<UiTask>> = OnceLock::new();
static UI_TASKS_RUN: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static ON_UI_TASK_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// libobs UI tasks run since the engine started (one is libobs marking the thread).
pub fn ui_tasks_run() -> u64 {
    UI_TASKS_RUN.load(Ordering::Relaxed)
}

unsafe extern "C" fn on_ui_task(task: ffi::obs_task_t, param: *mut c_void, wait: bool) {
    let Some(task) = task else {
        return;
    };
    // A task that queues another one with `wait` from the task thread runs it at once.
    if ON_UI_TASK_THREAD.with(Cell::get) {
        unsafe { task(param) };
        UI_TASKS_RUN.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let (done, finished) = if wait {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let queued = UI_TASKS.get().is_some_and(|tx| tx.send(UiTask { task, param: param as usize, done }).is_ok());
    if !queued {
        log!("[laurel] libobs UI task not run: no task thread");
    } else if let Some(finished) = finished {
        let _ = finished.recv();
    }
}

/// Owns the libobs core; dropping it shuts libobs down.
pub struct Obs;

#[derive(Clone, Copy, Debug)]
pub struct VideoConfig {
    pub base_width: u32,
    pub base_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub fps: u32,
    /// DXGI adapter index (see gpu.rs).
    pub adapter: u32,
}

#[derive(Debug)]
pub struct EncoderType {
    pub id: String,
    pub video: bool,
    pub codec: String,
    pub name: String,
}

#[derive(Debug)]
pub struct CoreStats {
    pub active_fps: f64,
    pub average_frame_time_ms: f64,
    pub lagged_frames: u32,
    pub skipped_frames: u32,
    pub total_frames: u32,
}

impl Obs {
    pub fn startup(locale: &str, module_config_dir: &Path) -> Result<Obs, String> {
        let locale = cstr(locale);
        let config = cstr(&module_config_dir.display().to_string());
        if unsafe { ffi::obs_startup(locale.as_ptr(), config.as_ptr(), std::ptr::null_mut()) } {
            Obs::run_ui_tasks();
            Ok(Obs)
        } else {
            Err("obs_startup failed".into())
        }
    }

    pub fn version(&self) -> String {
        opt_string(unsafe { ffi::obs_get_version_string() }).unwrap_or_default()
    }

    pub fn add_data_path(&self, path: &Path) {
        // libobs expects a trailing separator.
        let p = cstr(&format!("{}/", path.display()));
        unsafe { ffi::obs_add_data_path(p.as_ptr()) }
    }

    pub fn reset_video(&self, cfg: VideoConfig) -> Result<(), String> {
        let module = cstr("libobs-d3d11");
        let mut ovi = ffi::obs_video_info {
            graphics_module: module.as_ptr(),
            fps_num: cfg.fps,
            fps_den: 1,
            base_width: cfg.base_width,
            base_height: cfg.base_height,
            output_width: cfg.output_width,
            output_height: cfg.output_height,
            output_format: ffi::VIDEO_FORMAT_NV12,
            adapter: cfg.adapter,
            gpu_conversion: true,
            colorspace: ffi::VIDEO_CS_709,
            range: ffi::VIDEO_RANGE_PARTIAL,
            scale_type: ffi::OBS_SCALE_BICUBIC,
        };
        match unsafe { ffi::obs_reset_video(&mut ovi) } {
            ffi::OBS_VIDEO_SUCCESS => Ok(()),
            rc => Err(format!("obs_reset_video failed with {rc}")),
        }
    }

    pub fn reset_audio(&self, samples_per_sec: u32) -> Result<(), String> {
        let oai = ffi::obs_audio_info { samples_per_sec, speakers: ffi::SPEAKERS_STEREO };
        if unsafe { ffi::obs_reset_audio(&oai) } {
            Ok(())
        } else {
            Err("obs_reset_audio failed".into())
        }
    }

    /// Loads one plugin from `<obs_root>/obs-plugins/64bit/<name>.dll`.
    pub fn load_module(&self, obs_root: &Path, name: &str) -> Result<(), String> {
        let bin = cstr(&obs_root.join(format!("obs-plugins/64bit/{name}.dll")).display().to_string());
        let data = cstr(&obs_root.join(format!("data/obs-plugins/{name}")).display().to_string());
        let mut module: *mut ffi::obs_module_t = std::ptr::null_mut();
        let rc = unsafe { ffi::obs_open_module(&mut module, bin.as_ptr(), data.as_ptr()) };
        if rc != ffi::MODULE_SUCCESS {
            return Err(format!("obs_open_module({name}) returned {rc}"));
        }
        if unsafe { ffi::obs_init_module(module) } {
            Ok(())
        } else {
            Err(format!("obs_init_module({name}) failed"))
        }
    }

    pub fn post_load_modules(&self) {
        unsafe { ffi::obs_post_load_modules() }
    }

    pub fn encoder_types(&self) -> Vec<EncoderType> {
        let mut out = Vec::new();
        let mut idx = 0;
        loop {
            let mut id: *const c_char = std::ptr::null();
            if !unsafe { ffi::obs_enum_encoder_types(idx, &mut id) } {
                break;
            }
            idx += 1;
            out.push(EncoderType {
                id: opt_string(id).unwrap_or_default(),
                video: unsafe { ffi::obs_get_encoder_type(id) } == ffi::OBS_ENCODER_VIDEO,
                codec: opt_string(unsafe { ffi::obs_get_encoder_codec(id) }).unwrap_or_default(),
                name: opt_string(unsafe { ffi::obs_encoder_get_display_name(id) }).unwrap_or_default(),
            });
        }
        out
    }

    /// Lists the (label, value) items of a list property of a source type, e.g.
    /// monitors of `monitor_capture` or devices of `wasapi_input_capture`.
    /// Integer lists report their values as decimal strings.
    pub fn source_list_property(&self, source_id: &str, property: &str) -> Vec<(String, String)> {
        list_source_property(source_id, property)
    }

    /// The list properties of an encoder type (rate control, presets, profiles…)
    /// with their items, and the encoder's default value for each.
    pub fn encoder_list_properties(&self, encoder_id: &str) -> Vec<ListProperty> {
        let id = cstr(encoder_id);
        let mut out = Vec::new();
        unsafe {
            let props = ffi::obs_get_encoder_properties(id.as_ptr());
            if props.is_null() {
                return out;
            }
            let defaults = ffi::obs_encoder_defaults(id.as_ptr());
            let mut prop = ffi::obs_properties_first(props);
            while !prop.is_null() {
                if ffi::obs_property_get_type(prop) == ffi::OBS_PROPERTY_LIST {
                    let name = opt_string(ffi::obs_property_name(prop)).unwrap_or_default();
                    let key = cstr(&name);
                    let default = if defaults.is_null() {
                        String::new()
                    } else if ffi::obs_property_list_format(prop) == ffi::OBS_COMBO_FORMAT_INT {
                        ffi::obs_data_get_int(defaults, key.as_ptr()).to_string()
                    } else {
                        opt_string(ffi::obs_data_get_string(defaults, key.as_ptr())).unwrap_or_default()
                    };
                    out.push(ListProperty { name, default, items: list_items(prop) });
                }
                if !ffi::obs_property_next(&mut prop) {
                    break;
                }
            }
            if !defaults.is_null() {
                ffi::obs_data_release(defaults);
            }
            ffi::obs_properties_destroy(props);
        }
        out
    }

    pub fn stats(&self) -> CoreStats {
        unsafe {
            let video = ffi::obs_get_video();
            CoreStats {
                active_fps: ffi::obs_get_active_fps(),
                average_frame_time_ms: ffi::obs_get_average_frame_time_ns() as f64 / 1e6,
                lagged_frames: ffi::obs_get_lagged_frames(),
                skipped_frames: ffi::video_output_get_skipped_frames(video),
                total_frames: ffi::video_output_get_total_frames(video),
            }
        }
    }
}

/// A list property: its name, the default value and the (label, value) items.
pub struct ListProperty {
    pub name: String,
    pub default: String,
    pub items: Vec<(String, String)>,
}

/// The (label, value) items of a list property; integer lists give decimal strings.
unsafe fn list_items(prop: *mut ffi::obs_property_t) -> Vec<(String, String)> {
    let int = ffi::obs_property_list_format(prop) == ffi::OBS_COMBO_FORMAT_INT;
    (0..ffi::obs_property_list_item_count(prop))
        .map(|i| {
            let value = if int { ffi::obs_property_list_item_int(prop, i).to_string() } else { opt_string(ffi::obs_property_list_item_string(prop, i)).unwrap_or_default() };
            (opt_string(ffi::obs_property_list_item_name(prop, i)).unwrap_or_default(), value)
        })
        .collect()
}

impl Obs {
    /// Starts the UI task thread (A18) and hands it to libobs; after obs_startup.
    fn run_ui_tasks() {
        let (tx, rx) = std::sync::mpsc::channel::<UiTask>();
        if UI_TASKS.set(tx).is_err() {
            return;
        }
        let started = std::thread::Builder::new().name("laurel: libobs UI tasks".into()).spawn(move || {
            ON_UI_TASK_THREAD.with(|on| on.set(true));
            for job in rx {
                unsafe { (job.task)(job.param as *mut c_void) };
                UI_TASKS_RUN.fetch_add(1, Ordering::Relaxed);
                if let Some(done) = job.done {
                    let _ = done.send(());
                }
            }
        });
        match started {
            Ok(_) => unsafe { ffi::obs_set_ui_task_handler(on_ui_task) },
            Err(e) => log!("[laurel] no thread for libobs UI tasks: {e}"),
        }
    }
}

impl Drop for Obs {
    fn drop(&mut self) {
        unsafe { ffi::obs_shutdown() }
    }
}

/// Owned `obs_data_t` settings object.
pub struct Data(*mut ffi::obs_data_t);

impl Data {
    pub fn new() -> Data {
        Data(unsafe { ffi::obs_data_create() })
    }

    pub fn str(self, key: &str, value: &str) -> Data {
        let (k, v) = (cstr(key), cstr(value));
        unsafe { ffi::obs_data_set_string(self.0, k.as_ptr(), v.as_ptr()) };
        self
    }

    pub fn int(self, key: &str, value: i64) -> Data {
        let k = cstr(key);
        unsafe { ffi::obs_data_set_int(self.0, k.as_ptr(), value) };
        self
    }

    pub fn bool(self, key: &str, value: bool) -> Data {
        let k = cstr(key);
        unsafe { ffi::obs_data_set_bool(self.0, k.as_ptr(), value) };
        self
    }

    pub fn ptr(&self) -> *mut ffi::obs_data_t {
        self.0
    }
}

impl Drop for Data {
    fn drop(&mut self) {
        unsafe { ffi::obs_data_release(self.0) }
    }
}

pub struct Source(*mut ffi::obs_source_t);

impl Source {
    pub fn new(id: &str, name: &str, settings: &Data) -> Result<Source, String> {
        let (id_c, name_c) = (cstr(id), cstr(name));
        let p = unsafe { ffi::obs_source_create(id_c.as_ptr(), name_c.as_ptr(), settings.ptr(), std::ptr::null_mut()) };
        if p.is_null() {
            Err(format!("could not create source {id}"))
        } else {
            Ok(Source(p))
        }
    }

    pub fn size(&self) -> (u32, u32) {
        unsafe { (ffi::obs_source_get_width(self.0), ffi::obs_source_get_height(self.0)) }
    }

    pub fn connect(&self, signal: &str, callback: ffi::signal_callback_t, data: *mut c_void) {
        let s = cstr(signal);
        unsafe { ffi::signal_handler_connect(ffi::obs_source_get_signal_handler(self.0), s.as_ptr(), callback, data) }
    }

    pub fn disconnect(&self, signal: &str, callback: ffi::signal_callback_t, data: *mut c_void) {
        let s = cstr(signal);
        unsafe { ffi::signal_handler_disconnect(ffi::obs_source_get_signal_handler(self.0), s.as_ptr(), callback, data) }
    }

    pub fn set_audio_mixers(&self, mask: u32) {
        unsafe { ffi::obs_source_set_audio_mixers(self.0, mask) }
    }

    /// Applies settings over the current ones (obs_source_update).
    pub fn update(&self, settings: &Data) {
        unsafe { ffi::obs_source_update(self.0, settings.ptr()) }
    }

    /// Routes the source to a global output channel (0 = video, 1+ = audio).
    pub fn set_output_channel(&self, channel: u32) {
        unsafe { ffi::obs_set_output_source(channel, self.0) }
    }

    /// A filter, or another source kept out of OBS's list of named sources.
    pub fn private(id: &str, name: &str, settings: &Data) -> Result<Source, String> {
        let (id_c, name_c) = (cstr(id), cstr(name));
        let p = unsafe { ffi::obs_source_create_private(id_c.as_ptr(), name_c.as_ptr(), settings.ptr()) };
        if p.is_null() {
            Err(format!("could not create source {id}"))
        } else {
            Ok(Source(p))
        }
    }

    /// Puts a filter on this source; the source holds its own reference to the
    /// filter and lets it go when it is destroyed (obs_source_destroy).
    pub fn add_filter(&self, filter: &Source) {
        unsafe { ffi::obs_source_filter_add(self.0, filter.0) }
    }

    pub fn remove_filter(&self, filter: &Source) {
        unsafe { ffi::obs_source_filter_remove(self.0, filter.0) }
    }

    pub fn filter_count(&self) -> usize {
        unsafe { ffi::obs_source_filter_count(self.0) }
    }
}

/// A one-item scene that fits its source into the canvas: scaled to fit, keeping
/// the aspect ratio, and centered (OBS_BOUNDS_SCALE_INNER). A source on an output
/// channel by itself is drawn at its own size in the canvas' top left corner. A
/// second item can sit on top of the first (see set_top).
pub struct Scene {
    scene: *mut ffi::obs_scene_t,
    item: *mut ffi::obs_sceneitem_t,
    top: *mut ffi::obs_sceneitem_t,
    canvas: (u32, u32),
}

impl Scene {
    pub fn new(name: &str, canvas: (u32, u32)) -> Result<Scene, String> {
        let name = cstr(name);
        let scene = unsafe { ffi::obs_scene_create(name.as_ptr()) };
        if scene.is_null() {
            return Err("obs_scene_create failed".into());
        }
        Ok(Scene { scene, item: std::ptr::null_mut(), top: std::ptr::null_mut(), canvas })
    }

    /// Shows `source` (or nothing), replacing the previous one.
    pub fn set(&mut self, source: Option<&Source>) {
        remove_item(&mut self.item);
        if let Some(source) = source {
            self.item = self.add(source);
        }
    }

    /// Shows `source` (or nothing) over the main item, replacing the previous top
    /// one. A source that draws nothing, like a game capture before its hook, lets
    /// the main item show through.
    pub fn set_top(&mut self, source: Option<&Source>) {
        remove_item(&mut self.top);
        if let Some(source) = source {
            self.top = self.add(source);
        }
    }

    /// Makes the top item's `source` the main one. Its new item goes in before the
    /// old ones go, so the source never stops showing: a game capture that stops
    /// showing drops its hook.
    pub fn promote_top(&mut self, source: &Source) {
        let item = self.add(source);
        remove_item(&mut self.item);
        remove_item(&mut self.top);
        self.item = item;
    }

    /// Adds `source` on top of the scene, fitted into the canvas.
    fn add(&self, source: &Source) -> *mut ffi::obs_sceneitem_t {
        let item = unsafe { ffi::obs_scene_add(self.scene, source.0) };
        if item.is_null() {
            return item;
        }
        let bounds = ffi::vec2 { x: self.canvas.0 as f32, y: self.canvas.1 as f32 };
        unsafe {
            ffi::obs_sceneitem_set_bounds_type(item, ffi::OBS_BOUNDS_SCALE_INNER);
            ffi::obs_sceneitem_set_bounds_alignment(item, ffi::OBS_ALIGN_CENTER);
            ffi::obs_sceneitem_set_bounds(item, &bounds);
        }
        item
    }

    /// Routes the scene to a global output channel.
    pub fn set_output_channel(&self, channel: u32) {
        unsafe { ffi::obs_set_output_source(channel, ffi::obs_scene_get_source(self.scene)) }
    }
}

fn remove_item(item: &mut *mut ffi::obs_sceneitem_t) {
    if !item.is_null() {
        unsafe { ffi::obs_sceneitem_remove(*item) };
        *item = std::ptr::null_mut();
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        self.set_top(None);
        self.set(None);
        unsafe { ffi::obs_scene_release(self.scene) };
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        unsafe { ffi::obs_source_release(self.0) }
    }
}

/// An extra reference to a source that another thread can hold; libobs sources
/// are reference counted and their API is thread safe.
pub struct SharedSource(*mut ffi::obs_source_t);

unsafe impl Send for SharedSource {}

impl SharedSource {
    pub fn of(source: &Source) -> SharedSource {
        SharedSource(unsafe { ffi::obs_source_get_ref(source.0) })
    }

    pub fn update(&self, settings: &Data) {
        unsafe { ffi::obs_source_update(self.0, settings.ptr()) }
    }

    /// Another reference to the same source.
    pub fn share(&self) -> SharedSource {
        SharedSource(unsafe { ffi::obs_source_get_ref(self.0) })
    }
}

impl Drop for SharedSource {
    fn drop(&mut self) {
        unsafe { ffi::obs_source_release(self.0) }
    }
}

/// The (label, value) items of a list property of a source type (see
/// Obs::source_list_property); usable from any thread once libobs is up.
pub fn list_source_property(source_id: &str, property: &str) -> Vec<(String, String)> {
    let id = cstr(source_id);
    let name = cstr(property);
    let mut items = Vec::new();
    unsafe {
        let props = ffi::obs_get_source_properties(id.as_ptr());
        if props.is_null() {
            return items;
        }
        let prop = ffi::obs_properties_get(props, name.as_ptr());
        if !prop.is_null() {
            items = list_items(prop);
        }
        ffi::obs_properties_destroy(props);
    }
    items
}

pub fn clear_output_channel(channel: u32) {
    unsafe { ffi::obs_set_output_source(channel, std::ptr::null_mut()) }
}

pub struct Encoder(*mut ffi::obs_encoder_t);

impl Encoder {
    pub fn video(id: &str, name: &str, settings: &Data) -> Result<Encoder, String> {
        let (id_c, name_c) = (cstr(id), cstr(name));
        let p = unsafe { ffi::obs_video_encoder_create(id_c.as_ptr(), name_c.as_ptr(), settings.ptr(), std::ptr::null_mut()) };
        if p.is_null() {
            return Err(format!("could not create video encoder {id}"));
        }
        unsafe { ffi::obs_encoder_set_video(p, ffi::obs_get_video()) };
        Ok(Encoder(p))
    }

    pub fn audio(id: &str, name: &str, settings: &Data, mixer: usize) -> Result<Encoder, String> {
        let (id_c, name_c) = (cstr(id), cstr(name));
        let p = unsafe { ffi::obs_audio_encoder_create(id_c.as_ptr(), name_c.as_ptr(), settings.ptr(), mixer, std::ptr::null_mut()) };
        if p.is_null() {
            return Err(format!("could not create audio encoder {id} for mixer {mixer}"));
        }
        unsafe { ffi::obs_encoder_set_audio(p, ffi::obs_get_audio()) };
        Ok(Encoder(p))
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { ffi::obs_encoder_release(self.0) }
    }
}

pub struct Output(*mut ffi::obs_output_t);

impl Output {
    pub fn new(id: &str, name: &str, settings: &Data) -> Result<Output, String> {
        let (id_c, name_c) = (cstr(id), cstr(name));
        let p = unsafe { ffi::obs_output_create(id_c.as_ptr(), name_c.as_ptr(), settings.ptr(), std::ptr::null_mut()) };
        if p.is_null() {
            Err(format!("could not create output {id}"))
        } else {
            Ok(Output(p))
        }
    }

    pub fn set_video_encoder(&self, enc: &Encoder) {
        unsafe { ffi::obs_output_set_video_encoder(self.0, enc.0) }
    }

    pub fn set_audio_encoder(&self, enc: &Encoder, track: usize) {
        unsafe { ffi::obs_output_set_audio_encoder(self.0, enc.0, track) }
    }

    pub fn start(&self) -> Result<(), String> {
        if unsafe { ffi::obs_output_start(self.0) } {
            Ok(())
        } else {
            Err(opt_string(unsafe { ffi::obs_output_get_last_error(self.0) }).unwrap_or_else(|| "obs_output_start failed".into()))
        }
    }

    pub fn stop(&self) {
        unsafe { ffi::obs_output_stop(self.0) }
    }

    pub fn active(&self) -> bool {
        unsafe { ffi::obs_output_active(self.0) }
    }

    pub fn frames_dropped(&self) -> i32 {
        unsafe { ffi::obs_output_get_frames_dropped(self.0) }
    }

    /// Calls a procedure of the output and returns a string result, if any.
    pub fn call(&self, proc_name: &str, result_key: Option<&str>) -> Result<Option<String>, String> {
        let name = cstr(proc_name);
        let mut cd = ffi::calldata_t { stack: std::ptr::null_mut(), size: 0, capacity: 0, fixed: false };
        unsafe {
            let ph = ffi::obs_output_get_proc_handler(self.0);
            if !ffi::proc_handler_call(ph, name.as_ptr(), &mut cd) {
                ffi::calldata_free(&mut cd);
                return Err(format!("procedure {proc_name} not found"));
            }
            let mut value = None;
            if let Some(key) = result_key {
                let k = cstr(key);
                let mut s: *const c_char = std::ptr::null();
                if ffi::calldata_get_string(&cd, k.as_ptr(), &mut s) {
                    value = opt_string(s);
                }
            }
            ffi::calldata_free(&mut cd);
            Ok(value)
        }
    }

    pub fn connect(&self, signal: &str, callback: ffi::signal_callback_t, data: *mut c_void) {
        let s = cstr(signal);
        unsafe { ffi::signal_handler_connect(ffi::obs_output_get_signal_handler(self.0), s.as_ptr(), callback, data) }
    }

    pub fn disconnect(&self, signal: &str, callback: ffi::signal_callback_t, data: *mut c_void) {
        let s = cstr(signal);
        unsafe { ffi::signal_handler_disconnect(ffi::obs_output_get_signal_handler(self.0), s.as_ptr(), callback, data) }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        unsafe { ffi::obs_output_release(self.0) }
    }
}

impl Output {
    /// A weak reference for another thread (health.rs): it never keeps the output
    /// alive, so the output is still destroyed where the recorder drops it.
    pub fn weak(&self) -> WeakOutput {
        WeakOutput(unsafe { ffi::obs_output_get_weak_output(self.0) })
    }
}

/// A weak reference to an output, usable from another thread: libobs' output
/// getters used here read counters and flags only.
pub struct WeakOutput(*mut ffi::obs_weak_output_t);

unsafe impl Send for WeakOutput {}

impl WeakOutput {
    /// The output's state now, if it still exists: (active, video frames sent to it
    /// since it last started).
    pub fn state(&self) -> Option<(bool, i32)> {
        let output = unsafe { ffi::obs_weak_output_get_output(self.0) };
        if output.is_null() {
            return None;
        }
        let state = unsafe { (ffi::obs_output_active(output), ffi::obs_output_get_total_frames(output)) };
        unsafe { ffi::obs_output_release(output) };
        Some(state)
    }
}

impl Drop for WeakOutput {
    fn drop(&mut self) {
        unsafe { ffi::obs_weak_output_release(self.0) }
    }
}

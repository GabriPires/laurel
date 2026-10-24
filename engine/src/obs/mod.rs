//! Thin RAII wrappers over the libobs C API (see `ffi`).
//!
//! libobs objects are reference counted and not thread safe from Rust's point of
//! view; the wrappers are used from the engine's main thread only.

pub mod ffi;

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;

pub fn cstr(s: &str) -> CString {
    CString::new(s).expect("string without NUL")
}

pub fn opt_string(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
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
            adapter: 0,
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
                let int = ffi::obs_property_list_format(prop) == ffi::OBS_COMBO_FORMAT_INT;
                for i in 0..ffi::obs_property_list_item_count(prop) {
                    let value = if int {
                        ffi::obs_property_list_item_int(prop, i).to_string()
                    } else {
                        opt_string(ffi::obs_property_list_item_string(prop, i)).unwrap_or_default()
                    };
                    items.push((opt_string(ffi::obs_property_list_item_name(prop, i)).unwrap_or_default(), value));
                }
            }
            ffi::obs_properties_destroy(props);
        }
        items
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
}

/// A one-item scene that fits its source into the canvas: scaled to fit, keeping
/// the aspect ratio, and centered (OBS_BOUNDS_SCALE_INNER). A source on an output
/// channel by itself is drawn at its own size in the canvas' top left corner.
pub struct Scene {
    scene: *mut ffi::obs_scene_t,
    item: *mut ffi::obs_sceneitem_t,
    canvas: (u32, u32),
}

impl Scene {
    pub fn new(name: &str, canvas: (u32, u32)) -> Result<Scene, String> {
        let name = cstr(name);
        let scene = unsafe { ffi::obs_scene_create(name.as_ptr()) };
        if scene.is_null() {
            return Err("obs_scene_create failed".into());
        }
        Ok(Scene { scene, item: std::ptr::null_mut(), canvas })
    }

    /// Shows `source` (or nothing), replacing the previous one.
    pub fn set(&mut self, source: Option<&Source>) {
        if !self.item.is_null() {
            unsafe { ffi::obs_sceneitem_remove(self.item) };
            self.item = std::ptr::null_mut();
        }
        let Some(source) = source else { return };
        let item = unsafe { ffi::obs_scene_add(self.scene, source.0) };
        if item.is_null() {
            return;
        }
        let bounds = ffi::vec2 { x: self.canvas.0 as f32, y: self.canvas.1 as f32 };
        unsafe {
            ffi::obs_sceneitem_set_bounds_type(item, ffi::OBS_BOUNDS_SCALE_INNER);
            ffi::obs_sceneitem_set_bounds_alignment(item, ffi::OBS_ALIGN_CENTER);
            ffi::obs_sceneitem_set_bounds(item, &bounds);
        }
        self.item = item;
    }

    /// Routes the scene to a global output channel.
    pub fn set_output_channel(&self, channel: u32) {
        unsafe { ffi::obs_set_output_source(channel, ffi::obs_scene_get_source(self.scene)) }
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
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
}

impl Drop for SharedSource {
    fn drop(&mut self) {
        unsafe { ffi::obs_source_release(self.0) }
    }
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

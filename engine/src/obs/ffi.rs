//! Hand-written declarations of the libobs C API used by the engine.
//!
//! obs.dll is linked with the `raw-dylib` kind, so no import library is needed when
//! cross-compiling. Signatures and struct layouts follow the libobs 32 public headers;
//! keep this file limited to what the engine calls.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_void};

pub type obs_data_t = c_void;
pub type obs_source_t = c_void;
pub type obs_scene_t = c_void;
pub type obs_sceneitem_t = c_void;
pub type obs_encoder_t = c_void;
pub type obs_output_t = c_void;
pub type obs_module_t = c_void;
pub type obs_properties_t = c_void;
pub type obs_property_t = c_void;

/// `enum obs_property_type`: a list (combo box) property.
pub const OBS_PROPERTY_LIST: i32 = 6;
pub type proc_handler_t = c_void;
pub type signal_handler_t = c_void;
pub type video_t = c_void;
pub type audio_t = c_void;

pub const MAX_AV_PLANES: usize = 8;

// enum video_format
pub const VIDEO_FORMAT_NV12: i32 = 2;
pub const VIDEO_FORMAT_RGBA: i32 = 6;
// enum video_colorspace
pub const VIDEO_CS_709: i32 = 2;
// enum video_range_type
pub const VIDEO_RANGE_PARTIAL: i32 = 1;
pub const VIDEO_RANGE_FULL: i32 = 2;
// enum obs_scale_type
pub const OBS_SCALE_BICUBIC: i32 = 2;
// enum speaker_layout
pub const SPEAKERS_STEREO: i32 = 2;
// enum obs_combo_format
pub const OBS_COMBO_FORMAT_INT: i32 = 1;
// enum obs_encoder_type
pub const OBS_ENCODER_AUDIO: i32 = 0;
pub const OBS_ENCODER_VIDEO: i32 = 1;

// enum obs_bounds_type
pub const OBS_BOUNDS_SCALE_INNER: i32 = 2;
// obs-defs.h alignment flags
pub const OBS_ALIGN_CENTER: u32 = 0;

pub const OBS_VIDEO_SUCCESS: i32 = 0;

#[repr(C)]
pub struct vec2 {
    pub x: f32,
    pub y: f32,
}
pub const MODULE_SUCCESS: i32 = 0;

#[repr(C)]
pub struct obs_video_info {
    pub graphics_module: *const c_char,
    pub fps_num: u32,
    pub fps_den: u32,
    pub base_width: u32,
    pub base_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub output_format: i32,
    pub adapter: u32,
    pub gpu_conversion: bool,
    pub colorspace: i32,
    pub range: i32,
    pub scale_type: i32,
}

#[repr(C)]
pub struct obs_audio_info {
    pub samples_per_sec: u32,
    pub speakers: i32,
}

#[repr(C)]
pub struct video_scale_info {
    pub format: i32,
    pub width: u32,
    pub height: u32,
    pub range: i32,
    pub colorspace: i32,
}

#[repr(C)]
pub struct video_data {
    pub data: [*mut u8; MAX_AV_PLANES],
    pub linesize: [u32; MAX_AV_PLANES],
    pub timestamp: u64,
}

/// `calldata_t`; initialize zeroed, free with `calldata_free`.
#[repr(C)]
pub struct calldata_t {
    pub stack: *mut u8,
    pub size: usize,
    pub capacity: usize,
    pub fixed: bool,
}

pub type signal_callback_t = unsafe extern "C" fn(data: *mut c_void, cd: *mut calldata_t);
pub type raw_video_callback_t = unsafe extern "C" fn(param: *mut c_void, frame: *mut video_data);
/// `va_list` is a plain pointer on x86_64 Windows, so it is passed through opaquely.
pub type log_handler_t = unsafe extern "C" fn(level: i32, msg: *const c_char, args: *mut c_void, param: *mut c_void);

// log levels (util/base.h)
pub const LOG_ERROR: i32 = 100;
pub const LOG_WARNING: i32 = 200;
pub const LOG_INFO: i32 = 300;

#[repr(C)]
pub struct dstr {
    pub array: *mut c_char,
    pub len: usize,
    pub capacity: usize,
}

#[link(name = "obs", kind = "raw-dylib")]
extern "C" {
    // core
    pub fn obs_startup(locale: *const c_char, module_config_path: *const c_char, store: *mut c_void) -> bool;
    pub fn obs_shutdown();
    pub fn obs_get_version_string() -> *const c_char;
    pub fn obs_add_data_path(path: *const c_char);
    pub fn obs_reset_video(ovi: *mut obs_video_info) -> i32;
    pub fn obs_reset_audio(oai: *const obs_audio_info) -> bool;
    pub fn obs_open_module(module: *mut *mut obs_module_t, path: *const c_char, data_path: *const c_char) -> i32;
    pub fn obs_init_module(module: *mut obs_module_t) -> bool;
    pub fn obs_post_load_modules();
    pub fn obs_get_video() -> *mut video_t;
    pub fn obs_get_audio() -> *mut audio_t;
    pub fn obs_set_output_source(channel: u32, source: *mut obs_source_t);
    pub fn obs_scene_create(name: *const c_char) -> *mut obs_scene_t;
    pub fn obs_scene_release(scene: *mut obs_scene_t);
    pub fn obs_scene_get_source(scene: *const obs_scene_t) -> *mut obs_source_t;
    pub fn obs_scene_add(scene: *mut obs_scene_t, source: *mut obs_source_t) -> *mut obs_sceneitem_t;
    pub fn obs_sceneitem_remove(item: *mut obs_sceneitem_t);
    pub fn obs_sceneitem_set_bounds_type(item: *mut obs_sceneitem_t, kind: i32);
    pub fn obs_sceneitem_set_bounds_alignment(item: *mut obs_sceneitem_t, alignment: u32);
    pub fn obs_sceneitem_set_bounds(item: *mut obs_sceneitem_t, bounds: *const vec2);

    // enumeration
    pub fn obs_enum_encoder_types(idx: usize, id: *mut *const c_char) -> bool;
    pub fn obs_get_encoder_type(encoder_id: *const c_char) -> i32;
    pub fn obs_get_encoder_codec(id: *const c_char) -> *const c_char;
    pub fn obs_encoder_get_display_name(id: *const c_char) -> *const c_char;

    // settings
    pub fn obs_data_create() -> *mut obs_data_t;
    pub fn obs_data_release(data: *mut obs_data_t);
    pub fn obs_data_get_int(data: *mut obs_data_t, name: *const c_char) -> i64;
    pub fn obs_data_get_string(data: *mut obs_data_t, name: *const c_char) -> *const c_char;
    pub fn obs_data_set_string(data: *mut obs_data_t, name: *const c_char, val: *const c_char);
    pub fn obs_data_set_int(data: *mut obs_data_t, name: *const c_char, val: i64);
    pub fn obs_data_set_bool(data: *mut obs_data_t, name: *const c_char, val: bool);

    // sources
    pub fn obs_source_create(id: *const c_char, name: *const c_char, settings: *mut obs_data_t, hotkey_data: *mut obs_data_t) -> *mut obs_source_t;
    pub fn obs_source_release(source: *mut obs_source_t);
    pub fn obs_source_get_ref(source: *mut obs_source_t) -> *mut obs_source_t;
    pub fn obs_source_update(source: *mut obs_source_t, settings: *mut obs_data_t);
    pub fn obs_source_get_width(source: *mut obs_source_t) -> u32;
    pub fn obs_source_get_height(source: *mut obs_source_t) -> u32;
    pub fn obs_source_set_audio_mixers(source: *mut obs_source_t, mixers: u32);

    // properties
    pub fn obs_get_source_properties(id: *const c_char) -> *mut obs_properties_t;
    pub fn obs_properties_get(props: *mut obs_properties_t, property: *const c_char) -> *mut obs_property_t;
    pub fn obs_property_list_item_count(p: *mut obs_property_t) -> usize;
    pub fn obs_property_list_item_name(p: *mut obs_property_t, idx: usize) -> *const c_char;
    pub fn obs_property_list_item_string(p: *mut obs_property_t, idx: usize) -> *const c_char;
    pub fn obs_property_list_item_int(p: *mut obs_property_t, idx: usize) -> i64;
    pub fn obs_property_list_format(p: *mut obs_property_t) -> i32;
    pub fn obs_properties_destroy(props: *mut obs_properties_t);
    pub fn obs_get_encoder_properties(id: *const c_char) -> *mut obs_properties_t;
    pub fn obs_encoder_defaults(id: *const c_char) -> *mut obs_data_t;
    pub fn obs_properties_first(props: *mut obs_properties_t) -> *mut obs_property_t;
    pub fn obs_property_next(p: *mut *mut obs_property_t) -> bool;
    pub fn obs_property_name(p: *mut obs_property_t) -> *const c_char;
    pub fn obs_property_get_type(p: *mut obs_property_t) -> i32;

    // encoders
    pub fn obs_video_encoder_create(id: *const c_char, name: *const c_char, settings: *mut obs_data_t, hotkey_data: *mut obs_data_t) -> *mut obs_encoder_t;
    pub fn obs_audio_encoder_create(id: *const c_char, name: *const c_char, settings: *mut obs_data_t, mixer_idx: usize, hotkey_data: *mut obs_data_t) -> *mut obs_encoder_t;
    pub fn obs_encoder_set_video(encoder: *mut obs_encoder_t, video: *mut video_t);
    pub fn obs_encoder_set_audio(encoder: *mut obs_encoder_t, audio: *mut audio_t);
    pub fn obs_encoder_release(encoder: *mut obs_encoder_t);

    // outputs
    pub fn obs_output_create(id: *const c_char, name: *const c_char, settings: *mut obs_data_t, hotkey_data: *mut obs_data_t) -> *mut obs_output_t;
    pub fn obs_output_set_video_encoder(output: *mut obs_output_t, encoder: *mut obs_encoder_t);
    pub fn obs_output_set_audio_encoder(output: *mut obs_output_t, encoder: *mut obs_encoder_t, idx: usize);
    pub fn obs_output_start(output: *mut obs_output_t) -> bool;
    pub fn obs_output_stop(output: *mut obs_output_t);
    pub fn obs_output_active(output: *const obs_output_t) -> bool;
    pub fn obs_output_get_last_error(output: *mut obs_output_t) -> *const c_char;
    pub fn obs_output_get_frames_dropped(output: *const obs_output_t) -> i32;
    pub fn obs_output_get_proc_handler(output: *const obs_output_t) -> *mut proc_handler_t;
    pub fn obs_output_get_signal_handler(output: *const obs_output_t) -> *mut signal_handler_t;
    pub fn obs_source_get_signal_handler(source: *const obs_source_t) -> *mut signal_handler_t;
    pub fn obs_output_release(output: *mut obs_output_t);

    // callbacks and calldata
    pub fn proc_handler_call(handler: *mut proc_handler_t, name: *const c_char, params: *mut calldata_t) -> bool;
    pub fn signal_handler_connect(handler: *mut signal_handler_t, signal: *const c_char, callback: signal_callback_t, data: *mut c_void);
    pub fn signal_handler_disconnect(handler: *mut signal_handler_t, signal: *const c_char, callback: signal_callback_t, data: *mut c_void);
    pub fn calldata_get_string(data: *const calldata_t, name: *const c_char, str: *mut *const c_char) -> bool;
    pub fn bfree(ptr: *mut c_void);
    pub fn base_set_log_handler(handler: log_handler_t, param: *mut c_void);
    pub fn dstr_vprintf(dst: *mut dstr, format: *const c_char, args: *mut c_void);

    // raw frames (screenshots)
    pub fn obs_add_raw_video_callback(conversion: *const video_scale_info, callback: raw_video_callback_t, param: *mut c_void);
    pub fn obs_remove_raw_video_callback(callback: raw_video_callback_t, param: *mut c_void);

    // statistics
    pub fn obs_get_active_fps() -> f64;
    pub fn obs_get_average_frame_time_ns() -> u64;
    pub fn obs_get_lagged_frames() -> u32;
    pub fn video_output_get_skipped_frames(video: *const video_t) -> u32;
    pub fn video_output_get_total_frames(video: *const video_t) -> u32;
}

/// Frees a calldata stack the way the inline `calldata_free` in libobs does.
pub unsafe fn calldata_free(cd: &mut calldata_t) {
    if !cd.fixed && !cd.stack.is_null() {
        bfree(cd.stack.cast());
        cd.stack = std::ptr::null_mut();
    }
}

//! Phase 3 feasibility spike (option B: cross-compiled from WSL).
//!
//! Loads libobs from an existing OBS Studio install, initializes D3D11 video,
//! loads the bundled plugins and prints the registered encoder, source and output
//! type IDs. Nothing in the OBS install is modified: module config is written to
//! a separate folder passed on the command line.
//!
//! Usage: libobs-hello.exe <obs-root> <config-dir>
//!
//! Only the modules Laurel needs are loaded (see MODULES); loading every plugin of
//! a user's OBS install pulls in frontend-only plugins that abort without the OBS UI.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;

#[repr(C)]
struct ObsVideoInfo {
    graphics_module: *const c_char,
    fps_num: u32,
    fps_den: u32,
    base_width: u32,
    base_height: u32,
    output_width: u32,
    output_height: u32,
    output_format: i32, // enum video_format
    adapter: u32,
    gpu_conversion: bool,
    colorspace: i32,  // enum video_colorspace
    range: i32,       // enum video_range_type
    scale_type: i32,  // enum obs_scale_type
}

const VIDEO_FORMAT_NV12: i32 = 2;
const VIDEO_CS_709: i32 = 2;
const VIDEO_RANGE_PARTIAL: i32 = 1;
const OBS_SCALE_BICUBIC: i32 = 2;
const OBS_VIDEO_SUCCESS: i32 = 0;
const MODULE_SUCCESS: i32 = 0;

/// Capture, audio, encoders and outputs needed by the MVP specs.
const MODULES: &[&str] = &[
    "win-capture",
    "win-wasapi",
    "obs-ffmpeg",
    "obs-x264",
    "obs-nvenc",
    "obs-qsv11",
    "obs-outputs",
];

#[link(name = "obs", kind = "raw-dylib")]
extern "C" {
    fn obs_startup(locale: *const c_char, module_config_path: *const c_char, store: *mut c_void) -> bool;
    fn obs_get_version_string() -> *const c_char;
    fn obs_add_data_path(path: *const c_char);
    fn obs_open_module(module: *mut *mut c_void, path: *const c_char, data_path: *const c_char) -> i32;
    fn obs_init_module(module: *mut c_void) -> bool;
    fn obs_reset_video(ovi: *mut ObsVideoInfo) -> i32;
    fn obs_post_load_modules();
    fn obs_enum_encoder_types(idx: usize, id: *mut *const c_char) -> bool;
    fn obs_enum_source_types(idx: usize, id: *mut *const c_char) -> bool;
    fn obs_enum_output_types(idx: usize, id: *mut *const c_char) -> bool;
    fn obs_shutdown();
}

fn cstr(s: &str) -> CString {
    CString::new(s).expect("path without NUL")
}

fn list(label: &str, f: unsafe extern "C" fn(usize, *mut *const c_char) -> bool) {
    let mut ids = Vec::new();
    let mut idx = 0;
    loop {
        let mut id: *const c_char = std::ptr::null();
        if !unsafe { f(idx, &mut id) } {
            break;
        }
        ids.push(unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned());
        idx += 1;
    }
    println!("{label} ({}): {}", ids.len(), ids.join(", "));
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: {} <obs-root> <config-dir>", args[0]);
        std::process::exit(2);
    }
    let root = Path::new(&args[1]);
    let config = cstr(&args[2]);
    let libobs_data = cstr(&format!("{}/", root.join("data/libobs").display()));

    unsafe {
        if !obs_startup(cstr("en-US").as_ptr(), config.as_ptr(), std::ptr::null_mut()) {
            eprintln!("obs_startup failed");
            std::process::exit(1);
        }
        let version = CStr::from_ptr(obs_get_version_string()).to_string_lossy();
        println!("libobs version: {version}");

        obs_add_data_path(libobs_data.as_ptr());
        let module = cstr("libobs-d3d11");
        let mut ovi = ObsVideoInfo {
            graphics_module: module.as_ptr(),
            fps_num: 60,
            fps_den: 1,
            base_width: 1920,
            base_height: 1080,
            output_width: 1920,
            output_height: 1080,
            output_format: VIDEO_FORMAT_NV12,
            adapter: 0,
            gpu_conversion: true,
            colorspace: VIDEO_CS_709,
            range: VIDEO_RANGE_PARTIAL,
            scale_type: OBS_SCALE_BICUBIC,
        };
        let rc = obs_reset_video(&mut ovi);
        println!("obs_reset_video (d3d11): {}", if rc == OBS_VIDEO_SUCCESS { "ok".to_string() } else { format!("error {rc}") });

        for name in MODULES {
            let bin = cstr(&root.join(format!("obs-plugins/64bit/{name}.dll")).display().to_string());
            let data = cstr(&root.join(format!("data/obs-plugins/{name}")).display().to_string());
            let mut module: *mut c_void = std::ptr::null_mut();
            let rc = obs_open_module(&mut module, bin.as_ptr(), data.as_ptr());
            let ok = rc == MODULE_SUCCESS && obs_init_module(module);
            println!("module {name}: {}", if ok { "loaded".to_string() } else { format!("failed (open rc {rc})") });
        }
        obs_post_load_modules();

        list("encoders", obs_enum_encoder_types);
        list("sources", obs_enum_source_types);
        list("outputs", obs_enum_output_types);

        obs_shutdown();
    }
    println!("shutdown ok");
}

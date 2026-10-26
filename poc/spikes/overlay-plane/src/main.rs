//! Spike: an in-game notice drawn through DirectComposition instead of a layered
//! WebView2 window, to see whether Windows puts it on a hardware overlay plane
//! (MPO) and keeps the game under it in independent flip.
//!
//! Usage: overlay-plane [--count N] [--every S] [--hold MS] [--opaque] [--layered]
//!                      [--capturable] [--info-only]
//!   --count N      notices to show (default 5), one every S seconds (default 20)
//!   --hold MS      how long each notice stays (default 1500)
//!   --opaque       an opaque square notice (no alpha) instead of a rounded one
//!   --layered      add WS_EX_LAYERED (click-through like the app's overlay)
//!   --capturable   do not exclude the window from screen capture
//!   --info-only    print what the adapters and outputs report, and exit
//! Each notice is logged as "[laurel] notice at hh:mm:ss.mmm" (tools/stutter.py) and
//! then with how its frames were presented: "overlay" means a hardware plane,
//! "composed" means the desktop window manager drew it.

use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use windows::core::{w, Interface, Result, BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DeviceContext, ID2D1Factory1, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION};
use windows::Win32::Graphics::DirectComposition::{DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, IDXGIAdapter1, IDXGIDevice, IDXGIFactory2, IDXGIOutput2, IDXGIOutput3, IDXGISurface, IDXGISwapChain1, IDXGISwapChainMedia,
    DXGI_CREATE_FACTORY_FLAGS, DXGI_FRAME_PRESENTATION_MODE_COMPOSED, DXGI_FRAME_PRESENTATION_MODE_COMPOSITION_FAILURE, DXGI_FRAME_PRESENTATION_MODE_NONE,
    DXGI_FRAME_PRESENTATION_MODE_OVERLAY, DXGI_FRAME_STATISTICS_MEDIA, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetWindowRect, PeekMessageW, RegisterClassExW, SetLayeredWindowAttributes,
    SetWindowDisplayAffinity, SetWindowPos, ShowWindow, TranslateMessage, HTTRANSPARENT, HWND_TOPMOST, LWA_ALPHA, MSG, PM_REMOVE, SWP_NOACTIVATE,
    SW_HIDE, SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WM_NCHITTEST, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

/// Logical size of one notice, as in the app's overlay (ui/overlay.css).
const WIDTH: f32 = 300.0;
const HEIGHT: f32 = 64.0;
const MARGIN: f32 = 20.0;

struct Options {
    count: u32,
    every: f64,
    hold: u64,
    opaque: bool,
    layered: bool,
    capturable: bool,
    info_only: bool,
}

fn options() -> Options {
    let args: Vec<String> = std::env::args().collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    Options {
        count: value("--count").and_then(|v| v.parse().ok()).unwrap_or(5),
        every: value("--every").and_then(|v| v.parse().ok()).unwrap_or(20.0),
        hold: value("--hold").and_then(|v| v.parse().ok()).unwrap_or(1500),
        opaque: args.iter().any(|a| a == "--opaque"),
        layered: args.iter().any(|a| a == "--layered"),
        capturable: args.iter().any(|a| a == "--capturable"),
        info_only: args.iter().any(|a| a == "--info-only"),
    }
}

fn clock() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}.{:03}", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// What the adapters and their outputs say about overlay planes, and whether the
/// desktop window manager's test switch for them is set.
fn report_support(device: &ID3D11Device) -> Result<bool> {
    let mut any = false;
    let mut mode = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, w!(r"SOFTWARE\Microsoft\Windows\Dwm"), w!("OverlayTestMode"), RRF_RT_REG_DWORD, None, Some(&mut mode as *mut u32 as _), Some(&mut size))
    };
    if status.is_ok() {
        println!("[overlay-plane] registry Dwm\\OverlayTestMode = {mode} (5 turns overlay planes off)");
    } else {
        println!("[overlay-plane] registry Dwm\\OverlayTestMode not set");
    }
    let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
    let mut a = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(a) } {
        let adapter: IDXGIAdapter1 = adapter;
        let desc = unsafe { adapter.GetDesc1()? };
        let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_string();
        let mut o = 0;
        while let Ok(output) = unsafe { adapter.EnumOutputs(o) } {
            let odesc = unsafe { output.GetDesc()? };
            let oname = String::from_utf16_lossy(&odesc.DeviceName).trim_end_matches('\0').to_string();
            let supports = output.cast::<IDXGIOutput2>().map(|o2| unsafe { o2.SupportsOverlays() }.as_bool()).unwrap_or(false);
            let flags = output.cast::<IDXGIOutput3>().ok().map(|o3| unsafe { o3.CheckOverlaySupport(DXGI_FORMAT_B8G8R8A8_UNORM, device) });
            let flags = match flags {
                Some(Ok(f)) => format!("0x{f:X} (1 direct, 2 scaling)"),
                Some(Err(e)) => format!("error {e}"),
                None => "not available".into(),
            };
            println!("[overlay-plane] adapter {a} \"{name}\" output {o} {oname}: SupportsOverlays={supports} CheckOverlaySupport(BGRA)={flags}");
            any |= supports;
            o += 1;
        }
        a += 1;
    }
    Ok(any)
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn pump() {
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

struct Notice {
    hwnd: HWND,
    swapchain: IDXGISwapChain1,
    dc: ID2D1DeviceContext,
    title: IDWriteTextFormat,
    detail: IDWriteTextFormat,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    width: u32,
    height: u32,
    scale: f32,
    opaque: bool,
}

impl Notice {
    fn new(device: &ID3D11Device, opts: &Options) -> Result<Notice> {
        let instance = unsafe { GetModuleHandleW(None)? };
        let class = w!("LaurelOverlayPlaneSpike");
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(window_proc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
        unsafe { RegisterClassExW(&wc) };
        let mut ex = WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT;
        if opts.layered {
            ex |= WS_EX_LAYERED;
        }
        let hwnd = unsafe { CreateWindowExW(ex, class, w!("Laurel notice"), WS_POPUP, 0, 0, 1, 1, None, None, Some(instance.into()), None)? };
        if opts.layered {
            unsafe { SetLayeredWindowAttributes(hwnd, Default::default(), 255, LWA_ALPHA)? };
        }
        if !opts.capturable {
            unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)? };
        }
        let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
        let (width, height) = ((WIDTH * scale).round() as u32, (HEIGHT * scale).round() as u32);

        let dxgi_device: IDXGIDevice = device.cast()?;
        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: if opts.opaque { DXGI_ALPHA_MODE_IGNORE } else { DXGI_ALPHA_MODE_PREMULTIPLIED },
            ..Default::default()
        };
        let swapchain = unsafe { factory.CreateSwapChainForComposition(device, &desc, None)? };
        let dcomp: IDCompositionDevice = unsafe { DCompositionCreateDevice(&dxgi_device)? };
        let target = unsafe { dcomp.CreateTargetForHwnd(hwnd, true)? };
        let visual = unsafe { dcomp.CreateVisual()? };
        unsafe {
            visual.SetContent(&swapchain)?;
            target.SetRoot(&visual)?;
            dcomp.Commit()?;
        }

        let d2d: ID2D1Factory1 = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let d2d_device = unsafe { d2d.CreateDevice(&dxgi_device)? };
        let dc = unsafe { d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)? };
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let title = unsafe { dwrite.CreateTextFormat(w!("Segoe UI"), None, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 15.0 * scale, w!("en-us"))? };
        let detail = unsafe { dwrite.CreateTextFormat(w!("Segoe UI"), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 13.0 * scale, w!("en-us"))? };
        Ok(Notice { hwnd, swapchain, dc, title, detail, _target: target, _visual: visual, width, height, scale, opaque: opts.opaque })
    }

    /// Draws the notice into the next back buffer and presents it (waits for vblank).
    fn present(&self, index: u32) -> Result<()> {
        let surface: IDXGISurface = unsafe { self.swapchain.GetBuffer(0)? };
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: ManuallyDrop::new(None),
        };
        let bitmap = unsafe { self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))? };
        let s = self.scale;
        let (w, h) = (self.width as f32, self.height as f32);
        unsafe {
            self.dc.SetTarget(&bitmap);
            self.dc.BeginDraw();
            self.dc.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            let bg = self.dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.063, g: 0.078, b: 0.094, a: 0.94 }, None)?;
            let line = self.dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.071, g: 0.851, b: 0.608, a: 1.0 }, None)?;
            let text = self.dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.93, g: 0.95, b: 0.94, a: 1.0 }, None)?;
            let dim = self.dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.65, g: 0.70, b: 0.68, a: 1.0 }, None)?;
            let rect = D2D_RECT_F { left: 0.5 * s, top: 0.5 * s, right: w - 0.5 * s, bottom: h - 0.5 * s };
            if self.opaque {
                self.dc.FillRectangle(&rect, &bg);
                self.dc.DrawRectangle(&rect, &line, 1.0 * s, None);
            } else {
                let rr = D2D1_ROUNDED_RECT { rect, radiusX: 8.0 * s, radiusY: 8.0 * s };
                self.dc.FillRoundedRectangle(&rr, &bg);
                self.dc.DrawRoundedRectangle(&rr, &line, 1.0 * s, None);
            }
            let title = wide("Clip saved");
            let detail = wide(&format!("Last 30 s  ·  notice {index}"));
            self.dc.DrawText(&title, &self.title, &D2D_RECT_F { left: 16.0 * s, top: 12.0 * s, right: w - 12.0 * s, bottom: 34.0 * s }, &text, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            self.dc.DrawText(&detail, &self.detail, &D2D_RECT_F { left: 16.0 * s, top: 34.0 * s, right: w - 12.0 * s, bottom: 56.0 * s }, &dim, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            self.dc.EndDraw(None, None)?;
            self.dc.SetTarget(None);
            self.swapchain.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }

    /// How the last presented frame reached the screen.
    fn mode(&self) -> &'static str {
        let Ok(media) = self.swapchain.cast::<IDXGISwapChainMedia>() else { return "no media interface" };
        let mut stats = DXGI_FRAME_STATISTICS_MEDIA::default();
        match unsafe { media.GetFrameStatisticsMedia(&mut stats) } {
            Err(_) => "no statistics yet",
            Ok(()) => match stats.CompositionMode {
                DXGI_FRAME_PRESENTATION_MODE_OVERLAY => "overlay",
                DXGI_FRAME_PRESENTATION_MODE_COMPOSED => "composed",
                DXGI_FRAME_PRESENTATION_MODE_NONE => "none",
                DXGI_FRAME_PRESENTATION_MODE_COMPOSITION_FAILURE => "composition failure",
                _ => "unknown",
            },
        }
    }

    /// Shows the notice at the top right of the foreground window's monitor work
    /// area (or of the foreground window, when it is smaller), presents it for
    /// `hold`, then hides it; returns how its frames were presented.
    fn show(&self, index: u32, hold: Duration) -> Result<Vec<(&'static str, u32)>> {
        let fg = unsafe { GetForegroundWindow() };
        let mut r = RECT::default();
        unsafe { GetWindowRect(fg, &mut r)? };
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            let _ = GetMonitorInfoW(MonitorFromWindow(fg, MONITOR_DEFAULTTOPRIMARY), &mut mi);
        }
        let right = r.right.min(mi.rcMonitor.right);
        let top = r.top.max(mi.rcMonitor.top);
        let m = (MARGIN * self.scale) as i32;
        self.present(index)?;
        unsafe {
            SetWindowPos(self.hwnd, Some(HWND_TOPMOST), right - self.width as i32 - m, top + m, self.width as i32, self.height as i32, SWP_NOACTIVATE)?;
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        println!("[laurel] notice at {}", clock());
        let started = Instant::now();
        let mut modes: Vec<(&'static str, u32)> = vec![];
        while started.elapsed() < hold {
            pump();
            self.present(index)?;
            let mode = self.mode();
            match modes.last_mut() {
                Some((m, n)) if *m == mode => *n += 1,
                _ => modes.push((mode, 1)),
            }
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        pump();
        Ok(modes)
    }
}

fn main() -> Result<()> {
    let opts = options();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let mut device: Option<ID3D11Device> = None;
    unsafe {
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, Default::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, None)?;
    }
    let device = device.expect("no D3D11 device");
    let any = report_support(&device)?;
    println!("[overlay-plane] some output reports overlay planes: {any}");
    if opts.info_only {
        return Ok(());
    }
    let notice = Notice::new(&device, &opts)?;
    println!(
        "[overlay-plane] notice {}x{} px, {}, {}, {}",
        notice.width,
        notice.height,
        if opts.opaque { "opaque" } else { "premultiplied alpha" },
        if opts.layered { "layered" } else { "not layered" },
        if opts.capturable { "capturable" } else { "excluded from capture" }
    );
    let start = Instant::now();
    for i in 1..=opts.count {
        let due = Duration::from_secs_f64(opts.every * i as f64);
        while start.elapsed() < due {
            pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        let modes = notice.show(i, Duration::from_millis(opts.hold))?;
        let summary: Vec<String> = modes.iter().map(|(m, n)| format!("{m} x{n}")).collect();
        println!("[overlay-plane] notice {i}: frames {}", summary.join(", "));
    }
    let _ = PCWSTR::null();
    let _ = BOOL(0);
    Ok(())
}

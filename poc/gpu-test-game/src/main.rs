//! Test fixture: a Direct3D 11 window that clears to one color every frame, on a
//! chosen adapter, so game capture can be tested across GPUs without a real game
//! (A13 of the stability tests). With the game on WARP (Microsoft's software
//! renderer) and the engine on a GPU, OBS cannot open the game's shared texture,
//! as on a laptop whose engine and game run on different GPUs.
//!
//! Usage: gpu-test-game [--adapter hardware|warp|N] [--seconds S] [--color RRGGBB]
//!                      [--size WxH] [--title TEXT]
//!   --adapter   hardware: the default GPU (default); warp: the software renderer;
//!               N: the DXGI adapter with that index (IDXGIFactory1::EnumAdapters1)
//!   --seconds   how long to run (default 60); closing the window also ends it
//!   --color     the clear color (default FF00FF)
//!   --size      client area (default 1280x720)
//!   --title     window title (default "Laurel GPU test game")

use std::time::{Duration, Instant};

use windows::core::{Interface, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, CreateDXGIFactory2, IDXGIDevice, IDXGIFactory1, IDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DispatchMessageW, LoadCursorW, PeekMessageW, PostQuitMessage, RegisterClassExW, TranslateMessage,
    CW_USEDEFAULT, IDC_ARROW, MSG, PM_REMOVE, WINDOW_EX_STYLE, WM_DESTROY, WM_QUIT, WNDCLASSEXW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

const CLASS: &str = "LaurelGpuTestGame";

struct Options {
    adapter: String,
    seconds: u64,
    color: [f32; 4],
    size: (u32, u32),
    title: String,
}

fn options() -> Options {
    let mut o = Options { adapter: "hardware".into(), seconds: 60, color: [1.0, 0.0, 1.0, 1.0], size: (1280, 720), title: "Laurel GPU test game".into() };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--adapter" => o.adapter = value,
            "--seconds" => o.seconds = value.parse().expect("bad --seconds"),
            "--color" => {
                let rgb = u32::from_str_radix(value.trim_start_matches('#'), 16).expect("bad --color");
                o.color = [(rgb >> 16 & 255) as f32 / 255.0, (rgb >> 8 & 255) as f32 / 255.0, (rgb & 255) as f32 / 255.0, 1.0];
            }
            "--size" => {
                let (w, h) = value.split_once('x').expect("bad --size");
                o.size = (w.parse().expect("bad --size"), h.parse().expect("bad --size"));
            }
            "--title" => o.title = value,
            other => panic!("unknown option {other}"),
        }
    }
    o
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The D3D11 device on the adapter asked for.
fn device(adapter: &str) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let (mut device, mut context) = (None, None);
    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
    unsafe {
        match adapter {
            "hardware" => D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, Default::default(), flags, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?,
            "warp" => D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, Default::default(), flags, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?,
            index => {
                let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
                let chosen = factory.EnumAdapters1(index.parse().expect("bad --adapter"))?;
                D3D11CreateDevice(&chosen, D3D_DRIVER_TYPE_UNKNOWN, Default::default(), flags, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?
            }
        }
    }
    Ok((device.expect("no device"), context.expect("no context")))
}

fn main() -> Result<()> {
    let o = options();
    let (device, context) = device(&o.adapter)?;
    let desc = unsafe { device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()? };
    let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
    println!(
        "[gpu-test-game] adapter {}: {} (luid {:08x}:{:08x}); pid {}",
        o.adapter,
        String::from_utf16_lossy(&desc.Description[..len]),
        desc.AdapterLuid.HighPart,
        desc.AdapterLuid.LowPart,
        unsafe { GetCurrentProcessId() }
    );

    let instance = unsafe { GetModuleHandleW(None)? };
    let class = HSTRING::from(CLASS);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: instance.into(),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&wc) };
    let mut frame = RECT { left: 0, top: 0, right: o.size.0 as i32, bottom: o.size.1 as i32 };
    unsafe { AdjustWindowRect(&mut frame, WS_OVERLAPPEDWINDOW, false)? };
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            &class,
            &HSTRING::from(o.title.as_str()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            frame.right - frame.left,
            frame.bottom - frame.top,
            None,
            None,
            Some(instance.into()),
            None,
        )?
    };
    println!("[gpu-test-game] window \"{}\" class {CLASS}, {}x{}", o.title, o.size.0, o.size.1);

    let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
    let chain_desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: o.size.0,
        Height: o.size.1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        ..Default::default()
    };
    let swapchain = unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &chain_desc, None, None)? };
    let buffer: ID3D11Texture2D = unsafe { swapchain.GetBuffer(0)? };
    let mut target: Option<ID3D11RenderTargetView> = None;
    unsafe { device.CreateRenderTargetView(&buffer, None, Some(&mut target))? };
    let target = target.expect("no render target");

    let end = Instant::now() + Duration::from_secs(o.seconds);
    let mut frames = 0u64;
    let mut msg = MSG::default();
    'run: while Instant::now() < end {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    break 'run;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            context.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            context.ClearRenderTargetView(&target, &o.color);
            swapchain.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        frames += 1;
    }
    println!("[gpu-test-game] {frames} frames presented");
    Ok(())
}

//! The GPU libobs renders on (A13 of the stability tests).
//!
//! libobs takes a DXGI adapter index (the order of IDXGIFactory1::EnumAdapters1).
//! Adapter 0 is the GPU that drives the display: on a laptop with two GPUs that is
//! usually the integrated one, while games run on the high-performance one. A game
//! capture has to open the game's shared texture on OBS's own GPU, and texture
//! encoders (NVENC, AMF) need OBS on their GPU, so on such a PC the engine starts on
//! the high-performance GPU, where Windows and the drivers put games by default.

use serde_json::{json, Value};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIFactory6, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
};
use windows::core::Interface;

#[derive(Clone, Debug)]
pub struct Adapter {
    /// Index for libobs (obs_video_info.adapter).
    pub index: u32,
    pub name: String,
    luid: (u32, i32),
    /// Microsoft Basic Render Driver (WARP), which DXGI lists after the GPUs.
    pub software: bool,
}

/// The adapter the engine renders on, and why it was chosen.
#[derive(Clone, Debug)]
pub struct Choice {
    pub adapter: Option<Adapter>,
    pub index: u32,
    pub reason: &'static str,
    pub gpus: usize,
}

impl Choice {
    pub fn json(&self) -> Value {
        json!({
            "index": self.index,
            "name": self.adapter.as_ref().map(|a| a.name.clone()),
            "reason": self.reason,
            "gpus": self.gpus,
        })
    }
}

/// Every adapter in libobs' order.
pub fn adapters() -> Vec<Adapter> {
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return vec![] };
    let mut list = vec![];
    for index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else { break };
        if let Some(found) = describe(index, &adapter) {
            list.push(found);
        }
    }
    list
}

fn describe(index: u32, adapter: &IDXGIAdapter1) -> Option<Adapter> {
    let desc = unsafe { adapter.GetDesc1() }.ok()?;
    let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
    Some(Adapter {
        index,
        name: String::from_utf16_lossy(&desc.Description[..len]),
        luid: (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart),
        software: desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
    })
}

/// The GPU Windows ranks first for performance (IDXGIFactory6, Windows 10 1803+).
fn high_performance() -> Option<(u32, i32)> {
    let factory = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }.ok()?;
    let factory6 = factory.cast::<IDXGIFactory6>().ok()?;
    let adapter: IDXGIAdapter1 = unsafe { factory6.EnumAdapterByGpuPreference(0, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) }.ok()?;
    describe(0, &adapter).map(|a| a.luid)
}

/// The adapter to render on: the one asked for (`--adapter`), else, with more than
/// one GPU, the high-performance one, else adapter 0 as before.
pub fn choose(requested: Option<u32>) -> Choice {
    let list = adapters();
    let gpus = list.iter().filter(|a| !a.software).count();
    let at = |index: u32| list.iter().find(|a| a.index == index).cloned();
    if let Some(index) = requested {
        return Choice { adapter: at(index), index, reason: "asked", gpus };
    }
    if gpus > 1 {
        if let Some(fast) = high_performance().and_then(|luid| list.iter().find(|a| a.luid == luid && !a.software)) {
            return Choice { adapter: Some(fast.clone()), index: fast.index, reason: "high performance", gpus };
        }
    }
    Choice { adapter: at(0), index: 0, reason: if gpus > 1 { "default" } else { "only GPU" }, gpus }
}

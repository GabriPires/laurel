//! The choice of the video encoder (spec quality.encoding): which of the encoders
//! OBS registered on this PC the engine tries, in which order, and the family the
//! app shows for each. OBS registers only the encoders whose GPU it found when it
//! loaded its plugins.

/// Video encoders in the order auto tries them: the texture encoders of NVIDIA, AMD
/// and Intel GPUs, which take the frames on the GPU, then NVENC's copy encoder, then
/// the CPU. HEVC and AV1 come before H.264, which is the fallback for every codec.
const H264: &[&str] = &["obs_nvenc_h264_tex", "h264_texture_amf", "obs_qsv11_v2", "obs_nvenc_h264_soft", "obs_x264"];
const HEVC: &[&str] = &["obs_nvenc_hevc_tex", "h265_texture_amf", "obs_qsv11_hevc", "obs_nvenc_hevc_soft"];
const AV1: &[&str] = &["obs_nvenc_av1_tex", "av1_texture_amf", "obs_qsv11_av1", "obs_nvenc_av1_soft"];

/// The encoder of a family (the choice the app offers: "nvenc", "amf", "qsv" or
/// "x264") for a codec ("h264", "hevc" or "av1"); None for anything else.
pub fn family_encoder(family: &str, codec: &str) -> Option<&'static str> {
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
pub fn encoder_family(id: &str) -> &'static str {
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

/// The encoders to try: the one asked for (an id, a family with the codec, or
/// "auto" for none), then the codec's list, then H.264's, keeping only those OBS
/// `registered` on this PC, each once.
pub fn candidates(requested: &str, codec: &str, registered: &[&str]) -> Vec<String> {
    let mut order: Vec<&str> = Vec::new();
    match family_encoder(requested, codec) {
        Some(id) => order.push(id),
        None if requested != "auto" => order.push(requested),
        None => {}
    }
    match codec {
        "hevc" => order.extend(HEVC),
        "av1" => order.extend(AV1),
        _ => {}
    }
    order.extend(H264);
    let mut out: Vec<String> = Vec::new();
    for id in order {
        if registered.contains(&id) && !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    }
    out
}

/// The encoders the app can offer, of those OBS `registered`: H.264's in the order
/// auto tries them, then HEVC's, then AV1's.
pub fn offered(registered: &[&str]) -> Vec<&'static str> {
    H264.iter().chain(HEVC).chain(AV1).copied().filter(|id| registered.contains(id)).collect()
}

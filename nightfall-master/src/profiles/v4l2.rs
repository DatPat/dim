use super::ProfileContext;
use super::ProfileType;
use super::StreamType;
use super::TranscodingProfile;

use crate::NightfallError;

/// Check whether any V4L2 M2M (memory-to-memory) video encoder device exists.
/// V4L2 M2M encoders appear as /dev/video0, /dev/video1, etc. (numeric suffix).
/// Note: /dev/media* are media controller devices (ISPs, etc.) and NOT M2M
/// encoders.  Named devices like /dev/video-cixdec0 are typically decoders.
fn v4l2m2m_device_exists() -> bool {
    let entries = match std::fs::read_dir("/dev") {
        Ok(e) => e,
        Err(_) => return false,
    };
    entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Only /dev/video<N> (numeric suffix) — these are the M2M device nodes
        // that ffmpeg's h264_v4l2m2m/hevc_v4l2m2m actually scan for.
        name.starts_with("video") && name[5..].parse::<u32>().is_ok()
    })
}

/// Check whether a V4L2 M2M hardware decoder is available for the given codec.
/// Named devices like /dev/video-cixdec0 are typically decoders on ARM SoCs.
pub fn v4l2m2m_decoder_exists() -> bool {
    let entries = match std::fs::read_dir("/dev") {
        Ok(e) => e,
        Err(_) => return false,
    };
    entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Named devices containing "dec" (e.g. /dev/video-cixdec0) are decoders.
        // Also check for /dev/video<N> which can be either encoder or decoder.
        name.starts_with("video") && name.contains("dec")
    })
}

/// Shared builder for V4L2 M2M ffmpeg arguments.
fn build_v4l2_args(ctx: ProfileContext, encoder: &str, extra_args: &[&str]) -> Option<Vec<String>> {
    let start_num = ctx.output_ctx.start_num.to_string();
    let stream = format!("0:{}", ctx.input_ctx.stream);
    let init_seg = format!("{}_init.mp4", &start_num);
    let seg_name = format!("{}/%d.m4s", ctx.output_ctx.outdir);
    let outdir = format!("{}/playlist.m3u8", ctx.output_ctx.outdir);

    let mut args = Vec::new();

    // V4L2 M2M decoders typically support h264/hevc only.  Use HW decode
    // for those codecs when available; for everything else (av1, etc.) fall
    // back to software decode while still using V4L2 HW encode.
    let v4l2_decode_codecs = ["h264", "hevc"];
    let use_hw_decode = !ctx.force_software_decode
        && v4l2m2m_decoder_exists()
        && v4l2_decode_codecs.contains(&ctx.input_ctx.codec.as_str());

    if use_hw_decode {
        args.push("-hwaccel".into());
        args.push("v4l2m2m".into());
    } else if ctx.input_ctx.codec == "av1" {
        args.push("-c:v".into());
        args.push("libdav1d".into());
    }

    args.append(&mut vec![
        "-y".into(),
        "-ss".into(),
        (ctx.output_ctx.start_num * ctx.output_ctx.target_gop).to_string(),
        "-i".into(),
        ctx.file.clone(),
        "-copyts".into(),
        "-map".into(),
        stream,
        "-c:0".into(),
        encoder.into(),
    ]);

    for arg in extra_args {
        args.push((*arg).into());
    }

    // V4L2 M2M encoders need an explicit pixel format — raw RGB or 10-bit
    // inputs cause VIDIOC_STREAMON failures.  Both yuv420p and nv12 work on
    // tested hardware (Linlon MVX); yuv420p is marginally faster.
    // Width is aligned to 16 pixels to satisfy hardware alignment requirements.
    // Cap height at 720 — some V4L2 drivers (e.g. Linlon MVX) crash at 1080p
    // due to driver-level memory corruption bugs.
    let max_v4l2_height = 720i64;
    let height = ctx
        .output_ctx
        .height
        .map(|h| h.min(max_v4l2_height))
        .unwrap_or(max_v4l2_height);
    args.push("-vf".into());
    args.push(format!("scale=trunc(oh*a/16)*16:{},format=nv12", height));

    if let Some(bitrate) = ctx.output_ctx.bitrate {
        args.push("-b:v".into());
        args.push(bitrate.to_string());
    }

    args.append(&mut vec![
        "-start_at_zero".into(),
        "-fps_mode".into(),
        "passthrough".into(),
        "-avoid_negative_ts".into(),
        "disabled".into(),
        "-max_muxing_queue_size".into(),
        "2048".into(),
        "-keyint_min".into(),
        "120".into(),
        "-g".into(),
        "120".into(),
        "-frag_duration".into(),
        "5000000".into(),
    ]);

    args.append(&mut super::video::get_discont_flags(&ctx));

    args.append(&mut vec![
        "-f".into(),
        "hls".into(),
        "-start_number".into(),
        start_num,
    ]);

    args.append(&mut vec![
        "-hls_flags".into(),
        "independent_segments+temp_file".into(),
        "-max_delay".into(),
        "5000000".into(),
    ]);

    args.append(&mut vec!["-hls_fmp4_init_filename".into(), init_seg]);

    args.append(&mut vec![
        "-hls_time".into(),
        ctx.output_ctx.target_gop.to_string(),
    ]);

    args.append(&mut vec![
        "-force_key_frames".into(),
        format!("expr:gte(t,n_forced*{})", ctx.output_ctx.target_gop),
    ]);

    args.append(&mut vec!["-hls_segment_type".into(), 1.to_string()]);
    args.append(&mut vec![
        "-loglevel".into(),
        "info".into(),
        "-progress".into(),
        "pipe:1".into(),
    ]);
    args.append(&mut vec!["-hls_segment_filename".into(), seg_name]);
    args.push(outdir);

    Some(args)
}

/// V4L2 M2M H.264 transcoding profile.
/// Uses h264_v4l2m2m encoder, commonly available on ARM SoCs.
#[cfg(unix)]
#[derive(Debug)]
pub struct V4l2TranscodeProfile;

#[cfg(unix)]
impl TranscodingProfile for V4l2TranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "V4l2TranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        if !v4l2m2m_device_exists() {
            return Err(NightfallError::ProfileNotSupported(
                "No V4L2 M2M video device found in /dev".into(),
            ));
        }
        Ok(())
    }

    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>> {
        // No -bf flag — V4L2 M2M encoders handle B-frames internally
        // (most don't support them at all)
        build_v4l2_args(ctx, "h264_v4l2m2m", &[])
    }

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec != "h264" {
            return Err(NightfallError::ProfileNotSupported(
                "Profile only supports h264 output streams.".into(),
            ));
        }
        Ok(())
    }

    fn tag(&self) -> &str {
        "h264_v4l2"
    }
}

/// V4L2 M2M HEVC transcoding profile.
/// Uses hevc_v4l2m2m encoder for H.265 output.
#[cfg(unix)]
#[derive(Debug)]
pub struct V4l2HevcTranscodeProfile;

#[cfg(unix)]
impl TranscodingProfile for V4l2HevcTranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "V4l2HevcTranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        if !v4l2m2m_device_exists() {
            return Err(NightfallError::ProfileNotSupported(
                "No V4L2 M2M video device found in /dev".into(),
            ));
        }
        Ok(())
    }

    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>> {
        build_v4l2_args(ctx, "hevc_v4l2m2m", &["-tag:v", "hvc1"])
    }

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec != "h265" {
            return Err(NightfallError::ProfileNotSupported(
                "Profile only supports h265 output streams.".into(),
            ));
        }
        Ok(())
    }

    fn tag(&self) -> &str {
        "hevc_v4l2"
    }
}

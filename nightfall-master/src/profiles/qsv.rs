use super::ProfileContext;
use super::ProfileType;
use super::StreamType;
use super::TranscodingProfile;

use crate::NightfallError;

/// Codecs Intel Quick Sync can decode. When `-hwaccel qsv` is emitted for
/// anything else, ffmpeg fails at startup and the session burns a fallback
/// for nothing.
fn check_qsv_input(ctx: &ProfileContext) -> Result<(), NightfallError> {
    const QSV_DECODE_CODECS: &[&str] = &[
        "h264",
        "hevc",
        "av1",
        "vp8",
        "vp9",
        "mpeg2video",
        "vc1",
        "mjpeg",
    ];

    if !ctx.force_software_decode && !QSV_DECODE_CODECS.contains(&ctx.input_ctx.codec.as_str()) {
        return Err(NightfallError::ProfileNotSupported(format!(
            "Input codec {} cannot be decoded by Quick Sync.",
            ctx.input_ctx.codec
        )));
    }

    Ok(())
}

/// Check that QSV hardware is likely available.
/// QSV requires Intel Media SDK / oneVPL which is Linux (and Windows) only.
fn qsv_is_available() -> Result<(), NightfallError> {
    if cfg!(target_os = "macos") {
        return Err(NightfallError::ProfileNotSupported(
            "QSV is not available on macOS".into(),
        ));
    }

    // On Linux, check for a DRI render node (Intel GPU).
    #[cfg(target_os = "linux")]
    {
        let render_nodes: Vec<_> = std::fs::read_dir("/dev/dri")
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|s| s.starts_with("renderD"))
                    .unwrap_or(false)
            })
            .collect();

        if render_nodes.is_empty() {
            return Err(NightfallError::ProfileNotSupported(
                "No DRI render node found in /dev/dri".into(),
            ));
        }
    }

    Ok(())
}

/// Intel QuickSync Video h264 transcoding profile.
#[derive(Debug)]
pub struct QsvTranscodeProfile;

impl TranscodingProfile for QsvTranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "QsvTranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        qsv_is_available()
    }

    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>> {
        build_qsv_args(ctx, "h264_qsv", None)
    }

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec != "h264" {
            return Err(NightfallError::ProfileNotSupported(
                "Profile only supports h264 output streams.".into(),
            ));
        }
        check_qsv_input(ctx)?;
        Ok(())
    }

    fn tag(&self) -> &str {
        "h264_qsv"
    }
}

/// Intel QuickSync Video HEVC transcoding profile.
#[derive(Debug)]
pub struct QsvHevcTranscodeProfile;

impl TranscodingProfile for QsvHevcTranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "QsvHevcTranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        qsv_is_available()
    }

    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>> {
        build_qsv_args(ctx, "hevc_qsv", Some(("hvc1",)))
    }

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec != "h265" {
            return Err(NightfallError::ProfileNotSupported(
                "Profile only supports h265 output streams.".into(),
            ));
        }
        check_qsv_input(ctx)?;
        Ok(())
    }

    fn tag(&self) -> &str {
        "hevc_qsv"
    }
}

/// Intel QuickSync Video AV1 transcoding profile.
#[derive(Debug)]
pub struct QsvAv1TranscodeProfile;

impl TranscodingProfile for QsvAv1TranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "QsvAv1TranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        qsv_is_available()
    }

    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>> {
        build_qsv_args(ctx, "av1_qsv", None)
    }

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec != "av1" {
            return Err(NightfallError::ProfileNotSupported(
                "Profile only supports av1 output streams.".into(),
            ));
        }
        check_qsv_input(ctx)?;
        Ok(())
    }

    fn tag(&self) -> &str {
        "av1_qsv"
    }
}

/// Shared builder for all QSV profiles.
///
/// Uses `-init_hw_device` + `hwupload` so that QSV encoding works regardless of
/// whether the GPU can hardware-decode the input codec.  If QSV decoding IS
/// available ffmpeg will still use it via `-hwaccel auto`.
///
/// `tag_v` is an optional `-tag:v` value (used for HEVC → hvc1).
fn build_qsv_args(
    ctx: ProfileContext,
    encoder: &str,
    tag_v: Option<(&str,)>,
) -> Option<Vec<String>> {
    let start_num = ctx.output_ctx.start_num.to_string();
    let stream = format!("0:{}", ctx.input_ctx.stream);
    let init_seg = format!("{}_init.mp4", &start_num);
    let seg_name = format!("{}/%d.m4s", ctx.output_ctx.outdir);
    let outdir = format!("{}/playlist.m3u8", ctx.output_ctx.outdir);

    let mut args = vec![
        "-init_hw_device".into(),
        "qsv=hw".into(),
        "-filter_hw_device".into(),
        "hw".into(),
    ];

    if !ctx.force_software_decode {
        // Full HW pipeline: QSV decode → QSV surfaces → QSV encode.
        // If the input codec can't be HW decoded, ffmpeg will fail and the
        // profile chain falls back to the next profile automatically.
        args.push("-hwaccel".into());
        args.push("qsv".into());
        args.push("-hwaccel_output_format".into());
        args.push("qsv".into());
    }

    if ctx.force_software_decode && ctx.input_ctx.codec == "av1" {
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

    if let Some((tag,)) = tag_v {
        args.push("-tag:v".into());
        args.push(tag.into());
    }

    args.push("-bf".into());
    args.push("0".into());

    // Build the video filter chain.
    // When HW decode is active, frames are already QSV surfaces — use
    // scale_qsv directly (no CPU round-trip).  When SW decode is used,
    // frames are in CPU memory and need format conversion + hwupload.
    // NOTE: scale_qsv only accepts -1 for "keep aspect ratio" (not -2 like
    // the software scale filter).
    if ctx.force_software_decode {
        let mut vf_parts = Vec::new();
        vf_parts.push("format=nv12".to_string());
        vf_parts.push("hwupload=extra_hw_frames=64".to_string());
        if let Some(height) = ctx.output_ctx.height {
            let width = ctx.output_ctx.width.unwrap_or(-1);
            vf_parts.push(format!("scale_qsv=w={}:h={}", width, height));
        }
        args.push("-vf".into());
        args.push(vf_parts.join(","));
    } else if let Some(height) = ctx.output_ctx.height {
        let width = ctx.output_ctx.width.unwrap_or(-1);
        args.push("-vf".into());
        args.push(format!("scale_qsv=w={}:h={}", width, height));
    }

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

use super::ProfileContext;
use super::ProfileType;
use super::StreamType;
use super::TranscodingProfile;

use crate::NightfallError;

/// AMD AMF h264 transcoding profile (Windows).
#[derive(Debug)]
pub struct AmfTranscodeProfile;

impl TranscodingProfile for AmfTranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "AmfTranscodeProfile"
    }

    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>> {
        build_amf_args(ctx, "h264_amf", None)
    }

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec != "h264" {
            return Err(NightfallError::ProfileNotSupported(format!(
                "Got output codec {} but profile only supports `h264`.",
                ctx.output_ctx.codec
            )));
        }
        Ok(())
    }

    fn tag(&self) -> &str {
        "h264_amf"
    }
}

/// AMD AMF HEVC transcoding profile (Windows).
#[derive(Debug)]
pub struct AmfHevcTranscodeProfile;

impl TranscodingProfile for AmfHevcTranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "AmfHevcTranscodeProfile"
    }

    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>> {
        build_amf_args(ctx, "hevc_amf", Some("hvc1"))
    }

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec != "h265" {
            return Err(NightfallError::ProfileNotSupported(format!(
                "Got output codec {} but profile only supports `h265`.",
                ctx.output_ctx.codec
            )));
        }
        Ok(())
    }

    fn tag(&self) -> &str {
        "hevc_amf"
    }
}

/// AMD AMF AV1 transcoding profile (Windows).
#[derive(Debug)]
pub struct AmfAv1TranscodeProfile;

impl TranscodingProfile for AmfAv1TranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "AmfAv1TranscodeProfile"
    }

    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>> {
        build_amf_args(ctx, "av1_amf", None)
    }

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec != "av1" {
            return Err(NightfallError::ProfileNotSupported(format!(
                "Got output codec {} but profile only supports `av1`.",
                ctx.output_ctx.codec
            )));
        }
        Ok(())
    }

    fn tag(&self) -> &str {
        "av1_amf"
    }
}

/// Shared builder for all AMF profiles.
///
/// AMF encoders accept CPU frames directly (the driver uploads them
/// internally), so decode is left to ffmpeg's defaults and scaling happens
/// with the software `scale` filter.
///
/// `tag_v` is an optional `-tag:v` value (used for HEVC → hvc1).
fn build_amf_args(ctx: ProfileContext, encoder: &str, tag_v: Option<&str>) -> Option<Vec<String>> {
    let start_num = ctx.output_ctx.start_num.to_string();
    let stream = format!("0:{}", ctx.input_ctx.stream);
    let init_seg = format!("{}_init.mp4", &start_num);
    let seg_name = format!("{}/%d.m4s", ctx.output_ctx.outdir);
    let outdir = format!("{}/playlist.m3u8", ctx.output_ctx.outdir);

    let mut args = vec![
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
    ];

    if let Some(tag) = tag_v {
        args.push("-tag:v".into());
        args.push(tag.into());
    }

    args.push("-bf".into());
    args.push("0".into());

    if let Some(height) = ctx.output_ctx.height {
        let width = ctx.output_ctx.width.unwrap_or(-2);
        args.push("-vf".into());
        args.push(format!("scale={}:{},format=nv12", width, height));
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
    ]);

    args.append(&mut vec![
        "-f".into(),
        "hls".into(),
        "-start_number".into(),
        start_num,
    ]);

    // needed so that in progress segments are named `tmp` and then renamed after the data is
    // on disk.
    // This in theory practically prevents the web server from returning a segment that is
    // in progress.
    args.append(&mut vec![
        "-hls_flags".into(),
        "temp_file".into(),
        "-max_delay".into(),
        "5000000".into(),
    ]);

    args.append(&mut super::video::get_discont_flags(&ctx));

    // args needed so we can distinguish between init fragments for new streams.
    // Basically on the web seeking works by reloading the entire video because of
    // discontinuity issues that browsers seem to not ignore like mpv.
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

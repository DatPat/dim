use super::video::get_discont_flags;
use super::ProfileContext;
use super::ProfileType;
use super::StreamType;
use super::TranscodingProfile;

use crate::NightfallError;

/// VP9 transmux (direct-play) profile: stream-copies VP9 video into fMP4/HLS
/// segments (CMAF-style vp09 sample entries), which Chromium and Firefox play
/// via MSE. Without this, VP9 sources could only be software-transcoded.
#[derive(Debug)]
pub struct Vp9TransmuxProfile;

impl TranscodingProfile for Vp9TransmuxProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::Transmux
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "Vp9TransmuxProfile"
    }

    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>> {
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
            "copy".into(),
        ];

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

        args.append(&mut vec![
            "-hls_flags".into(),
            "temp_file".into(),
            "-max_delay".into(),
            "5000000".into(),
        ]);

        args.append(&mut vec!["-hls_fmp4_init_filename".into(), init_seg]);

        args.append(&mut vec![
            "-hls_time".into(),
            ctx.output_ctx.target_gop.to_string(),
        ]);

        args.append(&mut get_discont_flags(&ctx));

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

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.height.is_some()
            || ctx.output_ctx.width.is_some()
            || ctx.output_ctx.bitrate.is_some()
        {
            return Err(NightfallError::ProfileNotSupported(
                "Transmuxed streams cannot be resized.".into(),
            ));
        }

        if ctx.input_ctx.codec == "vp9" && ctx.output_ctx.codec == "vp9" {
            return Ok(());
        }

        Err(NightfallError::ProfileNotSupported(
            "Profile only supports vp9 input and output codecs.".into(),
        ))
    }

    fn tag(&self) -> &str {
        "vp9_copy"
    }
}

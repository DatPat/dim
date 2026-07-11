use std::process::Command;
use std::sync::OnceLock;

use super::video::get_discont_flags;
use super::ProfileContext;
use super::ProfileType;
use super::StreamType;
use super::TranscodingProfile;

use crate::NightfallError;

fn svtav1_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new(super::ffmpeg_bin())
            .args(["-hide_banner", "-encoders"])
            .output()
            .map(|out| {
                String::from_utf8_lossy(&out.stdout).contains("libsvtav1")
            })
            .unwrap_or(false)
    })
}

#[derive(Debug)]
pub struct Av1TransmuxProfile;

impl TranscodingProfile for Av1TransmuxProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::Transmux
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "Av1TransmuxProfile"
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

        if ctx.input_ctx.codec == "av1" && ctx.output_ctx.codec == "av1" {
            return Ok(());
        }

        Err(NightfallError::ProfileNotSupported(
            "Profile only supports av1 input and output codecs.".into(),
        ))
    }

    fn tag(&self) -> &str {
        "av1_copy"
    }
}

#[derive(Debug)]
pub struct Av1TranscodeProfile;

impl TranscodingProfile for Av1TranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::Transcode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "Av1TranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        if !svtav1_available() {
            return Err(NightfallError::ProfileNotSupported(
                "ffmpeg was not built with libsvtav1 (AV1 software encoder)".into(),
            ));
        }
        Ok(())
    }

    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>> {
        let start_num = ctx.output_ctx.start_num.to_string();
        let stream = format!("0:{}", ctx.input_ctx.stream);
        let init_seg = format!("{}_init.mp4", &start_num);
        let seg_name = format!("{}/%d.m4s", ctx.output_ctx.outdir);
        let outdir = format!("{}/playlist.m3u8", ctx.output_ctx.outdir);

        let mut args = vec!["-y".into()];

        if ctx.input_ctx.codec == "av1" {
            args.push("-c:v".into());
            args.push("libdav1d".into());
        }

        args.append(&mut vec![
            "-ss".into(),
            (ctx.output_ctx.start_num * ctx.output_ctx.target_gop).to_string(),
            "-i".into(),
            ctx.file.clone(),
            "-copyts".into(),
            "-map".into(),
            stream,
            "-c:0".into(),
            "libsvtav1".into(),
            "-preset".into(),
            "8".into(),
            "-svtav1-params".into(),
            "tile-columns=2:tile-rows=1".into(),
        ]);

        if let Some(height) = ctx.output_ctx.height {
            let width = ctx.output_ctx.width.unwrap_or(-2);
            args.push("-vf".into());
            args.push(format!("scale={}:{},format=yuv420p", width, height));
        } else {
            args.push("-pix_fmt".into());
            args.push("yuv420p".into());
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

        args.append(&mut get_discont_flags(&ctx));

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
        if ctx.output_ctx.codec == "av1" {
            return Ok(());
        }

        Err(NightfallError::ProfileNotSupported(format!(
            "Got output codec {} but profile only supports `av1`.",
            ctx.output_ctx.codec
        )))
    }

    fn tag(&self) -> &str {
        "av1"
    }
}

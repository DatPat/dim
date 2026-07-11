use super::ProfileContext;
use super::ProfileType;
use super::StreamType;
use super::TranscodingProfile;

use crate::NightfallError;

/// Codecs NVDEC can decode. When `-hwaccel cuda -hwaccel_output_format cuda`
/// is emitted for anything else, ffmpeg fails at startup and the session
/// burns a fallback for nothing.
fn check_nvdec_input(ctx: &ProfileContext) -> Result<(), NightfallError> {
    const NVDEC_CODECS: &[&str] = &[
        "h264",
        "hevc",
        "av1",
        "vp8",
        "vp9",
        "mpeg1video",
        "mpeg2video",
        "mpeg4",
        "vc1",
        "mjpeg",
    ];

    if !ctx.force_software_decode && !NVDEC_CODECS.contains(&ctx.input_ctx.codec.as_str()) {
        return Err(NightfallError::ProfileNotSupported(format!(
            "Input codec {} cannot be decoded by NVDEC.",
            ctx.input_ctx.codec
        )));
    }

    Ok(())
}

fn nvidia_device_exists() -> bool {
    match std::fs::read_dir("/dev") {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .any(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("nvidia") && name[6..].parse::<u32>().is_ok()
            }),
        Err(_) => false,
    }
}

/// Cuda(NVENC/NVDEC) transcoding profiles.
/// This is a nvidia exclusive transcoding profile that leverages cuda. This profile will
/// automatically be enabled if any of your GPUs support encoding and decoding h264 with the
/// profiles `Main`, `High` and `ConstrainedBaseline`. This profile will only transcode h264 input
/// streams.
#[cfg(unix)]
#[derive(Debug)]
pub struct CudaTranscodeProfile;

#[cfg(unix)]
impl TranscodingProfile for CudaTranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "CudaTranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        if !nvidia_device_exists() {
            return Err(NightfallError::ProfileNotSupported(
                "No NVIDIA GPU found in /dev".into(),
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

        // ffmpeg -hwaccel cuda -hwaccel_output_format cuda -i input -c:v h264_nvenc -preset slow output
        let mut args = Vec::new();

        if !ctx.force_software_decode {
            args.push("-hwaccel".into());
            args.push("cuda".into());
            args.push("-hwaccel_output_format".into());
            args.push("cuda".into());
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
            "h264_nvenc".into(),
            "-bf".into(),
            "0".into(),
        ]);

        if ctx.force_software_decode {
            // Software decode → need to upload frames to GPU for NVENC
            if let Some(height) = ctx.output_ctx.height {
                let width = ctx.output_ctx.width.unwrap_or(-2);
                args.push("-vf".into());
                args.push(format!("format=nv12,scale={}:{},hwupload_cuda", width, height));
            } else {
                args.push("-vf".into());
                args.push("format=nv12,hwupload_cuda".into());
            }
        } else if let Some(height) = ctx.output_ctx.height {
            let width = ctx.output_ctx.width.unwrap_or(-2); // defaults to scaling by 2
            args.push("-vf".into());
            args.push(format!("scale_cuda={}:{}", width, height));
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

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec != "h264" {
            return Err(NightfallError::ProfileNotSupported(
                "Profile only supports h264 output streams.".into(),
            ));
        }
        // TODO: At runtime check which file formats are supported by the current gpu for enc/dec.
        check_nvdec_input(ctx)?;
        Ok(())
    }

    fn tag(&self) -> &str {
        "h264_cuda"
    }
}

/// Cuda(NVENC/NVDEC) HEVC transcoding profile.
/// This profile leverages NVENC to encode h265/HEVC output streams.
#[cfg(unix)]
#[derive(Debug)]
pub struct CudaHevcTranscodeProfile;

#[cfg(unix)]
impl TranscodingProfile for CudaHevcTranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "CudaHevcTranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        if !nvidia_device_exists() {
            return Err(NightfallError::ProfileNotSupported(
                "No NVIDIA GPU found in /dev".into(),
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

        let mut args = Vec::new();

        if !ctx.force_software_decode {
            args.push("-hwaccel".into());
            args.push("cuda".into());
            args.push("-hwaccel_output_format".into());
            args.push("cuda".into());
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
            "hevc_nvenc".into(),
            "-tag:v".into(),
            "hvc1".into(),
            "-bf".into(),
            "0".into(),
        ]);

        if ctx.force_software_decode {
            if let Some(height) = ctx.output_ctx.height {
                let width = ctx.output_ctx.width.unwrap_or(-2);
                args.push("-vf".into());
                args.push(format!("format=nv12,scale={}:{},hwupload_cuda", width, height));
            } else {
                args.push("-vf".into());
                args.push("format=nv12,hwupload_cuda".into());
            }
        } else if let Some(height) = ctx.output_ctx.height {
            let width = ctx.output_ctx.width.unwrap_or(-2);
            args.push("-vf".into());
            args.push(format!("scale_cuda={}:{}", width, height));
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

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec != "h265" {
            return Err(NightfallError::ProfileNotSupported(
                "Profile only supports h265 output streams.".into(),
            ));
        }
        check_nvdec_input(ctx)?;
        Ok(())
    }

    fn tag(&self) -> &str {
        "hevc_cuda"
    }
}

/// Cuda(NVENC/NVDEC) AV1 transcoding profile.
/// This profile leverages NVENC to encode AV1 output streams.
#[cfg(unix)]
#[derive(Debug)]
pub struct CudaAv1TranscodeProfile;

#[cfg(unix)]
impl TranscodingProfile for CudaAv1TranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "CudaAv1TranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        if !nvidia_device_exists() {
            return Err(NightfallError::ProfileNotSupported(
                "No NVIDIA GPU found in /dev".into(),
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

        let mut args = Vec::new();

        if !ctx.force_software_decode {
            args.push("-hwaccel".into());
            args.push("cuda".into());
            args.push("-hwaccel_output_format".into());
            args.push("cuda".into());
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
            "av1_nvenc".into(),
            "-bf".into(),
            "0".into(),
        ]);

        if ctx.force_software_decode {
            if let Some(height) = ctx.output_ctx.height {
                let width = ctx.output_ctx.width.unwrap_or(-2);
                args.push("-vf".into());
                args.push(format!("format=nv12,scale={}:{},hwupload_cuda", width, height));
            } else {
                args.push("-vf".into());
                args.push("format=nv12,hwupload_cuda".into());
            }
        } else if let Some(height) = ctx.output_ctx.height {
            let width = ctx.output_ctx.width.unwrap_or(-2);
            args.push("-vf".into());
            args.push(format!("scale_cuda={}:{}", width, height));
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

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec != "av1" {
            return Err(NightfallError::ProfileNotSupported(
                "Profile only supports av1 output streams.".into(),
            ));
        }
        check_nvdec_input(ctx)?;
        Ok(())
    }

    fn tag(&self) -> &str {
        "av1_cuda"
    }
}

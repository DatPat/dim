use super::ProfileContext;
use super::ProfileType;
use super::StreamType;
use super::TranscodingProfile;

use crate::NightfallError;

use std::fs;
use std::path::PathBuf;

/// Discover the first usable VAAPI DRI render node, returning its profiles, vendor string,
/// and device path.
fn discover_vaapi_device() -> Option<(Vec<rusty_vainfo::Profile>, String, PathBuf)> {
    let hw_targets = fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(Result::ok)
        .filter(|x| x.file_name().to_string_lossy().find("render").is_some())
        .map(|x| x.path())
        .collect::<Vec<_>>();

    for target in hw_targets {
        if let Ok(x) = rusty_vainfo::VaInstance::with_drm(&target) {
            return Some((
                x.profiles().unwrap_or_default(),
                x.vendor_string(),
                target,
            ));
        }
    }

    Some((Vec::new(), "<null_device>".into(), PathBuf::new()))
}

/// Validate that the VAAPI device can hardware-decode the input stream: the
/// input codec+profile must map to a VA profile the device exposes with the
/// VLD (decode) entrypoint. Skipped entirely when software decode is forced.
fn check_vaapi_input(
    device_profiles: &[rusty_vainfo::Profile],
    ctx: &ProfileContext,
) -> Result<(), NightfallError> {
    if ctx.force_software_decode {
        return Ok(());
    }

    let va_profile = match [ctx.input_ctx.codec.as_str(), ctx.input_ctx.profile.as_str()] {
        ["h264", "High"] => "VAProfileH264High",
        ["h264", "Main"] => "VAProfileH264Main",
        ["h264", "Baseline"] => "VAProfileH264Baseline",
        ["h264", "Constrained Baseline"] => "VAProfileH264ConstrainedBaseline",
        ["hevc", "Main"] => "VAProfileHEVCMain",
        ["hevc", "Main 10"] => "VAProfileHEVCMain10",
        ["av1", _] => "VAProfileAV1Profile0",
        // 8-bit VP9 only: Profile 2 (10-bit) would need a P010 hwdownload
        // path, so it falls through to software transcode instead.
        ["vp9", "Profile 0"] => "VAProfileVP9Profile0",
        [codec, profile] => {
            return Err(NightfallError::ProfileNotSupported(format!(
                "Input {} ({}) cannot be hardware-decoded via VAAPI.",
                codec, profile
            )))
        }
    };

    let decode_entrypoint = "VAEntrypointVLD".to_string();
    if !device_profiles
        .iter()
        .any(|x| x.name == va_profile && x.entrypoints.contains(&decode_entrypoint))
    {
        return Err(NightfallError::ProfileNotSupported(format!(
            "HW Acceleration device doesnt support decoding {} content (needs {}).",
            ctx.input_ctx.codec, va_profile
        )));
    }

    Ok(())
}

/// Vaapi transcoding profiles.
/// This is a unix exclusive transcoding profile that leverages vaapi. This profile will
/// automatically be enabled if any of your GPUs support encoding and decoding h264 with the
/// profiles `Main`, `High` and `ConstrainedBaseline`. This profile will only transcode h264 input
/// streams.
#[cfg(unix)]
#[derive(Debug)]
pub struct VaapiTranscodeProfile {
    profiles: Vec<rusty_vainfo::Profile>,
    vendor: String,
    dri: PathBuf,
}

impl VaapiTranscodeProfile {
    pub fn new() -> Option<Self> {
        let (profiles, vendor, dri) = discover_vaapi_device()?;
        Some(Self {
            profiles,
            vendor,
            dri,
        })
    }

    fn hw_scaling_supported(&self) -> bool {
        let required_profiles = ["VAProfileH264Main", "VAProfileH264High"];

        let enc_slice = "VAEntrypointEncSlice".to_string();

        for profile in required_profiles {
            let device_profile = if let Some(x) = self.profiles.iter().find(|x| x.name == profile) {
                x
            } else {
                continue;
            };

            // NOTE: We should probably warn the client here that scaling wont work because they
            // possibly have the free intel quicksync driver installed (if dri is a intel igpu).
            return device_profile.entrypoints.contains(&enc_slice);
        }

        false
    }
}

#[cfg(unix)]
impl TranscodingProfile for VaapiTranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "VaapiTranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        // Currently this profile only supports HW Encoding + decoding.
        let required_features = [
            "VAEntrypointEncSlice".to_string(),
            "VAEntrypointVLD".to_string(),
        ];

        // NOTE: These could technically be less restrictive and we could match for them inside
        // build. Although I doubt that there are actually any devices that dont support all three
        // of these profiles.
        // see: https://github.com/intel/libva/blob/6e86b4fb4dafa123b1e31821f61da88f10cfbe91/va/va.h#L493
        let required_profiles = [
            "VAProfileH264ConstrainedBaseline",
            "VAProfileH264Main",
            "VAProfileH264High",
        ];

        // Enable this profile if any of the above VA profiles supports both
        // required entrypoints (decode + encode). Requiring the entrypoints is
        // essential: a decode-only device advertises the profile but only with
        // VAEntrypointVLD, and enabling it there makes h264_vaapi encode fail
        // at runtime.
        for profile in required_profiles {
            let Some(device_profile) = self.profiles.iter().find(|x| x.name == profile) else {
                continue;
            };

            if required_features
                .iter()
                .all(|f| device_profile.entrypoints.iter().any(|e| e == f))
            {
                return Ok(());
            }
        }

        Err(NightfallError::ProfileNotSupported(format!(
            "Device {} doesnt seem to support hardware encoding+decoding of h264 (Supported profiles: {})",
            self.vendor,
            self.profiles
                .iter()
                .map(|x| x.name.clone())
                .collect::<Vec<_>>()
                .join(" | ")
        )))
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
            args.push("vaapi".into());
            args.push("-vaapi_device".into());
            args.push(self.dri.to_string_lossy().into());
            args.push("-hwaccel_output_format".into());
            args.push("vaapi".into());
        } else {
            args.push("-vaapi_device".into());
            args.push(self.dri.to_string_lossy().into());
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
            "h264_vaapi".into(),
            "-bf".into(),
            "0".into(),
        ]);

        args.push("-vf".into());

        if ctx.force_software_decode {
            if let Some(height) = ctx.output_ctx.height {
                let width = ctx.output_ctx.width.unwrap_or(-2);
                args.push(format!("format=nv12,scale={}:{},hwupload", width, height));
            } else {
                args.push("format=nv12,hwupload".into());
            }
        } else if let Some(height) = ctx.output_ctx.height {
            let mut vfilter = Vec::new();
            let width = ctx.output_ctx.width.unwrap_or(-2); // defaults to scaling by 2

            if self.hw_scaling_supported() {
                vfilter.push(format!("scale_vaapi={}:{}", width, height));
            }

            vfilter.push("hwdownload".into());

            // TODO: Detect if input file is 10-bit with a less hacky way.
            if ctx.input_ctx.profile.as_str() == "Main 10" {
                vfilter.push("format=p010le".into());
            }

            vfilter.push("format=nv12".into());

            if !self.hw_scaling_supported() {
                vfilter.push(format!("scale={}:{}", width, height));
            }

            vfilter.push("hwupload".into());

            args.push(vfilter.join(","));
        } else {
            args.push("hwdownload,format=nv12,hwupload".into());
        }

        if let Some(bitrate) = ctx.output_ctx.bitrate {
            // NOTE: it seems that when the non-free qsv driver is not installed then we cant use
            // -b:v. This might be a way to detect whether we can use -b:v flag but im not too
            // sure.
            if !self.hw_scaling_supported() {
                args.push("-maxrate".into());
                args.push(bitrate.to_string());
            } else {
                args.push("-b:v".into());
                args.push(bitrate.to_string());
            }
        }

        args.append(&mut vec![
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
            start_num.clone(),
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
            "-sc_threshold:v:0".into(),
            "0".into(),
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

        check_vaapi_input(&self.profiles, ctx)
    }

    fn tag(&self) -> &str {
        "h264_vaapi"
    }
}

/// Vaapi HEVC transcoding profile.
#[cfg(unix)]
#[derive(Debug)]
pub struct VaapiHevcTranscodeProfile {
    profiles: Vec<rusty_vainfo::Profile>,
    vendor: String,
    dri: PathBuf,
}

impl VaapiHevcTranscodeProfile {
    pub fn new() -> Option<Self> {
        let (profiles, vendor, dri) = discover_vaapi_device()?;
        Some(Self {
            profiles,
            vendor,
            dri,
        })
    }

    fn hw_scaling_supported(&self) -> bool {
        let required_profiles = ["VAProfileHEVCMain", "VAProfileHEVCMain10"];

        let enc_slice = "VAEntrypointEncSlice".to_string();

        for profile in required_profiles {
            let device_profile = if let Some(x) = self.profiles.iter().find(|x| x.name == profile) {
                x
            } else {
                continue;
            };

            return device_profile.entrypoints.contains(&enc_slice);
        }

        false
    }
}

#[cfg(unix)]
impl TranscodingProfile for VaapiHevcTranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "VaapiHevcTranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        let required_features = [
            "VAEntrypointEncSlice".to_string(),
            "VAEntrypointVLD".to_string(),
        ];

        let profile_name = "VAProfileHEVCMain";

        let device_profile = self.profiles.iter().find(|x| x.name == profile_name).ok_or(
            NightfallError::ProfileNotSupported(format!(
                "Device {} doesnt support profile {} (Supported profiles: {})",
                self.vendor,
                profile_name,
                self.profiles
                    .iter()
                    .map(|x| x.name.clone())
                    .collect::<Vec<_>>()
                    .join(" | ")
            )),
        )?;

        for feature in &required_features {
            if !device_profile.entrypoints.contains(feature) {
                return Err(NightfallError::ProfileNotSupported(format!(
                    "Device {} doesnt support entrypoint {} for {}.",
                    self.vendor, feature, profile_name
                )));
            }
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
            args.push("vaapi".into());
            args.push("-vaapi_device".into());
            args.push(self.dri.to_string_lossy().into());
            args.push("-hwaccel_output_format".into());
            args.push("vaapi".into());
        } else {
            args.push("-vaapi_device".into());
            args.push(self.dri.to_string_lossy().into());
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
            "hevc_vaapi".into(),
            "-tag:v".into(),
            "hvc1".into(),
            "-bf".into(),
            "0".into(),
        ]);

        args.push("-vf".into());

        if ctx.force_software_decode {
            if let Some(height) = ctx.output_ctx.height {
                let width = ctx.output_ctx.width.unwrap_or(-2);
                args.push(format!("format=nv12,scale={}:{},hwupload", width, height));
            } else {
                args.push("format=nv12,hwupload".into());
            }
        } else if let Some(height) = ctx.output_ctx.height {
            let mut vfilter = Vec::new();
            let width = ctx.output_ctx.width.unwrap_or(-2);

            if self.hw_scaling_supported() {
                vfilter.push(format!("scale_vaapi={}:{}", width, height));
            }

            vfilter.push("hwdownload".into());

            if ctx.input_ctx.profile.as_str() == "Main 10" {
                vfilter.push("format=p010le".into());
            }

            vfilter.push("format=nv12".into());

            if !self.hw_scaling_supported() {
                vfilter.push(format!("scale={}:{}", width, height));
            }

            vfilter.push("hwupload".into());

            args.push(vfilter.join(","));
        } else {
            args.push("hwdownload,format=nv12,hwupload".into());
        }

        if let Some(bitrate) = ctx.output_ctx.bitrate {
            if !self.hw_scaling_supported() {
                args.push("-maxrate".into());
                args.push(bitrate.to_string());
            } else {
                args.push("-b:v".into());
                args.push(bitrate.to_string());
            }
        }

        args.append(&mut vec![
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
            start_num.clone(),
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
            "-sc_threshold:v:0".into(),
            "0".into(),
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

        check_vaapi_input(&self.profiles, ctx)
    }

    fn tag(&self) -> &str {
        "hevc_vaapi"
    }
}

/// Vaapi AV1 transcoding profile.
#[cfg(unix)]
#[derive(Debug)]
pub struct VaapiAv1TranscodeProfile {
    profiles: Vec<rusty_vainfo::Profile>,
    vendor: String,
    dri: PathBuf,
}

impl VaapiAv1TranscodeProfile {
    pub fn new() -> Option<Self> {
        let (profiles, vendor, dri) = discover_vaapi_device()?;
        Some(Self {
            profiles,
            vendor,
            dri,
        })
    }
}

#[cfg(unix)]
impl TranscodingProfile for VaapiAv1TranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::HardwareTranscode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Video
    }

    fn name(&self) -> &str {
        "VaapiAv1TranscodeProfile"
    }

    fn is_enabled(&self) -> Result<(), NightfallError> {
        let profile_name = "VAProfileAV1Profile0";

        let device_profile = self.profiles.iter().find(|x| x.name == profile_name).ok_or(
            NightfallError::ProfileNotSupported(format!(
                "Device {} doesnt support profile {} (Supported profiles: {})",
                self.vendor,
                profile_name,
                self.profiles
                    .iter()
                    .map(|x| x.name.clone())
                    .collect::<Vec<_>>()
                    .join(" | ")
            )),
        )?;

        let enc_slice = "VAEntrypointEncSlice".to_string();
        if !device_profile.entrypoints.contains(&enc_slice) {
            return Err(NightfallError::ProfileNotSupported(format!(
                "Device {} doesnt support VAEntrypointEncSlice for {}.",
                self.vendor, profile_name
            )));
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
            args.push("vaapi".into());
            args.push("-vaapi_device".into());
            args.push(self.dri.to_string_lossy().into());
            args.push("-hwaccel_output_format".into());
            args.push("vaapi".into());
        } else {
            args.push("-vaapi_device".into());
            args.push(self.dri.to_string_lossy().into());
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
            "av1_vaapi".into(),
            "-bf".into(),
            "0".into(),
        ]);

        args.push("-vf".into());

        if ctx.force_software_decode {
            if let Some(height) = ctx.output_ctx.height {
                let width = ctx.output_ctx.width.unwrap_or(-2);
                args.push(format!("format=nv12,scale={}:{},hwupload", width, height));
            } else {
                args.push("format=nv12,hwupload".into());
            }
        } else if let Some(height) = ctx.output_ctx.height {
            let mut vfilter = Vec::new();
            let width = ctx.output_ctx.width.unwrap_or(-2);

            vfilter.push("hwdownload".into());

            if ctx.input_ctx.profile.as_str() == "Main 10" {
                vfilter.push("format=p010le".into());
            }

            vfilter.push("format=nv12".into());
            vfilter.push(format!("scale={}:{}", width, height));
            vfilter.push("hwupload".into());

            args.push(vfilter.join(","));
        } else {
            args.push("hwdownload,format=nv12,hwupload".into());
        }

        if let Some(bitrate) = ctx.output_ctx.bitrate {
            args.push("-b:v".into());
            args.push(bitrate.to_string());
        }

        args.append(&mut vec![
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
            start_num.clone(),
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
            "-sc_threshold:v:0".into(),
            "0".into(),
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

        check_vaapi_input(&self.profiles, ctx)
    }

    fn tag(&self) -> &str {
        "av1_vaapi"
    }
}

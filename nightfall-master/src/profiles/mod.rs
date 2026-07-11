#[cfg(windows)]
pub mod amf;
pub mod audio;
pub mod av1;
#[cfg(all(unix, feature = "cuda"))]
pub mod cuda;
pub mod h265;
#[cfg(feature = "qsv")]
pub mod qsv;
pub mod subtitle;
pub mod vp9;
#[cfg(all(unix, feature = "v4l2"))]
pub mod v4l2;
#[cfg(all(unix, feature = "vaapi"))]
pub mod vaapi;
pub mod video;

#[cfg(windows)]
pub use amf::AmfTranscodeProfile;
#[cfg(windows)]
pub use amf::AmfHevcTranscodeProfile;
#[cfg(windows)]
pub use amf::AmfAv1TranscodeProfile;
pub use audio::AacTranscodeProfile;
#[cfg(all(unix, feature = "cuda"))]
pub use cuda::CudaTranscodeProfile;
#[cfg(all(unix, feature = "cuda"))]
pub use cuda::CudaHevcTranscodeProfile;
#[cfg(all(unix, feature = "cuda"))]
pub use cuda::CudaAv1TranscodeProfile;
#[cfg(feature = "qsv")]
pub use qsv::QsvTranscodeProfile;
#[cfg(feature = "qsv")]
pub use qsv::QsvHevcTranscodeProfile;
#[cfg(feature = "qsv")]
pub use qsv::QsvAv1TranscodeProfile;
#[cfg(feature = "ssa_transmux")]
pub use subtitle::AssExtractProfile;
pub use subtitle::WebvttTranscodeProfile;
use tracing::debug;
use tracing::info;
use tracing::warn;
#[cfg(all(unix, feature = "v4l2"))]
pub use v4l2::V4l2TranscodeProfile;
#[cfg(all(unix, feature = "v4l2"))]
pub use v4l2::V4l2HevcTranscodeProfile;
#[cfg(all(unix, feature = "vaapi"))]
pub use vaapi::VaapiTranscodeProfile;
#[cfg(all(unix, feature = "vaapi"))]
pub use vaapi::VaapiHevcTranscodeProfile;
#[cfg(all(unix, feature = "vaapi"))]
pub use vaapi::VaapiAv1TranscodeProfile;
pub use av1::Av1TranscodeProfile;
pub use av1::Av1TransmuxProfile;
pub use h265::H265TranscodeProfile;
pub use h265::HevcTransmuxProfile;
pub use video::H264TranscodeProfile;
pub use vp9::Vp9TransmuxProfile;
pub use video::H264TransmuxProfile;
pub use video::RawVideoTranscodeProfile;

use crate::NightfallError;
use serde::Serialize;
use std::fmt::Debug;

use once_cell::sync::OnceCell;

#[derive(Clone, Debug, Serialize)]
pub struct DetectedDevice {
    pub path: String,
    pub vendor: String,
    pub kind: String,
    pub codecs: Vec<String>,
}

pub fn detect_devices() -> Vec<DetectedDevice> {
    let mut devices = Vec::new();

    #[cfg(all(unix, feature = "vaapi"))]
    {
        devices.extend(detect_vaapi_devices());
    }

    #[cfg(unix)]
    {
        devices.extend(detect_nvidia_devices());
    }

    #[cfg(all(unix, feature = "v4l2"))]
    {
        devices.extend(detect_v4l2_devices());
    }

    devices
}

#[cfg(all(unix, feature = "vaapi"))]
fn detect_vaapi_devices() -> Vec<DetectedDevice> {
    use std::collections::BTreeSet;

    let hw_targets = match std::fs::read_dir("/dev/dri") {
        Ok(entries) => {
            let mut paths: Vec<_> = entries
                .filter_map(Result::ok)
                .filter(|x| x.file_name().to_string_lossy().contains("render"))
                .map(|x| x.path())
                .collect();
            paths.sort();
            paths
        }
        Err(_) => return Vec::new(),
    };

    let mut devices = Vec::new();

    for target in hw_targets {
        if let Ok(instance) = rusty_vainfo::VaInstance::with_drm(&target) {
            let vendor = instance.vendor_string();
            let profiles = instance.profiles().unwrap_or_default();

            let mut codecs = BTreeSet::new();
            for profile in &profiles {
                if profile.name.starts_with("VAProfileH264") {
                    codecs.insert("h264".to_string());
                } else if profile.name.starts_with("VAProfileHEVC") {
                    codecs.insert("h265".to_string());
                } else if profile.name.starts_with("VAProfileAV1") {
                    codecs.insert("av1".to_string());
                } else if profile.name.starts_with("VAProfileVP9") {
                    codecs.insert("vp9".to_string());
                }
            }

            let kind = if vendor.to_lowercase().contains("intel") {
                "vaapi/qsv"
            } else {
                "vaapi"
            };

            devices.push(DetectedDevice {
                path: target.to_string_lossy().into_owned(),
                vendor,
                kind: kind.to_string(),
                codecs: codecs.into_iter().collect(),
            });
        }
    }

    devices
}

#[cfg(unix)]
fn detect_nvidia_devices() -> Vec<DetectedDevice> {
    let mut devices = Vec::new();

    if let Ok(entries) = std::fs::read_dir("/dev") {
        let mut paths: Vec<_> = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("nvidia") && name[6..].parse::<u32>().is_ok()
            })
            .collect();
        paths.sort_by_key(|e| e.path());

        for entry in paths {
            devices.push(DetectedDevice {
                path: entry.path().to_string_lossy().into_owned(),
                vendor: "NVIDIA".to_string(),
                kind: "cuda".to_string(),
                codecs: vec!["h264".into(), "h265".into(), "av1".into()],
            });
        }
    }

    devices
}

#[cfg(all(unix, feature = "v4l2"))]
fn detect_v4l2_devices() -> Vec<DetectedDevice> {
    let mut devices = Vec::new();

    if let Ok(entries) = std::fs::read_dir("/dev") {
        let mut paths: Vec<_> = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("video")
            })
            .collect();
        paths.sort_by_key(|e| e.path());

        for entry in paths {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let suffix = &name[5..];

            if suffix.parse::<u32>().is_ok() {
                // /dev/video<N> — M2M encoder devices
                devices.push(DetectedDevice {
                    path: entry.path().to_string_lossy().into_owned(),
                    vendor: "V4L2 M2M".to_string(),
                    kind: "v4l2".to_string(),
                    codecs: vec!["h264".into(), "h265".into()],
                });
            } else if suffix.contains("dec") {
                // /dev/video-cixdec0 etc. — M2M decoder devices
                devices.push(DetectedDevice {
                    path: entry.path().to_string_lossy().into_owned(),
                    vendor: "V4L2 M2M".to_string(),
                    kind: "v4l2-decoder".to_string(),
                    codecs: vec!["h264".into(), "h265".into(), "av1".into()],
                });
            }
        }
    }

    devices
}

static PROFILES: OnceCell<Vec<Box<dyn TranscodingProfile>>> = OnceCell::new();
static FFMPEG_BIN: OnceCell<String> = OnceCell::new();

/// The ffmpeg binary configured at init time. Probes that shell out to ffmpeg
/// (e.g. encoder availability checks) must use this rather than assuming
/// `ffmpeg` is on $PATH.
pub(crate) fn ffmpeg_bin() -> String {
    FFMPEG_BIN
        .get()
        .cloned()
        .unwrap_or_else(|| "ffmpeg".to_string())
}

pub fn profiles_init(ffmpeg_bin: String) {
    let _ = FFMPEG_BIN.set(ffmpeg_bin);

    let devices = detect_devices();
    if devices.is_empty() {
        warn!("No hardware acceleration devices detected");
    } else {
        for dev in &devices {
            info!(
                path = %dev.path,
                vendor = %dev.vendor,
                kind = %dev.kind,
                codecs = ?dev.codecs,
                "Detected hardware device"
            );
        }
    }

    let profiles: Vec<Option<Box<dyn TranscodingProfile>>> = vec![
        Some(Box::new(AacTranscodeProfile)),
        Some(Box::new(H264TranscodeProfile)),
        Some(Box::new(H264TransmuxProfile)),
        Some(Box::new(HevcTransmuxProfile)),
        Some(Box::new(H265TranscodeProfile)),
        Some(Box::new(Av1TransmuxProfile)),
        Some(Box::new(Vp9TransmuxProfile)),
        Some(Box::new(Av1TranscodeProfile)),
        Some(Box::new(RawVideoTranscodeProfile)),
        Some(Box::new(WebvttTranscodeProfile)),
        #[cfg(feature = "ssa_transmux")]
        Some(Box::new(AssExtractProfile)),
        #[cfg(all(unix, feature = "cuda"))]
        Some(Box::new(CudaTranscodeProfile)),
        #[cfg(all(unix, feature = "cuda"))]
        Some(Box::new(CudaHevcTranscodeProfile)),
        #[cfg(all(unix, feature = "cuda"))]
        Some(Box::new(CudaAv1TranscodeProfile)),
        #[cfg(all(unix, feature = "vaapi"))]
        VaapiTranscodeProfile::new().map(|x| Box::new(x) as _),
        #[cfg(all(unix, feature = "vaapi"))]
        VaapiHevcTranscodeProfile::new().map(|x| Box::new(x) as _),
        #[cfg(all(unix, feature = "vaapi"))]
        VaapiAv1TranscodeProfile::new().map(|x| Box::new(x) as _),
        #[cfg(feature = "qsv")]
        Some(Box::new(QsvTranscodeProfile)),
        #[cfg(feature = "qsv")]
        Some(Box::new(QsvHevcTranscodeProfile)),
        #[cfg(feature = "qsv")]
        Some(Box::new(QsvAv1TranscodeProfile)),
        #[cfg(all(unix, feature = "v4l2"))]
        Some(Box::new(V4l2TranscodeProfile)),
        #[cfg(all(unix, feature = "v4l2"))]
        Some(Box::new(V4l2HevcTranscodeProfile)),
        #[cfg(windows)]
        Some(Box::new(AmfTranscodeProfile)),
        #[cfg(windows)]
        Some(Box::new(AmfHevcTranscodeProfile)),
        #[cfg(windows)]
        Some(Box::new(AmfAv1TranscodeProfile)),
    ];

    let profiles = profiles.into_iter().filter_map(|x| x).collect::<Vec<_>>();

    let enabled: Vec<Box<dyn TranscodingProfile>> = profiles
        .into_iter()
        .filter(|x| {
            if let Err(e) = x.is_enabled() {
                warn!(
                    profile = x.name(),
                    reason = %e,
                    "Disabling profile"
                );

                false
            } else {
                info!(profile = x.name(), "Enabling profile");

                true
            }
        })
        .collect();

    let hw_profiles: Vec<&str> = enabled
        .iter()
        .filter(|x| x.profile_type() == ProfileType::HardwareTranscode)
        .map(|x| x.tag())
        .collect();

    if hw_profiles.is_empty() {
        warn!("No hardware transcoding profiles are available. Hardware acceleration will not work.");
    } else {
        info!(profiles = ?hw_profiles, "Available hardware transcoding profiles");
    }

    let _ = PROFILES.set(enabled);
}

pub fn get_active_profiles() -> Vec<&'static dyn TranscodingProfile> {
    PROFILES
        .get()
        .expect("nightfall::PROFILES not initialized.")
        .iter()
        .map(AsRef::as_ref)
        .collect()
}

pub fn get_profile_for(
    stream_type: StreamType,
    ctx: &ProfileContext,
) -> Vec<&'static dyn TranscodingProfile> {
    let mut profiles: Vec<_> = PROFILES
        .get()
        .expect("nightfall::PROFILES not initialized.")
        .iter()
        .filter(|x| {
            x.stream_type() == stream_type
                && if let Err(e) = x.supports(ctx) {
                    if x.profile_type() == ProfileType::HardwareTranscode {
                        warn!(
                            profile = x.name(),
                            tag = x.tag(),
                            reason = %e,
                            output_codec = %ctx.output_ctx.codec,
                            "Hardware profile rejected by supports()"
                        );
                    } else {
                        debug!(
                            profile = x.name(),
                            reason = %e,
                            "Profile not supported for ctx"
                        );
                    }

                    false
                } else {
                    true
                }
        })
        .map(AsRef::as_ref)
        .collect();

    profiles.sort_by_key(|x| x.profile_type());

    profiles
}

pub fn get_profile_for_with_type(
    stream_type: StreamType,
    profile_type: ProfileType,
    ctx: &ProfileContext,
) -> Vec<&'static dyn TranscodingProfile> {
    let mut profiles: Vec<_> = PROFILES
        .get()
        .expect("nightfall::PROFILES not initialized.")
        .iter()
        .filter(|x| {
            x.profile_type() == profile_type
                && x.stream_type() == stream_type
                && if let Err(e) = x.supports(ctx) {
                    debug!(
                        profile = x.name(),
                        reason = %e,
                        "Profile not supported for ctx"
                    );

                    false
                } else {
                    true
                }
        })
        .map(AsRef::as_ref)
        .collect();

    profiles.sort_by_key(|x| x.profile_type());

    profiles
}

pub trait TranscodingProfile: Debug + Send + Sync + 'static {
    /// Function must return what kind of profile it is.
    fn profile_type(&self) -> ProfileType;

    /// Function will return what type of stream this profile is for.
    fn stream_type(&self) -> StreamType;

    /// This function gets called at run-time to check whether this profile is enabled.
    /// By default this function is auto-implemented to return `true`, however for complex
    /// profiles such as VAAPI we may want at run-time to check whether ffmpeg will actually
    /// transcode the given file.
    fn is_enabled(&self) -> Result<(), NightfallError> {
        Ok(())
    }

    /// Function will build a list of arguments to be passed to ffmpeg for the profile which
    /// implements this trait. The function will return `None` if the parameters supplied in the
    /// context are invalid or cant be used here.
    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>>;

    /// Function will return whether the conversion to `codec_out` is possible. Some
    /// implementations of this function (HWAccelerated profiles) will also check whether
    /// a direct conversion betwen`codec_in` and `codec_out` is possible.
    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError>;

    /// Return tag of this profile.
    fn tag(&self) -> &str;

    /// Return name of this profile.
    fn name(&self) -> &str;

    /// Function will return whether this profile emit data over stdout instead of progress information.
    fn is_stdio_stream(&self) -> bool {
        false
    }
}

/// A context which contains information we may need when building the ffmpeg arguments.
#[derive(Clone, Debug)]
pub struct ProfileContext {
    pub file: String,
    pub pre_args: Vec<String>,
    pub input_ctx: InputCtx,
    pub output_ctx: OutputCtx,
    pub ffmpeg_bin: String,
    pub force_software_decode: bool,
}

#[derive(Clone, Debug)]
pub struct InputCtx {
    pub stream: usize,
    pub audio_channels: u64,
    pub codec: String,
    pub pix_fmt: String,
    pub profile: String,
    pub bframes: Option<u64>,
    pub fps: f64,
    pub bitrate: u64,
    pub seek: Option<i64>,
}

impl Default for InputCtx {
    fn default() -> Self {
        Self {
            stream: 0,
            codec: String::new(),
            audio_channels: 2,
            pix_fmt: String::new(),
            profile: String::new(),
            bframes: None,
            fps: 0.0,
            bitrate: 0,
            seek: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OutputCtx {
    pub codec: String,
    pub start_num: u32,
    pub outdir: String,
    pub max_to_transcode: Option<u64>,
    pub bitrate: Option<u64>,
    pub height: Option<i64>,
    pub width: Option<i64>,
    pub audio_channels: u64,
    pub target_gop: u32,
}

impl Default for OutputCtx {
    fn default() -> Self {
        Self {
            codec: String::new(),
            start_num: 0,
            outdir: String::new(),
            max_to_transcode: None,
            bitrate: None,
            height: None,
            width: None,
            audio_channels: 2,
            target_gop: 5,
        }
    }
}

impl Default for ProfileContext {
    fn default() -> Self {
        Self {
            file: String::new(),
            pre_args: Vec::new(),
            input_ctx: Default::default(),
            output_ctx: Default::default(),
            ffmpeg_bin: "ffmpeg".into(),
            force_software_decode: false,
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd)]
pub enum ProfileType {
    Transcode,
    Transmux,
    HardwareTranscode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamType {
    Video,
    Audio,
    Subtitle,
}

use crate::utils::ffpath;

use std::error::Error;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use once_cell::sync::OnceCell;

use serde::Deserialize;
use serde::Serialize;

fn default_syncplay_port() -> u16 {
    8999
}

/// Dim's shared community TMDB key — used when no key is configured.
pub fn default_tmdb_key() -> String {
    "38c372f5bc572c8aadde7a802638534e".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HwAccelMethod {
    Off,
    Auto,
    Vaapi,
    Cuda,
    Qsv,
    V4l2,
    Amf,
}

impl Default for HwAccelMethod {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DecodeMethod {
    Auto,
    Software,
}

impl Default for DecodeMethod {
    fn default() -> Self {
        Self::Auto
    }
}

fn deserialize_hwaccel<'de, D>(deserializer: D) -> Result<HwAccelMethod, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum HwAccelCompat {
        New(HwAccelMethod),
        Legacy(bool),
    }

    match HwAccelCompat::deserialize(deserializer)? {
        HwAccelCompat::New(method) => Ok(method),
        HwAccelCompat::Legacy(true) => Ok(HwAccelMethod::Auto),
        HwAccelCompat::Legacy(false) => Ok(HwAccelMethod::Off),
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DefaultCodec {
    H264,
    H265,
    Av1,
}

impl Default for DefaultCodec {
    fn default() -> Self {
        Self::H264
    }
}

/// Codec audio tracks are converted to whenever audio has to be transcoded.
/// Every option must be decodable via MSE in fMP4; the stream route falls back
/// to AAC when the requesting browser can't play the configured one.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DefaultAudioCodec {
    #[default]
    Aac,
    Opus,
    Flac,
    Ac3,
    Eac3,
}

impl DefaultAudioCodec {
    /// Codec tag as used by nightfall's `OutputCtx::codec` and the client's
    /// `supported_audio_codecs` list.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Opus => "opus",
            Self::Flac => "flac",
            Self::Ac3 => "ac3",
            Self::Eac3 => "eac3",
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
// Every field falls back to `GlobalSettings::default()` when absent so configs
// written by older versions never fail to parse (a full parse failure would
// silently reset the whole config).
#[serde(default)]
pub struct GlobalSettings {
    pub enable_ssl: bool,
    pub port: u16,
    pub priv_key: Option<String>,
    pub ssl_cert: Option<String>,

    pub cache_dir: String,
    pub metadata_dir: String,
    pub quiet_boot: bool,

    pub disable_auth: bool,

    pub verbose: bool,
    pub secret_key: Option<[u8; 32]>,
    #[serde(deserialize_with = "deserialize_hwaccel")]
    pub enable_hwaccel: HwAccelMethod,
    #[serde(default)]
    pub default_video_codec: DefaultCodec,
    #[serde(default)]
    pub default_audio_codec: DefaultAudioCodec,
    #[serde(default)]
    pub decode_method: DecodeMethod,
    pub version: String,

    #[serde(default)]
    pub syncplay_enabled: bool,
    #[serde(default = "default_syncplay_port")]
    pub syncplay_port: u16,
    #[serde(default)]
    pub syncplay_password: String,
    #[serde(default)]
    pub syncplay_motd: String,

    #[serde(default)]
    pub webhook_api_key: String,

    /// TMDB API key used by the metadata scanner; falls back to Dim's shared key.
    #[serde(default = "default_tmdb_key")]
    pub tmdb_api_key: String,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            enable_ssl: false,
            port: 8000,
            priv_key: None,
            ssl_cert: None,
            cache_dir: {
                cfg_if::cfg_if! {
                    if #[cfg(target_family = "unix")] {
                        "/tmp/streaming_cache".into()
                    } else {
                        "./streaming_cache".into()
                    }
                }
            },
            metadata_dir: {
                cfg_if::cfg_if! {
                    if #[cfg(target_family = "unix")] {
                        if std::path::Path::new("/opt/dim/config").exists() {
                            "/opt/dim/config/metadata".into()
                        } else {
                            "/opt/dim/metadata".into()
                        }
                    } else {
                        "./metadata".into()
                    }
                }
            },
            quiet_boot: false,
            disable_auth: false,
            verbose: false,
            secret_key: None,
            enable_hwaccel: HwAccelMethod::Off,
            default_video_codec: DefaultCodec::H264,
            default_audio_codec: DefaultAudioCodec::Aac,
            decode_method: DecodeMethod::Auto,
            version: String::new(),
            syncplay_enabled: false,
            syncplay_port: 8999,
            syncplay_password: String::new(),
            syncplay_motd: String::new(),
            webhook_api_key: String::new(),
            tmdb_api_key: default_tmdb_key(),
        }
    }
}

static GLOBAL_SETTINGS: Lazy<Mutex<GlobalSettings>> =
    Lazy::new(|| Mutex::new(GlobalSettings::default()));
static SETTINGS_PATH: OnceCell<String> = OnceCell::new();

pub fn get_global_settings() -> GlobalSettings {
    let lock = GLOBAL_SETTINGS.lock().unwrap();
    lock.clone()
}

pub fn init_global_settings(path: Option<String>) -> Result<(), Box<dyn Error>> {
    let path = path.unwrap_or(ffpath("config/config.toml"));
    let _ = SETTINGS_PATH.set(path.clone());
    let mut content = String::new();
    OpenOptions::new()
        .write(true)
        .create(true)
        .read(true)
        .open(path)?
        .read_to_string(&mut content)?;
    {
        let mut lock = GLOBAL_SETTINGS.lock().unwrap();
        *lock = match toml::from_str(&content) {
            Ok(settings) => settings,
            Err(e) => {
                if !content.trim().is_empty() {
                    tracing::error!(
                        error = %e,
                        "Failed to parse config file; falling back to default settings"
                    );
                }
                GlobalSettings::default()
            }
        };
    }
    let _ = set_global_settings(get_global_settings());
    Ok(())
}

pub fn set_global_settings(settings: GlobalSettings) -> Result<(), Box<dyn Error>> {
    let path = SETTINGS_PATH
        .get()
        .cloned()
        .unwrap_or(ffpath("config/config.toml"));
    {
        let mut lock = GLOBAL_SETTINGS.lock().unwrap();
        *lock = settings;
    }
    let settings = get_global_settings();
    File::create(path)?
        .write(toml::to_string_pretty(&settings).unwrap().as_ref())
        .unwrap();
    Ok(())
}

#[cfg(test)]
mod audio_codec_tests {
    use super::*;

    #[test]
    fn missing_audio_codec_defaults_to_aac() {
        let s: GlobalSettings = serde_json::from_str(r#"{"default_video_codec":"h265"}"#).unwrap();
        assert_eq!(s.default_audio_codec, DefaultAudioCodec::Aac);
        assert_eq!(s.default_video_codec, DefaultCodec::H265);
    }

    #[test]
    fn audio_codec_roundtrips() {
        for (raw, codec) in [
            ("aac", DefaultAudioCodec::Aac),
            ("opus", DefaultAudioCodec::Opus),
            ("flac", DefaultAudioCodec::Flac),
            ("ac3", DefaultAudioCodec::Ac3),
            ("eac3", DefaultAudioCodec::Eac3),
        ] {
            let s: GlobalSettings =
                serde_json::from_str(&format!(r#"{{"default_audio_codec":"{raw}"}}"#)).unwrap();
            assert_eq!(s.default_audio_codec, codec);
            assert_eq!(codec.as_str(), raw);
            let back = serde_json::to_value(&s).unwrap();
            assert_eq!(back["default_audio_codec"], raw);
        }
    }
}

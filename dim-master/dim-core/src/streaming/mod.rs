pub mod ffprobe;
pub mod direct_play;

use cfg_if::cfg_if;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

use crate::utils::ffpath;

lazy_static::lazy_static! {
    pub static ref STREAMING_SESSION: Arc<RwLock<HashMap<String, HashMap<String, String>>>> = Arc::new(RwLock::new(HashMap::new()));
    pub static ref FFMPEG_BIN: &'static str = Box::leak(ffpath("utils/ffmpeg").into_boxed_str());
    pub static ref FFPROBE_BIN: &'static str = {
        cfg_if! {
            if #[cfg(test)] {
                "/usr/bin/ffprobe"
            } else {
                Box::leak(ffpath("utils/ffprobe").into_boxed_str())
            }
        }
    };
}

use std::process::Command;

/// ffcheck - Check if "ffmpeg" and "ffprobe" are accessable through `std::process::Command`.
///
/// This will run `ffmpeg -version` and `ffprobe -version` and return a vec of the stdout
/// output if successfull or the binaries name if not.
///
/// # Example
///
/// ```ignore
/// use streaming::ffcheck;
///
/// for result in ffcheck() {
///     match result {
///         Ok(stdout) => println!("{:?}", stdout),
///         Err(program) => eprintln!("Failed to get the `-version` output of {:?}", program),
///     }
/// }
/// ```
pub fn ffcheck() -> Vec<Result<Box<str>, &'static str>> {
    let mut results = vec![];

    for program in [*FFMPEG_BIN, *FFPROBE_BIN].iter() {
        if let Ok(output) = Command::new(program).arg("-version").output() {
            let stdout = String::from_utf8(output.stdout)
                .expect("Failed to decode subprocess stdout.")
                .into_boxed_str();

            results.push(Ok(stdout));
        } else {
            results.push(Err(*program));
        }
    }

    results
}

#[derive(Clone, Copy)]
pub struct Quality {
    pub height: u64,
    pub bitrate: u64,
}

/// Build the list of transcode quality tiers for a source of the given
/// height/bitrate: the presets that don't upscale or inflate bitrate, plus an
/// original-resolution tier when the source exceeds the largest preset (so
/// e.g. 4K sources aren't silently capped at 1080p).
pub fn get_qualities(height: u64, bitrate: u64) -> Vec<Quality> {
    let mut qualities: Vec<Quality> = VIDEO_QUALITIES
        .iter()
        .filter(|x| x.height <= height && x.bitrate <= bitrate)
        .copied()
        .collect();

    if qualities.first().map_or(true, |q| q.height < height) {
        qualities.insert(0, Quality { height, bitrate });
    }

    qualities
}

pub const VIDEO_QUALITIES: [Quality; 3] = [
    Quality {
        height: 1080,
        bitrate: 10_000_000,
    },
    Quality {
        height: 720,
        bitrate: 5_000_000,
    },
    Quality {
        height: 480,
        bitrate: 1_000_000,
    },
];

#[derive(Clone)]
pub struct Avc1Level {
    pub level: u64,
    pub macro_blocks_rate: u64,
    pub max_frame_size: u64,
    pub max_bitrate: u64,
}

impl ToString for Avc1Level {
    fn to_string(&self) -> String {
        format!("avc1.6400{:02x}", self.level)
    }
}

/// An external (sidecar) subtitle file found next to a video file.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalSubtitle {
    pub path: std::path::PathBuf,
    /// ffprobe-style codec name derived from the extension ("subrip", "ass", "webvtt").
    pub codec: &'static str,
    /// ISO-639-2/B code parsed from the filename tags, if any.
    pub lang: Option<String>,
    /// Human-readable track label, e.g. "English (Forced) (External)".
    pub label: String,
}

/// Find sidecar subtitle files for a video: `<video>.<ext>` plus the
/// Plex/Jellyfin convention `<video>.<tags...>.<ext>` where tags are a
/// language (2/3-letter code or spelled out) and/or flags like "forced",
/// "sdh", "cc", "hi", "default". Unrecognized tags are kept as label text so
/// files like `<video>.signs.srt` still surface.
pub fn find_external_subtitles(video_path: impl AsRef<std::path::Path>) -> Vec<ExternalSubtitle> {
    let video_path = video_path.as_ref();
    let (Some(dir), Some(video_stem)) = (
        video_path.parent(),
        video_path.file_stem().and_then(|s| s.to_str()),
    ) else {
        return Vec::new();
    };

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut found = Vec::new();

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let codec = match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref()
        {
            Some("srt") => "subrip",
            Some("ass") | Some("ssa") => "ass",
            Some("vtt") | Some("webvtt") => "webvtt",
            _ => continue,
        };

        let Some(sub_stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };

        // The sidecar stem must be the video stem, optionally followed by
        // dot-separated tags.
        if sub_stem.len() < video_stem.len()
            || !sub_stem[..video_stem.len()].eq_ignore_ascii_case(video_stem)
        {
            continue;
        }
        let rest = &sub_stem[video_stem.len()..];
        if !rest.is_empty() && !rest.starts_with('.') {
            continue;
        }

        let mut lang = None;
        let mut lang_name = None;
        let mut flags: Vec<String> = Vec::new();
        let mut extra: Vec<String> = Vec::new();

        for tag in rest.split('.').filter(|t| !t.is_empty()) {
            match tag.to_lowercase().as_str() {
                "forced" => flags.push("Forced".into()),
                "sdh" | "cc" | "hi" => flags.push("SDH".into()),
                "default" => {}
                _ => {
                    if lang.is_none() {
                        if let Some((code, name)) = crate::utils::lang_from_subtitle_tag(tag) {
                            lang = Some(code.to_string());
                            lang_name = Some(name.to_string());
                            continue;
                        }
                    }
                    extra.push(tag.to_string());
                }
            }
        }

        let mut label = lang_name.unwrap_or_else(|| "Unknown".into());
        if !extra.is_empty() {
            label = format!("{} [{}]", label, extra.join(" "));
        }
        for flag in flags {
            label = format!("{} ({})", label, flag);
        }
        label.push_str(" (External)");

        found.push(ExternalSubtitle {
            path,
            codec,
            lang,
            label,
        });
    }

    // Directory iteration order is filesystem-dependent; sort for stable
    // track ordering across manifest requests.
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// Build an RFC-6381 `vp09.PP.LL.DD` codec string for a VP9 stream.
///
/// Browsers gate MSE support on profile and bit depth; the level component is
/// informational, so an approximation from resolution is sufficient.
pub fn get_vp9_tag(stream: &ffprobe::Stream) -> String {
    let profile = match stream.profile.as_deref() {
        Some("Profile 1") => 1,
        Some("Profile 2") => 2,
        Some("Profile 3") => 3,
        _ => 0,
    };

    let bit_depth = match stream.pix_fmt.as_deref() {
        Some(fmt) if fmt.contains("12") => 12,
        Some(fmt) if fmt.contains("10") => 10,
        _ => 8,
    };

    let level = match stream.height.unwrap_or(1080) {
        h if h <= 480 => 21,
        h if h <= 720 => 30,
        h if h <= 1080 => 40,
        h if h <= 1440 => 50,
        _ => 51,
    };

    format!("vp09.{:02}.{:02}.{:02}", profile, level, bit_depth)
}

pub fn level_to_tag(level: i64) -> Option<Avc1Level> {
    let level = level as u64;
    AVC1_LEVELS.iter().find(|&x| x.level == level).cloned()
}

pub fn get_avc1_tag(width: u64, height: u64, bitrate: u64, framerate: u64) -> Avc1Level {
    let macro_blocks = (width as f64 / 16.0) * (height as f64 / 16.0);
    let blocks_per_sec = macro_blocks * framerate as f64;

    let mut avc1_levels = AVC1_LEVELS.iter().filter(|&x| {
        x.max_bitrate > bitrate
            && (macro_blocks as u64) < x.max_frame_size
            && blocks_per_sec < x.macro_blocks_rate as f64
    });

    // Extremely high bitrate/resolution sources can exceed every level's
    // limits; report the highest level instead of panicking.
    avc1_levels
        .next()
        .cloned()
        .unwrap_or_else(|| AVC1_LEVELS[AVC1_LEVELS.len() - 1].clone())
}

pub const AVC1_LEVELS: [Avc1Level; 20] = [
    Avc1Level {
        level: 9,
        macro_blocks_rate: 1_485,
        max_frame_size: 99,
        max_bitrate: 128_000,
    },
    Avc1Level {
        level: 10,
        macro_blocks_rate: 1_485,
        max_frame_size: 99,
        max_bitrate: 64_000,
    },
    Avc1Level {
        level: 11,
        macro_blocks_rate: 3_000,
        max_frame_size: 396,
        max_bitrate: 192_000,
    },
    Avc1Level {
        level: 12,
        macro_blocks_rate: 6_000,
        max_frame_size: 396,
        max_bitrate: 384_000,
    },
    Avc1Level {
        level: 13,
        macro_blocks_rate: 11_880,
        max_frame_size: 396,
        max_bitrate: 768_000,
    },
    Avc1Level {
        level: 20,
        macro_blocks_rate: 11_880,
        max_frame_size: 396,
        max_bitrate: 2_000_000,
    },
    Avc1Level {
        level: 21,
        macro_blocks_rate: 19_800,
        max_frame_size: 792,
        max_bitrate: 4_000_000,
    },
    Avc1Level {
        level: 22,
        macro_blocks_rate: 20_250,
        max_frame_size: 1_620,
        max_bitrate: 4_000_000,
    },
    Avc1Level {
        level: 30,
        macro_blocks_rate: 40_500,
        max_frame_size: 1_620,
        max_bitrate: 10_000_000,
    },
    Avc1Level {
        level: 31,
        macro_blocks_rate: 108_000,
        max_frame_size: 3600,
        max_bitrate: 14_000_000,
    },
    Avc1Level {
        level: 32,
        macro_blocks_rate: 216_000,
        max_frame_size: 5_120,
        max_bitrate: 20_000_000,
    },
    Avc1Level {
        level: 40,
        macro_blocks_rate: 245_760,
        max_frame_size: 8_192,
        max_bitrate: 20_000_000,
    },
    Avc1Level {
        level: 41,
        macro_blocks_rate: 245_760,
        max_frame_size: 8_192,
        max_bitrate: 50_000_000,
    },
    Avc1Level {
        level: 42,
        macro_blocks_rate: 522_240,
        max_frame_size: 8_704,
        max_bitrate: 50_000_000,
    },
    Avc1Level {
        level: 50,
        macro_blocks_rate: 589_824,
        max_frame_size: 22_080,
        max_bitrate: 135_000_000,
    },
    Avc1Level {
        level: 51,
        macro_blocks_rate: 983_040,
        max_frame_size: 36_864,
        max_bitrate: 240_000_000,
    },
    Avc1Level {
        level: 52,
        macro_blocks_rate: 2_073_600,
        max_frame_size: 36_864,
        max_bitrate: 240_000_000,
    },
    Avc1Level {
        level: 60,
        macro_blocks_rate: 4_177_920,
        max_frame_size: 139_264,
        max_bitrate: 240_000_000,
    },
    Avc1Level {
        level: 61,
        macro_blocks_rate: 8_355_840,
        max_frame_size: 139_264,
        max_bitrate: 480_000_000,
    },
    Avc1Level {
        level: 62,
        macro_blocks_rate: 16_711_680,
        max_frame_size: 139_264,
        max_bitrate: 800_000_000,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn vp9_stream(profile: &str, pix_fmt: &str, height: i64) -> ffprobe::Stream {
        let mut s: ffprobe::Stream = serde_json::from_str("{}").unwrap();
        s.codec_name = "vp9".into();
        s.profile = Some(profile.into());
        s.pix_fmt = Some(pix_fmt.into());
        s.height = Some(height);
        s
    }

    #[test]
    fn external_subtitle_discovery() {
        let dir = std::env::temp_dir().join(format!("dim-subs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let video = dir.join("Show.S01E01.1080p.mkv");
        for f in [
            "Show.S01E01.1080p.mkv",
            "Show.S01E01.1080p.srt",           // bare sidecar
            "Show.S01E01.1080p.en.srt",        // 2-letter lang
            "Show.S01E01.1080p.ger.forced.srt",// 3-letter lang + flag
            "Show.S01E01.1080p.English.sdh.ass",
            "Show.S01E01.1080p.signs.vtt",     // unknown tag kept as label
            "Show.S01E02.1080p.en.srt",        // different episode — excluded
            "unrelated.srt",                   // different stem — excluded
        ] {
            std::fs::write(dir.join(f), b"1\n00:00:01,000 --> 00:00:02,000\nhi\n").unwrap();
        }

        let subs = find_external_subtitles(&video);
        let by_name: Vec<(String, Option<String>, String, &str)> = subs
            .iter()
            .map(|s| {
                (
                    s.path.file_name().unwrap().to_string_lossy().into_owned(),
                    s.lang.clone(),
                    s.label.clone(),
                    s.codec,
                )
            })
            .collect();

        assert_eq!(subs.len(), 5, "{:?}", by_name);
        assert!(by_name.contains(&(
            "Show.S01E01.1080p.srt".into(),
            None,
            "Unknown (External)".into(),
            "subrip"
        )));
        assert!(by_name.contains(&(
            "Show.S01E01.1080p.en.srt".into(),
            Some("eng".into()),
            "English (External)".into(),
            "subrip"
        )));
        assert!(by_name.contains(&(
            "Show.S01E01.1080p.ger.forced.srt".into(),
            Some("ger".into()),
            "German (Forced) (External)".into(),
            "subrip"
        )));
        assert!(by_name.contains(&(
            "Show.S01E01.1080p.English.sdh.ass".into(),
            Some("eng".into()),
            "English (SDH) (External)".into(),
            "ass"
        )));
        assert!(by_name.contains(&(
            "Show.S01E01.1080p.signs.vtt".into(),
            None,
            "Unknown [signs] (External)".into(),
            "webvtt"
        )));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vp9_codec_strings() {
        assert_eq!(get_vp9_tag(&vp9_stream("Profile 0", "yuv420p", 360)), "vp09.00.21.08");
        assert_eq!(get_vp9_tag(&vp9_stream("Profile 0", "yuv420p", 1080)), "vp09.00.40.08");
        assert_eq!(get_vp9_tag(&vp9_stream("Profile 2", "yuv420p10le", 2160)), "vp09.02.51.10");
        // missing metadata falls back to profile 0 / 8-bit / 1080p level
        let mut s = vp9_stream("Profile 0", "yuv420p", 0);
        s.profile = None;
        s.pix_fmt = None;
        s.height = None;
        assert_eq!(get_vp9_tag(&s), "vp09.00.40.08");
    }
}

//! Validate the fixed-duration DASH timeline before stream-copying video.
//!
//! `-hls_time` is a target, not a guarantee: copying video cannot insert
//! keyframes. A source with 10.344s GOPs produces 10.344s segments even when
//! the manifest promises 10s, accumulating over a minute of drift by 33:00.
//! Until direct streams have an indexed SegmentTimeline and matching seek
//! support, only offer them when every boundary fits the declared timeline.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, OnceCell};

pub const SEGMENT_SECONDS: u32 = 10;
// Absolute tolerance, never added to the next boundary. Allows timestamp
// rounding / frame reordering without allowing cumulative GOP drift.
const TOLERANCE_SECONDS: f64 = 0.1;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const CACHE_LIMIT: usize = 64;

#[derive(Clone, Eq, Hash, PartialEq)]
struct CacheKey {
    file: std::path::PathBuf,
    length: u64,
    modified: SystemTime,
    stream: i64,
    start_time: u64,
    ffprobe: String,
}

type ProbeCache = HashMap<CacheKey, Arc<OnceCell<bool>>>;
static CACHE: Lazy<Mutex<ProbeCache>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Packet-only probe: does not decode video. Rejects incompatible files early;
/// compatible files are scanned through EOF and cached by file fingerprint.
/// Errors / timeouts must fall back to transcoding, not fail playback.
pub async fn has_compatible_timing(
    ffprobe: &str,
    file: &Path,
    stream: i64,
    start_time: f64,
) -> io::Result<bool> {
    let metadata = tokio::fs::metadata(file).await?;
    let key = CacheKey {
        file: file.to_path_buf(),
        length: metadata.len(),
        modified: metadata.modified()?,
        stream,
        start_time: start_time.to_bits(),
        ffprobe: ffprobe.into(),
    };
    let cell = {
        let mut cache = CACHE.lock().await;
        if !cache.contains_key(&key) && cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.entry(key).or_default().clone()
    };
    let result = cell
        .get_or_try_init(|| async {
            tokio::time::timeout(PROBE_TIMEOUT, probe(ffprobe, file, stream, start_time))
                .await
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "direct-play timing probe timed out",
                    )
                })?
        })
        .await?;
    Ok(*result)
}

async fn probe(ffprobe: &str, file: &Path, stream: i64, start_time: f64) -> io::Result<bool> {
    let mut child = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            &stream.to_string(),
            "-show_packets",
            "-show_entries",
            "packet=pts_time,duration_time,flags",
            "-of",
            "compact=p=0:nk=0",
        ])
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut timing = TimingCheck::new(start_time);
    while let Some(line) = lines.next_line().await? {
        if !timing.packet(&line) {
            child.kill().await?;
            return Ok(false);
        }
    }
    if !child.wait().await?.success() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ffprobe could not validate video timing",
        ));
    }
    Ok(timing.first_pts.is_some())
}

struct TimingCheck {
    start_time: f64,
    first_pts: Option<f64>,
    next_segment: u32,
}

impl TimingCheck {
    fn new(start_time: f64) -> Self {
        Self {
            start_time,
            first_pts: None,
            next_segment: 1,
        }
    }

    fn packet(&mut self, line: &str) -> bool {
        if line.is_empty() {
            return true;
        }
        let mut pts = None;
        let mut flags = "";
        for field in line.split('|') {
            if let Some(value) = field.strip_prefix("pts_time=") {
                pts = value.parse::<f64>().ok().filter(|v| v.is_finite());
            } else if let Some(value) = field.strip_prefix("flags=") {
                flags = value;
            }
        }
        let Some(pts) = pts else { return false };
        if !self.start_time.is_finite() || flags.contains('C') || flags.contains('D') {
            return false;
        }
        let Some(first) = self.first_pts else {
            if !flags.contains('K') || (pts - self.start_time).abs() > TOLERANCE_SECONDS {
                return false;
            }
            self.first_pts = Some(pts);
            return true;
        };

        // Mirrors the HLS muxer's cumulative cut target relative to the
        // first video packet. Earlier scene-cut keyframes do not end a segment.
        let boundary = self.next_segment as f64 * SEGMENT_SECONDS as f64;
        if flags.contains('K') && pts - first >= boundary {
            if (pts - self.start_time - boundary).abs() > TOLERANCE_SECONDS {
                return false;
            }
            self.next_segment += 1;
        }

        // The target passed without a usable keyframe. No need to read the
        // rest of a large file just to discover its next GOP is too late.
        pts - self.start_time
            <= self.next_segment as f64 * SEGMENT_SECONDS as f64 + TOLERANCE_SECONDS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(points: &[(f64, bool)]) -> bool {
        let mut check = TimingCheck::new(0.0);
        points.iter().all(|(pts, key)| {
            check.packet(&format!(
                "pts_time={pts:.6}|duration_time=0.041708|flags={}",
                if *key { "K__" } else { "___" }
            ))
        })
    }

    #[test]
    fn accepts_aligned_keyframes_and_ignores_scene_cuts() {
        assert!(check(&[
            (0.0, true),
            (3.0, true),
            (9.95, false),
            (10.0, true),
            (15.0, true),
            (20.0, true),
            (29.99, false)
        ]));
    }

    #[test]
    fn rejects_reacher_gops_instead_of_accumulating_68_seconds_of_drift() {
        assert!(!check(&[(0.0, true), (10.343, true), (20.687, true)]));
    }

    #[test]
    fn validates_later_boundaries_too() {
        assert!(!check(&[
            (0.0, true),
            (10.0, true),
            (20.0, true),
            (30.344, true)
        ]));
    }

    #[test]
    fn keyframe_before_cut_target_cannot_satisfy_the_boundary() {
        assert!(!check(&[(0.0, true), (9.999999, true), (10.2, false)]));
    }

    #[test]
    fn rounding_tolerance_does_not_accumulate() {
        assert!(check(&[
            (0.0, true),
            (10.01, true),
            (20.02, true),
            (30.03, true)
        ]));
        let points: Vec<_> = (0..=20).map(|n| (n as f64 * 10.01, true)).collect();
        assert!(!check(&points));
    }

    #[test]
    fn rejects_missing_boundary_even_if_no_later_keyframe_exists() {
        assert!(!check(&[(0.0, true), (10.2, false)]));
    }

    #[test]
    fn accepts_short_final_segment() {
        assert!(check(&[(0.0, true), (10.0, true), (17.5, false)]));
    }

    #[test]
    fn handles_container_offset_and_reordered_frames() {
        let mut timing = TimingCheck::new(100.0);
        for line in [
            "pts_time=100|flags=K__",
            "pts_time=100.12|flags=___",
            "pts_time=100.04|flags=___",
            "pts_time=110|flags=K__",
        ] {
            assert!(timing.packet(line));
        }
        assert!(!check(&[(1.0, true)]));
    }

    #[test]
    fn unknown_or_corrupt_timestamps_are_not_approved() {
        for line in [
            "pts_time=N/A|flags=K__",
            "pts_time=NaN|flags=K__",
            "pts_time=inf|flags=K__",
            "pts_time=0|flags=___",
            "pts_time=0|flags=KC_",
        ] {
            assert!(!TimingCheck::new(0.0).packet(line));
        }
    }
}

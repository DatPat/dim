//! Exercises the real packet probe. Requires ffmpeg / ffprobe on PATH.
use dim_core::streaming::direct_play::has_compatible_timing;
use dim_core::streaming::ffprobe::FFProbeCtx;
use std::path::Path;
use std::process::{Command, Stdio};

fn video(path: &Path, gop: &str) {
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=size=32x32:rate=25:duration=31",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            gop,
            "-keyint_min",
            gop,
            "-sc_threshold",
            "0",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .expect("ffmpeg is required for this integration test");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "requires ffmpeg with libx264 and ffprobe on PATH"]
async fn real_probe_accepts_aligned_video_and_invalidates_cache_when_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("timing.mkv");
    video(&file, "250"); // 25fps * 10s
    assert!(has_compatible_timing("ffprobe", &file, 0, 0.0)
        .await
        .unwrap());
    assert!(has_compatible_timing("ffprobe", &file, 0, 0.0)
        .await
        .unwrap());

    // Same pathname, different source: the cached approval must not survive.
    video(&file, "259"); // 10.36s, like the reported Reacher source
    assert!(!has_compatible_timing("ffprobe", &file, 0, 0.0)
        .await
        .unwrap());
}

#[tokio::test]
#[ignore = "requires ffmpeg with libx264 and ffprobe on PATH"]
async fn probe_failure_is_an_error_not_a_direct_play_approval() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("timing.mkv");
    video(&file, "250");
    assert!(
        has_compatible_timing("missing-ffprobe-binary", &file, 0, 0.0)
            .await
            .is_err()
    );
    // Failure must not poison a later successful probe.
    assert!(has_compatible_timing("ffprobe", &file, 0, 0.0)
        .await
        .unwrap());
}

#[tokio::test]
#[ignore = "set DIM_TIMING_SAMPLE to an incompatible media sample; requires ffprobe"]
async fn affected_media_sample_falls_back_to_transcoding() {
    let file = std::env::var("DIM_TIMING_SAMPLE").expect("set DIM_TIMING_SAMPLE");
    let info = FFProbeCtx::new("ffprobe").get_meta(&file).await.unwrap();
    let stream = info.get_primary("video").unwrap();
    assert!(!has_compatible_timing(
        "ffprobe",
        Path::new(&file),
        stream.index,
        info.get_start_time().unwrap(),
    )
    .await
    .unwrap());
}

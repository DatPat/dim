//! Integration test: AV1 → H264 transcode through nightfall session manager.
//!
//! Exercises the same code path that the streaming endpoint uses:
//! profile_init → create session → chunk_init_request → chunk_request
//!
//! Run with:
//!   cargo test --test av1_transcode -- --nocapture

use nightfall::profiles::*;
use std::time::Duration;
use xtra::spawn::Tokio;

/// Create a short AV1 test file if it doesn't exist.
fn ensure_test_file() -> String {
    let path = "/tmp/dim-test/test_av1.mkv";
    if std::path::Path::new(path).exists() {
        return path.into();
    }
    let _ = std::fs::create_dir_all("/tmp/dim-test");
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f", "lavfi", "-i", "testsrc=duration=30:size=1920x960:rate=24",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=30",
            "-c:v", "libsvtav1", "-preset", "12", "-crf", "40",
            "-c:a", "ac3", "-ac", "6",
            "-t", "30",
            path,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("ffmpeg not found");
    assert!(status.success(), "Failed to create test file");
    path.into()
}

#[tokio::test]
async fn test_av1_to_h264_session_lifecycle() {
    tracing_subscriber::fmt()
        .with_env_filter("nightfall=debug,info")
        .init();

    let test_file = ensure_test_file();

    // Initialize profiles just like dim's main.rs does
    profiles_init("ffmpeg".into());

    let active = get_active_profiles();
    eprintln!("\n=== Active profiles ===");
    for p in &active {
        eprintln!("  {} (tag={}, type={:?}, stream={:?})",
            p.name(), p.tag(), p.profile_type(), p.stream_type());
    }

    // Build the profile context matching what dim does for 1080p transcode
    let ctx = ProfileContext {
        file: test_file,
        input_ctx: InputCtx {
            stream: 0,
            codec: "av1".into(),
            pix_fmt: "yuv420p".into(),
            profile: "Main".into(),
            ..Default::default()
        },
        output_ctx: OutputCtx {
            codec: "h264".into(),
            start_num: 0,
            bitrate: Some(10_000_000),
            height: Some(1080),
            ..Default::default()
        },
        ..Default::default()
    };

    // Get the profile chain
    let profile_chain = get_profile_for(StreamType::Video, &ctx);
    eprintln!("\n=== Profile chain for AV1 → H264 ===");
    for p in &profile_chain {
        eprintln!("  {} (tag={}, type={:?})", p.name(), p.tag(), p.profile_type());
    }
    assert!(!profile_chain.is_empty(), "No profiles available for AV1 → H264");

    // Print the ffmpeg args that the first-choice profile would generate
    let first_choice = profile_chain.last().unwrap(); // last = highest priority (popped first)
    eprintln!("\n=== First-choice profile: {} ===", first_choice.name());
    if let Some(args) = first_choice.build(ctx.clone()) {
        eprintln!("ffmpeg args:\n  ffmpeg {}", args.join(" \\\n    "));
    }

    // Create state manager
    let outdir = "/tmp/dim-test/sessions";
    let _ = std::fs::remove_dir_all(outdir);
    let state = nightfall::StateManager::new(&mut Tokio::Global, outdir.into(), "ffmpeg".into());

    // Create session (same as dim's state.create(profile_chain, ctx))
    let session_id = state.create(profile_chain, ctx).await.unwrap();
    eprintln!("\n=== Session created: {} ===", session_id);

    // Spawn GC task like dim does
    let state_gc = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(1000));
        loop {
            interval.tick().await;
            let _ = state_gc.garbage_collect().await;
        }
    });

    // Request init.mp4 — polls until the first chunk is ready
    eprintln!("\n=== Requesting init.mp4 (chunk 0) ===");
    let init_result = timeout_poll(
        || state.chunk_init_request(session_id.clone(), 0),
        Duration::from_millis(100),
        150, // 15 seconds
    ).await;

    match &init_result {
        Ok(path) => eprintln!("  init.mp4 ready: {}", path),
        Err(e) => eprintln!("  init.mp4 FAILED: {:?}", e),
    }
    let init_path = init_result.expect("init.mp4 should be ready within 15 seconds");
    assert!(std::path::Path::new(&init_path).exists(), "init.mp4 file should exist");

    // Request chunks 0-2
    for chunk in 0..3 {
        eprintln!("\n=== Requesting chunk {} ===", chunk);
        let chunk_result = timeout_poll(
            || state.chunk_request(session_id.clone(), chunk),
            Duration::from_millis(100),
            100,
        ).await;

        match &chunk_result {
            Ok(path) => {
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                eprintln!("  chunk {} ready: {} ({} bytes)", chunk, path, size);
            }
            Err(e) => eprintln!("  chunk {} FAILED: {:?}", chunk, e),
        }
        chunk_result.unwrap_or_else(|_| panic!("chunk {} should be ready", chunk));
    }

    // Check ffmpeg stderr for errors
    eprintln!("\n=== ffmpeg stderr ===");
    match state.get_stderr(session_id.clone()).await {
        Ok(stderr) => {
            // Print last 500 chars
            let start = stderr.len().saturating_sub(500);
            eprintln!("{}", &stderr[start..]);
        }
        Err(e) => eprintln!("  Could not get stderr: {:?}", e),
    }

    // Clean up
    let _ = state.die(session_id).await;
    eprintln!("\n=== Test passed ===");
}

/// Polls an async function until it returns Ok or we hit the tick limit.
/// Same logic as dim-web's timeout_segment.
async fn timeout_poll<F, Fut, T>(
    f: F,
    tick_dur: Duration,
    tick_limit: usize,
) -> Result<T, nightfall::error::NightfallError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, nightfall::error::NightfallError>>,
{
    for tick in 0..tick_limit {
        match f().await {
            Err(nightfall::error::NightfallError::ChunkNotDone) => {
                if tick % 20 == 0 {
                    eprintln!("  ... waiting (tick {}/{})", tick, tick_limit);
                }
                tokio::time::sleep(tick_dur).await;
            }
            other => return other,
        }
    }
    Err(nightfall::error::NightfallError::ChunkNotDone)
}

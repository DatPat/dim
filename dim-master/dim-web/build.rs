use std::env;
use std::error::Error;
use std::path::Path;
use std::process::Command;

fn main() -> Result<(), Box<dyn Error>> {
    let out_dir = env::var("CARGO_TARGET_DIR")
        .unwrap_or_else(|_| {
            let out = env::var("OUT_DIR").unwrap();
            let mut p = std::path::PathBuf::from(&out);
            for _ in 0..4 { p.pop(); }
            p.to_string_lossy().into_owned()
        });

    let db_file = format!("{out_dir}/dim_dev.db");
    println!("cargo:rustc-env=DATABASE_URL=sqlite://{db_file}");

    // Prefer env var overrides (set by Docker build args) over git commands,
    // since .git is not available inside the Docker build context.
    let git_tag = env::var("GIT_TAG").ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            Command::new("git")
                .args(&["describe", "--abbrev=0"])
                .output()
                .ok()
                .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
                .unwrap_or_else(|| "unknown".into())
        });
    println!("cargo:rustc-env=GIT_TAG={}", git_tag.trim());

    let git_sha = env::var("GIT_SHA").ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            Command::new("git")
                .args(&["rev-parse", "HEAD"])
                .output()
                .ok()
                .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
                .unwrap_or_else(|| "unknown".into())
        });
    println!("cargo:rustc-env=GIT_SHA_256={}", git_sha.trim());

    // Build timestamp (UTC)
    let build_time = Command::new("date")
        .args(&["-u", "+%Y-%m-%d %H:%M UTC"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=BUILD_TIME={}", build_time.trim());

    // Target architecture
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=BUILD_ARCH={}", arch);

    // Build profile (release vs debug)
    let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=BUILD_PROFILE={}", profile);

    // CPU target features (e.g. avx2, neon, sse4.1)
    let features: Vec<String> = env::vars()
        .filter_map(|(k, _)| {
            k.strip_prefix("CARGO_CFG_TARGET_FEATURE_")
                .map(|f| f.to_lowercase())
        })
        .collect();
    // CARGO_CFG_TARGET_FEATURE is a single comma-separated var
    let features_csv = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
    let cpu_features = if features_csv.is_empty() && features.is_empty() {
        "none".to_string()
    } else if !features_csv.is_empty() {
        features_csv
    } else {
        features.join(",")
    };
    println!("cargo:rustc-env=BUILD_CPU_FEATURES={}", cpu_features);

    if Path::new("../ui/build").exists() {
        println!("cargo:rustc-cfg=feature=\"embed_ui\"");
    } else {
        println!("cargo:warning=`ui/build` does not exist.");
        println!("cargo:warning=If you wish to embed the webui, run `yarn build` in `ui`.");
    }

    println!("cargo:rerun-if-changed=ui/build");
    println!("cargo:rerun-if-changed=build.rs");
    // Write a timestamp file so the next build sees it changed → build.rs always reruns
    let ts_path = std::path::PathBuf::from(env::var("OUT_DIR").unwrap()).join(".build_ts");
    std::fs::write(&ts_path, build_time.trim()).ok();
    println!("cargo:rerun-if-changed={}", ts_path.display());

    Ok(())
}

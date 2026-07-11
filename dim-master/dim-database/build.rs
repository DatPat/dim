use std::env;
use std::error::Error;
use std::fs;
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let out_dir = env::var("CARGO_TARGET_DIR")
        .unwrap_or_else(|_| {
            let out = env::var("OUT_DIR").unwrap();
            let mut p = std::path::PathBuf::from(&out);
            for _ in 0..4 { p.pop(); }
            p.to_string_lossy().into_owned()
        });

    let db_file = format!("{out_dir}/dim_dev.db");
    println!("cargo:rustc-env=DATABASE_URL=sqlite://{db_file}");
    println!(
        "cargo:warning=Generating {:?} from latest migrations.",
        db_file
    );

    let _ = fs::remove_file(&db_file);

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::from_str(db_file.as_ref())?.create_if_missing(true),
        )
        .await?;

    // Load migrations from disk at RUNTIME of this script. The macro form
    // (`sqlx::migrate!()`) embeds the migration set when the build script
    // BINARY is compiled — `rerun-if-changed` re-executes the script but
    // doesn't recompile it, so newly added migrations silently didn't apply
    // to the compile-time dev database.
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")?;
    let migrations = std::path::Path::new(&manifest_dir).join("migrations");
    sqlx::migrate::Migrator::new(migrations)
        .await?
        .run(&pool)
        .await
        .map_err(|e| {
            println!("cargo:error=Migration failed: {:?}", e);
            e
        })?;

    println!("cargo:warning=Built database {}.", db_file);

    // Paths are relative to the crate root. The old values pointed at a
    // nonexistent `database/` prefix, so the build script binary (which embeds
    // the migration set via `sqlx::migrate!` at ITS compile time) was never
    // rebuilt when migrations changed — new migrations silently didn't apply
    // to the compile-time dev database.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=migrations");

    Ok(())
}

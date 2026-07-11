//! Module contains all the code for the new generation media scanner.

pub mod daemon;
pub mod error;
mod mediafile;
pub mod movie;
pub mod nfo;
pub mod smart_scan;
#[cfg(test)]
mod tests;
pub mod tv_show;

use self::mediafile::Error as CreatorError;
use self::mediafile::MediafileCreator;
use crate::core::EventTx;

use async_trait::async_trait;

use dim_database::library::Library;
use dim_database::library::MediaType;
use dim_database::mediafile::InsertableMediaFile;
use dim_database::mediafile::MediaFile;

use dim_extern_api::filename::detect_language;
use dim_extern_api::filename::normalize_title;
use dim_extern_api::filename::strip_release_group;
use dim_extern_api::filename::Anitomy;
use dim_extern_api::filename::CombinedExtractor;
use dim_extern_api::filename::FilenameMetadata;
use dim_extern_api::filename::Metadata;
use dim_extern_api::filename::TorrentMetadata;
use dim_extern_api::ExternalQueryIntoShow;

use futures::FutureExt;
use ignore::WalkBuilder;
use itertools::Itertools;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use tracing::error;
use tracing::info;
use tracing::instrument;
use tracing::warn;

pub use error::Error;

pub(super) static SUPPORTED_EXTS: &[&str] = &[
    "001", "3g2", "3gp", "amv", "asf", "asx", "avi", "bin", "bivx", "divx", "dv", "dvr-ms", "f4v",
    "fli", "flv", "ifo", "img", "iso", "m2t", "m2ts", "m2v", "m4v", "mkv", "mk3d", "mov", "mp4",
    "mpe", "mpeg", "mpg", "mts", "mxf", "nrg", "nsv", "nuv", "ogg", "ogm", "ogv", "pva", "qt",
    "rec", "rm", "rmvb", "strm", "svq3", "tp", "ts", "ty", "viv", "vob", "vp3", "webm", "wmv",
    "wtv", "xvid",
];

/// Function recursively walks the paths passed and returns all files in those directories.
/// FIXME: THIS IS NOT ASYNC-SAFE!!!
/// NOTE: I've noticed that walking a directory mounted over ssh is very slow, 80 files in like 300
/// seconds. Doubt theres a way to fix this but we could alliviate the UX-degradation by sending
/// the files over a channel instead of returning them at once.
pub fn get_subfiles(paths: impl Iterator<Item = impl AsRef<Path>>) -> Vec<PathBuf> {
    let mut files = Vec::with_capacity(2048);
    for path in paths {
        let mut subfiles = WalkBuilder::new(path)
            // we want to follow all symlinks in case of complex dir structures
            .follow_links(true)
            .add_custom_ignore_filename(".plexignore")
            .build()
            .filter_map(Result::ok)
            // ignore all hidden files.
            .filter(|f| {
                !f.path()
                    .iter()
                    .any(|s| s.to_str().map(|x| x.starts_with('.')).unwrap_or(false))
            })
            // check whether `f` has a supported extension
            .filter(|f| {
                f.path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .map_or(false, |e| SUPPORTED_EXTS.contains(&e))
            })
            .map(|f| f.into_path())
            .collect();

        files.append(&mut subfiles);
    }

    files
}

/// Returns true if a directory name is generic and should not be used as a title fallback.
pub(crate) fn is_generic_dir_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    // Reject season directories like "Season 01", "S01", etc.
    if lower.starts_with("season") || lower.starts_with("series") {
        return true;
    }
    if lower.len() <= 3 && lower.starts_with('s') && lower[1..].chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    matches!(
        lower.as_str(),
        "subs" | "subtitles" | "extras" | "extra" | "featurettes" | "behind the scenes"
            | "deleted scenes" | "interviews" | "scenes" | "shorts" | "trailers"
            | "media" | "movies" | "tv" | "tv shows" | "videos" | "video"
            | "downloads" | "tmp" | "temp"
    )
}

/// Split a trailing "(YYYY)" disambiguation year off a title:
/// "Berserk (2016)" → ("Berserk", Some(2016)).
pub(crate) fn split_trailing_year(name: &str) -> (String, Option<i64>) {
    let trimmed = name.trim_end();
    if let Some(open) = trimmed.rfind('(') {
        if trimmed.ends_with(')') {
            let inner = &trimmed[open + 1..trimmed.len() - 1];
            if inner.len() == 4 && inner.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(year) = inner.parse::<i64>() {
                    if (1900..=2100).contains(&year) {
                        return (trimmed[..open].trim_end().to_string(), Some(year));
                    }
                }
            }
        }
    }
    (name.to_string(), None)
}

/// Extracts a season number from a directory name like "Season 1", "Series 02", "S01", "Staffel 3".
pub(crate) fn extract_season_from_dir(name: &str) -> Option<i64> {
    let lower = name.to_lowercase();
    let lower = lower.trim();

    for prefix in &["season", "series", "staffel"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            return rest.trim().parse::<i64>().ok();
        }
    }
    // "S01", "s1" — short form
    if lower.starts_with('s') && lower.len() <= 4 {
        return lower[1..].parse::<i64>().ok();
    }
    None
}

/// Find a `SxxEyy` pattern (case-insensitive) in `s`, returning
/// (start of 'S', end index exclusive). Requires a word boundary before the
/// 'S' and a non-digit after the episode digits.
fn find_sxx_eyy(s: &str) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if (b[i] == b's' || b[i] == b'S')
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
        {
            let mut j = i + 1;
            let season_start = j;
            while j < b.len() && b[j].is_ascii_digit() && j - season_start < 2 {
                j += 1;
            }
            if j > season_start && j < b.len() && (b[j] == b'e' || b[j] == b'E') {
                let ep_start = j + 1;
                let mut k = ep_start;
                while k < b.len() && b[k].is_ascii_digit() && k - ep_start < 4 {
                    k += 1;
                }
                if k > ep_start && (k == b.len() || !b[k].is_ascii_digit()) {
                    return Some((i, k));
                }
            }
        }
        i += 1;
    }
    None
}

/// Pre-parse filename normalization that removes tokens known to confuse the
/// filename parsers into extracting bogus season/episode numbers:
///
/// - bracketed CRC32 hashes (`[8A3AE688]` → episode 688)
/// - usenet-style UUIDs (`18a9b70c-f419-4962-a072-2b2e54543d16` → episode 545)
/// - episode version suffixes (`S01E04v2` breaks SxxEyy extraction)
/// - separators glued to SxxEyy (`My.Love.Story-S01E02` parses as one title)
/// - `-NN END` finale markers (`Dasu-23 END` hides the trailing episode)
pub(crate) fn preclean_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());

    // Strip bracketed 8-char hex hashes.
    let mut rest = name;
    while let Some(open) = rest.find('[') {
        let candidate = &rest[open + 1..];
        if candidate.len() >= 9
            && candidate.as_bytes()[8] == b']'
            && candidate[..8].chars().all(|c| c.is_ascii_hexdigit())
        {
            out.push_str(&rest[..open]);
            rest = &rest[open + 10..];
        } else {
            out.push_str(&rest[..open + 1]);
            rest = &rest[open + 1..];
        }
    }
    out.push_str(rest);

    // Strip UUIDs (8-4-4-4-12 hex groups).
    let is_uuid_at = |b: &[u8], i: usize| -> bool {
        if i + 36 > b.len() {
            return false;
        }
        let groups = [8usize, 4, 4, 4, 12];
        let mut p = i;
        for (gi, glen) in groups.iter().enumerate() {
            if !b[p..p + glen].iter().all(|c| c.is_ascii_hexdigit()) {
                return false;
            }
            p += glen;
            if gi < 4 {
                if b[p] != b'-' {
                    return false;
                }
                p += 1;
            }
        }
        true
    };
    let bytes = out.as_bytes().to_vec();
    let mut cleaned = String::with_capacity(out.len());
    let mut i = 0;
    while i < bytes.len() {
        if is_uuid_at(&bytes, i) {
            i += 36;
        } else {
            cleaned.push(bytes[i] as char);
            i += 1;
        }
    }
    let mut s = cleaned;

    // Strip version suffix directly after SxxEyy ("S01E04v2" → "S01E04") and
    // un-glue a separator right before it ("Story-S01E02" → "Story S01E02").
    if let Some((start, end)) = find_sxx_eyy(&s) {
        let b = s.as_bytes();
        if end < b.len() && (b[end] == b'v' || b[end] == b'V') {
            let mut k = end + 1;
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            if k > end + 1 {
                s = format!("{}{}", &s[..end], &s[k..]);
            }
        }
        if start > 0 {
            let prev = s.as_bytes()[start - 1];
            if prev == b'-' || prev == b'.' || prev == b'_' {
                s.replace_range(start - 1..start, " ");
            }
        }
    }

    // "-23 END" → "-23" so the trailing-episode cleanup can see the number.
    for marker in [" END", " end", " FIN", " FINAL"] {
        if let Some(stripped) = s.strip_suffix(marker) {
            if stripped
                .rfind('-')
                .map_or(false, |d| stripped[d + 1..].chars().all(|c| c.is_ascii_digit()) && !stripped[d + 1..].is_empty())
            {
                s = stripped.to_string();
            }
            break;
        }
        // Also handle "…-23 END [BD …]" where a bracket group follows.
        if let Some(pos) = s.find(&format!("{} [", marker)) {
            let (head, tail) = s.split_at(pos);
            if head
                .rfind('-')
                .map_or(false, |d| head[d + 1..].chars().all(|c| c.is_ascii_digit()) && !head[d + 1..].is_empty())
            {
                s = format!("{}{}", head, &tail[marker.len()..]);
                break;
            }
        }
    }

    s
}

pub fn parse_filenames(
    files: impl Iterator<Item = impl AsRef<Path>>,
) -> Vec<(PathBuf, Vec<Metadata>)> {
    let mut metadata = Vec::new();

    for file in files {
        let raw_filename = match file.as_ref().file_stem().and_then(OsStr::to_str) {
            Some(x) => x,
            None => {
                warn!(file = ?file.as_ref(), "Received a filename that is not unicode");
                continue;
            }
        };

        // Strip outer parentheses that some naming schemes use,
        // e.g. "(Vigilante S01E08)" → "Vigilante S01E08".
        let filename = raw_filename
            .strip_prefix('(')
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(raw_filename);

        let filename = &preclean_filename(filename);

        let mut metas = IntoIterator::into_iter([
            TorrentMetadata::from_str(&filename),
            Anitomy::from_str(&filename),
            CombinedExtractor::from_str(&filename),
        ])
        .filter_map(|x| x)
        .collect::<Vec<_>>();

        // Fix titles that have a trailing episode number stuck in them
        // (e.g. "Seihantai na Kimi to Boku-04" → title "Seihantai na Kimi to Boku", ep 4).
        // Also handles version suffixes like "-09v2".
        // Also clean up trailing " [" garbage from TNP.
        for meta in &mut metas {
            let name = meta.name.trim_end_matches(" [").trim().to_string();
            // Check for trailing "-NNvN" or "-NN" pattern.
            if let Some(dash_pos) = name.rfind('-') {
                let after_dash = &name[dash_pos + 1..];
                // Strip optional version suffix (e.g. "09v2" → "09")
                let digits_part = after_dash
                    .find(|c: char| !c.is_ascii_digit())
                    .map(|i| &after_dash[..i])
                    .unwrap_or(after_dash);
                if !digits_part.is_empty() && digits_part.len() <= 3 {
                    // Verify the rest after digits is either empty or a version tag (vN)
                    let rest = &after_dash[digits_part.len()..];
                    let is_version_suffix = rest.is_empty()
                        || (rest.starts_with('v')
                            && rest[1..].chars().all(|c| c.is_ascii_digit()));
                    if is_version_suffix {
                        let ep: i64 = digits_part.parse().unwrap_or(0);
                        if ep > 0 && meta.episode.is_none() {
                            meta.episode = Some(ep);
                        }
                        meta.name = name[..dash_pos].trim().to_string();
                        continue;
                    }
                }
            }
            if name != meta.name {
                meta.name = name;
            }
        }

        // Sanitize obviously bogus season/episode values that come from
        // resolution strings (e.g. "1920x1080" → season=1920) or hash values
        // (e.g. "[47218EED]" → season=47218) being misinterpreted as metadata.
        {
            // Common video resolution widths and heights, used to detect when
            // parsers misinterpret "WIDTHxHEIGHT" as year + episode.
            const RES_W: &[i64] = &[640, 720, 1280, 1920, 2560, 3840, 7680];
            const RES_H: &[i64] = &[360, 480, 576, 720, 1080, 1440, 2160, 4320];

            for meta in &mut metas {
                if meta.season.map_or(false, |s| s > 100) {
                    meta.season = None;
                }
                if meta.episode.map_or(false, |e| e > 10000) {
                    meta.episode = None;
                }
                // Episode numbers are 1-based; a parsed 0 is always garbage
                // (e.g. a hex hash ending in "0").
                if meta.episode == Some(0) {
                    meta.episode = None;
                }
                // If year matches a resolution width and episode matches a
                // resolution height, both are from "WIDTHxHEIGHT" misparse.
                if let (Some(y), Some(e)) = (meta.year, meta.episode) {
                    if RES_W.contains(&y) && RES_H.contains(&e) {
                        meta.year = None;
                        meta.episode = None;
                    }
                }
                // A "year" that matches a resolution width AND appears as
                // "WIDTHx" in the filename (e.g. "1920x1080") is a misparse.
                if let Some(y) = meta.year {
                    if RES_W.contains(&y) && filename.contains(&format!("{}x", y)) {
                        meta.year = None;
                    }
                }
            }
        }

        // Detect language tag from the full filename (e.g. "German" → "de-DE").
        let language = detect_language(filename);

        // Set the detected language on all metadata entries.
        for meta in &mut metas {
            if meta.language.is_none() {
                meta.language = language.clone();
            }
        }

        // Parent directory fallback: use the parent dir name as a title if it
        // looks like one. If the immediate parent is generic (e.g. "Season 1"),
        // also check the grandparent — TV shows are commonly structured as
        // "Show Name/Season 1/episode.mkv".
        let parent = file.as_ref().parent();
        let parent_name = parent
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str());

        // A season directory ("Season 2", "S03", …) is authoritative: it
        // beats whatever the filename parsers guessed (which is frequently a
        // hash fragment or a "part 2" marker).
        let dir_season = parent_name.and_then(extract_season_from_dir);
        if let Some(s) = dir_season {
            for meta in &mut metas {
                meta.season = Some(s);
            }
        }

        let fallback_dir_name = match parent_name {
            Some(pname) => {
                let cleaned = normalize_title(pname);
                if !cleaned.is_empty() && !is_generic_dir_name(&cleaned) {
                    Some(cleaned)
                } else {
                    // Parent is generic (e.g. "Season 1"), try grandparent.
                    parent
                        .and_then(|p| p.parent())
                        .and_then(|gp| gp.file_name())
                        .and_then(|n| n.to_str())
                        .map(|gpname| normalize_title(gpname))
                        .filter(|c| !c.is_empty() && !is_generic_dir_name(c))
                }
            }
            None => None,
        };

        if let Some(dir_name) = fallback_dir_name {
            // Show folders often carry a disambiguation year — "Berserk (2016)"
            // — which belongs in the year field, not the search string.
            let (dir_name, dir_year) = split_trailing_year(&dir_name);
            // Inherit season/episode from the first metadata entry that has
            // plausible values, rather than blindly from the first entry
            // (which may have bogus parser output like season=None after sanitization).
            let best_season = dir_season.or_else(|| metas.iter().find_map(|m| m.season));
            let best_episode = metas.iter().find_map(|m| m.episode);
            metas.push(Metadata {
                name: dir_name,
                year: dir_year,
                season: best_season,
                episode: best_episode,
                language: language.clone(),
            });
        }

        if metas.is_empty() {
            warn!(file = ?file.as_ref(), "Failed to parse the filename and extract metadata.");
            continue;
        }

        // Release group fallback: for every parser result whose title starts
        // with what looks like a release group prefix (e.g. "hdmedia grinch"),
        // add an extra metadata entry with the prefix stripped ("grinch").
        // Applying this to only the first entry made sibling files of the same
        // show search different titles depending on which parser won, which
        // fed duplicate-show matches.
        let stripped_variants: Vec<Metadata> = metas
            .iter()
            .filter_map(|meta| {
                strip_release_group(&meta.name)
                    .filter(|s| !s.is_empty())
                    .map(|stripped| Metadata {
                        name: stripped,
                        year: meta.year,
                        season: meta.season,
                        episode: meta.episode,
                        language: meta.language.clone(),
                    })
            })
            .collect();
        metas.extend(stripped_variants);

        // Deduplicate by (lowercase name, year) to avoid redundant TMDB searches.
        // When merging duplicates, keep the most complete entry (prefer one with
        // season/episode filled in, since some parsers extract more fields).
        metas.dedup_by(|a, b| {
            let same = a.name.to_lowercase() == b.name.to_lowercase() && a.year == b.year;
            if same {
                // Merge: fill in b's missing fields from a (b is kept, a is removed).
                if b.season.is_none() && a.season.is_some() {
                    b.season = a.season;
                }
                if b.episode.is_none() && a.episode.is_some() {
                    b.episode = a.episode;
                }
                if b.language.is_none() && a.language.is_some() {
                    b.language = a.language.clone();
                }
            }
            same
        });

        metadata.push((file.as_ref().into(), metas));
    }

    metadata
}

#[derive(Clone)]
pub struct WorkUnit(pub MediaFile, pub Vec<Metadata>);

/// Trait that must be implemented by a media matcher. Matchers are responsible for fetching their
/// own external metadata but it is provided a metadata provider at initialization time.
#[async_trait]
pub trait MediaMatcher: Send + Sync {
    async fn batch_match(
        &self,
        tx: &mut dim_database::Transaction<'_>,
        provider: Arc<dyn ExternalQueryIntoShow>,
        work: Vec<WorkUnit>,
    ) -> Result<(), Error>;

    /// Match a WorkUnit to a specific external id.
    async fn match_to_id(
        &self,
        tx: &mut dim_database::Transaction<'_>,
        provider: Arc<dyn ExternalQueryIntoShow>,
        work: WorkUnit,
        external_id: &str,
    ) -> Result<(), Error>;
}

pub async fn insert_mediafiles(
    conn: &mut dim_database::DbConnection,
    library_id: i64,
    dirs: Vec<impl AsRef<Path> + Send + 'static>,
) -> Result<Vec<WorkUnit>, Error> {
    let now = Instant::now();
    let subfiles = tokio::task::spawn_blocking(|| get_subfiles(dirs.into_iter()))
        .await
        .unwrap();
    let elapsed = now.elapsed();

    info!(
        elapsed_ms = elapsed.as_millis(),
        files = subfiles.len(),
        "Walked all target directories."
    );

    let parsed = parse_filenames(subfiles.iter());

    let mut instance = MediafileCreator::new(conn.clone(), library_id).await;

    let insertable_futures =
        parsed
            .clone()
            .into_iter()
            .map(|(path, meta)| instance.construct_mediafile(path, meta[0].clone()).boxed())
            .chunks(4)
            .into_iter()
            .map(|chunk| chunk.collect())
            .collect::<Vec<
                Vec<
                    Pin<Box<dyn Future<Output = Result<InsertableMediaFile, CreatorError>> + Send>>,
                >,
            >>();

    let mut insertables = vec![];

    for chunk in insertable_futures.into_iter() {
        let results: Vec<Result<InsertableMediaFile, CreatorError>> =
            futures::future::join_all(chunk).await;

        for result in results {
            match result {
                Ok(mfile) => insertables.push(mfile),
                Err(CreatorError::FileExists) => continue,
                Err(e) => {
                    // One unreadable/broken file must not abort the whole
                    // library scan — skip it and index the rest.
                    error!(error = ?e, "Failed to construct mediafile, skipping file.");
                    continue;
                }
            }
        }
    }

    let mut mediafiles = vec![];

    for chunk in insertables.chunks(256) {
        mediafiles.append(&mut instance.insert_batch(chunk.iter()).await?);
    }

    // Build a map from file path to parsed metadata so we can pair each
    // inserted mediafile with its metadata regardless of skipped entries.
    let meta_map: HashMap<String, Vec<Metadata>> = parsed
        .into_iter()
        .filter_map(|(path, meta)| path.to_str().map(|s| (s.to_owned(), meta)))
        .collect();

    Ok(mediafiles
        .into_iter()
        .filter_map(|mfile| {
            meta_map
                .get(&mfile.target_file)
                .map(|meta| WorkUnit(mfile, meta.clone()))
        })
        .collect())
}

#[instrument(skip(conn, dirs, tx))]
pub async fn start_custom(
    conn: &mut dim_database::DbConnection,
    library_id: i64,
    dirs: Vec<impl AsRef<Path> + Send + 'static>,
    tx: EventTx,
    media_type: MediaType,
    provider: Arc<dyn ExternalQueryIntoShow>,
) -> Result<(), Error> {
    info!(library_id, "Scanning library");

    tx.send(
        dim_events::Message {
            id: library_id,
            event_type: dim_events::PushEventType::EventStartedScanning,
        }
        .to_string(),
    )
    .map_err(|x| Error::EventDispatch(x.into()))?;

    let matcher = match media_type {
        MediaType::Movie => Arc::new(movie::MovieMatcher) as Arc<dyn MediaMatcher>,
        MediaType::Tv => Arc::new(tv_show::TvMatcher) as Arc<dyn MediaMatcher>,
        _ => unimplemented!(),
    };

    let now = Instant::now();
    let workunits = insert_mediafiles(conn, library_id, dirs).await?;
    let workunits_size = workunits.len();

    info!(
        library_id,
        units = workunits_size,
        elapsed_ms = now.elapsed().as_millis(),
        "Walked and inserted mediafiles."
    );

    // NOTE: itertools::GroupBy is used across an await point and thus must also be Sync. This
    // breaks some of our higher-level logic where we spawn this task. Thus we collect it before
    // we proceed consuming it.
    let chunk_iter = workunits
        .into_iter()
        .chunks(128)
        .into_iter()
        .map(|x| x.collect())
        .collect::<Vec<_>>();

    // TODO: We can receive work over a channel so that we can in parallel create new mediafiles
    // and match objects.
    for unit in chunk_iter.into_iter() {
        let mut lock = conn.writer().lock_owned().await;
        let mut tx = dim_database::write_tx(&mut lock)
            .await
            .map_err(|e| Error::DatabaseError(e.into()))?;

        if let Err(e) = matcher.batch_match(&mut tx, provider.clone(), unit).await {
            error!(error = ?e, "Failed to match batch of mediafiles.");
        }

        tx.commit()
            .await
            .map_err(|e| Error::DatabaseError(e.into()))?;
    }

    info!(
        library_id,
        units = workunits_size,
        elapsed_ms = now.elapsed().as_millis(),
        "Finished scanning library."
    );

    tx.send(
        dim_events::Message {
            id: library_id,
            event_type: dim_events::PushEventType::EventStoppedScanning,
        }
        .to_string(),
    )
    .map_err(|e| Error::EventDispatch(e.into()))?;

    Ok(())
}

pub async fn start(
    conn: &mut dim_database::DbConnection,
    library_id: i64,
    tx: EventTx,
    provider: Arc<dyn ExternalQueryIntoShow>,
) -> Result<(), Error> {
    let mut tx_ = conn
        .read()
        .begin()
        .await
        .map_err(|e| Error::DatabaseError(e.into()))?;

    let lib = Library::get_one(&mut tx_, library_id)
        .await
        .map_err(|e| Error::LibraryNotFound(e))?;

    start_custom(
        conn,
        library_id,
        lib.locations,
        tx,
        lib.media_type,
        provider,
    )
    .await
}

/// Function formats the path where assets are stored.
pub fn format_path(x: Option<String>) -> String {
    x.map(|x| format!("images/{}", x.trim_start_matches('/')))
        .unwrap_or_default()
}

/// Enqueue an asset for background download so it is available before the UI
/// requests it.  The fetcher stores files at `{METADATA_PATH}/{outfile}`, and
/// the image serving route strips the `images/` prefix from the DB local_path,
/// so we must do the same here.
pub async fn enqueue_asset_download(asset: &dim_database::asset::Asset) {
    if let Some(ref url) = asset.remote_url {
        let outfile = asset
            .local_path
            .strip_prefix("images/")
            .unwrap_or(&asset.local_path);
        crate::fetcher::insert_into_queue(url.clone(), outfile.to_string(), false).await;
    }
}

//! Directory-context-aware scanner for TV libraries.
//!
//! Instead of relying solely on filename-derived titles for TMDB search, this
//! module leverages the fact that files in the same directory usually belong to
//! the same show.  If a directory already contains matched files, new files only
//! need season/episode extraction — not a TMDB title search.

use super::error::Error;
use super::movie::best_title_similarity;
use super::tv_show::TvMatcher;
use super::{
    extract_season_from_dir, insert_mediafiles, is_generic_dir_name, parse_filenames,
    MediaMatcher, WorkUnit,
};
use crate::core::EventTx;

use dim_database::library::Library;
use dim_database::mediafile::MediaFile;
use dim_extern_api::{ExternalQueryIntoShow, ExternalQueryShow};

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use tracing::{error, info, warn};

/// Returns the "show directory" for a file — the directory that represents the
/// show itself.  If the immediate parent is a season-style directory (e.g.
/// "Season 2"), returns the grandparent instead.
fn show_directory(file_path: &str) -> Option<&str> {
    let path = Path::new(file_path);
    let parent = path.parent()?;
    let parent_name = parent.file_name()?.to_str()?;

    if is_generic_dir_name(parent_name) {
        // Parent is "Season X" etc — the show dir is one level up.
        parent.parent()?.to_str()
    } else {
        parent.to_str()
    }
}

/// For a file inside a "Season X" directory, return the season number.
fn season_from_path(file_path: &str) -> Option<i64> {
    let parent = Path::new(file_path).parent()?;
    let name = parent.file_name()?.to_str()?;
    extract_season_from_dir(name)
}

/// Walk TMDB seasons (skipping specials) and resolve an absolute episode number
/// to a (season, relative episode) pair.
async fn resolve_absolute_episode(
    provider: &dyn ExternalQueryShow,
    external_id: &str,
    absolute_ep: i64,
) -> Option<(
    dim_extern_api::ExternalSeason,
    dim_extern_api::ExternalEpisode,
)> {
    let seasons = provider.seasons_for_id(external_id).await.ok()?;
    let mut running: i64 = 0;

    for season in &seasons {
        if season.season_number == 0 {
            continue; // skip specials
        }
        let episodes = provider
            .episodes_for_season(external_id, season.season_number)
            .await
            .ok()?;
        let count = episodes.len() as i64;
        if absolute_ep <= running + count {
            let relative = absolute_ep - running;
            return episodes
                .into_iter()
                .find(|e| e.episode_number as i64 == relative)
                .map(|e| (season.clone(), e));
        }
        running += count;
    }
    None
}

/// Smart scan entry point for TV libraries.
///
/// 1. Discover & insert new files (same as normal scan).
/// 2. Build directory→show mapping from already-matched files.
/// 3. For unmatched files in known directories, match via show name + season/ep.
/// 4. For unmatched files in unknown directories, fall through to normal TMDB
///    title-similarity matching.
pub async fn smart_scan(
    conn: &mut dim_database::DbConnection,
    library_id: i64,
    event_tx: EventTx,
    provider: Arc<dyn ExternalQueryIntoShow>,
) -> Result<(), Error> {
    info!(library_id, "Smart scan: starting");

    let lib = {
        let mut tx = conn
            .read()
            .begin()
            .await
            .map_err(|e| Error::DatabaseError(e.into()))?;
        Library::get_one(&mut tx, library_id)
            .await
            .map_err(Error::LibraryNotFound)?
    };

    event_tx
        .send(
            dim_events::Message {
                id: library_id,
                event_type: dim_events::PushEventType::EventStartedScanning,
            }
            .to_string(),
        )
        .map_err(|e| Error::EventDispatch(e.into()))?;

    // --- Phase 1: discover & insert new files ---
    let now = Instant::now();
    let _new_workunits = insert_mediafiles(conn, library_id, lib.locations).await?;
    info!(
        library_id,
        elapsed_ms = now.elapsed().as_millis(),
        "Smart scan: file discovery complete"
    );

    // --- Phase 2: build directory → show mapping ---
    let dir_map = {
        let mut tx = conn
            .read()
            .begin()
            .await
            .map_err(|e| Error::DatabaseError(e.into()))?;

        let mappings = MediaFile::get_show_mapping_for_lib(&mut tx, library_id)
            .await
            .unwrap_or_else(|e| {
                warn!(?e, "Smart scan: failed to load show mapping");
                vec![]
            });

        let mut map: HashMap<String, (String, i64)> = HashMap::new();
        for m in &mappings {
            if let Some(dir) = show_directory(&m.target_file) {
                map.entry(dir.to_owned())
                    .or_insert_with(|| (m.show_name.clone(), m.show_id));
            }
        }
        map
    };

    info!(
        library_id,
        known_dirs = dir_map.len(),
        "Smart scan: directory mapping built"
    );

    // --- Phase 3: load all unmatched files ---
    let unmatched = {
        let mut tx = conn
            .read()
            .begin()
            .await
            .map_err(|e| Error::DatabaseError(e.into()))?;
        MediaFile::get_by_lib_null_media(&mut tx, library_id)
            .await
            .unwrap_or_default()
    };

    if unmatched.is_empty() {
        info!(library_id, "Smart scan: no unmatched files");
        event_tx
            .send(
                dim_events::Message {
                    id: library_id,
                    event_type: dim_events::PushEventType::EventStoppedScanning,
                }
                .to_string(),
            )
            .map_err(|e| Error::EventDispatch(e.into()))?;
        return Ok(());
    }

    info!(
        library_id,
        count = unmatched.len(),
        "Smart scan: processing unmatched files"
    );

    // --- Phase 4: partition into known-dir vs unknown-dir ---
    // (file, show_name, existing show media id)
    let mut known_dir_files: Vec<(MediaFile, String, i64)> = Vec::new();
    let mut unknown_dir_files: Vec<MediaFile> = Vec::new();

    for mf in unmatched {
        if let Some(dir) = show_directory(&mf.target_file) {
            if let Some((show_name, show_id)) = dir_map.get(dir) {
                known_dir_files.push((mf, show_name.clone(), *show_id));
                continue;
            }
        }
        unknown_dir_files.push(mf);
    }

    info!(
        library_id,
        known = known_dir_files.len(),
        unknown = unknown_dir_files.len(),
        "Smart scan: partitioned files"
    );

    // --- Phase 5: match known-dir files using directory context ---
    if !known_dir_files.is_empty() {
        let provider_show: Arc<dyn ExternalQueryShow> = provider
            .clone()
            .into_query_show()
            .expect("TV library needs a show provider");

        // Cache: show_name → Option<external_id>
        let mut tmdb_cache: HashMap<String, Option<String>> = HashMap::new();
        let matcher = TvMatcher;

        for (mf, show_name, show_id) in &known_dir_files {
            // Prefer the provider id stored on the existing show row — a
            // fuzzy name re-search can resolve to a DIFFERENT entity and
            // split the show into a duplicate entry.
            let stored_eid = {
                let mut tx = conn
                    .read()
                    .begin()
                    .await
                    .map_err(|e| Error::DatabaseError(e.into()))?;
                dim_database::media::Media::get_external_id(&mut tx, *show_id)
                    .await
                    .ok()
                    .flatten()
            };

            let external_id = match stored_eid {
                Some(eid) => Some(eid),
                None => match tmdb_cache.get(show_name) {
                    Some(cached) => cached.clone(),
                    None => {
                        let eid = lookup_external_id(&*provider_show, show_name).await;
                        tmdb_cache.insert(show_name.clone(), eid.clone());
                        eid
                    }
                },
            };

            let Some(external_id) = external_id else {
                info!(
                    show_name,
                    file = %mf.target_file,
                    "Smart scan: couldn't find TMDB match for known show, skipping"
                );
                continue;
            };

            // Extract season/episode from filename + directory.
            let parsed = parse_filenames(std::iter::once(&mf.target_file));
            let mut metadata = match parsed.into_iter().next() {
                Some((_, meta)) => meta,
                None => continue,
            };

            // Override season from directory name if available.
            let dir_season = season_from_path(&mf.target_file);
            for m in &mut metadata {
                if dir_season.is_some() && m.season.is_none() {
                    m.season = dir_season;
                }
            }

            // If we have no season at all (absolute numbering), try to resolve.
            let has_season = metadata.iter().any(|m| m.season.is_some());
            let has_episode = metadata.iter().any(|m| m.episode.is_some());

            if !has_season && has_episode {
                // Absolute numbering — resolve to (season, episode).
                let abs_ep = metadata.iter().find_map(|m| m.episode).unwrap();
                if let Some((season, episode)) =
                    resolve_absolute_episode(&*provider_show, &external_id, abs_ep).await
                {
                    info!(
                        file = %mf.target_file,
                        abs_ep,
                        season = season.season_number,
                        episode = episode.episode_number,
                        "Smart scan: resolved absolute episode"
                    );
                    for m in &mut metadata {
                        m.season = Some(season.season_number as i64);
                        m.episode = Some(episode.episode_number as i64);
                    }
                }
            }

            let work = WorkUnit(mf.clone(), metadata);
            let mut lock = conn.writer().lock_owned().await;
            let mut tx = dim_database::write_tx(&mut lock)
                .await
                .map_err(|e| Error::DatabaseError(e.into()))?;

            if let Err(e) = matcher
                .match_to_id(&mut tx, provider.clone(), work, &external_id)
                .await
            {
                info!(
                    file = %mf.target_file,
                    error = ?e,
                    "Smart scan: failed to match known-dir file"
                );
            } else {
                info!(file = %mf.target_file, "Smart scan: matched via directory context");
            }

            tx.commit()
                .await
                .map_err(|e| Error::DatabaseError(e.into()))?;
        }
    }

    // --- Phase 6: fall through to normal batch_match for unknown-dir files ---
    if !unknown_dir_files.is_empty() {
        info!(
            library_id,
            count = unknown_dir_files.len(),
            "Smart scan: falling through to title-based matching for unknown dirs"
        );

        let matcher: Arc<dyn MediaMatcher> = Arc::new(TvMatcher);

        let workunits: Vec<WorkUnit> = unknown_dir_files
            .into_iter()
            .filter_map(|mf| {
                let parsed = parse_filenames(std::iter::once(&mf.target_file));
                parsed
                    .into_iter()
                    .next()
                    .map(|(_, meta)| WorkUnit(mf, meta))
            })
            .collect();

        let chunks: Vec<Vec<WorkUnit>> = {
            let mut result = Vec::new();
            let mut current = Vec::new();
            for unit in workunits {
                current.push(unit);
                if current.len() >= 128 {
                    result.push(std::mem::take(&mut current));
                }
            }
            if !current.is_empty() {
                result.push(current);
            }
            result
        };

        for chunk in chunks {
            let mut lock = conn.writer().lock_owned().await;
            let mut tx = dim_database::write_tx(&mut lock)
                .await
                .map_err(|e| Error::DatabaseError(e.into()))?;

            if let Err(e) = matcher
                .batch_match(&mut tx, provider.clone(), chunk)
                .await
            {
                error!(error = ?e, "Smart scan: batch_match failed for unknown-dir chunk");
            }

            tx.commit()
                .await
                .map_err(|e| Error::DatabaseError(e.into()))?;
        }
    }

    info!(library_id, "Smart scan: finished");

    event_tx
        .send(
            dim_events::Message {
                id: library_id,
                event_type: dim_events::PushEventType::EventStoppedScanning,
            }
            .to_string(),
        )
        .map_err(|e| Error::EventDispatch(e.into()))?;

    Ok(())
}

/// Search TMDB by a known show name and return the best-matching external_id.
async fn lookup_external_id(
    provider: &dyn ExternalQueryShow,
    show_name: &str,
) -> Option<String> {
    let results = provider.search(show_name, None, None).await.ok()?;
    if results.is_empty() {
        return None;
    }

    // On ties keep the EARLIEST result — provider lists are popularity-ordered
    // and `max_by` returns the last maximum.
    let best = results
        .iter()
        .fold(None::<(&_, f64)>, |best, x| {
            let score = best_title_similarity(show_name, x);
            match best {
                Some((_, best_score)) if best_score >= score => best,
                _ => Some((x, score)),
            }
        })
        .map(|(x, _)| x)?
        .clone();

    let score = best_title_similarity(show_name, &best);
    if score < 0.3 {
        warn!(
            show_name,
            best_result = %best.title,
            %score,
            "Smart scan: TMDB match below threshold for known show"
        );
        return None;
    }

    Some(best.external_id)
}

/// After a user manually matches a file to a TMDB ID, try to match unmatched
/// siblings in the same directory using the same ID.
pub async fn match_siblings(
    conn: &dim_database::DbConnection,
    matched_file: &MediaFile,
    tmdb_id: &str,
    provider: Arc<dyn ExternalQueryIntoShow>,
) {
    let dir = match show_directory(&matched_file.target_file) {
        Some(d) => d.to_owned(),
        None => return,
    };

    // Also try the parent directory if the file is directly in the show dir
    // (no Season subdir).  And try the specific subdirectory.
    let file_parent = Path::new(&matched_file.target_file)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or(&dir)
        .to_owned();

    let siblings = {
        let mut tx = match conn.read().begin().await {
            Ok(tx) => tx,
            Err(e) => {
                warn!(?e, "Sibling match: failed to get read tx");
                return;
            }
        };

        // Search in the show directory (covers all Season subdirs).
        let mut files = MediaFile::get_unmatched_in_dir(
            &mut tx,
            matched_file.library_id,
            &dir,
        )
        .await
        .unwrap_or_default();

        // If the file_parent is different (a Season subdir), also search there
        // specifically — the LIKE pattern already covers it, but let's also
        // search the parent dir itself in case files sit at show-dir level.
        if file_parent != dir {
            if let Ok(mut extra) = MediaFile::get_unmatched_in_dir(
                &mut tx,
                matched_file.library_id,
                &file_parent,
            )
            .await
            {
                files.append(&mut extra);
            }
        }

        // Deduplicate by id.
        files.sort_by_key(|f| f.id);
        files.dedup_by_key(|f| f.id);
        files
    };

    if siblings.is_empty() {
        return;
    }

    info!(
        count = siblings.len(),
        dir = %dir,
        tmdb_id,
        "Sibling match: attempting to match siblings"
    );

    let matcher = TvMatcher;

    for mf in siblings {
        let parsed = parse_filenames(std::iter::once(&mf.target_file));
        let mut metadata = match parsed.into_iter().next() {
            Some((_, meta)) => meta,
            None => continue,
        };

        // Override season from directory if available.
        let dir_season = season_from_path(&mf.target_file);
        for m in &mut metadata {
            if dir_season.is_some() && m.season.is_none() {
                m.season = dir_season;
            }
        }

        let work = WorkUnit(mf.clone(), metadata);
        let mut lock = conn.writer().lock_owned().await;
        let mut tx = match dim_database::write_tx(&mut lock).await {
            Ok(tx) => tx,
            Err(e) => {
                warn!(?e, "Sibling match: failed to get write tx");
                return;
            }
        };

        if let Err(e) = matcher
            .match_to_id(&mut tx, provider.clone(), work, tmdb_id)
            .await
        {
            info!(
                file = %mf.target_file,
                error = ?e,
                "Sibling match: failed to match"
            );
        } else {
            info!(file = %mf.target_file, "Sibling match: matched");
        }

        if let Err(e) = tx.commit().await {
            warn!(?e, "Sibling match: commit failed");
            return;
        }
    }
}

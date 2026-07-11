//! Sonarr/Radarr webhook receiver.
//!
//! Instead of merely posting a notification, an import event drives the
//! scanner directly: the reported file is located inside a Dim library
//! (re-rooting the *arr path onto the library location when the two see the
//! media tree under different mount prefixes), ingested, and matched against
//! the authoritative external id carried by the payload (tmdb/tvdb/imdb) —
//! bypassing filename guessing entirely. Delete and rename events keep the
//! index in sync on setups where inotify does not fire (network mounts,
//! macOS bind mounts).

use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::response::Json;
use axum::Extension;

use dim_core::core::EventTx;
use dim_core::scanner::movie;
use dim_core::scanner::parse_filenames;
use dim_core::scanner::tv_show;
use dim_core::scanner::MediaMatcher;
use dim_core::scanner::WorkUnit;

use dim_database::library::Library;
use dim_database::library::MediaType;
use dim_database::media::Media;
use dim_database::mediafile::MediaFile;
use dim_database::mediafile::UpdateMediaFile;
use dim_database::notification::{InsertableNotification, Notification};
use dim_database::season::Season;
use dim_database::user::User;
use dim_database::DbConnection;

use dim_extern_api::filename::Metadata;

use http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};

// -- Payload shapes (subset of the Sonarr/Radarr webhook contracts) --

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SonarrPayload {
    event_type: String,
    series: Option<SonarrSeries>,
    #[serde(default)]
    episodes: Vec<SonarrEpisode>,
    episode_file: Option<ArrFile>,
    #[serde(default)]
    renamed_episode_files: Vec<ArrRenamedFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SonarrSeries {
    title: String,
    path: Option<String>,
    year: Option<i64>,
    tvdb_id: Option<i64>,
    tmdb_id: Option<i64>,
    imdb_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SonarrEpisode {
    season_number: i64,
    episode_number: i64,
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RadarrPayload {
    event_type: String,
    movie: Option<RadarrMovie>,
    movie_file: Option<ArrFile>,
    #[serde(default)]
    renamed_movie_files: Vec<ArrRenamedFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RadarrMovie {
    title: String,
    year: Option<i64>,
    folder_path: Option<String>,
    tmdb_id: Option<i64>,
    imdb_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArrFile {
    path: Option<String>,
    relative_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArrRenamedFile {
    previous_path: Option<String>,
    previous_relative_path: Option<String>,
    relative_path: Option<String>,
}

// -- Normalized event --

/// Paths as reported by the *arr application. `folder` is the series/movie
/// folder, `relative` the file path inside it — together they let us re-root
/// the file onto a Dim library location when mount prefixes differ.
#[derive(Debug, Clone, Default)]
struct ArrPaths {
    file: Option<String>,
    folder: Option<String>,
    relative: Option<String>,
}

#[derive(Debug)]
enum ArrEvent {
    Test,
    Ignored,
    Import {
        title: String,
        year: Option<i64>,
        /// (season, first episode, last episode) for TV imports.
        episodes: Option<(i64, i64, i64)>,
        episode_title: Option<String>,
        /// Namespaced ids in preference order, e.g. ["tmdb:1396", "tvdb:81189"].
        external_ids: Vec<String>,
        paths: ArrPaths,
    },
    FileDelete {
        paths: ArrPaths,
    },
    Rename {
        folder: Option<String>,
        files: Vec<ArrRenamedFile>,
    },
    MediaDelete {
        folder: Option<String>,
    },
}

fn normalize_sonarr(body: &serde_json::Value) -> Result<ArrEvent, StatusCode> {
    let payload: SonarrPayload =
        serde_json::from_value(body.clone()).map_err(|_| StatusCode::BAD_REQUEST)?;

    let series = payload.series;
    let folder = series.as_ref().and_then(|s| s.path.clone());

    Ok(match payload.event_type.as_str() {
        "Test" => ArrEvent::Test,
        "Download" | "EpisodeFileImported" => {
            let series = series.ok_or(StatusCode::BAD_REQUEST)?;

            let mut external_ids = vec![];
            if let Some(id) = series.tmdb_id.filter(|x| *x > 0) {
                external_ids.push(format!("tmdb:{id}"));
            }
            if let Some(id) = series.tvdb_id.filter(|x| *x > 0) {
                external_ids.push(format!("tvdb:{id}"));
            }
            if let Some(id) = series.imdb_id.filter(|x| !x.is_empty()) {
                external_ids.push(format!("imdb:{id}"));
            }

            let episodes = match payload.episodes.as_slice() {
                [] => None,
                eps => Some((
                    eps[0].season_number,
                    eps[0].episode_number,
                    eps[eps.len() - 1].episode_number,
                )),
            };

            ArrEvent::Import {
                title: series.title,
                year: series.year,
                episodes,
                episode_title: payload.episodes.first().and_then(|e| e.title.clone()),
                external_ids,
                paths: ArrPaths {
                    file: payload.episode_file.as_ref().and_then(|f| f.path.clone()),
                    folder,
                    relative: payload
                        .episode_file
                        .as_ref()
                        .and_then(|f| f.relative_path.clone()),
                },
            }
        }
        "EpisodeFileDelete" => ArrEvent::FileDelete {
            paths: ArrPaths {
                file: payload.episode_file.as_ref().and_then(|f| f.path.clone()),
                folder,
                relative: payload
                    .episode_file
                    .as_ref()
                    .and_then(|f| f.relative_path.clone()),
            },
        },
        "SeriesDelete" => ArrEvent::MediaDelete { folder },
        "Rename" => ArrEvent::Rename {
            folder,
            files: payload.renamed_episode_files,
        },
        _ => ArrEvent::Ignored,
    })
}

fn normalize_radarr(body: &serde_json::Value) -> Result<ArrEvent, StatusCode> {
    let payload: RadarrPayload =
        serde_json::from_value(body.clone()).map_err(|_| StatusCode::BAD_REQUEST)?;

    let movie = payload.movie;
    let folder = movie.as_ref().and_then(|m| m.folder_path.clone());

    Ok(match payload.event_type.as_str() {
        "Test" => ArrEvent::Test,
        "Download" | "MovieFileImported" => {
            let movie = movie.ok_or(StatusCode::BAD_REQUEST)?;

            let mut external_ids = vec![];
            if let Some(id) = movie.tmdb_id.filter(|x| *x > 0) {
                external_ids.push(format!("tmdb:{id}"));
            }
            if let Some(id) = movie.imdb_id.filter(|x| !x.is_empty()) {
                external_ids.push(format!("imdb:{id}"));
            }

            ArrEvent::Import {
                title: movie.title,
                year: movie.year,
                episodes: None,
                episode_title: None,
                external_ids,
                paths: ArrPaths {
                    file: payload.movie_file.as_ref().and_then(|f| f.path.clone()),
                    folder,
                    relative: payload
                        .movie_file
                        .as_ref()
                        .and_then(|f| f.relative_path.clone()),
                },
            }
        }
        "MovieFileDelete" => ArrEvent::FileDelete {
            paths: ArrPaths {
                file: payload.movie_file.as_ref().and_then(|f| f.path.clone()),
                folder,
                relative: payload
                    .movie_file
                    .as_ref()
                    .and_then(|f| f.relative_path.clone()),
            },
        },
        "MovieDelete" => ArrEvent::MediaDelete { folder },
        "Rename" => ArrEvent::Rename {
            folder,
            files: payload.renamed_movie_files,
        },
        _ => ArrEvent::Ignored,
    })
}

// -- Receiver --

#[derive(Deserialize)]
pub struct WebhookQuery {
    apikey: String,
}

pub async fn receive_webhook(
    State(AppState { conn, event_tx, .. }): State<AppState>,
    Path(source): Path<String>,
    Query(query): Query<WebhookQuery>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Result<impl IntoResponse, StatusCode> {
    let settings = dim_core::settings::get_global_settings();

    if settings.webhook_api_key.is_empty() || query.apikey != settings.webhook_api_key {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let media_type = match source.as_str() {
        "sonarr" => MediaType::Tv,
        "radarr" => MediaType::Movie,
        _ => return Err(StatusCode::NOT_FOUND),
    };

    let event = match media_type {
        MediaType::Tv => normalize_sonarr(&body)?,
        _ => normalize_radarr(&body)?,
    };

    match event {
        ArrEvent::Test | ArrEvent::Ignored => Ok(StatusCode::OK.into_response()),
        event => {
            // Processing hits the filesystem (with retries for slow mounts)
            // and the metadata provider; don't make the *arr app wait on it.
            let mut conn = conn.clone();
            tokio::spawn(async move {
                process_event(&mut conn, event_tx, media_type, event).await;
            });

            Ok(StatusCode::ACCEPTED.into_response())
        }
    }
}

async fn process_event(
    conn: &mut DbConnection,
    event_tx: EventTx,
    media_type: MediaType,
    event: ArrEvent,
) {
    match event {
        ArrEvent::Import {
            title,
            year,
            episodes,
            episode_title,
            external_ids,
            paths,
        } => {
            process_import(
                conn,
                event_tx,
                media_type,
                title,
                year,
                episodes,
                episode_title,
                external_ids,
                paths,
            )
            .await
        }
        ArrEvent::FileDelete { paths } => process_file_delete(conn, paths).await,
        ArrEvent::Rename { folder, files } => process_rename(conn, folder, files).await,
        ArrEvent::MediaDelete { folder } => process_media_delete(conn, media_type, folder).await,
        ArrEvent::Test | ArrEvent::Ignored => {}
    }
}

/// Libraries of the given media type, with locations populated.
async fn libraries_of(conn: &mut DbConnection, media_type: MediaType) -> Vec<Library> {
    let Ok(mut tx) = conn.read().begin().await else {
        return vec![];
    };

    let mut libraries = vec![];
    for mut lib in Library::get_all(&mut tx).await {
        if lib.media_type != media_type {
            continue;
        }
        lib.locations = Library::get_locations(&mut tx, lib.id)
            .await
            .unwrap_or_default();
        libraries.push(lib);
    }

    libraries
}

/// Resolve the file reported by the *arr app to a path Dim can actually see.
/// Tries the reported path as-is first, then re-roots `<folder basename>/
/// <relative path>` onto each library location — the two applications point
/// at the same media tree, just possibly under different mount prefixes.
fn resolve_on_disk(libraries: &[Library], paths: &ArrPaths) -> Option<(i64, String)> {
    let folder_base = paths
        .folder
        .as_deref()
        .and_then(|f| PathBuf::from(f).file_name().map(|b| b.to_os_string()));

    for lib in libraries {
        for loc in &lib.locations {
            if let Some(file) = &paths.file {
                if file.starts_with(loc.as_str()) && std::path::Path::new(file).is_file() {
                    return Some((lib.id, file.clone()));
                }
            }

            if let Some(rel) = &paths.relative {
                let mut candidates = vec![];
                if let Some(base) = &folder_base {
                    candidates.push(PathBuf::from(loc).join(base).join(rel));
                }
                candidates.push(PathBuf::from(loc).join(rel));

                for candidate in candidates {
                    if candidate.is_file() {
                        if let Some(s) = candidate.to_str() {
                            return Some((lib.id, s.to_string()));
                        }
                    }
                }
            }
        }
    }

    None
}

/// Locate an already-indexed mediafile from *arr-reported paths: exact match
/// on the reported path, else a `%/<folder basename>/<relative>` suffix match
/// to bridge mount-prefix differences.
async fn find_indexed_mediafile(
    tx: &mut dim_database::Transaction<'_>,
    file: Option<&str>,
    folder: Option<&str>,
    relative: Option<&str>,
) -> Option<MediaFile> {
    if let Some(file) = file {
        if let Ok(mf) = MediaFile::get_by_file(tx, file).await {
            return Some(mf);
        }
    }

    let base = folder.and_then(|f| {
        PathBuf::from(f)
            .file_name()
            .and_then(|b| b.to_str().map(|s| s.to_string()))
    })?;
    let relative = relative?;

    let pattern = format!("%/{base}/{relative}");
    let ids = MediaFile::get_ids_by_file_pattern(tx, &pattern)
        .await
        .ok()?;

    match ids.as_slice() {
        [id] => MediaFile::get_one(tx, *id).await.ok(),
        [] => None,
        ids => {
            warn!(?pattern, count = ids.len(), "Ambiguous suffix match, skipping");
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_import(
    conn: &mut DbConnection,
    event_tx: EventTx,
    media_type: MediaType,
    title: String,
    year: Option<i64>,
    episodes: Option<(i64, i64, i64)>,
    episode_title: Option<String>,
    external_ids: Vec<String>,
    paths: ArrPaths,
) {
    let display_title = match (media_type, episodes) {
        (MediaType::Tv, Some((season, first, last))) if first != last => {
            format!("{title} S{season:02}E{first:02}-E{last:02}")
        }
        (MediaType::Tv, Some((season, first, _))) => {
            format!("{title} S{season:02}E{first:02}")
        }
        _ => match year {
            Some(year) => format!("{title} ({year})"),
            None => title.clone(),
        },
    };

    let libraries = libraries_of(conn, media_type).await;

    // The import event fires right after the *arr app moves the file, but a
    // network mount on Dim's side may lag behind — retry briefly.
    let mut resolved = None;
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        resolved = resolve_on_disk(&libraries, &paths);
        if resolved.is_some() {
            break;
        }
    }

    let Some((library_id, file_path)) = resolved else {
        warn!(
            ?paths,
            ?media_type,
            "Webhook import: file not found under any library location"
        );
        push_notification(
            conn,
            &event_tx,
            format!("{display_title} added"),
            Some(format!(
                "File not visible to Dim: {}",
                paths.file.or(paths.relative).unwrap_or_default()
            )),
            None,
            None,
        )
        .await;
        return;
    };

    info!(library_id, file_path, ?external_ids, "Webhook import resolved");

    let library = libraries.iter().find(|l| l.id == library_id);
    let provider_name = library.map(|l| l.provider.as_str()).unwrap_or("tmdb");
    let provider = dim_core::providers::provider_for(provider_name, media_type);

    let matcher = match media_type {
        MediaType::Movie => Arc::new(movie::MovieMatcher) as Arc<dyn MediaMatcher>,
        MediaType::Tv => Arc::new(tv_show::TvMatcher) as Arc<dyn MediaMatcher>,
        _ => return,
    };

    // Ingest the file; if it is already indexed (e.g. inotify beat us to it,
    // or this is a re-fire), rebuild a work unit so the authoritative id
    // still overrides whatever the filename matcher decided.
    let mut workunits = match dim_core::scanner::insert_mediafiles(
        conn,
        library_id,
        vec![file_path.clone()],
    )
    .await
    {
        Ok(units) => units,
        Err(e) => {
            error!(error = ?e, file_path, "Webhook import: failed to insert mediafile");
            return;
        }
    };

    if workunits.is_empty() {
        let Ok(mut tx) = conn.read().begin().await else {
            return;
        };
        if let Ok(mf) = MediaFile::get_by_file(&mut tx, &file_path).await {
            if let Some((_, metadata)) =
                parse_filenames(IntoIterator::into_iter([&mf.target_file])).pop()
            {
                workunits.push(WorkUnit(mf, metadata));
            }
        }
    }

    if workunits.is_empty() {
        error!(file_path, "Webhook import: no work unit for file");
        return;
    }

    // The payload's title/season/episode numbers are authoritative — put
    // them in front so the matcher tries them before filename guesses.
    let payload_meta = Metadata {
        name: title.clone(),
        year,
        season: episodes.map(|(s, ..)| s),
        episode: episodes.map(|(_, first, _)| first),
        language: None,
    };
    for WorkUnit(_, metadata) in workunits.iter_mut() {
        metadata.insert(0, payload_meta.clone());
    }

    let mut matched = false;
    {
        let mut lock = conn.writer().lock_owned().await;
        let Ok(mut tx) = dim_database::write_tx(&mut lock).await else {
            return;
        };

        'outer: for unit in &workunits {
            for external_id in &external_ids {
                match matcher
                    .match_to_id(&mut tx, provider.clone(), unit.clone(), external_id)
                    .await
                {
                    Ok(()) => {
                        matched = true;
                        continue 'outer;
                    }
                    Err(e) => {
                        warn!(%external_id, error = ?e, "Webhook import: id match failed");
                    }
                }
            }

            // No id worked — fall back to the regular filename matcher so we
            // are never worse off than a plain scan.
            if let Err(e) = matcher
                .batch_match(&mut tx, provider.clone(), vec![unit.clone()])
                .await
            {
                error!(error = ?e, "Webhook import: fallback match failed");
            } else {
                matched = true;
            }
        }

        if let Err(e) = tx.commit().await {
            error!(error = ?e, "Webhook import: failed to commit match");
            return;
        }
    }

    if !matched {
        return;
    }

    // Enrich the notification with the media the file landed on so the UI
    // can deep-link to it (for episodes, link to the show).
    let (media_id, poster_path) = notification_target(conn, &file_path).await;

    push_notification(
        conn,
        &event_tx,
        format!("{display_title} added"),
        episode_title,
        media_id,
        poster_path,
    )
    .await;
}

/// Resolve the media object a freshly-matched file belongs to. Episodes are
/// walked up to their show, which is what the UI can actually link to.
async fn notification_target(
    conn: &mut DbConnection,
    file_path: &str,
) -> (Option<i64>, Option<String>) {
    let Ok(mut tx) = conn.read().begin().await else {
        return (None, None);
    };

    let Ok(mf) = MediaFile::get_by_file(&mut tx, file_path).await else {
        return (None, None);
    };

    let Ok(media) = Media::get_of_mediafile(&mut tx, mf.id).await else {
        return (None, None);
    };

    if media.media_type == MediaType::Episode {
        if let Ok(episode) = dim_database::episode::Episode::get_by_id(&mut tx, media.id).await {
            if let Ok(show_id) = Season::get_tvshowid(&mut tx, episode.seasonid).await {
                if let Ok(show) = Media::get(&mut tx, show_id).await {
                    return (Some(show.id), show.poster_path);
                }
            }
        }
        return (None, None);
    }

    (Some(media.id), media.poster_path)
}

async fn process_file_delete(conn: &mut DbConnection, paths: ArrPaths) {
    let mut lock = conn.writer().lock_owned().await;
    let Ok(mut tx) = dim_database::write_tx(&mut lock).await else {
        return;
    };

    let Some(mf) = find_indexed_mediafile(
        &mut tx,
        paths.file.as_deref(),
        paths.folder.as_deref(),
        paths.relative.as_deref(),
    )
    .await
    else {
        info!(?paths, "Webhook delete: file not indexed, nothing to do");
        return;
    };

    info!(mediafile_id = mf.id, target_file = mf.target_file, "Webhook delete");
    remove_mediafile(&mut tx, mf.id).await;

    if let Err(e) = tx.commit().await {
        error!(error = ?e, "Webhook delete: failed to commit");
    }
}

async fn process_rename(
    conn: &mut DbConnection,
    folder: Option<String>,
    files: Vec<ArrRenamedFile>,
) {
    let mut lock = conn.writer().lock_owned().await;
    let Ok(mut tx) = dim_database::write_tx(&mut lock).await else {
        return;
    };

    for file in files {
        let Some(mf) = find_indexed_mediafile(
            &mut tx,
            file.previous_path.as_deref(),
            folder.as_deref(),
            file.previous_relative_path.as_deref(),
        )
        .await
        else {
            continue;
        };

        // The indexed path and the previous relative path share a suffix;
        // swapping it for the new relative path keeps Dim's mount prefix.
        let new_target = match (&file.previous_relative_path, &file.relative_path) {
            (Some(prev_rel), Some(new_rel)) => mf
                .target_file
                .strip_suffix(prev_rel.as_str())
                .map(|prefix| format!("{prefix}{new_rel}")),
            _ => None,
        };

        let Some(new_target) = new_target else {
            warn!(
                mediafile_id = mf.id,
                "Webhook rename: could not derive new path"
            );
            continue;
        };

        info!(mediafile_id = mf.id, from = mf.target_file, to = new_target, "Webhook rename");

        let update = UpdateMediaFile {
            target_file: Some(new_target),
            ..Default::default()
        };
        if let Err(e) = update.update(&mut tx, mf.id).await {
            error!(error = ?e, mediafile_id = mf.id, "Webhook rename: failed to update");
        }
    }

    if let Err(e) = tx.commit().await {
        error!(error = ?e, "Webhook rename: failed to commit");
    }
}

async fn process_media_delete(
    conn: &mut DbConnection,
    media_type: MediaType,
    folder: Option<String>,
) {
    let Some(base) = folder
        .as_deref()
        .and_then(|f| PathBuf::from(f).file_name().and_then(|b| b.to_str().map(String::from)))
        .filter(|b| b.len() >= 2)
    else {
        warn!(?folder, "Webhook media delete: unusable folder path");
        return;
    };

    let library_ids: Vec<i64> = libraries_of(conn, media_type)
        .await
        .into_iter()
        .map(|l| l.id)
        .collect();

    let mut lock = conn.writer().lock_owned().await;
    let Ok(mut tx) = dim_database::write_tx(&mut lock).await else {
        return;
    };

    let pattern = format!("%/{base}/%");
    let Ok(ids) = MediaFile::get_ids_by_file_pattern(&mut tx, &pattern).await else {
        return;
    };

    info!(?pattern, count = ids.len(), "Webhook media delete");

    for id in ids {
        // The pattern is only a folder name — restrict to libraries of the
        // right media type so a like-named folder elsewhere is untouched.
        match MediaFile::get_one(&mut tx, id).await {
            Ok(mf) if library_ids.contains(&mf.library_id) => {
                remove_mediafile(&mut tx, id).await;
            }
            _ => {}
        }
    }

    if let Err(e) = tx.commit().await {
        error!(error = ?e, "Webhook media delete: failed to commit");
    }
}

/// Delete a mediafile and purge the media object it belonged to when no
/// files remain (mirrors the fs-watcher daemon's remove handling).
async fn remove_mediafile(tx: &mut dim_database::Transaction<'_>, mediafile_id: i64) {
    let media = Media::get_of_mediafile(tx, mediafile_id).await;

    if let Err(e) = MediaFile::delete(tx, mediafile_id).await {
        error!(error = ?e, mediafile_id, "Failed to remove mediafile");
        return;
    }

    if let Ok(media) = media {
        if let Ok(files) = MediaFile::get_of_media(tx, media.id).await {
            if files.is_empty() {
                if let Err(e) = Media::delete(tx, media.id).await {
                    error!(error = ?e, media_id = media.id, "Failed to delete ghost media");
                }
            }
        }
    }
}

async fn push_notification(
    conn: &mut DbConnection,
    event_tx: &EventTx,
    title: String,
    body: Option<String>,
    media_id: Option<i64>,
    poster_path: Option<String>,
) {
    let mut lock = conn.writer().lock_owned().await;
    let Ok(mut tx) = dim_database::write_tx(&mut lock).await else {
        return;
    };

    let notif = match Notification::insert(
        &mut tx,
        InsertableNotification {
            user_id: None,
            category: "media_added".to_string(),
            title: title.clone(),
            body: body.clone(),
            media_id,
            poster_path: poster_path.clone(),
        },
    )
    .await
    {
        Ok(n) => n,
        Err(e) => {
            error!(error = ?e, "Failed to insert webhook notification");
            return;
        }
    };

    if let Err(e) = tx.commit().await {
        error!(error = ?e, "Failed to commit webhook notification");
        return;
    }
    drop(lock);

    let _ = event_tx.send(
        dim_events::Message {
            id: notif.id,
            event_type: dim_events::PushEventType::EventNewNotification {
                notification_id: notif.id,
                title,
                body,
                media_id,
                poster_path,
                category: "media_added".to_string(),
            },
        }
        .to_string(),
    );
}

// -- Notification API (authenticated) --

#[derive(Deserialize)]
pub struct NotificationListParams {
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    50
}

pub async fn get_notifications(
    State(AppState { conn, .. }): State<AppState>,
    Extension(user): Extension<User>,
    Query(params): Query<NotificationListParams>,
) -> Result<impl IntoResponse, StatusCode> {
    let mut tx = conn
        .read()
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let notifs = Notification::get_unread_for_user(&mut tx, user.id.0, params.limit)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(&json!(notifs)).into_response())
}

pub async fn get_unread_count(
    State(AppState { conn, .. }): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, StatusCode> {
    let mut tx = conn
        .read()
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let count = Notification::get_unread_count(&mut tx, user.id.0)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(&json!({ "count": count })).into_response())
}

pub async fn mark_read(
    State(AppState { conn, .. }): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, StatusCode> {
    let mut lock = conn.writer().lock_owned().await;
    let mut tx = dim_database::write_tx(&mut lock)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Notification::mark_read(&mut tx, id, user.id.0)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn mark_all_read(
    State(AppState { conn, .. }): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, StatusCode> {
    let mut lock = conn.writer().lock_owned().await;
    let mut tx = dim_database::write_tx(&mut lock)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Notification::mark_all_read(&mut tx, user.id.0)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

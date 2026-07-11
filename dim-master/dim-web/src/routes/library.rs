#![warn(warnings)]

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;

use dim_core::errors::DimError;
use dim_core::scanner::daemon::FsWatcher;
use dim_core::scanner::movie::MovieMatcher;
use dim_core::scanner::tv_show::TvMatcher;
use dim_core::scanner::{parse_filenames, MediaMatcher, WorkUnit};
use dim_database::compact_mediafile::CompactMediafile;
use dim_database::library::{InsertableLibrary, Library, MediaType};
use dim_database::media::Media;
use dim_database::mediafile::MediaFile;
use dim_database::user::User;

use dim_extern_api::ExternalQueryIntoShow;

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use http::StatusCode;
use serde::Deserialize;
use serde::Serialize;

use crate::error::DimErrorWrapper;
use crate::AppState;

/// Method maps to `POST /api/v1/library`, it adds a new library to the database, starts a new
/// scanner for it, then dispatches a event to all clients notifying them that a new library has
/// been created. This method can only be accessed by authenticated users. Method returns 200 OK
///
pub async fn library_post(
    Extension(user): Extension<User>,
    State(state): State<AppState>,
    Json(new_library): Json<InsertableLibrary>,
) -> Response {
    if !user.has_role("owner") {
        return (
            StatusCode::UNAUTHORIZED,
            "User account is not allowed to add a library.".to_string(),
        )
            .into_response();
    }
    let mut lock = state.conn.writer().lock_owned().await;

    let mut tx = match dim_database::write_tx(&mut lock).await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!(?err, "Error getting connection");
            return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
        }
    };

    let id = match new_library.insert(&mut tx).await {
        Ok(id) => id,
        Err(err) => {
            tracing::error!(?err, "Error inserting library");
            return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
        }
    };

    match tx.commit().await {
        Ok(_) => (),
        Err(err) => {
            tracing::error!(?err, "Error committing transaction");
            return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
        }
    }
    drop(lock);

    let tx_clone = state.event_tx.clone();

    let provider =
        dim_core::providers::provider_for(&new_library.provider, new_library.media_type);

    let mut fs_watcher = FsWatcher::new(
        state.conn.clone(),
        id,
        new_library.media_type,
        tx_clone.clone(),
        Arc::clone(&provider),
    );

    // Run the initial scan to completion BEFORE arming the filesystem
    // watcher — running both concurrently made them race over the same
    // files (duplicate matching work, and duplicate media rows when the
    // two paths resolved different title variants for one show).
    let mut conn = state.conn.clone();
    tokio::spawn(async move {
        let _ = dim_core::scanner::start(&mut conn, id, tx_clone, provider).await;
        let _ = fs_watcher.start_daemon().await;
    });

    Json(serde_json::json!({ "id": id })).into_response()
}

/// Method mapped to `POST /api/v1/library/<id>/scan` triggers a rescan of the library.
/// For TV libraries, uses smart scan (directory-context-aware matching).
/// For movie libraries, uses the standard scanner.
pub async fn library_scan(
    Extension(user): Extension<User>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, DimErrorWrapper> {
    if !user.has_role("owner") {
        return Err(DimErrorWrapper(DimError::Unauthorized));
    }

    let lib = {
        let mut tx = state.conn.read().begin().await.map_err(|err| {
            DimErrorWrapper(DimError::DatabaseError {
                description: err.to_string(),
            })
        })?;
        Library::get_one(&mut tx, id).await.map_err(|_| DimErrorWrapper(DimError::LibraryNotFound))?
    };

    let provider = dim_core::providers::provider_for(&lib.provider, lib.media_type);

    let mut conn = state.conn.clone();
    let tx_clone = state.event_tx.clone();

    match lib.media_type {
        MediaType::Tv => {
            tokio::spawn(async move {
                if let Err(e) = dim_core::scanner::smart_scan::smart_scan(
                    &mut conn, id, tx_clone, provider,
                ).await {
                    tracing::error!(?e, library_id = id, "Smart scan failed");
                }
            });
        }
        _ => {
            tokio::spawn(async move {
                dim_core::scanner::start(&mut conn, id, tx_clone, provider).await
            });
        }
    }

    Ok(StatusCode::OK)
}

/// Method mapped to `POST /api/v1/library/<id>/automatch` attempts to auto-match all unmatched
/// mediafiles in the library by searching TMDB and running the similarity-based matcher.
pub async fn library_automatch(
    Extension(user): Extension<User>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, DimErrorWrapper> {
    if !user.has_role("owner") {
        return Err(DimErrorWrapper(DimError::Unauthorized));
    }

    let lib = {
        let mut tx = state.conn.read().begin().await.map_err(|err| {
            DimErrorWrapper(DimError::DatabaseError {
                description: err.to_string(),
            })
        })?;
        Library::get_one(&mut tx, id).await.map_err(|_| DimErrorWrapper(DimError::LibraryNotFound))?
    };

    let provider: Arc<dyn ExternalQueryIntoShow> =
        dim_core::providers::provider_for(&lib.provider, lib.media_type);

    let matcher: Arc<dyn MediaMatcher> = match lib.media_type {
        MediaType::Movie => Arc::new(MovieMatcher),
        MediaType::Tv => Arc::new(TvMatcher),
        _ => unreachable!(),
    };

    let conn = state.conn.clone();
    let tx_clone = state.event_tx.clone();

    tokio::spawn(async move {
        tx_clone
            .send(
                dim_events::Message {
                    id,
                    event_type: dim_events::PushEventType::EventStartedScanning,
                }
                .to_string(),
            )
            .ok();

        let unmatched = {
            let mut tx = match conn.read().begin().await {
                Ok(tx) => tx,
                Err(e) => {
                    tracing::error!(?e, "automatch: failed to get read tx");
                    return;
                }
            };
            match MediaFile::get_by_lib_null_media(&mut tx, id).await {
                Ok(files) => files,
                Err(e) => {
                    tracing::error!(?e, "automatch: failed to fetch unmatched files");
                    return;
                }
            }
        };

        if unmatched.is_empty() {
            tracing::info!(library_id = id, "automatch: no unmatched files");
        } else {
            tracing::info!(library_id = id, count = unmatched.len(), "automatch: starting");

            let workunits: Vec<WorkUnit> = unmatched
                .into_iter()
                .filter_map(|mf| {
                    let parsed = parse_filenames(std::iter::once(&mf.target_file));
                    parsed.into_iter().next().map(|(_, meta)| WorkUnit(mf, meta))
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
                let mut tx = match dim_database::write_tx(&mut lock).await {
                    Ok(tx) => tx,
                    Err(e) => {
                        tracing::error!(?e, "automatch: failed to get write tx");
                        return;
                    }
                };

                if let Err(e) = matcher.batch_match(&mut tx, provider.clone(), chunk).await {
                    tracing::error!(?e, "automatch: batch_match failed");
                }

                if let Err(e) = tx.commit().await {
                    tracing::error!(?e, "automatch: commit failed");
                }
            }

            tracing::info!(library_id = id, "automatch: finished");
        }

        tx_clone
            .send(
                dim_events::Message {
                    id,
                    event_type: dim_events::PushEventType::EventStoppedScanning,
                }
                .to_string(),
            )
            .ok();
    });

    Ok(StatusCode::OK)
}

/// Method mapped to `DELETE /api/v1/library/<id>` deletes the library with the supplied id from the path.
pub async fn library_delete(
    Extension(user): Extension<User>,
    State(AppState { conn, .. }): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, DimErrorWrapper> {
    if !user.has_role("owner") {
        return Err(DimErrorWrapper(DimError::Unauthorized));
    }
    // First we mark the library as scheduled for deletion which will make the library and all its
    // content hidden. This is necessary because huge libraries take a long time to delete.
    {
        let mut lock = conn.writer().lock_owned().await;
        let mut tx = dim_database::write_tx(&mut lock).await.map_err(|err| {
            DimErrorWrapper(DimError::DatabaseError {
                description: err.to_string(),
            })
        })?;
        if Library::mark_hidden(&mut tx, id).await.map_err(|err| {
            DimErrorWrapper(DimError::DatabaseError {
                description: err.to_string(),
            })
        })? < 1
        {
            return Err(DimError::LibraryNotFound.into());
        }
        tx.commit().await.map_err(|err| {
            DimErrorWrapper(DimError::DatabaseError {
                description: err.to_string(),
            })
        })?;
    }

    let delete_lib_fut = async move {
        let inner = async {
            let mut lock = conn.writer().lock_owned().await;
            let mut tx = dim_database::write_tx(&mut lock).await?;

            Library::delete(&mut tx, id).await?;
            Media::delete_by_lib_id(&mut tx, id).await?;
            MediaFile::delete_by_lib_id(&mut tx, id).await?;

            tx.commit().await?;

            Ok::<_, dim_database::error::DatabaseError>(())
        };

        if let Err(e) = inner.await {
            tracing::error!(reason = ?e, "Failed to delete library and its content.");
        } else {
            tracing::info!("Deleted library");
        }
    };

    tokio::spawn(delete_lib_fut);

    Ok(StatusCode::NO_CONTENT)
}

/// Method mapped to `GET /api/v1/library` returns a list of all libraries in the database
pub async fn library_get_all(State(state): State<AppState>) -> Response {
    let mut tx = match state.conn.read().begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!(?err, "Error getting connection");
            return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
        }
    };

    let libraries = Library::get_all(&mut tx).await;

    Json(libraries).into_response()
}

/// Method mapped to `GET /api/v1/library/<id>` returns info about the library with the supplied
/// id. Method can only be accessed by authenticated users.
///
pub async fn library_get_one(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    let mut tx = match state.conn.read().begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!(?err, "Error getting connection");
            return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
        }
    };

    let lib = match Library::get_one(&mut tx, id).await {
        Ok(library) => library,
        Err(err) => {
            tracing::error!(?err, "Error getting library");
            return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
        }
    };

    Json(lib).into_response()
}

/// Method mapped to `GET /api/v1/library/<id>/media` returns all the movies/tv shows that belong
/// to the library with the id supplied. Method can only be accessed by authenticated users.
///
pub async fn library_get_media(
    State(AppState { conn, .. }): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    let mut result = HashMap::new();
    let mut tx = match conn.read().begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!(?err, "Error getting connection");
            return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
        }
    };

    let lib = match Library::get_one(&mut tx, id).await {
        Ok(library) => library,
        Err(err) => {
            tracing::error!(?err, "Error getting library");
            return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
        }
    };

    #[derive(Serialize)]
    struct Record {
        id: i64,
        name: String,
        poster_path: Option<String>,
    }

    let mut data = match sqlx::query_as!(
        Record,
        r#"SELECT _tblmedia.id, name, assets.local_path as poster_path FROM _tblmedia
        LEFT JOIN assets ON _tblmedia.poster = assets.id
        WHERE library_id = ? AND NOT media_type = "episode""#,
        id
    )
    .fetch_all(&mut tx)
    .await
    {
        Ok(res) => res,
        Err(err) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
        }
    };

    if data.is_empty() {
        return (StatusCode::NOT_FOUND, "No media found".to_string()).into_response();
    }

    data.sort_by(|a, b| a.name.cmp(&b.name));

    result.insert(lib.name, data);

    Json(result).into_response()
}

#[derive(Deserialize)]
pub struct UnmatchedArgs {
    search: Option<String>,
}

/// Method mapped to `GET /api/v1/library/<id>/unmatched` returns a list of all unmatched medias
/// to be displayed in the library pages.
///
pub async fn library_get_unmatched(
    State(AppState { conn, .. }): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<UnmatchedArgs>,
) -> Response {
    let mut tx = match conn.read().begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!(?err, "Error getting connection");
            return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
        }
    };

    // let mut files = CompactMediafile::unmatched_for_library(&mut tx, id)
    //     .await
    //     .map_err(|_| errors::DimError::NotFoundError)?;

    let mut files = match CompactMediafile::unmatched_for_library(&mut tx, id).await {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(?err, "Error getting unmatched files");
            return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
        }
    };

    // we want to pre-sort to ensure our tree is somewhat ordered.
    files.sort_by(|a, b| a.target_file.cmp(&b.target_file));

    if let Some(search) = params.search {
        let matcher = SkimMatcherV2::default();

        let mut matched_files = files
            .into_iter()
            .filter_map(|x| {
                let file_string = x.target_file.to_string_lossy();

                matcher
                    .fuzzy_match(&file_string, &search)
                    .map(|score| (x, score))
            })
            .collect::<Vec<_>>();

        matched_files.sort_by(|(_, a), (_, b)| b.cmp(&a));

        files = matched_files.into_iter().map(|(file, _)| file).collect();
    }

    let count = files.len();

    #[derive(Serialize)]
    struct Record {
        id: i64,
        name: String,
        duration: Option<i64>,
        file: String,
    }

    let entry = crate::tree::Entry::build_with(
        files,
        |x| {
            x.target_file
                .iter()
                .map(|x| x.to_string_lossy().to_string())
                .collect()
        },
        |k, v| Record {
            id: v.id,
            name: v.name,
            duration: v.duration,
            file: k.to_string(),
        },
    );

    #[derive(Serialize)]
    struct Response {
        count: usize,
        files: Vec<crate::tree::Entry<Record>>,
    }

    let entries = match entry {
        crate::tree::Entry::Directory { files, .. } => files,
        _ => unreachable!(),
    };

    Json(Response {
        files: entries,
        count,
    })
    .into_response()
}

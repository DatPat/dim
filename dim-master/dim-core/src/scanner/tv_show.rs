#![allow(unstable_name_collisions)]
#![allow(unused_imports)]

use super::movie::asset_from_url;
use super::movie::{best_title_similarity, title_similarity, TITLE_SIMILARITY_THRESHOLD};
use super::MediaMatcher;
use super::Metadata;
use super::WorkUnit;
use dim_extern_api::ExternalEpisode;
use dim_extern_api::ExternalMedia;

use crate::inspect::ResultExt;
use dim_extern_api::ExternalQueryIntoShow;
use dim_extern_api::ExternalQueryShow;
use dim_extern_api::ExternalSeason;

use async_trait::async_trait;

use dim_database::episode::Episode;
use dim_database::episode::InsertableEpisode;
use dim_database::genre::Genre;
use dim_database::genre::InsertableGenre;
use dim_database::genre::InsertableGenreMedia;
use dim_database::library::MediaType;
use dim_database::media::InsertableMedia;
use dim_database::media::Media;
use dim_database::mediafile::MediaFile;
use dim_database::mediafile::UpdateMediaFile;
use dim_database::movie::Movie;
use dim_database::season::InsertableSeason;
use dim_database::season::Season;
use dim_database::tv::TVShow;
use dim_database::Transaction;

use chrono::prelude::Utc;
use chrono::Datelike;

use serde::Serialize;
use std::sync::Arc;
use tracing::error;
use tracing::info;
use tracing::instrument;

use displaydoc::Display;
use thiserror::Error;

#[derive(Clone, Debug, Display, Error, Serialize)]
pub enum Error {
    /// Failed to insert poster into database: {0:?}
    PosterInsert(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to insert backdrop into database: {0:?}
    BackdropInsert(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to decouple genres from media: {0:?}
    GenreDecouple(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to create or get genre: {0:?}
    GetOrInsertGenre(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to attach genre to media object: {0:?}
    CoupleGenre(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to update mediafile to point to new parent: {0:?}
    UpdateMediafile(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to get children count for movie: {0:?}
    ChildrenCount(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to cleanup child-less parent: {0:?}
    ChildCleanup(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to insert or get tv object: {0:?}
    GetOrInsertMedia(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to insert or get season: {0:?}
    GetOrInsertSeason(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to insert media object for episode: {0:?}
    GetOrInsertMediaEpisode(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to insert episode object: {0:?}
    GetOrInsertEpisode(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to get season id for episode: {0:?}
    GetSeasonId(#[serde(skip)] dim_database::DatabaseError),
    /// Failed to get tvshowid for season: {0:?}
    GetTvId(#[serde(skip)] dim_database::DatabaseError),
    /// Season not found
    SeasonNotFound,
    /// Episode not found
    EpisodeNotFound,
}

#[derive(Clone, Copy)]
pub struct TvMatcher;

/// Per-batch memoization: show and season rows only need to be ensured once
/// per scan batch, not once per episode file. For a 200-episode show this
/// avoids ~199 redundant rounds of poster-asset inserts, download enqueues,
/// and genre rebuilds.
#[derive(Default)]
pub struct TvMatchCache {
    /// provider external id → media id
    shows: std::collections::HashMap<String, i64>,
    /// (show media id, season_number) → season id
    seasons: std::collections::HashMap<(i64, i64), i64>,
}

impl TvMatcher {
    async fn match_to_result<'life0>(
        &self,
        tx: &mut Transaction<'life0>,
        file: MediaFile,
        result: (ExternalMedia, ExternalSeason, ExternalEpisode),
        cache: &mut TvMatchCache,
        force_refresh: bool,
    ) -> Result<(i64, i64, i64), Error> {
        let (emedia, eseason, eepisode) = result;

        let cached_show = (!emedia.external_id.is_empty() && !force_refresh)
            .then(|| cache.shows.get(&emedia.external_id).copied())
            .flatten();

        let parent_id = match cached_show {
            Some(id) => id,
            None => {
                let parent_id = self
                    .ensure_show(tx, &file, &emedia, force_refresh)
                    .await?;
                if !emedia.external_id.is_empty() {
                    cache.shows.insert(emedia.external_id.clone(), parent_id);
                }
                parent_id
            }
        };

        let season_key = (parent_id, eseason.season_number as i64);
        let seasonid = match cache.seasons.get(&season_key).copied() {
            Some(id) => id,
            None => {
                let id = self.match_to_season(tx, parent_id, eseason).await?;
                cache.seasons.insert(season_key, id);
                id
            }
        };
        let episodeid = self
            .match_to_episode(tx, file.clone(), seasonid, eepisode)
            .await?;

        // If the mediafile used to belong to a different episode/season/show we want to
        // recursively search if we need to delete the parents. If the parents have 0 children, we
        // want to erase their existance.
        match file.media_id {
            Some(x) if x != episodeid => {
                let season_id = Episode::get_seasonid(tx, x)
                    .await
                    .inspect_err(
                        |error| error!(?error, id = %x, "Failed to get seasonid for episode"),
                    )
                    .map_err(Error::GetSeasonId)?;

                let tvshow_id = Season::get_tvshowid(tx, season_id).await.inspect_err(
                    |error| error!(?error, id = %x, "Failed to get tvshowid for season/episode."),
                ).map_err(Error::GetTvId)?;

                let count = Movie::count_children(tx, x).await.inspect_err(
                    |error| error!(?error, id = %x, "Failed to obtain children count for episode."),
                ).map_err(Error::ChildrenCount)?;

                if count == 0 {
                    Media::delete(tx, x)
                        .await
                        .inspect_err(
                            |error| error!(?error, id = %x, "Failed to delete child-less episode"),
                        )
                        .map_err(Error::ChildCleanup)?;
                }

                let count = Season::count_children(tx, season_id)
                    .await
                    .inspect_err(
                        |error| error!(?error, id = %x, "Failed to get children count for season"),
                    )
                    .map_err(Error::ChildrenCount)?;

                if count == 0 {
                    Season::delete_by_id(tx, season_id)
                        .await
                        .inspect_err(
                            |error| error!(?error, id = %x, "Failed to delete child-less season"),
                        )
                        .map_err(Error::ChildCleanup)?;
                }

                let count = TVShow::count_children(tx, tvshow_id).await.inspect_err(
                    |error| error!(?error, id = %x, "Failed to get children count for tv show."),
                ).map_err(Error::ChildrenCount)?;

                if count == 0 {
                    Media::delete(tx, tvshow_id)
                        .await
                        .inspect_err(
                            |error| error!(?error, id = %x, "Failed to delete child-less tv show"),
                        )
                        .map_err(Error::ChildCleanup)?;
                }
            }
            _ => {}
        }

        Ok((parent_id, seasonid, episodeid))
    }

    /// Get-or-create the show-level media row, including artwork and genres.
    /// Show-level work only happens for newly created rows (or on an explicit
    /// `force_refresh`, i.e. a manual rematch) — existing shows keep their
    /// artwork/genres stable and skip the redundant asset churn.
    async fn ensure_show(
        &self,
        tx: &mut Transaction<'_>,
        file: &MediaFile,
        emedia: &ExternalMedia,
        force_refresh: bool,
    ) -> Result<i64, Error> {
        let posters = emedia
            .posters
            .iter()
            .filter_map(|x| asset_from_url(x))
            .collect::<Vec<_>>();

        let mut poster_ids = vec![];

        for poster in posters {
            let asset = poster
                .insert(&mut *tx)
                .await
                .inspect_err(|error| error!(?error, "Failed to insert asset into db."))
                .map_err(Error::PosterInsert)?;

            poster_ids.push(asset);
        }

        let backdrops = emedia
            .backdrops
            .iter()
            .filter_map(|x| asset_from_url(x))
            .collect::<Vec<_>>();

        let mut backdrop_ids = vec![];

        for backdrop in backdrops {
            let asset = backdrop
                .insert(&mut *tx)
                .await
                .inspect_err(|error| error!(?error, "Failed to insert asset into db."))
                .map_err(Error::BackdropInsert)?;

            backdrop_ids.push(asset);
        }

        // Enqueue poster/backdrop downloads so images are ready before UI requests them.
        for asset in poster_ids.iter().chain(backdrop_ids.iter()) {
            super::enqueue_asset_download(asset).await;
        }

        let media = InsertableMedia {
            media_type: MediaType::Tv,
            library_id: file.library_id,
            name: emedia.title.clone(),
            description: emedia.description.clone(),
            rating: emedia.rating,
            year: emedia.release_date.map(|x| x.year() as _),
            added: Utc::now().to_string(),
            poster: poster_ids.first().map(|x| x.id),
            backdrop: backdrop_ids.first().map(|x| x.id),
        };

        let (parent_id, created) = media
            .lazy_insert_with_external_id(tx, Some(&emedia.external_id))
            .await
            .inspect_err(|error| error!(?error, ?file, "Failed to lazy insert tv show"))
            .map_err(Error::GetOrInsertMedia)?;

        if let Some(ref imdb) = emedia.imdb_id {
            let _ = Media::set_imdb_id(tx, parent_id, imdb)
                .await
                .inspect_err(|error| error!(?error, "Failed to store imdb id."));
        }

        if force_refresh && !created {
            // Manual rematch: the user asked for fresh metadata — replace the
            // artwork with the newly matched show's.
            dim_database::media::UpdateMedia {
                poster: media.poster,
                backdrop: media.backdrop,
                ..Default::default()
            }
            .update(tx, parent_id)
            .await
            .inspect_err(|error| error!(?error, "Failed to refresh media art."))
            .map_err(Error::GetOrInsertMedia)?;
        }

        if created || force_refresh {
            // Rebuild the genre list only when the row is new or explicitly
            // rematched — rebuilding it per episode file was pure churn.
            Genre::decouple_all(tx, parent_id)
                .await
                .inspect_err(|error| error!(?error, "Failed to decouple genres from media."))
                .map_err(Error::GenreDecouple)?;

            for name in emedia.genres.clone() {
                let genre = InsertableGenre { name }
                    .insert(tx)
                    .await
                    .inspect_err(|error| error!(?error, "Failed to create or get genre."))
                    .map_err(Error::GetOrInsertGenre)?;

                InsertableGenreMedia::insert_pair(genre, parent_id, tx)
                    .await
                    .inspect_err(
                        |error| error!(?error, %parent_id, "Failed to attach genre to media object."),
                    )
                    .map_err(Error::CoupleGenre)?;
            }
        }

        Ok(parent_id)
    }

    // FIXME: In cases where we can match against a show but not find a specific season or episode,
    // we want to backfill the data as Season 0 or as an Extra.
    async fn match_to_season(
        &self,
        tx: &mut Transaction<'_>,
        parent_id: i64,
        result: ExternalSeason,
    ) -> Result<i64, Error> {
        let posters = result
            .posters
            .iter()
            .filter_map(|x| asset_from_url(x))
            .collect::<Vec<_>>();

        let mut poster_ids = vec![];

        for poster in posters {
            let asset = poster
                .insert(&mut *tx)
                .await
                .inspect_err(|error| error!(?error, "Failed to insert asset into db."))
                .map_err(Error::PosterInsert)?;

            poster_ids.push(asset);
        }

        for asset in &poster_ids {
            super::enqueue_asset_download(asset).await;
        }

        let season = InsertableSeason {
            season_number: result.season_number as _,
            added: Utc::now().to_string(),
            poster: poster_ids.first().map(|x| x.id),
        };

        let season_id = season
            .insert(tx, parent_id)
            .await
            .inspect_err(|error| error!(?error, "Failed to insert season object."))
            .map_err(Error::GetOrInsertSeason)?;

        Ok(season_id)
    }

    async fn match_to_episode(
        &self,
        tx: &mut Transaction<'_>,
        file: MediaFile,
        seasonid: i64,
        result: ExternalEpisode,
    ) -> Result<i64, Error> {
        let stills = result
            .stills
            .iter()
            .filter_map(|x| asset_from_url(x))
            .collect::<Vec<_>>();

        let mut still_ids = vec![];

        for still in stills {
            let asset = still
                .insert(&mut *tx)
                .await
                .inspect_err(|error| error!(?error, "Failed to insert asset into db."))
                .map_err(Error::PosterInsert)?;

            still_ids.push(asset);
        }

        for asset in &still_ids {
            super::enqueue_asset_download(asset).await;
        }

        let media = InsertableMedia {
            library_id: file.library_id,
            name: result.title_or_episode(),
            added: Utc::now().to_string(),
            media_type: MediaType::Episode,
            description: result.description.clone(),
            backdrop: still_ids.first().map(|x| x.id),
            ..Default::default()
        };

        let episode = InsertableEpisode {
            episode: result.episode_number as _,
            seasonid,
            media,
        };

        // NOTE: `InsertableEpisode::insert` creates the episode's media row
        // itself when needed. A separate blind insert here leaked an orphaned
        // media row for every episode on every scan.
        let episode_id = episode
            .insert(&mut *tx)
            .await
            .inspect_err(|error| error!(?error, ?file, "Failed to insert episode."))
            .map_err(Error::GetOrInsertEpisode)?;

        let updated_mediafile = UpdateMediaFile {
            media_id: Some(episode_id),
            ..Default::default()
        };

        updated_mediafile
            .update(&mut *tx, file.id)
            .await
            .inspect_err(|error| error!(?error, ?file, "Failed to update mediafile media id."))
            .map_err(Error::UpdateMediafile)?;

        Ok(episode_id)
    }

    #[instrument(skip(provider, metadata))]
    async fn lookup_metadata(
        provider: Arc<dyn ExternalQueryShow>,
        file: MediaFile,
        metadata: Vec<Metadata>,
    ) -> Option<(MediaFile, (ExternalMedia, ExternalSeason, ExternalEpisode))> {
        for meta in metadata {
            // Skip metadata with obviously bogus season/episode numbers
            // (e.g. parser misinterpreting resolution or hash values).
            if meta.season.unwrap_or(0) > 100 || meta.episode.unwrap_or(0) > 10000 {
                info!(?meta, "Skipping metadata with implausible season/episode numbers.");
                continue;
            }

            // A language tag in the filename usually marks the AUDIO language
            // while the title stays English — searching TMDB with that
            // language returns TRANSLATED titles (e.g. "Money Heist" +
            // lang=de-DE → "Haus des Geldes"), which then fail the title
            // similarity gate. Try the tagged language first, but fall back
            // to a language-neutral search before giving up on this variant.
            let mut lang_attempts: Vec<Option<&str>> = vec![meta.language.as_deref()];
            if meta.language.is_some() {
                lang_attempts.push(None);
            }

            let mut chosen = None;
            for lang in lang_attempts {
                let mut provided = match provider
                    .search(meta.name.as_ref(), meta.year.map(|x| x as _), lang)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        error!(?meta, error = ?e, "Failed to find a tv show match.");
                        continue;
                    }
                };

                // Retry without year if the year-constrained search returned nothing.
                if provided.is_empty() && meta.year.is_some() {
                    info!(?meta, "Retrying search without year constraint.");
                    provided = provider
                        .search(meta.name.as_ref(), None, lang)
                        .await
                        .unwrap_or_default();
                }

                if provided.is_empty() {
                    continue;
                }

                // Pick the best result by title similarity (checking both English
                // and original-language titles) instead of blindly taking the first.
                // On ties keep the EARLIEST result: providers return
                // popularity-ordered lists, and `max_by` returns the LAST
                // maximum — which hands "Titans" to the 2000 show instead of
                // the 2018 one.
                let first = provided
                    .iter()
                    .fold(None::<(&ExternalMedia, f64)>, |best, x| {
                        let score = best_title_similarity(&meta.name, x);
                        match best {
                            Some((_, best_score)) if best_score >= score => best,
                            _ => Some((x, score)),
                        }
                    })
                    .map(|(x, _)| x)
                    .unwrap()
                    .clone();

                let mut score = best_title_similarity(&meta.name, &first);
                if score < TITLE_SIMILARITY_THRESHOLD {
                    // The provider's own search may have matched on an
                    // alternative title it doesn't return inline (e.g. a
                    // romaji query against a show whose canonical titles are
                    // an English localization + Japanese-script original).
                    // Rescore against alternative titles before rejecting.
                    if let Ok(alt_titles) = provider.alternative_titles(&first.external_id).await {
                        for alt in &alt_titles {
                            score = score.max(title_similarity(&meta.name, alt));
                        }
                    }
                }
                if score < TITLE_SIMILARITY_THRESHOLD {
                    info!(
                        query = %meta.name,
                        result = %first.title,
                        original_title = ?first.original_title,
                        ?lang,
                        %score,
                        "TV result below similarity threshold (incl. alternative titles), skipping."
                    );
                    continue;
                }

                chosen = Some(first);
                break;
            }

            let Some(first) = chosen else {
                continue;
            };

            let Ok(seasons) = provider.seasons_for_id(&first.external_id).await else {
                info!(
                    ?meta,
                    "Failed to find season match with the current metadata set."
                );
                continue;
            };

            // Try to find the exact season requested.
            let desired_season = meta.season.unwrap_or(0);
            if let Some(season) = seasons
                .iter()
                .find(|x| x.season_number as i64 == desired_season)
            {
                let Ok(episodes) = provider
                    .episodes_for_season(&first.external_id, desired_season as _)
                    .await
                else {
                    info!(?meta, "Failed to fetch episodes with current metadata set.");
                    continue;
                };

                if let Some(episode) = episodes
                    .into_iter()
                    .find(|x| x.episode_number as i64 == meta.episode.unwrap_or(0))
                {
                    return Some((file, (first, season.clone(), episode)));
                }
            }

            let desired_episode = meta.episode.unwrap_or(0);

            // Absolute-numbering fallback: anime releases frequently number
            // episodes across the whole show ("Berserk - 13" in Season 2 of a
            // 12-episodes-per-season show). Walk the seasons in order,
            // subtracting each season's episode count, and accept the landing
            // spot when it agrees with the season directory (or when no
            // specific season beyond 1 was requested).
            if desired_episode > 0 {
                let mut ordered: Vec<_> = seasons
                    .iter()
                    .filter(|s| s.season_number > 0)
                    .collect();
                ordered.sort_by_key(|s| s.season_number);

                let mut remaining = desired_episode;
                for candidate_season in ordered {
                    let Ok(eps) = provider
                        .episodes_for_season(
                            &first.external_id,
                            candidate_season.season_number as _,
                        )
                        .await
                    else {
                        break;
                    };

                    if let Some(ep) = eps
                        .iter()
                        .find(|e| e.episode_number as i64 == remaining)
                    {
                        let landing = candidate_season.season_number as i64;
                        // Only trust the mapping when it actually crossed a
                        // season boundary AND lands in the season the
                        // directory named (or there was no season constraint
                        // beyond the default 1).
                        if remaining != desired_episode
                            && (landing == desired_season || desired_season <= 1)
                        {
                            info!(
                                ?meta,
                                absolute_episode = desired_episode,
                                season = landing,
                                episode = remaining,
                                "Mapped absolute episode number through season episode counts."
                            );
                            return Some((file, (first, candidate_season.clone(), ep.clone())));
                        }
                        break;
                    }

                    let count = eps
                        .iter()
                        .map(|e| e.episode_number as i64)
                        .max()
                        .unwrap_or(eps.len() as i64);
                    if remaining <= count {
                        break;
                    }
                    remaining -= count;
                }
            }

            // Broader season fallback: if exact season not found (or episode not in it),
            // search all available seasons for the episode number.
            for candidate_season in &seasons {
                // Skip the season we already tried above.
                if candidate_season.season_number as i64 == desired_season {
                    continue;
                }
                if let Ok(eps) = provider
                    .episodes_for_season(
                        &first.external_id,
                        candidate_season.season_number as _,
                    )
                    .await
                {
                    if let Some(ep) = eps
                        .into_iter()
                        .find(|e| e.episode_number as i64 == desired_episode)
                    {
                        info!(
                            ?meta,
                            found_season = candidate_season.season_number,
                            "Found episode in alternate season."
                        );
                        return Some((file, (first, candidate_season.clone(), ep)));
                    }
                }
            }

            info!(
                ?meta,
                "Provider didnt return our desired season/episode with current metadata."
            );
        }

        None
    }
}

#[async_trait]
impl MediaMatcher for TvMatcher {
    async fn batch_match(
        &self,
        tx: &mut Transaction<'_>,
        provider: Arc<dyn ExternalQueryIntoShow>,
        work: Vec<WorkUnit>,
    ) -> Result<(), super::Error> {
        let provider_show: Arc<dyn ExternalQueryShow> = provider
            .into_query_show()
            .expect("Scanner needs a show provider");

        let mut cache = TvMatchCache::default();

        // Files carrying an explicit provider id (NFO sidecars, Sonarr-style
        // folder tags) skip fuzzy title matching entirely.
        let mut search_units = Vec::new();
        for unit in work {
            match super::nfo::discover_external_id(std::path::Path::new(&unit.0.target_file)) {
                Some(external_id) => {
                    info!(
                        file = %unit.0.target_file,
                        %external_id,
                        "Matching via explicit provider id"
                    );
                    if let Err(error) = self
                        .match_unit_to_external_id(
                            tx,
                            provider_show.clone(),
                            unit,
                            &external_id,
                            &mut cache,
                            false,
                        )
                        .await
                    {
                        error!(?error, %external_id, "failed to match via explicit id");
                    }
                }
                None => search_units.push(unit),
            }
        }

        let metadata_futs = search_units
            .into_iter()
            .map(|WorkUnit(file, metadata)| {
                let provider_show = Arc::clone(&provider_show);
                tokio::spawn(Self::lookup_metadata(provider_show, file, metadata))
            })
            .collect::<Vec<_>>();

        let metadata = futures::future::join_all(metadata_futs).await;

        for meta in metadata.into_iter() {
            if let Ok(Some((file, provided))) = meta {
                // One bad file must not abort (and roll back) the whole batch.
                if let Err(error) = self
                    .match_to_result(tx, file, provided, &mut cache, false)
                    .await
                {
                    error!(?error, "failed to match to result");
                }
            }
        }

        Ok(())
    }

    async fn match_to_id(
        &self,
        tx: &mut Transaction<'_>,
        provider: Arc<dyn ExternalQueryIntoShow>,
        work: WorkUnit,
        external_id: &str,
    ) -> Result<(), super::Error> {
        let provider: Arc<dyn ExternalQueryShow> = provider
            .into_query_show()
            .expect("Scanner needs a show provider");

        // Manual rematch — refresh artwork/genres.
        self.match_unit_to_external_id(
            tx,
            provider,
            work,
            external_id,
            &mut TvMatchCache::default(),
            true,
        )
        .await
    }
}

impl TvMatcher {
    /// Match a single work unit against a known provider id (season/episode
    /// resolved from the unit's filename metadata).
    async fn match_unit_to_external_id(
        &self,
        tx: &mut Transaction<'_>,
        provider: Arc<dyn ExternalQueryShow>,
        work: WorkUnit,
        external_id: &str,
        cache: &mut TvMatchCache,
        force_refresh: bool,
    ) -> Result<(), super::Error> {
        let WorkUnit(file, metadata) = work;

        let provided = match provider.search_by_id(external_id).await {
            Ok(provided) => provided,
            Err(e) => {
                error!(%external_id, error = ?e, "Failed to find a movie match.");
                return Err(super::Error::InvalidExternalId);
            }
        };

        let Ok(all_seasons) = provider.seasons_for_id(external_id).await else {
            return Err(Error::SeasonNotFound.into());
        };

        let mut season_result = None;
        let mut episode_result = None;

        for meta in &metadata {
            // Skip metadata with obviously bogus season/episode numbers
            // (e.g. parser misinterpreting resolution or hash values).
            if meta.season.unwrap_or(0) > 100 || meta.episode.unwrap_or(0) > 10000 {
                info!(?meta, "Skipping metadata with implausible season/episode numbers.");
                continue;
            }

            let desired_season = meta.season.unwrap_or(0);

            // Try exact season match first.
            if let Some(season) = all_seasons
                .iter()
                .find(|x| x.season_number as i64 == desired_season)
            {
                let Ok(episodes) = provider
                    .episodes_for_season(external_id, desired_season as _)
                    .await
                else {
                    info!(?meta, "Failed to fetch episodes with current metadata set.");
                    continue;
                };

                if let Some(episode) = episodes
                    .into_iter()
                    .find(|x| x.episode_number as i64 == meta.episode.unwrap_or(0))
                {
                    season_result = Some(season.clone());
                    episode_result = Some(episode);
                    break;
                }
            }

            // Broader fallback: search all seasons for the desired episode.
            let desired_episode = meta.episode.unwrap_or(0);
            if desired_episode > 0 {
                for candidate_season in &all_seasons {
                    if candidate_season.season_number as i64 == desired_season {
                        continue; // Already tried above.
                    }
                    if let Ok(eps) = provider
                        .episodes_for_season(
                            external_id,
                            candidate_season.season_number as _,
                        )
                        .await
                    {
                        if let Some(ep) = eps
                            .into_iter()
                            .find(|e| e.episode_number as i64 == desired_episode)
                        {
                            info!(
                                ?meta,
                                found_season = candidate_season.season_number,
                                "Found episode in alternate season for manual match."
                            );
                            season_result = Some(candidate_season.clone());
                            episode_result = Some(ep);
                            break;
                        }
                    }
                }
                if season_result.is_some() {
                    break;
                }
            }

            info!(
                ?meta,
                "Provider didnt return our desired season/episode with current metadata."
            );
        }

        let Some(season_result) = season_result else {
            return Err(Error::SeasonNotFound.into());
        };
        let Some(episode_result) = episode_result else {
            return Err(Error::EpisodeNotFound.into());
        };

        self.match_to_result(
            tx,
            file,
            (provided, season_result, episode_result),
            cache,
            force_refresh,
        )
        .await
        .inspect_err(|error| error!(?error, "failed to match to result"))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::mediafile::create_library;
    use super::TvMatcher;

    use dim_extern_api::ExternalEpisode;
    use dim_extern_api::ExternalMedia;
    use dim_extern_api::ExternalSeason;

    use dim_database::episode::Episode;
    use dim_database::media::Media;
    use dim_database::mediafile::InsertableMediaFile;
    use dim_database::mediafile::MediaFile;
    use dim_database::rw_pool::write_tx;
    use dim_database::season::Season;
    use dim_database::tv::TVShow;

    #[tokio::test(flavor = "multi_thread")]
    async fn match_show() {
        const MATCHER: TvMatcher = TvMatcher;

        let mut conn = dim_database::get_conn_memory()
            .await
            .expect("Failed to obtain a in-memory db pool.");
        let library = create_library(&mut conn).await;

        let mut lock = conn.writer.lock_owned().await;
        let mut tx = write_tx(&mut lock).await.unwrap();

        let mut mediafile = InsertableMediaFile {
            library_id: library,
            target_file: "test.mp4".into(),
            raw_name: "test".into(),
            ..Default::default()
        };

        let mfile_id = mediafile.insert(&mut tx).await.unwrap();

        let emedia = ExternalMedia {
            title: "Show 1".into(),
            ..Default::default()
        };

        let eseason = ExternalSeason {
            season_number: 1,
            ..Default::default()
        };

        let eepisode = ExternalEpisode {
            episode_number: 1,
            ..Default::default()
        };

        let mut result = (emedia, eseason, eepisode);

        let mfile = MediaFile::get_one(&mut tx, mfile_id).await.unwrap();
        let (m1, s1, e1) = MATCHER
            .match_to_result(&mut tx, mfile, result.clone(), &mut Default::default(), false)
            .await
            .unwrap();

        mediafile.target_file = "test2.mp4".into();
        let mfile2_id = mediafile.insert(&mut tx).await.unwrap();

        let mfile2 = MediaFile::get_one(&mut tx, mfile2_id).await.unwrap();
        let (m2, s2, e2) = MATCHER
            .match_to_result(&mut tx, mfile2, result.clone(), &mut Default::default(), false)
            .await
            .unwrap();

        // We attach two mediafiles to the same episode, season and show
        assert_eq!(m1, m2);
        assert_eq!(s1, s2);
        assert_eq!(e1, e2);

        mediafile.target_file = "test3.mp4".into();
        result.2.episode_number = 2;
        let mfile3_id = mediafile.insert(&mut tx).await.unwrap();

        let mfile3 = MediaFile::get_one(&mut tx, mfile3_id).await.unwrap();
        let (m3, s3, e3) = MATCHER
            .match_to_result(&mut tx, mfile3, result.clone(), &mut Default::default(), false)
            .await
            .unwrap();

        // we attach a third mediafile to the same show and season but different episode
        assert_eq!(m2, m3);
        assert_eq!(s2, s3);
        assert_ne!(e2, e3);

        mediafile.target_file = "test4.mp4".into();
        result.1.season_number = 2;
        let mfile4_id = mediafile.insert(&mut tx).await.unwrap();

        let mfile4 = MediaFile::get_one(&mut tx, mfile4_id).await.unwrap();
        let (m4, s4, e4) = MATCHER
            .match_to_result(&mut tx, mfile4, result.clone(), &mut Default::default(), false)
            .await
            .unwrap();

        // we attach a fourth mediafile to a different season and episode but same show
        assert_eq!(m4, m3);
        assert_ne!(s4, s3);
        assert_ne!(e4, e3);

        // we should have two seasons matched
        let seasons = Season::get_all(&mut tx, m4).await.unwrap();
        assert_eq!(seasons.len(), 2);

        // First season should have two episodes.
        let episodes = Episode::get_all_of_season(&mut tx, s2).await.unwrap();
        assert_eq!(episodes.len(), 2);

        // Last season should have one episode
        let episodes = Episode::get_all_of_season(&mut tx, s4).await.unwrap();
        assert_eq!(episodes.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rematch_episode() {
        crate::setup_test_logging();
        const MATCHER: TvMatcher = TvMatcher;

        let mut conn = dim_database::get_conn_memory()
            .await
            .expect("Failed to obtain a in-memory db pool.");
        let library = create_library(&mut conn).await;

        let mut lock = conn.writer.lock_owned().await;
        let mut tx = write_tx(&mut lock).await.unwrap();

        let mut mediafile = InsertableMediaFile {
            library_id: library,
            target_file: "test.mp4".into(),
            raw_name: "test".into(),
            ..Default::default()
        };

        let mfile_id = mediafile.insert(&mut tx).await.unwrap();

        mediafile.target_file = "test1.mp4".into();
        let mfile2_id = mediafile.insert(&mut tx).await.unwrap();

        let emedia = ExternalMedia {
            title: "Show 1".into(),
            ..Default::default()
        };

        let eseason = ExternalSeason {
            season_number: 1,
            ..Default::default()
        };

        let eepisode = ExternalEpisode {
            episode_number: 1,
            ..Default::default()
        };

        let mut result = (emedia, eseason, eepisode);

        let mfile = MediaFile::get_one(&mut tx, mfile_id).await.unwrap();
        let mfile2 = MediaFile::get_one(&mut tx, mfile2_id).await.unwrap();

        let (t1, s1, _e1) = MATCHER
            .match_to_result(&mut tx, mfile.clone(), result.clone(), &mut Default::default(), false)
            .await
            .unwrap();

        MATCHER
            .match_to_result(&mut tx, mfile2.clone(), result.clone(), &mut Default::default(), false)
            .await
            .unwrap();

        let mfile = MediaFile::get_one(&mut tx, mfile_id).await.unwrap();
        let mfile2 = MediaFile::get_one(&mut tx, mfile2_id).await.unwrap();

        // Rematch mfile 1 to a different show
        result.1.season_number = 1;
        result.0.title = "Show 2".into();

        let _seasons = Season::get_all(&mut tx, t1).await.unwrap();

        let (t2, s2, _e2) = MATCHER
            .match_to_result(&mut tx, mfile, result.clone(), &mut Default::default(), false)
            .await
            .unwrap();

        // Season 1 of t1 should have only one episode at this point.
        let episodes = Episode::get_all_of_season(&mut tx, s1).await.unwrap();
        assert_eq!(episodes.len(), 1);

        result.2.episode_number = 2;
        MATCHER
            .match_to_result(&mut tx, mfile2, result.clone(), &mut Default::default(), false)
            .await
            .unwrap();

        // Season 1 of t1 should not exist now because no episodes are linked to it anymore
        let res = Season::get_by_id(&mut tx, s1).await;
        assert!(res.is_err());

        // Tv show should not exist anymore because no seasons are linked to it.
        let res = Media::get(&mut tx, t1).await;
        assert!(res.is_err());

        // New show should exist and have one season with two episodes.
        let count = TVShow::count_children(&mut tx, t2).await.unwrap();
        assert_eq!(count, 1);

        let episodes = Episode::get_all_of_season(&mut tx, s2).await.unwrap();
        assert_eq!(episodes.len(), 2);
    }
}

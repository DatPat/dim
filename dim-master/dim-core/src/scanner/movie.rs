#![allow(unstable_name_collisions)]
#![allow(unused_imports)]

use crate::inspect::ResultExt;
use crate::scanner::format_path;
use dim_extern_api::ExternalMedia;
use dim_extern_api::ExternalQueryIntoShow;

use super::MediaMatcher;
use super::WorkUnit;

use async_trait::async_trait;
use chrono::prelude::Utc;
use chrono::Datelike;

use dim_database::asset::InsertableAsset;
use dim_database::genre::Genre;
use dim_database::genre::InsertableGenre;
use dim_database::genre::InsertableGenreMedia;
use dim_database::library::MediaType;
use dim_database::media::InsertableMedia;
use dim_database::media::Media;
use dim_database::mediafile::MediaFile;
use dim_database::mediafile::UpdateMediaFile;
use dim_database::movie::Movie;
use dim_database::Transaction;

use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::error;
use tracing::info;
use tracing::warn;

use url::Url;

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
    /// Failed to insert or get media object: {0:?}
    GetOrInsertMedia(#[serde(skip)] dim_database::DatabaseError),
}

pub fn asset_from_url(url: &str) -> Option<InsertableAsset> {
    let url = Url::parse(url).ok()?;
    let filename = uuid::Uuid::new_v4().as_hyphenated().to_string();
    let local_path = format_path(Some(format!("{filename}.jpg")));

    Some(InsertableAsset {
        remote_url: Some(url.into()),
        local_path,
        file_ext: "jpg".into(),
    })
}

/// Returns true if the string contains any CJK Unified Ideograph characters,
/// indicating it is likely Chinese/Japanese/Korean text where word boundaries
/// are not marked by whitespace.
fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c,
            '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
            | '\u{3400}'..='\u{4DBF}' // CJK Extension A
            | '\u{3040}'..='\u{309F}' // Hiragana
            | '\u{30A0}'..='\u{30FF}' // Katakana
            | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
        )
    })
}

/// Strip punctuation that filenames commonly drop (apostrophes, colons, etc.)
/// so "Cat's Eye" and "Cats Eye" are compared equally.
fn strip_punctuation(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect()
}

/// Score how similar a query title is to a result title.
/// Returns a value between 0.0 (no match) and 1.0 (exact match).
///
/// For CJK text (where words aren't separated by spaces), uses character-level
/// overlap instead of word-level Jaccard similarity.
pub(crate) fn title_similarity(query: &str, result: &str) -> f64 {
    let q = strip_punctuation(&query.to_lowercase());
    let r = strip_punctuation(&result.to_lowercase());
    if q == r {
        return 1.0;
    }
    if r.contains(&q) || q.contains(&r) {
        return 0.8;
    }

    // For CJK text, use character-level overlap since there are no word boundaries.
    if has_cjk(&q) || has_cjk(&r) {
        let q_chars: HashSet<char> = q.chars().filter(|c| !c.is_whitespace()).collect();
        let r_chars: HashSet<char> = r.chars().filter(|c| !c.is_whitespace()).collect();
        let common = q_chars.intersection(&r_chars).count();
        let total = q_chars.len().max(r_chars.len());
        if total == 0 {
            return 0.0;
        }
        return common as f64 / total as f64;
    }

    let q_words: HashSet<&str> = q.split_whitespace().collect();
    let r_words: HashSet<&str> = r.split_whitespace().collect();
    let common = q_words.intersection(&r_words).count();
    let total = q_words.len().max(r_words.len());
    if total == 0 {
        0.0
    } else {
        common as f64 / total as f64
    }
}

/// Score a query against an ExternalMedia, checking both the English title and
/// the original-language title, returning the best score.
pub(crate) fn best_title_similarity(query: &str, media: &ExternalMedia) -> f64 {
    let score = title_similarity(query, &media.title);
    match &media.original_title {
        Some(orig) if !orig.is_empty() => score.max(title_similarity(query, orig)),
        _ => score,
    }
}

pub(crate) const TITLE_SIMILARITY_THRESHOLD: f64 = 0.3;

#[derive(Clone, Copy)]
pub struct MovieMatcher;

impl MovieMatcher {
    /// Method will match a mediafile to a new media. Caller must ensure that the mediafile supplied is not coupled to a media object. If it is coupled we will assume that we can
    /// replace the metadata supplied to it.
    #[tracing::instrument(skip(self, tx))]
    async fn match_to_result<'life0>(
        &self,
        tx: &mut Transaction<'life0>,
        file: MediaFile,
        provided: ExternalMedia,
    ) -> Result<i64, Error> {
        // TODO: Push posters and backdrops to download queue. Push CDC events.
        let posters = provided
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

        let backdrops = provided
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

        let media = InsertableMedia {
            media_type: MediaType::Movie,
            library_id: file.library_id,
            name: provided.title,
            description: provided.description,
            rating: provided.rating,
            year: provided.release_date.map(|x| x.year() as _),
            added: Utc::now().to_string(),
            poster: poster_ids.first().map(|x| x.id),
            backdrop: backdrop_ids.first().map(|x| x.id),
        };

        // Dedup by the provider's id — matching by title string minted a new
        // media row whenever the provider returned a different title variant
        // (language, year, retitle) for the same movie.
        let (media_id, created) = media
            .lazy_insert_with_external_id(tx, Some(&provided.external_id))
            .await
            .map_err(Error::GetOrInsertMedia)?;

        if let Some(ref imdb) = provided.imdb_id {
            let _ = Media::set_imdb_id(tx, media_id, imdb)
                .await
                .inspect_err(|error| error!(?error, "Failed to store imdb id."));
        }

        // Link all backdrops and posters to our media and enqueue downloads.
        for poster in poster_ids {
            let _ = poster
                .into_media_poster(tx, media_id)
                .await
                .inspect_err(|error| warn!(?error, "Failed to link poster to media."));
            super::enqueue_asset_download(&poster).await;
        }

        for backdrop in backdrop_ids {
            let _ = backdrop
                .into_media_backdrop(tx, media_id)
                .await
                .inspect_err(|error| warn!(?error, "Failed to link backdrop."));
            super::enqueue_asset_download(&backdrop).await;
        }

        // Rebuild the genre list only for new rows or when a mediafile is
        // being (re)pointed at this media — rebuilding on every rescan of an
        // unchanged file is pure churn.
        if created || file.media_id != Some(media_id) {
            Genre::decouple_all(tx, media_id)
                .await
                .inspect_err(|error| error!(?error, "Failed to decouple genres from media."))
                .map_err(Error::GenreDecouple)?;

            for name in provided.genres {
                let genre = InsertableGenre { name }
                    .insert(tx)
                    .await
                    .inspect_err(|error| error!(?error, "Failed to create or get genre."))
                    .map_err(Error::GetOrInsertGenre)?;

                InsertableGenreMedia::insert_pair(genre, media_id, tx)
                    .await
                    .inspect_err(
                        |error| error!(?error, %media_id, "Failed to attach genre to media object."),
                    )
                    .map_err(Error::CoupleGenre)?;
            }
        }

        // Update mediafile to point to a new parent media_id. We also want to set raw_name and
        // raw_year to what its parent has so that when we refresh metadata, files that were
        // matched manually (due to bogus filenames) dont get unmatched, or matched wrongly.
        UpdateMediaFile {
            media_id: Some(media_id),
            raw_name: Some(media.name),
            raw_year: media.year,
            ..Default::default()
        }
        .update(tx, file.id)
        .await
        .inspect_err(|error| error!(?error, "Failed to update mediafile to point to new parent."))
        .map_err(Error::UpdateMediafile)?;

        // Sometimes we rematch against a media object that already exists but we are the last
        // child for the parent. When this happens we want to cleanup.
        match file.media_id {
            Some(old_id) => {
                let count = Movie::count_children(tx, old_id)
                    .await
                    .inspect_err(|error| error!(?error, %old_id, "Failed to grab children count."))
                    .map_err(Error::ChildrenCount)?;

                if count == 0 {
                    Media::delete(tx, old_id)
                        .await
                        .inspect_err(
                            |error| error!(?error, %old_id, "Failed to cleanup child-less parent."),
                        )
                        .map_err(Error::ChildCleanup)?;
                }
            }
            _ => {}
        }

        Ok(media_id)
    }
}

#[async_trait]
impl MediaMatcher for MovieMatcher {
    async fn batch_match(
        &self,
        tx: &mut Transaction<'_>,
        provider: Arc<dyn ExternalQueryIntoShow>,
        work: Vec<WorkUnit>,
    ) -> Result<(), super::Error> {
        // Files carrying an explicit provider id (NFO sidecars, Radarr-style
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
                    match provider.search_by_id(&external_id).await {
                        Ok(provided) => {
                            if let Err(error) =
                                self.match_to_result(tx, unit.0, provided).await
                            {
                                error!(?error, %external_id, "failed to match via explicit id");
                            }
                        }
                        Err(error) => {
                            error!(?error, %external_id, "failed to resolve explicit id");
                        }
                    }
                }
                None => search_units.push(unit),
            }
        }
        let work = search_units;

        let metadata_futs = work
            .into_iter()
            .map(|WorkUnit(file, metadata)| async {
                for meta in metadata {
                    // Language tags usually mark the AUDIO language while the
                    // title stays English — a language-scoped search returns
                    // translated titles that fail the similarity gate. Fall
                    // back to a language-neutral search before giving up.
                    let mut lang_attempts: Vec<Option<&str>> = vec![meta.language.as_deref()];
                    if meta.language.is_some() {
                        lang_attempts.push(None);
                    }

                    for lang in lang_attempts {
                        let mut results = match provider
                            .search(meta.name.as_ref(), meta.year.map(|x| x as _), lang)
                            .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                error!(?meta, error = ?e, "Failed to find a movie match.");
                                continue;
                            }
                        };

                        // Retry without year if the year-constrained search returned nothing.
                        if results.is_empty() && meta.year.is_some() {
                            info!(?meta, "Retrying movie search without year constraint.");
                            results = provider
                                .search(meta.name.as_ref(), None, lang)
                                .await
                                .unwrap_or_default();
                        }

                        if results.is_empty() {
                            continue;
                        }

                        // Pick the best result by title similarity (checking both English
                        // and original-language titles) instead of blindly taking the first.
                        // On ties keep the EARLIEST result — provider lists are
                        // popularity-ordered and `max_by` returns the last maximum.
                        let best = results
                            .iter()
                            .fold(None::<(&_, f64)>, |best, x| {
                                let score = best_title_similarity(&meta.name, x);
                                match best {
                                    Some((_, best_score)) if best_score >= score => best,
                                    _ => Some((x, score)),
                                }
                            })
                            .map(|(x, _)| x);

                        if let Some(best) = best {
                            let mut score = best_title_similarity(&meta.name, best);
                            if score < TITLE_SIMILARITY_THRESHOLD {
                                // The provider's search may have matched on an
                                // alternative title it doesn't return inline
                                // (romaji, regional releases) — rescore before
                                // rejecting.
                                if let Ok(alt_titles) =
                                    provider.alternative_titles(&best.external_id).await
                                {
                                    for alt in &alt_titles {
                                        score = score.max(title_similarity(&meta.name, alt));
                                    }
                                }
                            }
                            if score >= TITLE_SIMILARITY_THRESHOLD {
                                return Some((file, best.clone()));
                            }
                            info!(
                                query = %meta.name,
                                result = %best.title,
                                original_title = ?best.original_title,
                                ?lang,
                                %score,
                                "Movie result below similarity threshold (incl. alternative titles), skipping."
                            );
                        }
                    }
                }

                None
            })
            .collect::<Vec<_>>();

        let metadata = futures::future::join_all(metadata_futs).await;

        for meta in metadata.into_iter() {
            if let Some((file, provided)) = meta {
                // One bad file must not abort (and roll back) the whole batch.
                if let Err(error) = self.match_to_result(tx, file, provided).await {
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
        let WorkUnit(file, _) = work;

        let provided = match provider.search_by_id(external_id).await {
            Ok(provided) => provided,
            Err(e) => {
                error!(%external_id, error = ?e, "Failed to find a movie match.");
                return Err(super::Error::InvalidExternalId);
            }
        };

        self.match_to_result(tx, file, provided)
            .await
            .inspect_err(|error| error!(?error, "failed to match file to external id."))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::mediafile::create_library;
    use super::{title_similarity, best_title_similarity};

    use super::MovieMatcher;
    use dim_extern_api::ExternalMedia;

    #[test]
    fn similarity_exact_match() {
        assert_eq!(title_similarity("Dunkelstadt", "Dunkelstadt"), 1.0);
    }

    #[test]
    fn similarity_case_insensitive() {
        assert_eq!(title_similarity("dunkelstadt", "Dunkelstadt"), 1.0);
    }

    #[test]
    fn similarity_substring() {
        assert!(title_similarity("Dunkelstadt", "Dunkelstadt S01E01") >= 0.8);
    }

    #[test]
    fn similarity_cjk_exact() {
        // Japanese title exact match
        assert_eq!(title_similarity("進撃の巨人", "進撃の巨人"), 1.0);
    }

    #[test]
    fn similarity_cjk_partial() {
        // Partial CJK overlap should still score above threshold
        let score = title_similarity("進撃の巨人", "進撃の巨人 Season 2");
        assert!(score >= 0.3, "CJK partial score {score} should be >= 0.3");
    }

    #[test]
    fn similarity_cross_language_zero() {
        // Japanese vs English should score near zero with word-based matching
        let score = title_similarity("進撃の巨人", "Attack on Titan");
        assert!(score < 0.3, "Cross-language score {score} should be < 0.3");
    }

    #[test]
    fn best_similarity_uses_original_title() {
        // When the filename is in the original language, best_title_similarity
        // should find the match via original_title even if the English title
        // doesn't match at all.
        let media = ExternalMedia {
            title: "Attack on Titan".into(),
            original_title: Some("進撃の巨人".into()),
            ..Default::default()
        };

        let score = best_title_similarity("進撃の巨人", &media);
        assert_eq!(score, 1.0, "Should match via original_title");
    }

    #[test]
    fn similarity_apostrophe_stripped() {
        // Filenames often drop apostrophes: "Cats Eye" vs TMDB "Cat's Eye"
        let score = title_similarity("Cats Eye", "Cat's Eye");
        assert_eq!(score, 1.0, "Apostrophe should be stripped: {score}");
    }

    #[test]
    fn similarity_colon_stripped() {
        // Filenames often drop colons: "Star Wars The Clone Wars" vs "Star Wars: The Clone Wars"
        let score = title_similarity("Star Wars The Clone Wars", "Star Wars: The Clone Wars");
        assert_eq!(score, 1.0);
    }

    #[test]
    fn best_similarity_german_show() {
        let media = ExternalMedia {
            title: "Dunkelstadt".into(),
            original_title: Some("Dunkelstadt".into()),
            ..Default::default()
        };

        let score = best_title_similarity("Dunkelstadt", &media);
        assert_eq!(score, 1.0);
    }

    use chrono::TimeZone;
    use dim_database::genre::Genre;
    use dim_database::media::Media;
    use dim_database::mediafile::InsertableMediaFile;
    use dim_database::mediafile::MediaFile;
    use dim_database::movie::Movie;
    use dim_database::rw_pool::write_tx;

    #[tokio::test(flavor = "multi_thread")]
    async fn match_to_movie() {
        let mut conn = dim_database::get_conn_memory()
            .await
            .expect("Failed to obtain a in-memory db pool.");
        let library = create_library(&mut conn).await;

        let mut lock = conn.writer.lock_owned().await;
        let mut tx = write_tx(&mut lock).await.unwrap();

        let mediafile = InsertableMediaFile {
            library_id: library,
            target_file: "test.mp4".into(),
            raw_name: "test".into(),
            ..Default::default()
        }
        .insert(&mut tx)
        .await
        .unwrap();

        let mfile = MediaFile::get_one(&mut tx, mediafile).await.unwrap();

        // no media should be linked to the mfile at this point
        assert_eq!(mfile.media_id, None);

        let dummy_external = ExternalMedia {
            external_id: "123".into(),
            title: "Test Title".into(),
            description: Some("test description".into()),
            release_date: chrono::Utc.with_ymd_and_hms(1983, 1, 10, 0, 0, 0).single(),
            posters: vec![],
            backdrops: vec![],
            genres: vec!["Comedy".into()],
            rating: Some(0.0),
            ..Default::default()
        };

        const MATCHER: MovieMatcher = MovieMatcher;

        let media_id = MATCHER
            .match_to_result(&mut tx, mfile, dummy_external)
            .await
            .unwrap();

        let mfile = MediaFile::get_one(&mut tx, mediafile).await.unwrap();

        // mediafile should now be linked to a media object
        assert_eq!(mfile.media_id, Some(media_id));

        let media_obj = Media::get(&mut tx, media_id).await.unwrap();
        assert_eq!(media_obj.name, "Test Title".to_string());

        // Genre should be Comedy at this point.
        let genres = Genre::get_by_media(&mut tx, media_id).await.unwrap();
        assert_eq!(genres[0].name, "Comedy".to_string());

        let dummy_external = ExternalMedia {
            external_id: "456".into(),
            title: "Other title".into(),
            description: None,
            genres: vec!["Anime".into()],
            ..Default::default()
        };

        let updated_media = MATCHER
            .match_to_result(&mut tx, mfile, dummy_external.clone())
            .await
            .unwrap();

        // in-place replacement doesnt exist, so we should get a new media id here.
        assert_ne!(media_id, updated_media);

        // the old object should be automatically erased.
        assert!(Media::get(&mut tx, media_id).await.is_err());

        let media_obj = Media::get(&mut tx, updated_media).await.unwrap();
        assert_eq!(media_obj.name, "Other title".to_string());
        assert_eq!(media_obj.description, None);

        // insert a new mediafile and link it to the same media object.
        let mfile2_id = InsertableMediaFile {
            library_id: library,
            target_file: "test2.mp4".into(),
            raw_name: "test".into(),
            ..Default::default()
        }
        .insert(&mut tx)
        .await
        .unwrap();

        let mfile2 = MediaFile::get_one(&mut tx, mfile2_id).await.unwrap();

        let mfile2_mediaid = MATCHER
            .match_to_result(&mut tx, mfile2, dummy_external)
            .await
            .unwrap();

        // the new mediafile should point to the same parent as the previous mediafile.
        assert_eq!(mfile2_mediaid, updated_media);

        let children_cnt = Movie::count_children(&mut tx, mfile2_mediaid)
            .await
            .unwrap();

        assert_eq!(children_cnt, 2);

        // now that the parent has multiple children, rematching should trigger the creation of a
        // new media and trigger a decoupling.
        let mfile = MediaFile::get_one(&mut tx, mediafile).await.unwrap();
        let dummy_external = ExternalMedia {
            external_id: "789".into(),
            title: "Other other title".into(),
            description: None,
            genres: vec!["Anime".into()],
            ..Default::default()
        };

        let updated_media = MATCHER
            .match_to_result(&mut tx, mfile, dummy_external)
            .await
            .unwrap();

        assert_ne!(updated_media, mfile2_mediaid);

        let media_obj = Media::get(&mut tx, updated_media).await.unwrap();
        assert_eq!(media_obj.name, "Other other title".to_string());
        assert_eq!(media_obj.description, None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rematch_new_genres() {
        let mut conn = dim_database::get_conn_memory()
            .await
            .expect("Failed to obtain a in-memory db pool.");
        let library = create_library(&mut conn).await;

        let mut lock = conn.writer.lock_owned().await;
        let mut tx = write_tx(&mut lock).await.unwrap();

        const MATCHER: MovieMatcher = MovieMatcher;

        let mfile_id = InsertableMediaFile {
            library_id: library,
            target_file: "test.mp4".into(),
            raw_name: "test".into(),
            ..Default::default()
        }
        .insert(&mut tx)
        .await
        .unwrap();

        let mfile = MediaFile::get_one(&mut tx, mfile_id).await.unwrap();

        let dummy_external = ExternalMedia {
            external_id: "123".into(),
            title: "Title".into(),
            description: None,
            genres: vec!["Anime".into()],
            ..Default::default()
        };

        let media = MATCHER
            .match_to_result(&mut tx, mfile.clone(), dummy_external)
            .await
            .unwrap();

        let genres = Genre::get_by_media(&mut tx, media).await.unwrap();

        // Only linked to genre Anime
        assert_eq!(genres.len(), 1);
        assert_eq!(genres[0].name, "Anime".to_string());

        let dummy_external = ExternalMedia {
            external_id: "123".into(),
            title: "Title".into(),
            description: None,
            genres: vec!["Comedy".into(), "Adventure".into()],
            ..Default::default()
        };

        MATCHER
            .match_to_result(&mut tx, mfile, dummy_external)
            .await
            .unwrap();

        let genres = Genre::get_by_media(&mut tx, media).await.unwrap();

        // Now linked to two genres
        assert_eq!(genres.len(), 2);
        assert_eq!(genres[0].name, "Comedy".to_string());
        assert_eq!(genres[1].name, "Adventure".to_string());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mass_rematch() {
        let mut conn = dim_database::get_conn_memory()
            .await
            .expect("Failed to obtain a in-memory db pool.");
        let library = create_library(&mut conn).await;

        let mut lock = conn.writer.lock_owned().await;
        let mut tx = write_tx(&mut lock).await.unwrap();

        const MATCHER: MovieMatcher = MovieMatcher;

        let mfile_id = InsertableMediaFile {
            library_id: library,
            target_file: "test.mp4".into(),
            raw_name: "test".into(),
            ..Default::default()
        }
        .insert(&mut tx)
        .await
        .unwrap();

        let mfile = MediaFile::get_one(&mut tx, mfile_id).await.unwrap();

        let mfile2_id = InsertableMediaFile {
            library_id: library,
            target_file: "test1.mp4".into(),
            raw_name: "test".into(),
            ..Default::default()
        }
        .insert(&mut tx)
        .await
        .unwrap();

        let mfile2 = MediaFile::get_one(&mut tx, mfile2_id).await.unwrap();

        // link two files to the same media.
        let dummy_external = ExternalMedia {
            external_id: "123".into(),
            title: "Test Title".into(),
            description: Some("test description".into()),
            release_date: chrono::Utc.with_ymd_and_hms(1983, 1, 10, 0, 0, 0).single(),
            posters: vec![],
            backdrops: vec![],
            genres: vec!["Comedy".into()],
            rating: Some(0.0),
            ..Default::default()
        };

        let media_id = MATCHER
            .match_to_result(&mut tx, mfile.clone(), dummy_external.clone())
            .await
            .unwrap();

        let media_id2 = MATCHER
            .match_to_result(&mut tx, mfile2.clone(), dummy_external.clone())
            .await
            .unwrap();

        assert_eq!(media_id, media_id2);
        assert!(Movie::count_children(&mut tx, media_id).await.unwrap() == 2);

        let dummy_external = ExternalMedia {
            external_id: "456".into(),
            title: "Other title".into(),
            description: None,
            genres: vec!["Anime".into()],
            ..Default::default()
        };

        let mfile = MediaFile::get_one(&mut tx, mfile_id).await.unwrap();

        // Match one child file from first media to a new one.
        let updated_media = MATCHER
            .match_to_result(&mut tx, mfile, dummy_external.clone())
            .await
            .unwrap();

        // we should now have two medias
        assert!(Media::get_all(&mut tx, library).await.unwrap().len() == 2);

        // now match second file, after this point we need to have only one media object in the
        // database.
        let mfile2 = MediaFile::get_one(&mut tx, mfile2_id).await.unwrap();

        let updated_media2 = MATCHER
            .match_to_result(&mut tx, mfile2, dummy_external.clone())
            .await
            .unwrap();

        assert!(updated_media == updated_media2);

        // we should now have one media
        assert_eq!(Media::get_all(&mut tx, library).await.unwrap().len(), 1);
    }

    /// Test refreshes metadata in-place for some media object. Refreshing only works if the new
    /// metadata has the same title and mediatype as the previous metadata. Otherwise it is
    /// rematched which will change the id of the object.
    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_metadata() {
        let mut conn = dim_database::get_conn_memory()
            .await
            .expect("Failed to obtain a in-memory db pool.");
        let library = create_library(&mut conn).await;

        let mut lock = conn.writer.lock_owned().await;
        let mut tx = write_tx(&mut lock).await.unwrap();

        const MATCHER: MovieMatcher = MovieMatcher;

        let mfile_id = InsertableMediaFile {
            library_id: library,
            target_file: "test.mp4".into(),
            raw_name: "test".into(),
            ..Default::default()
        }
        .insert(&mut tx)
        .await
        .unwrap();

        let mfile = MediaFile::get_one(&mut tx, mfile_id).await.unwrap();

        // link two files to the same media.
        let mut dummy_external = ExternalMedia {
            external_id: "123".into(),
            title: "Test Title".into(),
            description: Some("test description".into()),
            release_date: chrono::Utc.with_ymd_and_hms(1983, 1, 10, 0, 0, 0).single(),
            posters: vec![],
            backdrops: vec![],
            genres: vec!["Comedy".into()],
            rating: Some(0.0),
            ..Default::default()
        };

        let media_id = MATCHER
            .match_to_result(&mut tx, mfile.clone(), dummy_external.clone())
            .await
            .unwrap();

        dummy_external.description = Some("new description".into());
        dummy_external.rating = Some(10.0);

        let refreshed_id = MATCHER
            .match_to_result(&mut tx, mfile.clone(), dummy_external.clone())
            .await
            .unwrap();

        // refreshing means that we just fetch updated metadata but dont rematch as its the same
        // media technically. as such the ids here should remain equal
        assert_eq!(media_id, refreshed_id);

        let media = Media::get(&mut tx, refreshed_id).await.unwrap();

        // we should see the new rating and description here
        assert_eq!(media.description, Some("new description".into()));
        assert_eq!(media.rating, Some(10.0));
    }
}

//! Construction of metadata providers for libraries.
//!
//! Each library selects a primary provider ("tmdb" | "tvmaze" | "anilist");
//! the others act as fallbacks in a [`ChainedProvider`], and id-based lookups
//! route by the id's namespace so mixed-provider libraries keep working.

use std::sync::Arc;

use dim_database::library::MediaType;
use dim_extern_api::anilist::AniListProvider;
use dim_extern_api::chain::ChainedProvider;
use dim_extern_api::tmdb::TMDBMetadataProvider;
use dim_extern_api::tvmaze::TvMazeProvider;
use dim_extern_api::ExternalQueryIntoShow;

/// The TMDB API key from settings, falling back to Dim's shared key when
/// blank.
pub fn tmdb_api_key() -> String {
    let key = crate::settings::get_global_settings().tmdb_api_key;
    if key.trim().is_empty() {
        crate::settings::default_tmdb_key()
    } else {
        key
    }
}

/// Build the metadata provider chain for a library.
pub fn provider_for(provider_name: &str, media_type: MediaType) -> Arc<dyn ExternalQueryIntoShow> {
    let tmdb = TMDBMetadataProvider::new(&tmdb_api_key());

    match media_type {
        MediaType::Movie => {
            let tmdb: Arc<dyn ExternalQueryIntoShow> = Arc::new(tmdb.movies());
            match provider_name {
                // AniList covers anime movies too.
                "anilist" => Arc::new(ChainedProvider::new(vec![
                    Arc::new(AniListProvider::new()),
                    tmdb,
                ])),
                _ => tmdb,
            }
        }
        MediaType::Tv => {
            let tmdb: Arc<dyn ExternalQueryIntoShow> = Arc::new(tmdb.tv_shows());
            let tvmaze: Arc<dyn ExternalQueryIntoShow> = Arc::new(TvMazeProvider::new());
            match provider_name {
                "tvmaze" => Arc::new(ChainedProvider::new(vec![tvmaze, tmdb])),
                "anilist" => Arc::new(ChainedProvider::new(vec![
                    Arc::new(AniListProvider::new()),
                    tmdb,
                ])),
                _ => Arc::new(ChainedProvider::new(vec![tmdb, tvmaze])),
            }
        }
        _ => unimplemented!("no metadata provider for media type {media_type:?}"),
    }
}

use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZeroU32;
use std::result::Result;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use async_trait::async_trait;

use tokio::sync::broadcast;
use tracing::instrument;

use crate::Result as QueryResult;
use crate::*;

use governor::clock::DefaultClock;
use governor::middleware::NoOpMiddleware;
use governor::state::direct::NotKeyed;
use governor::state::InMemoryState;
use governor::Quota;
use governor::RateLimiter;

use super::cache_control::{AbortOnDropHandle, CacheEviction, CacheKey, CacheMap, CacheValue};
use super::raw_client::TMDBClient;
use super::*;

/// How long items should be cached for. Defaults to 12 hours.
const CACHED_ITEM_TTL: Duration = Duration::from_secs(60 * 60 * 12);
/// How many requests we can send per second.
// TMDB's guidance is ~40-50 req/s; the old 128/s quota (amplified by
// unthrottled retries) was itself a source of 429 storms.
const REQ_QUOTA: NonZeroU32 = unsafe { NonZeroU32::new_unchecked(40) };

type Governor = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

/// TMDB Metadata Provider produces `ExternalQuery` implementors, and handles request coalescing and caching locally.
///
/// This type is already internally full of Arc's, there is no need to wrap it in another one.
pub struct TMDBMetadataProvider {
    pub(super) api_key: Arc<str>,
    pub(super) http_client: reqwest::Client,
    cache: CacheMap,
    cache_size: Arc<AtomicUsize>,
    cache_eviction: Arc<AbortOnDropHandle>,
    pub(self) governor: Arc<Governor>,
}

impl Clone for TMDBMetadataProvider {
    fn clone(&self) -> Self {
        Self {
            api_key: self.api_key.clone(),
            http_client: self.http_client.clone(),
            cache: self.cache.clone(),
            cache_size: self.cache_size.clone(),
            cache_eviction: self.cache_eviction.clone(),
            governor: self.governor.clone(),
        }
    }
}

impl TMDBMetadataProvider {
    /// Create a new metadata provider instance with this API key.
    pub fn new(api_key: &str) -> Self {
        let http_client = reqwest::ClientBuilder::new()
            .user_agent(APP_USER_AGENT)
            .brotli(true)
            .tcp_keepalive(Some(Duration::from_millis(16_000)))
            .tcp_nodelay(true)
            .build()
            .expect("building this client should never fail.");

        let api_key: Arc<str> = api_key.to_owned().into_boxed_str().into();

        let cache: CacheMap = Default::default();
        let cache_size = Arc::new(AtomicUsize::new(0));

        let cache_eviction = Arc::new(
            CacheEviction::new(cache.clone(), cache_size.clone(), 102_400_000).start_policy(),
        );

        let governor = Arc::new(Governor::direct(Quota::per_second(REQ_QUOTA)));

        Self {
            // FIXME: Make max cache size configurable at start-time.
            api_key,
            http_client,
            cache,
            cache_size,
            cache_eviction,
            governor,
        }
    }

    /// curry this metadata provider to supply search results for TV shows.
    #[inline(always)]
    pub fn tv_shows(&self) -> MetadataProviderOf<TvShows> {
        MetadataProviderOf {
            provider: self.clone(),
            _key: PhantomData,
        }
    }

    /// curry this metadata provider to supply search results for movies.
    #[inline(always)]
    pub fn movies(&self) -> MetadataProviderOf<Movies> {
        MetadataProviderOf {
            provider: self.clone(),
            _key: PhantomData,
        }
    }

    /// insert a default [CacheValue] if the slot at a given key is not present or it has expired.
    fn insert_value_if_empty(&self, key: &CacheKey) -> (CacheValue, bool) {
        // grab the entry or instert RwLock::new(None) if not present.
        let mut entry = self.cache.entry(key.clone()).or_default();

        // fast path: cache hits, no writers, the value is present and it hasn't TTL'd
        // TODO: Test TTL mechanism.
        let needs_cleanup = {
            let read_guard = entry.value();
            if let Some(value) = read_guard.as_ref() {
                if matches!(value, CacheValue::Body { ttl, .. } if *ttl > Instant::now()) {
                    return (value.clone(), false);
                }

                true
            } else {
                false
            }
        };

        // slow path: get a write guard, if the slot is still uninit when we acquire; initialize it.s
        let slot = entry.value_mut();

        match slot.as_ref() {
            // someone initialized the slot before we got the write guard, use their value.
            Some(value) if !needs_cleanup => return (value.clone(), false),
            _ => {}
        }

        // we're still first or the old value needs to be cleaned, get rid of it.
        let (tx, _) = broadcast::channel(1);
        let value = CacheValue::RequestInFlight { tx };

        if let Some(old) = slot.replace(value.clone()) {
            // Unsure how relaxed ordering will hold up on non-x86 targets.
            self.cache_size.fetch_sub(old.mem_size(), Ordering::Relaxed);
        }

        (value, true)
    }

    /// perform request coalescing; when two futures are made with the same key the duplicates wait for the original to broadcast the results.
    async fn coalesce_request<F, Fut>(
        &self,
        key: &CacheKey,
        make_request_future: F,
        ttl: Duration,
    ) -> Result<Arc<str>, Error>
    where
        F: FnOnce(TMDBClient) -> Fut,
        Fut: Future<Output = Result<Arc<str>, TMDBClientRequestError>> + Send + 'static,
    {
        let (value, we_own_future) = { self.insert_value_if_empty(key) };

        match (we_own_future, value) {
            // only if we own the future that was spawned should we upate the
            // value once we get the response.
            (true, CacheValue::RequestInFlight { tx }) => {
                let client = TMDBClient {
                    provider: self.clone(),
                };

                let tx_ = tx.clone();
                let fut = make_request_future(client);

                // we want to ratelimit locally. Thus we wait for a permit until we spawn the
                // request future.
                self.governor.until_ready().await;

                tokio::spawn(async move {
                    let output = fut.await;
                    let _ = tx_.send(output);
                });

                match { CacheValue::RequestInFlight { tx } }.data().await {
                    Ok(text) => {
                        let body = Arc::clone(&text);

                        let value = CacheValue::Body {
                            text,
                            ttl: Instant::now() + ttl,
                        };

                        // Increase our memory usage tracker.
                        self.cache_size
                            .fetch_add(value.mem_size(), Ordering::Relaxed);

                        match self.cache.get_mut(key) {
                            Some(mut entry_ref) => entry_ref.replace(value),
                            None => unreachable!("slot was None when original task got its result"),
                        };

                        Ok(body)
                    }
                    // if an error was yeeted back during the request then purge the cache entry.
                    Err(error) => {
                        // clear the cache for values that could not be populated.
                        let _ = self.cache.remove(&key);
                        Err(error.into())
                    }
                }
            }

            (_, value) => Ok(value.data().await?),
        }
    }

    /// perform a TMDB search for `title` and optionally `year` of a specific search type (movies or TV shows.)
    ///
    /// request coalescing is applied internally.
    async fn search(
        &self,
        title: &str,
        year: Option<i32>,
        media_type: MediaSearchType,
        language: Option<&str>,
    ) -> QueryResult<Vec<ExternalMedia>> {
        let title = title.to_string();
        let language_owned = language.map(|s| s.to_string());
        let key = CacheKey::Search {
            title: title.clone(),
            year,
            language: language_owned.clone(),
        };

        let st = self
            .coalesce_request(
                &key,
                |client| async move {
                    client
                        .search(media_type, &title, year, language_owned.as_deref())
                        .await
                        .map(|st| st.into())
                },
                CACHED_ITEM_TTL,
            )
            .await?;

        let mut search = serde_json::from_str::<SearchResponse>(&st).map_err(|error| {
            Error::DeserializationError {
                body: st,
                error: format!("{error}"),
            }
        })?;

        // fill in the genre names for the search results.
        {
            let key = CacheKey::GenreList { media_type };
            let st = self
                .coalesce_request(
                    &key,
                    |client| async move { client.genre_list(media_type).await.map(|st| st.into()) },
                    CACHED_ITEM_TTL,
                )
                .await?;

            let genre_list = serde_json::from_str::<GenreList>(&st).map_err(|error| {
                Error::DeserializationError {
                    body: st,
                    error: format!("{error}"),
                }
            })?;

            let mut genre_id_cache = HashMap::<u64, Genre>::with_capacity(search.results.len());
            for media in search.results.iter_mut() {
                if let Some(TMDBMediaObject {
                    genre_ids: Some(ids),
                    genres,
                    ..
                }) = media
                {
                    let genre_vec = genres.insert(vec![]);

                    for genre_id in ids.iter().cloned() {
                        if let Some(genre) = genre_id_cache.get(&genre_id) {
                            genre_vec.push(Genre {
                                id: genre_id,
                                name: genre.name.clone(),
                            });
                        } else if let Some(genre) =
                            genre_list.genres.iter().find(|x| x.id == genre_id)
                        {
                            genre_id_cache.insert(genre_id, genre.clone());
                            genre_vec.push(Genre {
                                id: genre_id,
                                name: genre.name.clone(),
                            });
                        }
                    }
                }
            }
        }

        let media = search
            .results
            .into_iter()
            .flatten()
            .map(ExternalMedia::from)
            .collect::<Vec<_>>();

        Ok(media)
    }

    /// Resolve an id from another catalogue (imdb/tvdb) into a TMDB media
    /// object via the `/find` endpoint.
    async fn find_by_source(
        &self,
        source: &str,
        raw_id: &str,
        media_type: MediaSearchType,
    ) -> QueryResult<ExternalMedia> {
        let source_param = format!("{source}_id");
        let raw_id_owned = raw_id.to_string();
        let key = CacheKey::ById {
            id: format!("find/{source}/{raw_id}"),
            ty: media_type,
        };

        let response_body = self
            .coalesce_request(
                &key,
                |client| async move {
                    client
                        .find_by_external_source(&source_param, &raw_id_owned)
                        .await
                        .map(|st| st.into())
                },
                CACHED_ITEM_TTL,
            )
            .await?;

        #[derive(serde::Deserialize)]
        struct FindResponse {
            #[serde(default)]
            movie_results: Vec<TMDBMediaObject>,
            #[serde(default)]
            tv_results: Vec<TMDBMediaObject>,
        }

        let found = serde_json::from_str::<FindResponse>(&response_body).map_err(|err| {
            Error::DeserializationError {
                body: response_body,
                error: format!("{err}"),
            }
        })?;

        let candidate = match media_type {
            MediaSearchType::Movie => found.movie_results.into_iter().next(),
            MediaSearchType::Tv => found.tv_results.into_iter().next(),
        };

        let media = candidate.ok_or(Error::NoResults {
            query: format!("{source}:{raw_id}"),
            year: None,
        })?;

        // Refetch through details so the full metadata (genres, runtime,
        // imdb id) is populated.
        self.search_by_id(&media.id.to_string(), media_type).await
    }

    async fn search_by_id(
        &self,
        external_id: &str,
        media_type: MediaSearchType,
    ) -> QueryResult<ExternalMedia> {
        let external_id = external_id.to_string();
        let key = CacheKey::ById {
            id: external_id.clone(),
            ty: media_type,
        };

        let response_body = self
            .coalesce_request(
                &key,
                |client| async move {
                    client
                        .get_details(media_type, &external_id)
                        .await
                        .map(|st| st.into())
                },
                CACHED_ITEM_TTL,
            )
            .await?;

        let details = serde_json::from_str::<TMDBMediaObject>(&response_body).map_err(|err| {
            Error::DeserializationError {
                body: response_body,
                error: format!("{err}"),
            }
        })?;

        Ok(details.into())
    }

    /// Alternative titles from the (cached) details response — regional and
    /// romaji titles TMDB's own search indexes but doesn't return inline.
    async fn alternative_titles_by_id(
        &self,
        external_id: &str,
        media_type: MediaSearchType,
    ) -> QueryResult<Vec<String>> {
        let external_id = external_id.to_string();
        let key = CacheKey::ById {
            id: external_id.clone(),
            ty: media_type,
        };

        let response_body = self
            .coalesce_request(
                &key,
                |client| async move {
                    client
                        .get_details(media_type, &external_id)
                        .await
                        .map(|st| st.into())
                },
                CACHED_ITEM_TTL,
            )
            .await?;

        let details = serde_json::from_str::<TMDBMediaObject>(&response_body).map_err(|err| {
            Error::DeserializationError {
                body: response_body,
                error: format!("{err}"),
            }
        })?;

        Ok(details
            .alternative_titles
            .and_then(|at| at.results)
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.title)
            .filter(|t| !t.is_empty())
            .collect())
    }

    async fn cast(
        &self,
        external_id: &str,
        media_type: MediaSearchType,
    ) -> QueryResult<Vec<ExternalActor>> {
        let external_id = external_id.to_string();
        let key = CacheKey::ActorById {
            id: external_id.clone(),
        };

        let resp = self
            .coalesce_request(
                &key,
                |client| async move {
                    client
                        .get_actor(media_type, &external_id)
                        .await
                        .map(|st| st.into())
                },
                CACHED_ITEM_TTL,
            )
            .await?;

        let actor =
            serde_json::from_str::<Cast>(&resp).map_err(|error| Error::DeserializationError {
                body: resp,
                error: format!("{error}"),
            })?;

        Ok(actor.cast.into_iter().map(|x| x.into()).collect())
    }

    async fn seasons_by_id(&self, external_id: &str) -> QueryResult<Vec<ExternalSeason>> {
        let external_id = external_id.to_string();
        let key = CacheKey::ById {
            id: external_id.clone(),
            ty: MediaSearchType::Tv,
        };

        // NOTE: We share the same key as search by id because the seasons are returned inline with
        // that response but the interface requires that theyre decoupled.
        let response_body = self
            .coalesce_request(
                &key,
                |client| async move {
                    client
                        .get_details(MediaSearchType::Tv, &external_id)
                        .await
                        .map(|st| st.into())
                },
                CACHED_ITEM_TTL,
            )
            .await?;

        let tv_details = serde_json::from_str::<TvSeasons>(&response_body).map_err(|err| {
            Error::DeserializationError {
                body: response_body,
                error: format!("{err}"),
            }
        })?;

        Ok(tv_details.into())
    }

    async fn episodes_by_id(
        &self,
        external_id: &str,
        season_number: u64,
    ) -> QueryResult<Vec<ExternalEpisode>> {
        let external_id = external_id.to_string();
        let key = CacheKey::Episodes {
            id: external_id.clone(),
            season_number,
        };

        let response_body = self
            .coalesce_request(
                &key,
                |client| async move {
                    client
                        .get_episodes(&external_id, season_number)
                        .await
                        .map(|st| st.into())
                },
                CACHED_ITEM_TTL,
            )
            .await?;

        let tv_details = serde_json::from_str::<TvEpisodes>(&response_body).map_err(|err| {
            Error::DeserializationError {
                body: response_body,
                error: format!("{err}"),
            }
        })?;

        Ok(tv_details.into())
    }
}

// -- MetadataProviderOf<T>

/// Used to key [`MetadataProviderOf`] to search for TV shows, compliments [Movies].
pub struct TvShows;

/// Used to key [`MetadataProviderOf`] to search for movies, compliments [TvShows].
pub struct Movies;

/// An instance of [`TMDBMetadataProvider`] with a generic parameter to infer the [`MediaType`](dim-database::library::MediaType) for searches.
pub struct MetadataProviderOf<K>
where
    K: sealed::AssocMediaTypeConst + Send + Sync + 'static,
{
    pub provider: TMDBMetadataProvider,
    _key: PhantomData<K>,
}

impl<K: sealed::AssocMediaTypeConst + Send + Sync + 'static> std::fmt::Debug
    for MetadataProviderOf<K>
{
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("TMDBMetadataProviderOf<K>")
            .field("key", &K::MEDIA_TYPE)
            .finish()
    }
}

mod sealed {
    use super::{MediaSearchType, Movies, TvShows};

    /// Used to associate a constant [MediaType] with another type.
    pub trait AssocMediaTypeConst {
        const MEDIA_TYPE: MediaSearchType;
    }

    impl AssocMediaTypeConst for TvShows {
        const MEDIA_TYPE: MediaSearchType = MediaSearchType::Tv;
    }

    impl AssocMediaTypeConst for Movies {
        const MEDIA_TYPE: MediaSearchType = MediaSearchType::Movie;
    }
}

/// Strip this provider's scheme prefix from an id; bare (legacy) ids pass
/// through unchanged.
fn strip_tmdb_prefix(external_id: &str) -> &str {
    match crate::parse_external_id(external_id) {
        (Some("tmdb"), raw) => raw,
        _ => external_id,
    }
}

fn prefix_media(mut media: ExternalMedia) -> ExternalMedia {
    media.external_id = format!("tmdb:{}", media.external_id);
    media
}

#[async_trait]
impl<K> ExternalQuery for MetadataProviderOf<K>
where
    K: sealed::AssocMediaTypeConst + Send + Sync + 'static,
{
    fn provider_scheme(&self) -> &'static str {
        "tmdb"
    }

    #[instrument]
    async fn search(&self, title: &str, year: Option<i32>, language: Option<&str>) -> QueryResult<Vec<ExternalMedia>> {
        Ok(self
            .provider
            .search(title, year, K::MEDIA_TYPE, language)
            .await?
            .into_iter()
            .map(prefix_media)
            .collect())
    }

    #[instrument]
    async fn search_by_id(&self, external_id: &str) -> QueryResult<ExternalMedia> {
        // Ids from other catalogues (NFO files, Sonarr/Radarr folder tags)
        // are resolved through TMDB's /find endpoint.
        let media = match crate::parse_external_id(external_id) {
            (Some("imdb"), raw) => self.provider.find_by_source("imdb", raw, K::MEDIA_TYPE).await?,
            (Some("tvdb"), raw) => self.provider.find_by_source("tvdb", raw, K::MEDIA_TYPE).await?,
            (Some("tmdb"), raw) => self.provider.search_by_id(raw, K::MEDIA_TYPE).await?,
            _ => self.provider.search_by_id(external_id, K::MEDIA_TYPE).await?,
        };
        Ok(prefix_media(media))
    }

    #[instrument]
    async fn cast(&self, external_id: &str) -> QueryResult<Vec<ExternalActor>> {
        self.provider
            .cast(strip_tmdb_prefix(external_id), K::MEDIA_TYPE)
            .await
    }

    #[instrument]
    async fn alternative_titles(&self, external_id: &str) -> QueryResult<Vec<String>> {
        self.provider
            .alternative_titles_by_id(strip_tmdb_prefix(external_id), K::MEDIA_TYPE)
            .await
    }
}

impl IntoQueryShow for MetadataProviderOf<TvShows> {
    fn as_query_show<'a>(&'a self) -> Option<&'a dyn ExternalQueryShow> {
        Some(self)
    }

    fn into_query_show(self: Arc<Self>) -> Option<Arc<dyn ExternalQueryShow>> {
        Some(self)
    }
}

impl IntoQueryShow for MetadataProviderOf<Movies> {
    fn as_query_show<'a>(&'a self) -> Option<&'a dyn ExternalQueryShow> {
        None
    }

    fn into_query_show(self: Arc<Self>) -> Option<Arc<dyn ExternalQueryShow>> {
        None
    }
}

impl ExternalQueryIntoShow for MetadataProviderOf<TvShows> {}
impl ExternalQueryIntoShow for MetadataProviderOf<Movies> {}

#[async_trait]
impl ExternalQueryShow for MetadataProviderOf<TvShows> {
    #[instrument]
    async fn seasons_for_id(&self, external_id: &str) -> QueryResult<Vec<ExternalSeason>> {
        let mut seasons = self
            .provider
            .seasons_by_id(strip_tmdb_prefix(external_id))
            .await?;
        seasons.sort_by(|a, b| a.season_number.cmp(&b.season_number));

        Ok(seasons)
    }

    #[instrument]
    async fn episodes_for_season(
        &self,
        external_id: &str,
        season_number: u64,
    ) -> QueryResult<Vec<ExternalEpisode>> {
        let mut episodes = self
            .provider
            .episodes_by_id(strip_tmdb_prefix(external_id), season_number)
            .await?;

        episodes.sort_by(|a, b| a.episode_number.cmp(&b.episode_number));

        Ok(episodes)
    }
}

//! TVmaze metadata provider — a free, keyless TV metadata API.
//!
//! Used as a fallback when TMDB can't match a show (or as the primary
//! provider when a library is configured for it). TV shows only.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::{
    parse_external_id, Error, ExternalActor, ExternalEpisode, ExternalMedia, ExternalQuery,
    ExternalQueryIntoShow, ExternalQueryShow, ExternalSeason, IntoQueryShow, Result,
};

use governor::clock::DefaultClock;
use governor::middleware::NoOpMiddleware;
use governor::state::direct::NotKeyed;
use governor::state::InMemoryState;
use governor::{Quota, RateLimiter};

const TVMAZE_BASE_URL: &str = "https://api.tvmaze.com";

/// TVmaze allows ~20 requests / 10 seconds per IP.
const REQ_PER_SEC: u32 = 2;

type Governor = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

pub struct TvMazeProvider {
    client: reqwest::Client,
    governor: Arc<Governor>,
}

impl std::fmt::Debug for TvMazeProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TvMazeProvider").finish()
    }
}

impl TvMazeProvider {
    pub fn new() -> Self {
        let client = reqwest::ClientBuilder::new()
            .user_agent(crate::tmdb::APP_USER_AGENT)
            .build()
            .expect("building this client should never fail");

        Self {
            client,
            governor: Arc::new(RateLimiter::direct(Quota::per_second(
                std::num::NonZeroU32::new(REQ_PER_SEC).unwrap(),
            ))),
        }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.governor.until_ready().await;

        let url = format!("{TVMAZE_BASE_URL}{path}");
        let response = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(Error::other)?;

        let status = response.status();
        let body = response.text().await.map_err(Error::other)?;

        if !status.is_success() {
            return Err(Error::RemoteApiError {
                code: status.as_u16(),
                message: body,
            });
        }

        serde_json::from_str::<T>(&body).map_err(|err| Error::DeserializationError {
            body: body.into(),
            error: format!("{err}"),
        })
    }
}

impl Default for TvMazeProvider {
    fn default() -> Self {
        Self::new()
    }
}

// -- API data models

#[derive(Deserialize, Debug)]
struct TvMazeSearchResult {
    show: TvMazeShow,
}

#[derive(Deserialize, Debug)]
struct TvMazeShow {
    id: u64,
    name: String,
    premiered: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    rating: Option<TvMazeRating>,
    image: Option<TvMazeImage>,
    #[serde(rename = "averageRuntime")]
    average_runtime: Option<u64>,
    externals: Option<TvMazeExternals>,
}

#[derive(Deserialize, Debug)]
struct TvMazeExternals {
    imdb: Option<String>,
}

#[derive(Deserialize, Debug)]
struct TvMazeRating {
    average: Option<f64>,
}

#[derive(Deserialize, Debug)]
struct TvMazeImage {
    original: Option<String>,
    medium: Option<String>,
}

impl TvMazeImage {
    fn best(&self) -> Option<String> {
        self.original.clone().or_else(|| self.medium.clone())
    }
}

#[derive(Deserialize, Debug)]
struct TvMazeSeason {
    number: i64,
    image: Option<TvMazeImage>,
    id: u64,
}

#[derive(Deserialize, Debug)]
struct TvMazeEpisode {
    id: u64,
    name: Option<String>,
    season: i64,
    number: Option<i64>,
    summary: Option<String>,
    image: Option<TvMazeImage>,
    runtime: Option<u64>,
}

/// TVmaze summaries are small HTML fragments — strip the tags.
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn parse_release_date(premiered: &Option<String>) -> Option<chrono::DateTime<chrono::Utc>> {
    let date = premiered.as_deref()?;
    let s = format!("{date} 00:00:00 +0000");
    chrono::DateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S %z")
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

impl From<TvMazeShow> for ExternalMedia {
    fn from(show: TvMazeShow) -> Self {
        ExternalMedia {
            external_id: format!("tvmaze:{}", show.id),
            title: show.name,
            original_title: None,
            description: show.summary.as_deref().map(strip_html),
            release_date: parse_release_date(&show.premiered),
            posters: show.image.as_ref().and_then(|i| i.best()).into_iter().collect(),
            backdrops: vec![],
            genres: show.genres,
            rating: show.rating.and_then(|r| r.average).map(|avg| avg / 10.0),
            duration: show
                .average_runtime
                .map(|mins| Duration::from_secs(mins * 60)),
            imdb_id: show.externals.and_then(|e| e.imdb),
        }
    }
}

fn raw_id(external_id: &str) -> &str {
    match parse_external_id(external_id) {
        (Some("tvmaze"), raw) => raw,
        _ => external_id,
    }
}

#[async_trait]
impl ExternalQuery for TvMazeProvider {
    fn provider_scheme(&self) -> &'static str {
        "tvmaze"
    }

    async fn search(
        &self,
        title: &str,
        year: Option<i32>,
        _language: Option<&str>,
    ) -> Result<Vec<ExternalMedia>> {
        let query = urlencoding_encode(title);
        let results: Vec<TvMazeSearchResult> =
            self.get_json(&format!("/search/shows?q={query}")).await?;

        let mut media: Vec<ExternalMedia> = results
            .into_iter()
            .map(|r| ExternalMedia::from(r.show))
            .collect();

        // Soft year filter: prefer matches on the requested year, but keep
        // everything if nothing matches (the caller re-ranks by title).
        if let Some(year) = year {
            let filtered: Vec<_> = media
                .iter()
                .filter(|m| {
                    m.release_date
                        .map(|d| chrono::Datelike::year(&d) == year)
                        .unwrap_or(false)
                })
                .cloned()
                .collect();
            if !filtered.is_empty() {
                media = filtered;
            }
        }

        Ok(media)
    }

    async fn search_by_id(&self, external_id: &str) -> Result<ExternalMedia> {
        let id = raw_id(external_id);
        let show: TvMazeShow = self.get_json(&format!("/shows/{id}")).await?;
        Ok(show.into())
    }

    async fn cast(&self, external_id: &str) -> Result<Vec<ExternalActor>> {
        #[derive(Deserialize)]
        struct CastEntry {
            person: Person,
            character: Character,
        }
        #[derive(Deserialize)]
        struct Person {
            id: u64,
            name: String,
            image: Option<TvMazeImage>,
        }
        #[derive(Deserialize)]
        struct Character {
            name: String,
        }

        let id = raw_id(external_id);
        let cast: Vec<CastEntry> = self.get_json(&format!("/shows/{id}/cast")).await?;

        Ok(cast
            .into_iter()
            .map(|c| ExternalActor {
                external_id: format!("tvmaze:{}", c.person.id),
                name: c.person.name,
                profile_path: c.person.image.as_ref().and_then(|i| i.best()),
                character: c.character.name,
            })
            .collect())
    }
}

#[async_trait]
impl ExternalQueryShow for TvMazeProvider {
    async fn seasons_for_id(&self, external_id: &str) -> Result<Vec<ExternalSeason>> {
        let id = raw_id(external_id);
        let seasons: Vec<TvMazeSeason> = self.get_json(&format!("/shows/{id}/seasons")).await?;

        let mut out: Vec<ExternalSeason> = seasons
            .into_iter()
            .filter(|s| s.number >= 0)
            .map(|s| ExternalSeason {
                external_id: format!("tvmaze:{}", s.id),
                title: None,
                description: None,
                posters: s.image.as_ref().and_then(|i| i.best()).into_iter().collect(),
                season_number: s.number as u64,
            })
            .collect();

        out.sort_by_key(|s| s.season_number);
        Ok(out)
    }

    async fn episodes_for_season(
        &self,
        external_id: &str,
        season_number: u64,
    ) -> Result<Vec<ExternalEpisode>> {
        let id = raw_id(external_id);
        let episodes: Vec<TvMazeEpisode> = self.get_json(&format!("/shows/{id}/episodes")).await?;

        let mut out: Vec<ExternalEpisode> = episodes
            .into_iter()
            .filter(|e| e.season as u64 == season_number)
            .filter_map(|e| {
                Some(ExternalEpisode {
                    external_id: format!("tvmaze:{}", e.id),
                    title: e.name.clone(),
                    description: e.summary.as_deref().map(strip_html),
                    episode_number: e.number? as u64,
                    stills: e.image.as_ref().and_then(|i| i.best()).into_iter().collect(),
                    duration: e.runtime.map(|mins| Duration::from_secs(mins * 60)),
                })
            })
            .collect();

        out.sort_by_key(|e| e.episode_number);
        Ok(out)
    }
}

impl IntoQueryShow for TvMazeProvider {
    fn as_query_show<'a>(&'a self) -> Option<&'a dyn ExternalQueryShow> {
        Some(self)
    }

    fn into_query_show(self: Arc<Self>) -> Option<Arc<dyn ExternalQueryShow>> {
        Some(self)
    }
}

impl ExternalQueryIntoShow for TvMazeProvider {}

/// Minimal percent-encoding for a query-string value.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

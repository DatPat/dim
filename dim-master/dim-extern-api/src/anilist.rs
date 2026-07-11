//! AniList metadata provider — free, keyless GraphQL API for anime.
//!
//! TMDB is notoriously weak at anime (absolute numbering, odd season splits,
//! missing OVAs/specials). AniList models each cour/season as a separate
//! entry, which we expose as a show with a single season whose episodes come
//! from `streamingEpisodes` when available, or are synthesized 1..=episodes
//! otherwise. Suitable for "Show/Episode 01-12" style anime libraries.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::{
    parse_external_id, Error, ExternalActor, ExternalEpisode, ExternalMedia, ExternalQuery,
    ExternalQueryIntoShow, ExternalQueryShow, ExternalSeason, IntoQueryShow, Result,
};

use governor::clock::DefaultClock;
use governor::middleware::NoOpMiddleware;
use governor::state::direct::NotKeyed;
use governor::state::InMemoryState;
use governor::{Quota, RateLimiter};

const ANILIST_URL: &str = "https://graphql.anilist.co";

type Governor = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

const MEDIA_FIELDS: &str = r#"
    id
    title { romaji english native }
    description(asHtml: false)
    startDate { year month day }
    coverImage { extraLarge large }
    bannerImage
    genres
    averageScore
    episodes
    duration
    streamingEpisodes { title thumbnail }
"#;

pub struct AniListProvider {
    client: reqwest::Client,
    governor: Arc<Governor>,
}

impl std::fmt::Debug for AniListProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AniListProvider").finish()
    }
}

impl AniListProvider {
    pub fn new() -> Self {
        let client = reqwest::ClientBuilder::new()
            .user_agent(crate::tmdb::APP_USER_AGENT)
            .build()
            .expect("building this client should never fail");

        Self {
            client,
            // AniList allows 90 req/min; stay well under it.
            governor: Arc::new(RateLimiter::direct(Quota::per_minute(
                std::num::NonZeroU32::new(60).unwrap(),
            ))),
        }
    }

    async fn graphql<T: serde::de::DeserializeOwned>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T> {
        self.governor.until_ready().await;

        let response = self
            .client
            .post(ANILIST_URL)
            .timeout(Duration::from_secs(15))
            .json(&json!({ "query": query, "variables": variables }))
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

        #[derive(Deserialize)]
        struct GraphQlResponse<T> {
            data: Option<T>,
        }

        let parsed: GraphQlResponse<T> =
            serde_json::from_str(&body).map_err(|err| Error::DeserializationError {
                body: body.clone().into(),
                error: format!("{err}"),
            })?;

        parsed.data.ok_or(Error::DeserializationError {
            body: body.into(),
            error: "GraphQL response contained no data".into(),
        })
    }

    async fn media_by_id(&self, id: &str) -> Result<AniListMedia> {
        let id: i64 = id.parse().map_err(|_| Error::NoResults {
            query: id.to_string(),
            year: None,
        })?;

        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "Media")]
            media: AniListMedia,
        }

        let query = format!("query ($id: Int) {{ Media(id: $id, type: ANIME) {{ {MEDIA_FIELDS} }} }}");
        let data: Data = self.graphql(&query, json!({ "id": id })).await?;
        Ok(data.media)
    }
}

impl Default for AniListProvider {
    fn default() -> Self {
        Self::new()
    }
}

// -- API data models

#[derive(Deserialize, Debug, Clone)]
struct AniListMedia {
    id: u64,
    title: AniListTitle,
    description: Option<String>,
    #[serde(rename = "startDate")]
    start_date: Option<AniListDate>,
    #[serde(rename = "coverImage")]
    cover_image: Option<AniListCover>,
    #[serde(rename = "bannerImage")]
    banner_image: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(rename = "averageScore")]
    average_score: Option<f64>,
    episodes: Option<u64>,
    duration: Option<u64>,
    #[serde(rename = "streamingEpisodes", default)]
    streaming_episodes: Vec<AniListStreamingEpisode>,
}

#[derive(Deserialize, Debug, Clone)]
struct AniListTitle {
    romaji: Option<String>,
    english: Option<String>,
    native: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct AniListDate {
    year: Option<i32>,
    month: Option<u32>,
    day: Option<u32>,
}

#[derive(Deserialize, Debug, Clone)]
struct AniListCover {
    #[serde(rename = "extraLarge")]
    extra_large: Option<String>,
    large: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct AniListStreamingEpisode {
    title: Option<String>,
    thumbnail: Option<String>,
}

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

impl From<AniListMedia> for ExternalMedia {
    fn from(media: AniListMedia) -> Self {
        let title = media
            .title
            .english
            .clone()
            .or_else(|| media.title.romaji.clone())
            .or_else(|| media.title.native.clone())
            .unwrap_or_default();

        let original_title = media.title.romaji.clone().or(media.title.native.clone());

        let release_date = media.start_date.as_ref().and_then(|d| {
            let year = d.year?;
            let month = d.month.unwrap_or(1);
            let day = d.day.unwrap_or(1);
            chrono::NaiveDate::from_ymd_opt(year, month, day)
                .and_then(|nd| nd.and_hms_opt(0, 0, 0))
                .map(|ndt| chrono::DateTime::from_naive_utc_and_offset(ndt, chrono::Utc))
        });

        ExternalMedia {
            external_id: format!("anilist:{}", media.id),
            title,
            original_title,
            description: media.description.as_deref().map(strip_html),
            release_date,
            posters: media
                .cover_image
                .as_ref()
                .and_then(|c| c.extra_large.clone().or_else(|| c.large.clone()))
                .into_iter()
                .collect(),
            backdrops: media.banner_image.clone().into_iter().collect(),
            genres: media.genres.clone(),
            rating: media.average_score.map(|s| s / 100.0),
            duration: media.duration.map(|mins| Duration::from_secs(mins * 60)),
            imdb_id: None,
        }
    }
}

fn raw_id(external_id: &str) -> &str {
    match parse_external_id(external_id) {
        (Some("anilist"), raw) => raw,
        _ => external_id,
    }
}

/// Parse an episode number out of a streamingEpisodes title
/// ("Episode 5 - The Name" → (5, "The Name")).
fn parse_streaming_title(title: &str) -> (Option<u64>, Option<String>) {
    let rest = title.trim().strip_prefix("Episode ").unwrap_or(title);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return (None, Some(title.to_string()));
    }
    let number = digits.parse().ok();
    let name = rest[digits.len()..]
        .trim_start_matches([' ', '-', ':'])
        .trim()
        .to_string();
    (number, if name.is_empty() { None } else { Some(name) })
}

#[async_trait]
impl ExternalQuery for AniListProvider {
    fn provider_scheme(&self) -> &'static str {
        "anilist"
    }

    async fn search(
        &self,
        title: &str,
        year: Option<i32>,
        _language: Option<&str>,
    ) -> Result<Vec<ExternalMedia>> {
        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "Page")]
            page: Page,
        }
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            media: Vec<AniListMedia>,
        }

        let query = format!(
            "query ($search: String) {{ Page(perPage: 10) {{ media(search: $search, type: ANIME) {{ {MEDIA_FIELDS} }} }} }}"
        );
        let data: Data = self.graphql(&query, json!({ "search": title })).await?;

        let mut media: Vec<ExternalMedia> =
            data.page.media.into_iter().map(Into::into).collect();

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
        Ok(self.media_by_id(raw_id(external_id)).await?.into())
    }

    async fn cast(&self, _external_id: &str) -> Result<Vec<ExternalActor>> {
        // Voice-actor credits don't map cleanly onto the actor model; leave
        // empty rather than misrepresent.
        Ok(vec![])
    }
}

#[async_trait]
impl ExternalQueryShow for AniListProvider {
    async fn seasons_for_id(&self, external_id: &str) -> Result<Vec<ExternalSeason>> {
        let media = self.media_by_id(raw_id(external_id)).await?;

        Ok(vec![ExternalSeason {
            external_id: format!("anilist:{}", media.id),
            title: None,
            description: None,
            posters: media
                .cover_image
                .as_ref()
                .and_then(|c| c.extra_large.clone().or_else(|| c.large.clone()))
                .into_iter()
                .collect(),
            season_number: 1,
        }])
    }

    async fn episodes_for_season(
        &self,
        external_id: &str,
        season_number: u64,
    ) -> Result<Vec<ExternalEpisode>> {
        if season_number != 1 {
            return Ok(vec![]);
        }

        let media = self.media_by_id(raw_id(external_id)).await?;

        // Prefer real episode data from streamingEpisodes; synthesize plain
        // numbered episodes otherwise so matching still works.
        let mut out: Vec<ExternalEpisode> = media
            .streaming_episodes
            .iter()
            .enumerate()
            .map(|(i, ep)| {
                let (parsed_number, parsed_title) = ep
                    .title
                    .as_deref()
                    .map(parse_streaming_title)
                    .unwrap_or((None, None));

                ExternalEpisode {
                    external_id: format!("anilist:{}:{}", media.id, i + 1),
                    title: parsed_title,
                    description: None,
                    episode_number: parsed_number.unwrap_or((i + 1) as u64),
                    stills: ep.thumbnail.clone().into_iter().collect(),
                    duration: media.duration.map(|mins| Duration::from_secs(mins * 60)),
                }
            })
            .collect();

        // Fall back to (or extend up to) the declared episode count. When the
        // count is unknown (still airing), synthesize a generous window so
        // new episodes still match.
        let declared = media.episodes.unwrap_or(100).max(out.len() as u64);
        let have: std::collections::HashSet<u64> =
            out.iter().map(|e| e.episode_number).collect();
        for number in 1..=declared {
            if !have.contains(&number) {
                out.push(ExternalEpisode {
                    external_id: format!("anilist:{}:{}", media.id, number),
                    title: None,
                    description: None,
                    episode_number: number,
                    stills: vec![],
                    duration: media.duration.map(|mins| Duration::from_secs(mins * 60)),
                });
            }
        }

        out.sort_by_key(|e| e.episode_number);
        Ok(out)
    }
}

impl IntoQueryShow for AniListProvider {
    fn as_query_show<'a>(&'a self) -> Option<&'a dyn ExternalQueryShow> {
        Some(self)
    }

    fn into_query_show(self: Arc<Self>) -> Option<Arc<dyn ExternalQueryShow>> {
        Some(self)
    }
}

impl ExternalQueryIntoShow for AniListProvider {}

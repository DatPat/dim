//! A provider that chains several metadata providers.
//!
//! Searches try each child in order until one returns usable results; id-based
//! lookups (`search_by_id`, seasons, episodes, cast) are routed to the child
//! whose scheme matches the id's namespace prefix, so mixed-provider libraries
//! keep working after a fallback match.

use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    parse_external_id, Error, ExternalActor, ExternalEpisode, ExternalMedia, ExternalQuery,
    ExternalQueryIntoShow, ExternalQueryShow, ExternalSeason, IntoQueryShow, Result,
};

pub struct ChainedProvider {
    children: Vec<Arc<dyn ExternalQueryIntoShow>>,
}

impl std::fmt::Debug for ChainedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainedProvider")
            .field(
                "children",
                &self
                    .children
                    .iter()
                    .map(|c| c.provider_scheme())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ChainedProvider {
    /// The first child is the primary provider; the rest are fallbacks in
    /// order. Must not be empty.
    pub fn new(children: Vec<Arc<dyn ExternalQueryIntoShow>>) -> Self {
        assert!(!children.is_empty(), "ChainedProvider needs >= 1 child");
        Self { children }
    }

    /// Route an id to the child that owns its namespace. Un-namespaced
    /// (legacy) ids go to the primary child.
    fn child_for_id(&self, external_id: &str) -> &Arc<dyn ExternalQueryIntoShow> {
        if let (Some(scheme), _) = parse_external_id(external_id) {
            if let Some(child) = self
                .children
                .iter()
                .find(|c| c.provider_scheme() == scheme)
            {
                return child;
            }
        }
        &self.children[0]
    }
}

#[async_trait]
impl ExternalQuery for ChainedProvider {
    fn provider_scheme(&self) -> &'static str {
        self.children[0].provider_scheme()
    }

    async fn search(
        &self,
        title: &str,
        year: Option<i32>,
        language: Option<&str>,
    ) -> Result<Vec<ExternalMedia>> {
        let mut last_err = None;

        for child in &self.children {
            match child.search(title, year, language).await {
                Ok(results) if !results.is_empty() => return Ok(results),
                Ok(_) => continue,
                Err(e) => {
                    tracing::debug!(
                        provider = child.provider_scheme(),
                        error = ?e,
                        "provider search failed, trying next in chain"
                    );
                    last_err = Some(e);
                }
            }
        }

        match last_err {
            Some(e) => Err(e),
            None => Ok(vec![]),
        }
    }

    async fn search_by_id(&self, external_id: &str) -> Result<ExternalMedia> {
        // The primary child handles foreign catalogue ids it can resolve
        // (e.g. TMDB resolves imdb:/tvdb: via /find), so only route to other
        // children for their own schemes.
        self.child_for_id(external_id).search_by_id(external_id).await
    }

    async fn cast(&self, external_id: &str) -> Result<Vec<ExternalActor>> {
        self.child_for_id(external_id).cast(external_id).await
    }

    async fn alternative_titles(&self, external_id: &str) -> Result<Vec<String>> {
        self.child_for_id(external_id)
            .alternative_titles(external_id)
            .await
    }
}

#[async_trait]
impl ExternalQueryShow for ChainedProvider {
    async fn seasons_for_id(&self, external_id: &str) -> Result<Vec<ExternalSeason>> {
        let child = self.child_for_id(external_id);
        match child.as_query_show() {
            Some(show) => show.seasons_for_id(external_id).await,
            None => Err(Error::NoResults {
                query: external_id.to_string(),
                year: None,
            }),
        }
    }

    async fn episodes_for_season(
        &self,
        external_id: &str,
        season_number: u64,
    ) -> Result<Vec<ExternalEpisode>> {
        let child = self.child_for_id(external_id);
        match child.as_query_show() {
            Some(show) => show.episodes_for_season(external_id, season_number).await,
            None => Err(Error::NoResults {
                query: external_id.to_string(),
                year: None,
            }),
        }
    }
}

impl IntoQueryShow for ChainedProvider {
    fn as_query_show<'a>(&'a self) -> Option<&'a dyn ExternalQueryShow> {
        // Only expose season/episode queries when the primary child can
        // handle them (movie chains must not pretend to be show providers).
        self.children[0].as_query_show().map(|_| self as _)
    }

    fn into_query_show(self: Arc<Self>) -> Option<Arc<dyn ExternalQueryShow>> {
        if self.children[0].as_query_show().is_some() {
            Some(self)
        } else {
            None
        }
    }
}

impl ExternalQueryIntoShow for ChainedProvider {}

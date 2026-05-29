//! ncrawler — Grafana scraper.
//!
//! Pre-split per SCOPE so no single file grows unbounded:
//!
//! - [`client`] — the only file that touches the `grafana` crate; owns
//!   the [`GrafanaClient`] seam, token resolution, and error mapping.
//! - [`api`] — `mode = Api`: dashboard + per-panel `/api/ds/query`.
//! - `visual` / `merge` — placeholders for `mode = Visual` / `Both`,
//!   landing in a later milestone (return [`ScrapeError::ModeUnsupported`]).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use starter_spi::secrets::SecretStore;
use url::Url;

use ncrawler_spi::{Artifact, Cancel, ScrapeError, ScrapeJob, Scraper};

pub mod api;
pub mod client;
pub mod merge;
pub mod visual;

pub use client::{resolve_token, GrafanaClient, GrafanaCrateClient};

/// The Grafana [`Scraper`]. Resolves the bearer token from the optional
/// [`SecretStore`] (keyed `ncrawler:grafana:<host>:token`) with a
/// `GRAFANA_TOKEN` env fallback, builds a [`GrafanaCrateClient`], and
/// dispatches on `mode`.
#[derive(Default)]
pub struct GrafanaScraper {
    store: Option<Arc<dyn SecretStore>>,
}

impl GrafanaScraper {
    /// A scraper with no secret store; tokens come from `GRAFANA_TOKEN`.
    pub fn new() -> Self {
        Self::default()
    }

    /// A scraper that resolves tokens from `store` first.
    pub fn with_store(store: Arc<dyn SecretStore>) -> Self {
        Self { store: Some(store) }
    }
}

#[async_trait]
impl Scraper for GrafanaScraper {
    fn name(&self) -> &str {
        "grafana"
    }

    async fn scrape(&self, job: ScrapeJob, cancel: &dyn Cancel) -> Result<Artifact, ScrapeError> {
        if cancel.is_cancelled() {
            return Err(ScrapeError::Cancelled);
        }
        let url = job
            .options
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ScrapeError::Other("grafana job is missing `url` option".to_owned()))?;
        let mode = job
            .options
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("api");

        let host = Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        let token = resolve_token(&host, self.store.as_deref());
        let client = GrafanaCrateClient::new(url, token.as_ref())?;
        let fetched_at = chrono::Utc::now();

        match mode {
            "api" => api::scrape(&client, &job, fetched_at).await,
            "visual" => visual::scrape(&client, &job, fetched_at).await,
            "both" => merge::scrape(&client, &job, fetched_at).await,
            other => Err(ScrapeError::ModeUnsupported(other.to_owned())),
        }
    }
}

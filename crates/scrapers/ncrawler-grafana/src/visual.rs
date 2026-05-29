//! `mode = Visual`: panel PNGs via the `grafana-image-renderer` plugin
//! (`/render/d-solo/...`).
//!
//! Placeholder — the visual path lands in a later milestone (SCOPE: M4).
//! The module exists now so the `client.rs` / `api.rs` / `visual.rs` /
//! `merge.rs` file layout is established without the Visual code yet.

use ncrawler_spi::{Artifact, ScrapeError, ScrapeJob};

use crate::client::GrafanaClient;

/// Not implemented yet; returns [`ScrapeError::ModeUnsupported`].
pub async fn scrape(
    _client: &dyn GrafanaClient,
    _job: &ScrapeJob,
    _fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<Artifact, ScrapeError> {
    Err(ScrapeError::ModeUnsupported(
        "grafana visual mode (renderer plugin) lands in a later milestone".to_owned(),
    ))
}

//! `mode = Both`: API data merged with renderer-plugin pixels, one
//! `Item` per panel with a matching `Asset`.
//!
//! Placeholder — the merge path lands in a later milestone (SCOPE: M4),
//! after `visual.rs`. Present now to establish the file layout.

use ncrawler_spi::{Artifact, ScrapeError, ScrapeJob};

use crate::client::GrafanaClient;

/// Not implemented yet; returns [`ScrapeError::ModeUnsupported`].
pub async fn scrape(
    _client: &dyn GrafanaClient,
    _job: &ScrapeJob,
    _fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<Artifact, ScrapeError> {
    Err(ScrapeError::ModeUnsupported(
        "grafana both mode (api+visual merge) lands in a later milestone".to_owned(),
    ))
}

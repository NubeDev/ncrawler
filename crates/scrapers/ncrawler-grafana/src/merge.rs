//! `mode = Both`: API data merged with renderer-plugin pixels — one
//! `Item` per panel carrying its `/api/ds/query` data, each paired with
//! a matching PNG `Asset` linked by `item_id` (SCOPE: Both mode).

use std::path::Path;

use ncrawler_spi::{Artifact, ScrapeError, ScrapeJob};

use crate::client::{GrafanaClient, RendererClient};
use crate::visual::{render_assets, VisualOpts};

/// Run the Both-mode scrape: the Api-mode artifact (panel items + data +
/// dashboard meta) with renderer-plugin PNGs attached. Assets link to
/// their panel item by `item_id`; there is exactly one asset per panel
/// the renderer produced.
pub async fn scrape(
    client: &dyn GrafanaClient,
    renderer: &RendererClient,
    job: &ScrapeJob,
    opts: &VisualOpts,
    assets_dir: &Path,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<Artifact, ScrapeError> {
    // Authoritative data first (also runs the SSRF guard on datasource
    // URLs); the renderer only adds pixels on top.
    let mut artifact = crate::api::scrape(client, job, fetched_at).await?;

    // Optionally restrict to the panels the operator asked for, matching
    // the Visual path's filter so data and pixels stay in lockstep.
    if !opts.panels.is_empty() {
        artifact.items.retain(|it| {
            it.id
                .strip_prefix("panel-")
                .and_then(|s| s.parse::<i64>().ok())
                .map(|id| opts.panels.contains(&id))
                .unwrap_or(false)
        });
    }

    let assets = render_assets(renderer, &artifact.items, &job.target, opts, assets_dir).await?;
    artifact.assets = assets;
    Ok(artifact)
}

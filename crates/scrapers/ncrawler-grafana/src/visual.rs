//! `mode = Visual`: panel PNGs via the `grafana-image-renderer` plugin
//! (`/render/d-solo/...`).
//!
//! The primary path is the renderer plugin (no local browser needed,
//! authoritative screenshots). When the plugin is absent the probe
//! returns [`ScrapeError::RendererPluginMissing`]; the operator can opt
//! into the best-effort `chromiumoxide` fallback with
//! `--visual-fallback chrome` (SCOPE: visual strategy).

use std::path::Path;

use serde_json::{json, Value};

use ncrawler_spi::{Artifact, Asset, Item, ScrapeError, ScrapeJob};

use crate::client::{GrafanaClient, RendererClient};

/// Knobs for the visual path, parsed from `ScrapeJob.options` by `lib.rs`.
#[derive(Debug, Clone)]
pub struct VisualOpts {
    pub width: u32,
    pub height: u32,
    pub from: String,
    pub to: String,
    /// Restrict to these panel ids; empty = all panels.
    pub panels: Vec<i64>,
    /// Dashboard URL used by the Chrome fallback only.
    pub dashboard_url: String,
    /// `--visual-fallback chrome` opt-in (best-effort, flaky).
    pub fallback_chrome: bool,
}

impl Default for VisualOpts {
    fn default() -> Self {
        Self {
            width: 1000,
            height: 500,
            from: "now-6h".to_owned(),
            to: "now".to_owned(),
            panels: Vec::new(),
            dashboard_url: String::new(),
            fallback_chrome: false,
        }
    }
}

/// Run the Visual-mode scrape: one minimal `Item::Panel` per panel plus
/// a matching PNG [`Asset`] (linked by `item_id`). PNGs are written into
/// `assets_dir`; `Asset.path` is stored relative to the artifact dir.
pub async fn scrape(
    client: &dyn GrafanaClient,
    renderer: &RendererClient,
    job: &ScrapeJob,
    opts: &VisualOpts,
    assets_dir: &Path,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<Artifact, ScrapeError> {
    let dash = client.dashboard_by_uid(&job.target).await?;
    let panels = selected_panels(&dash, &opts.panels);

    let items: Vec<Item> = panels
        .iter()
        .filter_map(|p| {
            let id = p.get("id").and_then(Value::as_i64)?;
            Some(crate::api::panel_item(id, p, None))
        })
        .collect();

    let assets = render_assets(renderer, &items, &job.target, opts, assets_dir).await?;

    let mut artifact = Artifact::new("grafana", job.target.clone(), fetched_at);
    artifact.items = items;
    artifact.assets = assets;
    artifact.meta = json!({ "dashboard": dash });
    Ok(artifact)
}

/// Render a PNG for every `Item::Panel` in `items`, write it to
/// `assets_dir/panel-{id}.png`, and return the [`Asset`]s with
/// `item_id` linked to the panel item. Shared by `visual` and `merge`.
///
/// Probes the renderer plugin once up front. When it is missing and the
/// Chrome fallback is NOT enabled, returns
/// [`ScrapeError::RendererPluginMissing`]; with the fallback enabled it
/// drives our `chromiumoxide` layer at the dashboard URL instead
/// (best-effort, one whole-dashboard PNG, `item_id = None`).
pub(crate) async fn render_assets(
    renderer: &RendererClient,
    items: &[Item],
    uid: &str,
    opts: &VisualOpts,
    assets_dir: &Path,
) -> Result<Vec<Asset>, ScrapeError> {
    match renderer.probe().await {
        Ok(()) => {}
        Err(ScrapeError::RendererPluginMissing) if opts.fallback_chrome => {
            return crate::chrome::fallback_screenshot(opts, assets_dir).await;
        }
        Err(e) => return Err(e),
    }

    std::fs::create_dir_all(assets_dir).map_err(|e| ScrapeError::Other(e.to_string()))?;
    let mut assets = Vec::with_capacity(items.len());
    for item in items {
        let Some(panel_id) = item
            .id
            .strip_prefix("panel-")
            .and_then(|s| s.parse::<i64>().ok())
        else {
            continue;
        };
        let png = renderer
            .render_panel(uid, panel_id, opts.width, opts.height, &opts.from, &opts.to)
            .await?;
        let file_name = format!("{}.png", item.id);
        std::fs::write(assets_dir.join(&file_name), &png)
            .map_err(|e| ScrapeError::Other(e.to_string()))?;
        assets.push(Asset {
            path: Path::new("assets").join(&file_name),
            mime: "image/png".to_owned(),
            label: item.title.clone().unwrap_or_else(|| item.id.clone()),
            item_id: Some(item.id.clone()),
        });
    }
    Ok(assets)
}

/// Panels from the dashboard JSON, filtered to `wanted` ids when the
/// caller restricted them (empty = all).
fn selected_panels(dash: &Value, wanted: &[i64]) -> Vec<Value> {
    crate::api::panel_list(dash)
        .into_iter()
        .filter(|p| {
            let Some(id) = p.get("id").and_then(Value::as_i64) else {
                return false;
            };
            wanted.is_empty() || wanted.contains(&id)
        })
        .collect()
}

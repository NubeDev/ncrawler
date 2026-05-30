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
pub mod audit;
pub mod chrome;
pub mod client;
pub mod data;
pub mod instance;
pub mod interp;
pub mod merge;
pub mod multi;
pub mod resolve;
pub mod selector;
pub mod stage;
pub mod status;
pub mod visual;

pub use audit::Audit;
pub use client::{resolve_token, GrafanaClient, GrafanaCrateClient, RendererClient};
pub use data::Selection;
pub use multi::{
    scrape_selection, DashboardError, MultiConfig, MultiSummary, SidecarOutcome,
    DEFAULT_CONCURRENCY, DEFAULT_SIDECAR_MAX_AGE_SECS,
};
pub use selector::{
    parse_inventory, DashboardEntry, DashboardSelector, Resolution, SelectorError, MAX_LIMIT,
};
pub use stage::Stage;
pub use status::{DataStatus, PanelStatus};
pub use visual::VisualOpts;

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

    /// Multi-dashboard API-mode scrape (REPORT §8 step 3).
    ///
    /// Resolves `selector` against the live `/api/search` inventory,
    /// writes the `_instance/<host>` sidecar once (refreshing a stale
    /// one), then fans out one per-dashboard artifact per resolved uid
    /// under the configured concurrency cap, persisting each into the
    /// store rooted at `job.options["out"]` (default `./artifacts`).
    /// Per-dashboard failures are collected, never fatal to siblings.
    ///
    /// Unlike [`Scraper::scrape`] this writes artifacts itself (it emits
    /// many), so the CLI does not re-`write` the result.
    pub async fn scrape_multi(
        &self,
        job: &ScrapeJob,
        selector: &selector::DashboardSelector,
        config: &multi::MultiConfig,
        cancel: &dyn Cancel,
    ) -> Result<multi::MultiSummary, ScrapeError> {
        if cancel.is_cancelled() {
            return Err(ScrapeError::Cancelled);
        }
        let url = job
            .options
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ScrapeError::Other("grafana job is missing `url` option".to_owned()))?;
        let host = Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "unknown-host".to_owned());
        let token = resolve_token(&host, self.store.as_deref());
        let client = GrafanaCrateClient::new(url, token.as_ref())?;

        let out = job
            .options
            .get("out")
            .and_then(Value::as_str)
            .unwrap_or("./artifacts");
        let store = ncrawler_core::ArtifactStore::new(out);
        let fetched_at = chrono::Utc::now();

        multi::scrape_selection(
            &client,
            &store,
            &host,
            selector,
            &job.options,
            &job.allow_hosts,
            fetched_at,
            config,
            cancel,
        )
        .await
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

        let host = Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        let token = resolve_token(&host, self.store.as_deref());
        let client = GrafanaCrateClient::new(url, token.as_ref())?;
        let fetched_at = chrono::Utc::now();

        // Staged pipeline (SCOPE: REPORTS-UPDATE). `audit` and `all` mint
        // a fresh artifact directory through the store, so they run here;
        // `data` / `report` operate on an existing dir and are driven
        // from the CLI directly.
        if let Some(stage) = job.options.get("stage").and_then(Value::as_str) {
            let stage = Stage::parse(stage)
                .ok_or_else(|| ScrapeError::Other(format!("unknown --stage `{stage}`")))?;
            let assets_dir = assets_dir_for(&job, fetched_at)?;
            let dir = assets_dir
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| assets_dir.clone());
            let mut artifact = match stage {
                Stage::Audit => stage::audit_artifact(&client, &job, url, &dir, fetched_at).await?,
                Stage::All => stage::all_artifact(&client, &job, url, &dir, fetched_at).await?,
                Stage::Data | Stage::Report => {
                    return Err(ScrapeError::Other(format!(
                        "--stage {stage:?} operates on an existing artifact dir; run it via the CLI",
                    )))
                }
            };
            // Optional early screenshot (`--with-shot`): one whole-dashboard
            // chrome capture, reusing the best-effort visual fallback.
            if job.options.get("with_shot").and_then(Value::as_bool) == Some(true) {
                let mut opts = visual_opts(&job, url);
                opts.base_url = url.to_owned();
                opts.token = token.clone();
                opts.fallback_chrome = true;
                opts.whole_dashboard = true;
                // Build the kiosk dashboard URL `<base>/d/<uid>?kiosk=tv` so
                // the capture lands on the dashboard itself, not the home
                // page (`visual_opts` only does this when `visual_whole` is
                // set, which `--with-shot` does not go through).
                let base = url.trim_end_matches('/');
                opts.dashboard_url = format!(
                    "{base}/d/{uid}/dashboard?kiosk=tv&from={from}&to={to}",
                    uid = job.target,
                    from = opts.from,
                    to = opts.to,
                );
                match chrome::fallback_screenshot(&opts, &assets_dir).await {
                    Ok(assets) => artifact.assets = assets,
                    Err(e) => tracing::warn!(error = %e, "--with-shot screenshot failed"),
                }
            }
            return Ok(artifact);
        }

        let mode = job
            .options
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("api");

        let artifact = match mode {
            "api" => api::scrape(&client, &job, fetched_at).await,
            "visual" | "both" => {
                let mut opts = visual_opts(&job, url);
                opts.base_url = url.to_owned();
                opts.token = token.clone();
                if opts.fallback_chrome {
                    tracing::warn!(
                        "--visual-fallback chrome enabled: the chromiumoxide path is \
                         best-effort and flaky (template vars, lazy panels, no \
                         all-queries-done signal); prefer the renderer plugin"
                    );
                }
                let renderer = RendererClient::new(url, token)?;
                let assets_dir = assets_dir_for(&job, fetched_at)?;
                Ok(if mode == "visual" {
                    visual::scrape(&client, &renderer, &job, &opts, &assets_dir, fetched_at).await?
                } else {
                    merge::scrape(&client, &renderer, &job, &opts, &assets_dir, fetched_at).await?
                })
            }
            other => return Err(ScrapeError::ModeUnsupported(other.to_owned())),
        }?;

        // Instance sidecar: written ONCE per scrape run (all API-backed
        // modes), so per-dashboard artifacts stop duplicating the
        // inventory (REPORT §6a). Reuses the on-disk store machinery.
        write_instance_sidecar(&client, &job, &host, fetched_at).await?;

        Ok(artifact)
    }
}

/// Fetch the instance-wide facts and persist them to the `_instance/<host>`
/// sidecar under the artifact root (`job.options["out"]`, default
/// `./artifacts`) via [`ncrawler_core::ArtifactStore`]. SSRF-gates the
/// surfaced URLs before writing.
async fn write_instance_sidecar(
    client: &dyn GrafanaClient,
    job: &ScrapeJob,
    host: &str,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), ScrapeError> {
    // A URL with no parseable host should never reach a real scrape, but
    // keep the sidecar layout well-formed if it does.
    let sidecar_host = if host.is_empty() {
        "unknown-host"
    } else {
        host
    };
    let sidecar = instance::fetch(client, sidecar_host, fetched_at).await;
    instance::enforce_ssrf(&job.allow_hosts, &sidecar)?;

    let out = job
        .options
        .get("out")
        .and_then(Value::as_str)
        .unwrap_or("./artifacts");
    ncrawler_core::ArtifactStore::new(out)
        .write_instance("grafana", &sidecar)
        .map_err(|e| ScrapeError::Other(format!("writing instance sidecar: {e}")))?;
    Ok(())
}

/// Parse the visual knobs out of `job.options`, defaulting per SCOPE.
fn visual_opts(job: &ScrapeJob, dashboard_url: &str) -> VisualOpts {
    let o = &job.options;
    // For whole-dashboard chrome capture we need `<base>/d/<uid>?kiosk=tv`.
    let whole = o.get("visual_whole").and_then(Value::as_bool).unwrap_or(false);
    let base = dashboard_url.trim_end_matches('/');
    let dash_url = if whole {
        let from = o
            .get("from")
            .and_then(Value::as_str)
            .unwrap_or("now-6h");
        let to = o.get("to").and_then(Value::as_str).unwrap_or("now");
        format!(
            "{base}/d/{uid}/dashboard?kiosk=tv&from={from}&to={to}",
            uid = job.target
        )
    } else {
        dashboard_url.to_owned()
    };
    let mut v = VisualOpts {
        dashboard_url: dash_url,
        fallback_chrome: o.get("visual_fallback").and_then(Value::as_str) == Some("chrome"),
        whole_dashboard: whole,
        ..VisualOpts::default()
    };
    if let Some(w) = o.get("width").and_then(Value::as_u64) {
        v.width = w as u32;
    }
    if let Some(h) = o.get("height").and_then(Value::as_u64) {
        v.height = h as u32;
    }
    if let Some(f) = o.get("from").and_then(Value::as_str) {
        v.from = f.to_owned();
    }
    if let Some(t) = o.get("to").and_then(Value::as_str) {
        v.to = t.to_owned();
    }
    if let Some(arr) = o.get("panels").and_then(Value::as_array) {
        v.panels = arr.iter().filter_map(Value::as_i64).collect();
    }
    v
}

/// Compute the artifact's `assets/` directory under the output root
/// (`job.options["out"]`, default `./artifacts`) using the SAME dirname
/// the [`ncrawler_core`] store derives, so a later `store.write` of the
/// returned artifact lands the PNGs in the right place. Creates it 0700
/// on unix via [`ncrawler_core`] semantics.
fn assets_dir_for(
    job: &ScrapeJob,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<std::path::PathBuf, ScrapeError> {
    let out = job
        .options
        .get("out")
        .and_then(Value::as_str)
        .unwrap_or("./artifacts");
    let dirname = ncrawler_core::dir_name(fetched_at, "grafana", &job.target);
    let assets = std::path::Path::new(out).join(dirname).join("assets");
    std::fs::create_dir_all(&assets).map_err(|e| ScrapeError::Other(e.to_string()))?;
    Ok(assets)
}

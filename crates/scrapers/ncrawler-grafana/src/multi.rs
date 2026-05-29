//! Multi-dashboard `mode = Api` scrape loop (REPORT §8 step 3).
//!
//! A single `ncrawler scrape grafana` invocation resolves the shared
//! [`DashboardSelector`] against the live `/api/search` inventory, writes
//! the `_instance/<host>` sidecar **once** at the start (or refreshes it
//! when the on-disk sidecar is older than the run's wall-clock cap), and
//! then fans out one per-dashboard [`Artifact`] per resolved uid through
//! [`crate::api::scrape`] — reusing the existing per-`<uid>/latest` store
//! layout.
//!
//! ## Bounded concurrency (token bucket)
//!
//! `--all` on the live `rd-esr` instance is 1000+ dashboards; firing them
//! at the upstream unbounded would DoS it. Every per-dashboard scrape must
//! first take a permit from a [`tokio::sync::Semaphore`] sized to the
//! configured concurrency (default [`DEFAULT_CONCURRENCY`]), so at most
//! that many dashboards are ever in flight. The permit is held across the
//! whole per-dashboard scrape (`dashboard` + per-panel `ds/query` +
//! `annotations`), not just the first request.
//!
//! ## Best-effort siblings
//!
//! A per-dashboard failure (401 on one uid, a 404, malformed JSON, an
//! SSRF reject) is **collected** and surfaced in the final
//! [`MultiSummary`]; it never aborts the sibling dashboards. A `--all`
//! run where one dashboard 401s still persists every other dashboard.
//!
//! The token-quarantine rules from SCOPE still hold: the bearer token is
//! resolved by the caller ([`crate::GrafanaScraper`]) and lives only
//! inside the [`GrafanaClient`]; nothing here logs it.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use futures::stream::{self, StreamExt};
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use ncrawler_core::{ArtifactStore, InstanceSidecar};
use ncrawler_spi::{Cancel, ScrapeError, ScrapeJob};

use crate::client::GrafanaClient;
use crate::instance;
use crate::selector::{parse_inventory, DashboardSelector, Resolution};

/// Default number of dashboards scraped concurrently. Conservative so a
/// `--all` run cannot hammer the upstream Grafana (REPORT §8 step 3).
pub const DEFAULT_CONCURRENCY: usize = 4;

/// Default freshness window for the `_instance` sidecar: if the on-disk
/// sidecar was written within this window it is reused, otherwise it is
/// refreshed at the start of the run.
pub const DEFAULT_SIDECAR_MAX_AGE_SECS: i64 = 3600;

/// Tunables for a multi-dashboard scrape.
#[derive(Debug, Clone)]
pub struct MultiConfig {
    /// Max dashboards in flight at once (the token-bucket size). Coerced
    /// to at least 1.
    pub concurrency: usize,
    /// Reuse an existing `_instance` sidecar when it is younger than this;
    /// refresh it otherwise.
    pub sidecar_max_age: Duration,
}

impl Default for MultiConfig {
    fn default() -> Self {
        Self {
            concurrency: DEFAULT_CONCURRENCY,
            sidecar_max_age: Duration::seconds(DEFAULT_SIDECAR_MAX_AGE_SECS),
        }
    }
}

/// What happened to the `_instance` sidecar this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarOutcome {
    /// No sidecar existed; a fresh one was written.
    Written,
    /// An existing sidecar was older than the cap and was refetched.
    Refreshed,
    /// An existing sidecar was fresh enough; left untouched (not refetched).
    SkippedFresh,
}

/// One per-dashboard failure, collected without aborting siblings.
#[derive(Debug, Clone)]
pub struct DashboardError {
    pub uid: String,
    pub error: String,
}

/// The outcome of a multi-dashboard scrape run.
#[derive(Debug, Clone)]
pub struct MultiSummary {
    /// What happened to the `_instance` sidecar.
    pub sidecar: SidecarOutcome,
    /// Resolved selection counts (selected vs live inventory total).
    pub resolution: Resolution,
    /// uids that scraped + persisted successfully, sorted.
    pub succeeded: Vec<String>,
    /// Per-dashboard failures, sorted by uid (siblings still succeeded).
    pub failed: Vec<DashboardError>,
}

impl MultiSummary {
    /// A short human-readable summary line for the CLI.
    pub fn summary_line(&self) -> String {
        format!(
            "{} dashboards: {} ok, {} failed (selected {} of {} in inventory)",
            self.succeeded.len() + self.failed.len(),
            self.succeeded.len(),
            self.failed.len(),
            self.resolution.selected_count(),
            self.resolution.inventory_total,
        )
    }
}

/// Resolve the selector against the live inventory, ensure the sidecar,
/// then fan out one per-dashboard scrape per resolved uid under a bounded
/// concurrency cap. See the module docs for the invariants.
#[allow(clippy::too_many_arguments)]
pub async fn scrape_selection(
    client: &dyn GrafanaClient,
    store: &ArtifactStore,
    host: &str,
    selector: &DashboardSelector,
    job_options: &Value,
    allow_hosts: &[String],
    fetched_at: DateTime<Utc>,
    config: &MultiConfig,
    cancel: &dyn Cancel,
) -> Result<MultiSummary, ScrapeError> {
    if cancel.is_cancelled() {
        return Err(ScrapeError::Cancelled);
    }

    // 1. Live inventory → resolved selection (scrape-time `--all` is the
    //    true live `/api/search` inventory, REPORT §2).
    let search = client.search().await?;
    let inventory = parse_inventory(&search);
    let resolution = selector.resolve_live(&inventory);
    let uids = resolution.uids();
    tracing::info!(
        selected = uids.len(),
        inventory = resolution.inventory_total,
        "resolved dashboard selection"
    );

    // 2. Sidecar: written once at the start, or refreshed when stale.
    let sidecar = ensure_sidecar(client, store, host, allow_hosts, fetched_at, config).await?;

    // 3. Fan out per-dashboard scrapes under the token bucket. A permit is
    //    held for the whole per-dashboard scrape so at most `concurrency`
    //    dashboards hit the upstream at once.
    let permits = config.concurrency.max(1);
    let sem = Arc::new(Semaphore::new(permits));

    let results: Vec<(String, Result<(), ScrapeError>)> = stream::iter(uids.iter().cloned())
        .map(|uid| {
            let sem = Arc::clone(&sem);
            async move {
                // Acquire BEFORE doing any network work.
                let _permit = sem
                    .acquire()
                    .await
                    .expect("semaphore is never closed during a run");
                if cancel.is_cancelled() {
                    return (uid, Err(ScrapeError::Cancelled));
                }
                let res =
                    scrape_one(client, store, &uid, job_options, allow_hosts, fetched_at).await;
                (uid, res)
            }
        })
        // Poll all resolved uids; the semaphore — not this bound — is the
        // real limiter, so the cap is exercised even on a huge `--all`.
        .buffer_unordered(uids.len().max(1))
        .collect()
        .await;

    let mut succeeded = Vec::new();
    let mut failed = Vec::new();
    for (uid, res) in results {
        match res {
            Ok(()) => succeeded.push(uid),
            Err(e) => {
                tracing::warn!(uid = %uid, error = %e, "dashboard scrape failed; siblings continue");
                failed.push(DashboardError {
                    uid,
                    error: e.to_string(),
                });
            }
        }
    }
    // Deterministic summary ordering (REPORT §6b stable ordering).
    succeeded.sort();
    failed.sort_by(|a, b| a.uid.cmp(&b.uid));

    Ok(MultiSummary {
        sidecar,
        resolution,
        succeeded,
        failed,
    })
}

/// Scrape one dashboard and persist it to the store under
/// `grafana/<uid>/latest`. Pure per-dashboard work — the sidecar is
/// handled once by the caller.
async fn scrape_one(
    client: &dyn GrafanaClient,
    store: &ArtifactStore,
    uid: &str,
    job_options: &Value,
    allow_hosts: &[String],
    fetched_at: DateTime<Utc>,
) -> Result<(), ScrapeError> {
    let job = ScrapeJob {
        source: "grafana".to_owned(),
        target: uid.to_owned(),
        allow_hosts: allow_hosts.to_vec(),
        // Reuse the run's window/options; force api mode (the fan-out is
        // API-mode per REPORT §8 step 3).
        options: with_api_mode(job_options),
    };
    let artifact = crate::api::scrape(client, &job, fetched_at).await?;
    store
        .write(&artifact)
        .map_err(|e| ScrapeError::Other(format!("writing artifact for {uid}: {e}")))?;
    Ok(())
}

/// Clone the run options but pin `mode = "api"` (the per-dashboard fan-out
/// is API-mode only; visual/both stay single-dashboard).
fn with_api_mode(job_options: &Value) -> Value {
    let mut map = job_options.as_object().cloned().unwrap_or_default();
    map.insert("mode".to_owned(), json!("api"));
    Value::Object(map)
}

/// Write the `_instance/<host>` sidecar once, unless a fresh one already
/// exists on disk (younger than `config.sidecar_max_age`), in which case
/// it is left untouched and NOT refetched.
async fn ensure_sidecar(
    client: &dyn GrafanaClient,
    store: &ArtifactStore,
    host: &str,
    allow_hosts: &[String],
    fetched_at: DateTime<Utc>,
    config: &MultiConfig,
) -> Result<SidecarOutcome, ScrapeError> {
    let existing = store
        .read_instance_sidecar("grafana", host)
        .map_err(|e| ScrapeError::Other(format!("reading instance sidecar: {e}")))?;

    let refresh = match &existing {
        None => SidecarOutcome::Written,
        Some(s) => {
            let age = fetched_at - s.fetched_at;
            if age < config.sidecar_max_age {
                tracing::info!(
                    host,
                    age_secs = age.num_seconds(),
                    "instance sidecar is fresh; reusing (not refetched)"
                );
                return Ok(SidecarOutcome::SkippedFresh);
            }
            tracing::info!(
                host,
                age_secs = age.num_seconds(),
                "instance sidecar is stale; refreshing"
            );
            SidecarOutcome::Refreshed
        }
    };

    let sidecar: InstanceSidecar = instance::fetch(client, host, fetched_at).await;
    instance::enforce_ssrf(allow_hosts, &sidecar)?;
    store
        .write_instance("grafana", &sidecar)
        .map_err(|e| ScrapeError::Other(format!("writing instance sidecar: {e}")))?;
    Ok(refresh)
}

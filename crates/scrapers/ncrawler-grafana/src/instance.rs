//! Per-instance sidecar fetch (REPORT §6a).
//!
//! Instance-wide facts (`/api/search`, `/api/datasources`, `/api/folders`,
//! `/api/health` + `/api/frontend/settings`) are persisted ONCE per scrape
//! run into an `_instance/<host>` sidecar via [`ncrawler_core`], instead of
//! being copied into every per-dashboard artifact's `meta.search`.
//!
//! Sees Grafana only through [`GrafanaClient`]; the `grafana` crate lives
//! entirely in `client.rs`.

use serde_json::{json, Value};

use ncrawler_core::InstanceSidecar;
use ncrawler_spi::ScrapeError;

use crate::client::GrafanaClient;

/// Fetch the instance-wide facts and assemble the sidecar.
///
/// Each endpoint is best-effort: a permissions/old-version failure on any
/// one logs a warning and leaves that field JSON `null` rather than
/// aborting the scrape (SCOPE: best-effort meta), so a sidecar is still
/// written. SSRF enforcement over the surfaced URLs is the caller's job
/// (via [`enforce_ssrf`]), keeping every endpoint gated at scrape time.
pub async fn fetch(
    client: &dyn GrafanaClient,
    host: &str,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> InstanceSidecar {
    let mut sidecar = InstanceSidecar::new(host, fetched_at);
    sidecar.search = best_effort("search", client.search().await);
    sidecar.datasources = best_effort("datasources", client.datasources().await);
    sidecar.folders = best_effort("folders", client.folders().await);
    let health = best_effort("health", client.health().await);
    let settings = best_effort("frontend/settings", client.frontend_settings().await);
    sidecar.instance = compose_instance(&health, &settings);
    sidecar
}

/// Reject the scrape if any URL surfaced by the sidecar's payloads has a
/// host outside the allow-list (SCOPE: SSRF guard at scrape phase). An
/// empty allow-list means "operator did not opt in" → allow all.
pub fn enforce_ssrf(allow_hosts: &[String], sidecar: &InstanceSidecar) -> Result<(), ScrapeError> {
    crate::api::enforce_allow_hosts(
        allow_hosts,
        &[
            &sidecar.search,
            &sidecar.datasources,
            &sidecar.folders,
            &sidecar.instance,
        ],
    )
}

/// Compose the sidecar's `instance` facts (version, edition,
/// rendererAvailable) from `/api/health` + `/api/frontend/settings`. The
/// shape is fixed (three keys, JSON `null` where unavailable) so the
/// sidecar is stable across runs (REPORT §6b stable ordering).
fn compose_instance(health: &Value, settings: &Value) -> Value {
    let version = settings
        .pointer("/buildInfo/version")
        .and_then(Value::as_str)
        .or_else(|| health.get("version").and_then(Value::as_str));
    let edition = settings
        .pointer("/buildInfo/edition")
        .and_then(Value::as_str);
    let renderer = settings.get("rendererAvailable").and_then(Value::as_bool);
    json!({
        "version": version,
        "edition": edition,
        "rendererAvailable": renderer,
    })
}

/// Unwrap a best-effort endpoint result, logging + nulling on failure.
fn best_effort(endpoint: &str, res: Result<Value, ScrapeError>) -> Value {
    match res {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                endpoint,
                error = %e,
                "instance sidecar endpoint failed; leaving field null"
            );
            Value::Null
        }
    }
}

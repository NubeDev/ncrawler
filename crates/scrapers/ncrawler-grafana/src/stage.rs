//! Stage runner: the `--stage {audit|data|report|all}` dispatch glue
//! (SCOPE: REPORTS-UPDATE stage model).
//!
//! `audit` and `all` run inside the [`crate::GrafanaScraper`] (they mint
//! a fresh artifact directory through the store), so their orchestration
//! lives here. `data` and `report` operate on an *existing* artifact
//! directory and are driven from the CLI directly against
//! [`crate::audit`] / [`crate::data`] / the report builder.

use std::path::Path;

use serde_json::{json, Value};

use ncrawler_spi::{Artifact, ScrapeError, ScrapeJob};

use crate::audit::{self, Audit};
use crate::client::GrafanaClient;
use crate::data::{self, Selection};

/// The four stages. `All` preserves the one-shot UX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Audit,
    Data,
    Report,
    All,
}

impl Stage {
    /// Parse the `--stage` value; `None` for an unknown token.
    pub fn parse(s: &str) -> Option<Stage> {
        match s {
            "audit" => Some(Stage::Audit),
            "data" => Some(Stage::Data),
            "report" => Some(Stage::Report),
            "all" => Some(Stage::All),
            _ => None,
        }
    }
}

/// Resolve the query window from the job options (Grafana defaults).
fn window(job: &ScrapeJob) -> (String, String) {
    let from = job
        .options
        .get("from")
        .and_then(Value::as_str)
        .unwrap_or("now-6h")
        .to_owned();
    let to = job
        .options
        .get("to")
        .and_then(Value::as_str)
        .unwrap_or("now")
        .to_owned();
    (from, to)
}

/// Per-panel timeout from the job options (`0` = unbounded, default 30s).
fn query_timeout(job: &ScrapeJob) -> Option<std::time::Duration> {
    let secs = job
        .options
        .get("query_timeout_secs")
        .and_then(Value::as_u64)
        .unwrap_or(30);
    (secs > 0).then(|| std::time::Duration::from_secs(secs))
}

/// Fetch the dashboard + datasource listing and build the Stage-1 plan.
/// Best-effort on datasources (an empty listing just omits ids).
async fn fetch_and_plan(
    client: &dyn GrafanaClient,
    job: &ScrapeJob,
    base_url: &str,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<(Value, Audit), ScrapeError> {
    let dash = client.dashboard_by_uid(&job.target).await?;
    let datasources = match client.datasources().await {
        Ok(ds) => ds,
        Err(e) => {
            tracing::warn!(error = %e, "GET /api/datasources failed; plan omits datasourceId");
            Value::Array(Vec::new())
        }
    };
    let (from, to) = window(job);
    let plan = audit::plan(
        &dash,
        &datasources,
        &job.target,
        base_url,
        &from,
        &to,
        fetched_at,
    );
    Ok((dash, plan))
}

/// STAGE 1 (`--stage audit`): write `audit.json` into `dir` and return a
/// metadata-only artifact (panel items carry no data). Runs ZERO
/// `/api/ds/query` calls.
pub async fn audit_artifact(
    client: &dyn GrafanaClient,
    job: &ScrapeJob,
    base_url: &str,
    dir: &Path,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<Artifact, ScrapeError> {
    let (dash, plan) = fetch_and_plan(client, job, base_url, fetched_at).await?;
    audit::write(dir, &plan)?;

    let search = client.search().await.unwrap_or(Value::Null);
    let annotations = client.annotations().await.unwrap_or(Value::Null);

    let items = crate::api::panel_list(&dash)
        .iter()
        .filter_map(|p| {
            let id = p.get("id").and_then(Value::as_i64)?;
            Some(crate::api::panel_item(id, p, None))
        })
        .collect();

    let mut artifact = Artifact::new("grafana", job.target.clone(), fetched_at);
    artifact.items = items;
    artifact.meta = json!({
        "dashboard": dash,
        "search": search,
        "annotations": annotations,
    });
    Ok(artifact)
}

/// STAGE `all`: audit + data in one shot. Writes `audit.json`,
/// `data/panel-<id>.json`, and `data-status.json` into `dir`, and
/// returns an artifact whose panel items carry the freshly-queried data
/// (so the legacy report path keeps working too).
pub async fn all_artifact(
    client: &dyn GrafanaClient,
    job: &ScrapeJob,
    base_url: &str,
    dir: &Path,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<Artifact, ScrapeError> {
    let (dash, plan) = fetch_and_plan(client, job, base_url, fetched_at).await?;
    audit::write(dir, &plan)?;

    // Run every queryable panel (no selection narrowing on a full run).
    let _status = data::execute(client, &plan, dir, &Selection::default(), query_timeout(job)).await?;

    // Build items from the plan, loading each panel's data file when one
    // was written, so the artifact mirrors what `mode = api` would emit.
    let mut ds_responses = Vec::new();
    let items = crate::api::panel_list(&dash)
        .iter()
        .filter_map(|p| {
            let id = p.get("id").and_then(Value::as_i64)?;
            let data = read_panel_data(dir, id);
            if let Some(d) = &data {
                ds_responses.push(d.clone());
            }
            Some(crate::api::panel_item(id, p, data))
        })
        .collect();

    // Same SSRF guard the api path runs, over the dashboard + responses.
    crate::api::enforce_ssrf(&job.allow_hosts, &dash, &ds_responses)?;

    let search = client.search().await.unwrap_or(Value::Null);
    let annotations = client.annotations().await.unwrap_or(Value::Null);

    let mut artifact = Artifact::new("grafana", job.target.clone(), fetched_at);
    artifact.items = items;
    artifact.meta = json!({
        "dashboard": dash,
        "search": search,
        "annotations": annotations,
    });
    Ok(artifact)
}

/// Load a panel's Stage-2 result file back into a JSON value, if present.
fn read_panel_data(dir: &Path, panel_id: i64) -> Option<Value> {
    let path = data::panel_file(dir, panel_id);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

//! `mode = Api`: fetch the dashboard, query every panel, and emit one
//! [`Item::Panel`] per panel with deterministic id `panel-{panelId}`.
//!
//! Sees Grafana only through [`GrafanaClient`]; the `grafana` crate
//! lives entirely in `client.rs`.

use serde_json::{json, Value};
use url::Url;

use ncrawler_spi::{Artifact, Item, ItemKind, ScrapeError, ScrapeJob};

use crate::client::GrafanaClient;

/// Run the Api-mode scrape against `client` for the `uid` in `job.target`.
pub async fn scrape(
    client: &dyn GrafanaClient,
    job: &ScrapeJob,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<Artifact, ScrapeError> {
    let dash = client.dashboard_by_uid(&job.target).await?;
    let panels = panel_list(&dash);

    let mut items = Vec::with_capacity(panels.len());
    let mut ds_responses = Vec::new();
    for panel in &panels {
        let Some(panel_id) = panel.get("id").and_then(Value::as_i64) else {
            // Rows and other id-less layout elements are not panels.
            continue;
        };
        let body = query_body(panel);
        let data = client.ds_query(&body).await?;
        ds_responses.push(data.clone());
        items.push(panel_item(panel_id, panel, data));
    }

    // SSRF guard: every absolute URL surfaced by the dashboard JSON and
    // the `/api/ds/query` responses is a potential datasource egress
    // target. Validate each host against the operator's allow-list
    // BEFORE returning the artifact (SCOPE: security).
    enforce_ssrf(&job.allow_hosts, &dash, &ds_responses)?;

    let search = client.search().await?;
    let annotations = client.annotations().await?;

    let mut artifact = Artifact::new("grafana", job.target.clone(), fetched_at);
    artifact.items = items;
    artifact.meta = json!({
        "dashboard": dash,
        "search": search,
        "annotations": annotations,
    });
    Ok(artifact)
}

/// Extract the panel array from a `{ "dashboard": { "panels": [...] } }`
/// response, tolerating a missing/!array field (best-effort meta).
fn panel_list(dash: &Value) -> Vec<Value> {
    dash.get("dashboard")
        .and_then(|d| d.get("panels"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Build the `/api/ds/query` body for one panel: its targets plus a
/// default time window. Kept minimal; datasource-specific shaping is
/// the `client.raw()` fallback's job.
fn query_body(panel: &Value) -> Value {
    json!({
        "queries": panel.get("targets").cloned().unwrap_or(Value::Array(vec![])),
        "from": "now-6h",
        "to": "now",
    })
}

/// One `Item::Panel` with stable id `panel-{panelId}`.
fn panel_item(panel_id: i64, panel: &Value, data: Value) -> Item {
    let title = panel
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let tags = panel
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Item {
        id: format!("panel-{panel_id}"),
        kind: ItemKind::Panel,
        title: title.clone(),
        text: title.unwrap_or_else(|| format!("panel {panel_id}")),
        data: Some(data),
        tags,
    }
}

/// Reject the scrape if any surfaced URL's host is outside the
/// allow-list. An empty list means "operator did not opt in" → allow
/// all (SCOPE: default no allow-list).
fn enforce_ssrf(
    allow_hosts: &[String],
    dash: &Value,
    ds_responses: &[Value],
) -> Result<(), ScrapeError> {
    if allow_hosts.is_empty() {
        return Ok(());
    }
    let mut hosts = Vec::new();
    collect_hosts(dash, &mut hosts);
    for resp in ds_responses {
        collect_hosts(resp, &mut hosts);
    }
    for host in hosts {
        if !host_allowed(&host, allow_hosts) {
            return Err(ScrapeError::SsrfBlocked(host));
        }
    }
    Ok(())
}

/// Walk a JSON value and collect the host of every absolute http(s) URL.
fn collect_hosts(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            if let Some(host) = absolute_url_host(s) {
                out.push(host);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_hosts(v, out)),
        Value::Object(map) => map.values().for_each(|v| collect_hosts(v, out)),
        _ => {}
    }
}

/// `Some(host)` if `s` parses as an absolute http/https URL.
fn absolute_url_host(s: &str) -> Option<String> {
    if !(s.starts_with("http://") || s.starts_with("https://")) {
        return None;
    }
    Url::parse(s)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
}

/// Exact match, or `*.suffix` wildcard match (e.g. `*.example.com`).
fn host_allowed(host: &str, allow_hosts: &[String]) -> bool {
    allow_hosts.iter().any(|pat| match pat.strip_prefix("*.") {
        Some(suffix) => host == suffix || host.ends_with(&format!(".{suffix}")),
        None => host == pat,
    })
}

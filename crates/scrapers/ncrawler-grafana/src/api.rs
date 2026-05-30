//! `mode = Api`: fetch the dashboard, query every panel, and emit one
//! [`Item::Panel`] per panel with deterministic id `panel-{panelId}`.
//!
//! Sees Grafana only through [`GrafanaClient`]; the `grafana` crate
//! lives entirely in `client.rs`.

use serde_json::{json, Value};
use url::Url;

use ncrawler_spi::{Artifact, Item, ItemKind, ScrapeError, ScrapeJob};

use crate::client::GrafanaClient;
use crate::interp::{parse_time, Interpolator};
use crate::resolve::DatasourceResolver;

/// Run the Api-mode scrape against `client` for the `uid` in `job.target`.
pub async fn scrape(
    client: &dyn GrafanaClient,
    job: &ScrapeJob,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<Artifact, ScrapeError> {
    let dash = client.dashboard_by_uid(&job.target).await?;
    let panels = panel_list(&dash);

    // Resolve the query time window (defaults match Grafana's `now-6h`..
    // `now`) so `${__from}` / `${__to}` and the body's `from`/`to` agree.
    let from_str = job
        .options
        .get("from")
        .and_then(Value::as_str)
        .unwrap_or("now-6h");
    let to_str = job
        .options
        .get("to")
        .and_then(Value::as_str)
        .unwrap_or("now");
    let from = parse_time(from_str, fetched_at);
    let to = parse_time(to_str, fetched_at);
    let interp = Interpolator::new(&dash, from, to);

    // Per-panel query timeout. Big time-series backends (TimescaleDB
    // hypertables, ...) can hang a single `/api/ds/query` indefinitely;
    // without a cap one slow panel stalls the whole dashboard sweep.
    // `0` disables it; default is 30s (matches Grafana's dataproxy).
    let query_timeout = job
        .options
        .get("query_timeout_secs")
        .and_then(Value::as_u64)
        .unwrap_or(30);
    let query_timeout = (query_timeout > 0).then(|| std::time::Duration::from_secs(query_timeout));

    // Datasource list is best-effort: without it we fall back to emitting
    // queries with no `datasourceId` (Grafana may still resolve the org
    // default), so a permissions/old-version failure here must not abort.
    let resolver = match client.datasources().await {
        Ok(ds) => DatasourceResolver::new(&ds),
        Err(e) => {
            tracing::warn!(error = %e, "GET /api/datasources failed; queries will omit datasourceId");
            DatasourceResolver::empty()
        }
    };

    let mut items = Vec::with_capacity(panels.len());
    let mut ds_responses = Vec::new();
    for panel in &panels {
        let Some(panel_id) = panel.get("id").and_then(Value::as_i64) else {
            // Rows and other id-less layout elements are not panels.
            continue;
        };
        // Non-data panels (text/row/dashlist/...) carry no resolvable
        // datasource; querying them yields a 400 from /api/ds/query. Emit
        // them as metadata-only items rather than failing the scrape.
        if !is_queryable(panel) {
            items.push(panel_item(panel_id, panel, None));
            continue;
        }
        let body = query_body(panel, &interp, &resolver, from, to);
        // Every target was hidden/empty: nothing to query (Grafana 7.x
        // 400s on an empty `queries` array). Emit metadata-only instead.
        let has_queries = matches!(body["queries"].as_array(), Some(q) if !q.is_empty());
        if !has_queries {
            items.push(panel_item(panel_id, panel, None));
            continue;
        }
        // A single panel's query failing (datasource shape the wrapper +
        // raw() fallback both reject, transient datasource error, ...)
        // must not abort the whole dashboard scrape: keep the panel as a
        // metadata-only item and move on (SCOPE: best-effort meta).
        match run_query(client, &body, query_timeout).await {
            Ok(data) => {
                ds_responses.push(data.clone());
                items.push(panel_item(panel_id, panel, Some(data)));
            }
            Err(e) => {
                tracing::warn!(panel_id, error = %e, "panel ds/query failed; emitting metadata-only");
                items.push(panel_item(panel_id, panel, None));
            }
        }
    }

    // SSRF guard: every absolute URL surfaced by the dashboard JSON and
    // the `/api/ds/query` responses is a potential datasource egress
    // target. Validate each host against the operator's allow-list
    // BEFORE returning the artifact (SCOPE: security).
    enforce_ssrf(&job.allow_hosts, &dash, &ds_responses)?;

    // Per-dashboard `meta` keeps ONLY `dashboard` + `annotations`
    // (REPORT §6a). The `/api/search` inventory is no longer duplicated
    // here — it lives once in the `_instance/<host>` sidecar.
    let annotations = client.annotations().await?;

    let mut artifact = Artifact::new("grafana", job.target.clone(), fetched_at);
    artifact.items = items;
    artifact.meta = json!({
        "dashboard": dash,
        "annotations": annotations,
    });
    Ok(artifact)
}

/// Extract the panel array from a `{ "dashboard": { "panels": [...] } }`
/// response, tolerating a missing/!array field (best-effort meta).
pub(crate) fn panel_list(dash: &Value) -> Vec<Value> {
    dash.get("dashboard")
        .and_then(|d| d.get("panels"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Whether a panel should be sent to `/api/ds/query`. Layout and static
/// panels (rows, text, dashboard/plugin lists, ...) carry no resolvable
/// datasource — and sometimes leftover `targets` — so querying them just
/// yields a 400. A queryable panel has a non-layout type and at least one
/// target.
pub(crate) fn is_queryable(panel: &Value) -> bool {
    const NON_DATA: &[&str] = &[
        "row",
        "text",
        "dashlist",
        "pluginlist",
        "alertlist",
        "news",
        "welcome",
        "gettingstarted",
    ];
    let ty = panel.get("type").and_then(Value::as_str).unwrap_or("");
    if NON_DATA.contains(&ty) {
        return false;
    }
    panel
        .get("targets")
        .and_then(Value::as_array)
        .is_some_and(|t| !t.is_empty())
}

/// Build the `/api/ds/query` body for one panel. Each target is
/// interpolated (dashboard variables + `${__from}`/`${__to}`) and tagged
/// with the numeric `datasourceId` Grafana's query endpoint requires,
/// resolved from the target's or panel's `datasource` reference (falling
/// back to the org default). `intervalMs` / `maxDataPoints` are derived
/// from the window so backend SQL macros (`$__timeGroup`, `$__interval`)
/// have sane values. `from`/`to` are epoch-ms strings matching the
/// interpolated `${__from}`/`${__to}`.
pub(crate) fn query_body(
    panel: &Value,
    interp: &Interpolator,
    resolver: &DatasourceResolver,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> Value {
    let from_ms = from.timestamp_millis();
    let to_ms = to.timestamp_millis();
    // ~100 buckets across the window, floored at 1s (matches Grafana's
    // default max-data-points heuristic closely enough for backend macros).
    let max_data_points: i64 = 100;
    let interval_ms = ((to_ms - from_ms) / max_data_points).max(1000);
    let panel_ds = panel.get("datasource");

    let targets = panel.get("targets").and_then(Value::as_array);
    let queries: Vec<Value> = targets
        .map(|ts| {
            ts.iter()
                .filter(|t| !t.get("hide").and_then(Value::as_bool).unwrap_or(false))
                .map(|t| {
                    let mut q = interp.interpolate_value(t);
                    let ds_ref = t.get("datasource").or(panel_ds);
                    if let Some((id, name)) = resolver.resolve(ds_ref, interp) {
                        let obj = q.as_object_mut().expect("target is a JSON object");
                        obj.insert("datasourceId".into(), id.into());
                        obj.entry("datasource").or_insert(Value::String(name));
                    }
                    let obj = q.as_object_mut().expect("target is a JSON object");
                    obj.entry("intervalMs").or_insert(interval_ms.into());
                    obj.entry("maxDataPoints").or_insert(max_data_points.into());
                    q
                })
                .collect()
        })
        .unwrap_or_default();

    json!({
        "queries": queries,
        "from": from_ms.to_string(),
        "to": to_ms.to_string(),
    })
}

/// Run one panel query, optionally bounded by `timeout`. A `None`
/// timeout runs unbounded; an elapsed timeout maps to
/// [`ScrapeError::Network`] so the caller's per-panel error path keeps
/// the panel as a metadata-only item instead of hanging the sweep.
async fn run_query(
    client: &dyn GrafanaClient,
    body: &Value,
    timeout: Option<std::time::Duration>,
) -> Result<Value, ScrapeError> {
    match timeout {
        None => client.ds_query(body).await,
        Some(dur) => match tokio::time::timeout(dur, client.ds_query(body)).await {
            Ok(res) => res,
            Err(_) => Err(ScrapeError::Network(format!(
                "ds/query exceeded {}s timeout",
                dur.as_secs()
            ))),
        },
    }
}

/// One `Item::Panel` with stable id `panel-{panelId}`.
pub(crate) fn panel_item(panel_id: i64, panel: &Value, data: Option<Value>) -> Item {
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
        data,
        tags,
    }
}

/// Reject the scrape if any surfaced URL's host is outside the
/// allow-list. An empty list means "operator did not opt in" → allow
/// all (SCOPE: default no allow-list).
pub(crate) fn enforce_ssrf(
    allow_hosts: &[String],
    dash: &Value,
    ds_responses: &[Value],
) -> Result<(), ScrapeError> {
    let mut values: Vec<&Value> = Vec::with_capacity(1 + ds_responses.len());
    values.push(dash);
    values.extend(ds_responses.iter());
    enforce_allow_hosts(allow_hosts, &values)
}

/// Reject if any URL host surfaced by `values` is outside `allow_hosts`.
/// Shared by the per-dashboard scrape and the instance sidecar so every
/// endpoint is gated at scrape time (SCOPE: SSRF guard). An empty list
/// means "operator did not opt in" → allow all.
pub(crate) fn enforce_allow_hosts(
    allow_hosts: &[String],
    values: &[&Value],
) -> Result<(), ScrapeError> {
    if allow_hosts.is_empty() {
        return Ok(());
    }
    let mut hosts = Vec::new();
    for v in values {
        collect_hosts(v, &mut hosts);
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

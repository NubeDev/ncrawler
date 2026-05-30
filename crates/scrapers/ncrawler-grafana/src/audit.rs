//! Stage 1 — the dashboard *audit*: structure + a per-panel query plan,
//! produced WITHOUT executing any `/api/ds/query` (SCOPE: REPORTS-UPDATE
//! Stage 1).
//!
//! `plan()` resolves variables and datasource ids exactly as `api.rs`
//! does, then emits the *exact* `/api/ds/query` body Stage 2 will POST.
//! That makes the plan auditable/diffable before any expensive query
//! runs, and turns Stage 2 into a dumb executor.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use ncrawler_spi::ScrapeError;

use crate::api::{is_queryable, panel_list, query_body};
use crate::interp::{parse_time, Interpolator};
use crate::resolve::DatasourceResolver;

/// `audit.json` filename inside the artifact directory.
pub const AUDIT_FILENAME: &str = "audit.json";

/// Stage-1 output: dashboard structure + the query plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Audit {
    pub dashboard_uid: String,
    /// Grafana base URL, stored so Stage 2 can rebuild a client from the
    /// artifact dir alone (Stage 2 takes no `--url`).
    pub base_url: String,
    pub title: String,
    pub folder: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    pub from: String,
    pub to: String,
    pub variables: Vec<AuditVar>,
    pub datasources: Vec<AuditDs>,
    pub panels: Vec<PanelPlan>,
}

/// A resolved dashboard variable (name + the value Stage 1 interpolated).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditVar {
    pub name: String,
    pub value: String,
}

/// One configured datasource, reduced to what the plan references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditDs {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    pub name: String,
    pub default: bool,
}

/// The plan for a single panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelPlan {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(rename = "type")]
    pub panel_type: String,
    pub queryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datasource_id: Option<i64>,
    pub targets: Vec<TargetPlan>,
    /// The exact `/api/ds/query` body Stage 2 will POST. `None` for
    /// non-queryable panels (rows/text/…) and panels whose targets are
    /// all hidden/empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_body: Option<Value>,
}

/// The plan for a single target within a panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetPlan {
    pub ref_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_sql: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interpolated_sql: Option<String>,
}

/// Build the audit/plan from an already-fetched dashboard + datasource
/// listing. Pure and offline — no network — so it is trivially testable
/// and replayable. `dash` is the `{ "meta", "dashboard" }` body
/// `dashboard_by_uid` returns.
pub fn plan(
    dash: &Value,
    datasources: &Value,
    uid: &str,
    base_url: &str,
    from_str: &str,
    to_str: &str,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Audit {
    let from = parse_time(from_str, fetched_at);
    let to = parse_time(to_str, fetched_at);
    let interp = Interpolator::new(dash, from, to);
    let resolver = DatasourceResolver::new(datasources);

    let inner = dash.get("dashboard");
    let title = inner
        .and_then(|d| d.get("title"))
        .and_then(Value::as_str)
        .unwrap_or(uid)
        .to_owned();
    let folder = dash
        .get("meta")
        .and_then(|m| m.get("folderTitle"))
        .and_then(Value::as_str)
        .unwrap_or("(none)")
        .to_owned();
    let version = inner.and_then(|d| d.get("version")).and_then(Value::as_i64);

    let panels = panel_list(dash)
        .iter()
        .filter_map(|p| panel_plan(p, &interp, &resolver, from, to))
        .collect();

    Audit {
        dashboard_uid: uid.to_owned(),
        base_url: base_url.trim_end_matches('/').to_owned(),
        title,
        folder,
        version,
        from: from_str.to_owned(),
        to: to_str.to_owned(),
        variables: resolved_variables(dash, &interp),
        datasources: datasource_list(datasources),
        panels,
    }
}

/// Plan one panel. Returns `None` for id-less layout elements (rows have
/// ids, but some legacy panels do not) so the plan only carries real
/// panels.
fn panel_plan(
    panel: &Value,
    interp: &Interpolator,
    resolver: &DatasourceResolver,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> Option<PanelPlan> {
    let id = panel.get("id").and_then(Value::as_i64)?;
    let panel_type = panel
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let title = panel
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let queryable = is_queryable(panel);

    let datasource_id = resolver
        .resolve(panel.get("datasource"), interp)
        .map(|(id, _name)| id);

    let targets = panel
        .get("targets")
        .and_then(Value::as_array)
        .map(|ts| {
            ts.iter()
                .filter(|t| !t.get("hide").and_then(Value::as_bool).unwrap_or(false))
                .enumerate()
                .map(|(i, t)| target_plan(i, t, interp))
                .collect()
        })
        .unwrap_or_default();

    // Build the exact body Stage 2 posts; drop it when nothing queryable.
    let body = if queryable {
        let b = query_body(panel, interp, resolver, from, to);
        let has_queries = matches!(b["queries"].as_array(), Some(q) if !q.is_empty());
        has_queries.then_some(b)
    } else {
        None
    };

    Some(PanelPlan {
        id,
        title,
        panel_type,
        queryable: queryable && body.is_some(),
        datasource_id,
        targets,
        query_body: body,
    })
}

/// Plan one target: ref id + template SQL + interpolated SQL.
fn target_plan(idx: usize, target: &Value, interp: &Interpolator) -> TargetPlan {
    let ref_id = target
        .get("refId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("T{idx}"));
    let template_sql = target
        .get("rawSql")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned);
    let interpolated_sql = template_sql.as_deref().map(|s| interp.interpolate(s));
    TargetPlan {
        ref_id,
        template_sql,
        interpolated_sql,
    }
}

/// Resolve every dashboard variable to the bare value Stage 1 used.
fn resolved_variables(dash: &Value, interp: &Interpolator) -> Vec<AuditVar> {
    dash.get("dashboard")
        .and_then(|d| d.get("templating"))
        .and_then(|t| t.get("list"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|v| {
                    let name = v.get("name").and_then(Value::as_str)?;
                    // Interpolating `$name` round-trips through the same
                    // resolution `query_body` uses, so the recorded value
                    // matches what the plan actually substituted.
                    let value = interp.interpolate(&format!("${{{name}}}"));
                    Some(AuditVar {
                        name: name.to_owned(),
                        value,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Map the `/api/datasources` listing into the audit's datasource list.
fn datasource_list(datasources: &Value) -> Vec<AuditDs> {
    datasources
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|d| {
                    let id = d.get("id").and_then(Value::as_i64)?;
                    Some(AuditDs {
                        id,
                        uid: d.get("uid").and_then(Value::as_str).map(str::to_owned),
                        name: d
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        default: d
                            .get("isDefault")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Write `audit.json` into the artifact directory (pretty, for diffs).
pub fn write(dir: &Path, audit: &Audit) -> Result<(), ScrapeError> {
    let path = dir.join(AUDIT_FILENAME);
    let json = serde_json::to_string_pretty(audit)
        .map_err(|e| ScrapeError::Other(format!("serialise audit: {e}")))?;
    std::fs::write(&path, json).map_err(|e| ScrapeError::Other(e.to_string()))
}

/// Read `audit.json` from the artifact directory.
pub fn read(dir: &Path) -> Result<Audit, ScrapeError> {
    let path = dir.join(AUDIT_FILENAME);
    let bytes = std::fs::read(&path)
        .map_err(|e| ScrapeError::Other(format!("read {}: {e}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| ScrapeError::Other(format!("parse audit.json: {e}")))
}

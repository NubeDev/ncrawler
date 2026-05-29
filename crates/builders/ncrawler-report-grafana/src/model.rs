//! The deterministic view model the renderer walks.
//!
//! Assembled from the `_instance` sidecar facts plus the selected
//! per-dashboard artifacts. All ordering is fixed here (REPORT §6b):
//! dashboards sort `(folder, title, uid)`, panels by panel id, variables
//! and tags/folders lexicographically — so re-renders diff cleanly.

use std::collections::BTreeMap;

use serde_json::Value;

use ncrawler_spi::Artifact;

use crate::sql::{self, DsEntry};

/// One datasource line for the overview.
#[derive(Debug, Clone)]
pub struct DsView {
    pub name: String,
    pub ds_type: String,
    pub is_default: bool,
}

/// Instance-wide facts (REPORT §1 Overview).
#[derive(Debug, Clone)]
pub struct InstanceView {
    pub host: String,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub renderer_available: Option<bool>,
    pub datasources: Vec<DsView>,
    pub dashboards_inventory: usize,
    pub dashboards_on_disk: usize,
    pub folders_count: usize,
}

/// One panel within a page (REPORT §3).
#[derive(Debug, Clone)]
pub struct PanelView {
    pub id: i64,
    pub title: String,
    pub ptype: String,
    pub datasource: String,
    pub value: Option<String>,
    pub template_sql: Vec<String>,
    pub executed_sql: Vec<String>,
    pub sample_rows: Option<String>,
}

/// One selected dashboard (REPORT §3 Pages).
#[derive(Debug, Clone)]
pub struct DashboardView {
    pub uid: String,
    pub title: String,
    pub folder: Option<String>,
    pub tags: Vec<String>,
    pub variables: Vec<(String, String)>,
    pub panels: Vec<PanelView>,
    pub primary_datasource: String,
    pub is_navigation: bool,
    pub nav_links: Vec<String>,
}

impl DashboardView {
    pub fn panel_count(&self) -> usize {
        self.panels.len()
    }
    pub fn variable_count(&self) -> usize {
        self.variables.len()
    }
}

/// The whole report model.
#[derive(Debug, Clone)]
pub struct RenderModel {
    pub instance: InstanceView,
    pub generated: chrono::DateTime<chrono::Utc>,
    pub dashboards: Vec<DashboardView>,
    pub scope: String,
    pub inventory_total: usize,
    pub on_disk_total: usize,
}

/// Sidecar facts handed to the assembler.
pub struct Facts<'a> {
    pub host: &'a str,
    pub generated: chrono::DateTime<chrono::Utc>,
    pub search: &'a Value,
    pub datasources: &'a Value,
    pub instance: &'a Value,
    pub folders: &'a Value,
}

impl RenderModel {
    /// Assemble the model from sidecar facts + the selected dashboard
    /// artifacts, with all stable ordering applied.
    pub fn assemble(
        facts: &Facts,
        dashboards: &[Artifact],
        scope: String,
        inventory_total: usize,
        on_disk_total: usize,
    ) -> Self {
        let ds_entries = DsEntry::parse_list(facts.datasources);
        let instance = instance_view(facts, &ds_entries, inventory_total, on_disk_total);
        let mut views: Vec<DashboardView> = dashboards
            .iter()
            .map(|a| dashboard_view(a, &ds_entries))
            .collect();
        views.sort_by(|a, b| {
            a.folder
                .cmp(&b.folder)
                .then_with(|| a.title.cmp(&b.title))
                .then_with(|| a.uid.cmp(&b.uid))
        });
        RenderModel {
            instance,
            generated: facts.generated,
            dashboards: views,
            scope,
            inventory_total,
            on_disk_total,
        }
    }
}

fn instance_view(
    facts: &Facts,
    ds: &[DsEntry],
    inventory_total: usize,
    on_disk_total: usize,
) -> InstanceView {
    InstanceView {
        host: facts.host.to_owned(),
        version: facts
            .instance
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        edition: facts
            .instance
            .get("edition")
            .and_then(Value::as_str)
            .map(str::to_owned),
        renderer_available: facts
            .instance
            .get("rendererAvailable")
            .and_then(Value::as_bool),
        datasources: ds
            .iter()
            .map(|d| DsView {
                name: d.name.clone(),
                ds_type: d.ds_type.clone(),
                is_default: d.is_default,
            })
            .collect(),
        dashboards_inventory: inventory_total,
        dashboards_on_disk: on_disk_total,
        folders_count: folder_count(facts),
    }
}

/// Folder count: the sidecar `/api/folders` length when present, else the
/// number of distinct folders across the `/api/search` inventory.
fn folder_count(facts: &Facts) -> usize {
    if let Some(a) = facts.folders.as_array() {
        return a.len();
    }
    facts
        .search
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r.get("folderTitle").and_then(Value::as_str))
                .filter(|s| !s.is_empty())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        })
        .unwrap_or(0)
}

/// The dashboard JSON object inside `meta.dashboard.dashboard`.
fn dash_obj(artifact: &Artifact) -> Option<&Value> {
    artifact.meta.pointer("/dashboard/dashboard")
}

fn dashboard_view(artifact: &Artifact, ds: &[DsEntry]) -> DashboardView {
    let dash = dash_obj(artifact);
    let uid = dash
        .and_then(|d| d.get("uid"))
        .and_then(Value::as_str)
        .unwrap_or(&artifact.target)
        .to_owned();
    let title = dash
        .and_then(|d| d.get("title"))
        .and_then(Value::as_str)
        .unwrap_or(&uid)
        .to_owned();
    let folder = artifact
        .meta
        .pointer("/dashboard/meta/folderTitle")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let tags = str_array(dash.and_then(|d| d.get("tags")));
    let variables = variables(dash);
    let panels = panel_views(artifact, ds);
    let primary_datasource = primary_ds(&panels);
    let is_navigation = title.to_lowercase().contains("navigation");
    let nav_links = if is_navigation {
        nav_links(dash)
    } else {
        Vec::new()
    };
    DashboardView {
        uid,
        title,
        folder,
        tags,
        variables,
        panels,
        primary_datasource,
        is_navigation,
        nav_links,
    }
}

/// Template variables as `(name, value)`, sorted by name. The value is the
/// `current.value` (joined when multi-valued); redaction is applied later.
fn variables(dash: Option<&Value>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = dash
        .and_then(|d| d.pointer("/templating/list"))
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(variable_pair).collect())
        .unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn variable_pair(v: &Value) -> Option<(String, String)> {
    let name = v.get("name").and_then(Value::as_str)?.to_owned();
    let value = match v.pointer("/current/value") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(","),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    Some((name, value))
}

fn panel_views(artifact: &Artifact, ds: &[DsEntry]) -> Vec<PanelView> {
    let panels = dash_obj(artifact)
        .and_then(|d| d.get("panels"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<PanelView> = panels
        .iter()
        .filter(|p| p.get("type").and_then(Value::as_str) != Some("row"))
        .filter_map(|p| panel_view(p, artifact, ds))
        .collect();
    out.sort_by_key(|p| p.id);
    out
}

fn panel_view(panel: &Value, artifact: &Artifact, ds: &[DsEntry]) -> Option<PanelView> {
    let id = panel.get("id").and_then(Value::as_i64)?;
    let title = panel
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let ptype = panel
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let datasource = sql::resolve_ds_name(panel.get("datasource"), ds);
    let data = item_data(artifact, id);
    Some(PanelView {
        id,
        title,
        ptype,
        datasource,
        value: data.and_then(sql::representative_value),
        template_sql: sql::template_sql(panel),
        executed_sql: data.map(sql::executed_sql).unwrap_or_default(),
        sample_rows: data.and_then(sql::sample_rows),
    })
}

/// The `data` payload of the `panel-{id}` item, if the scrape captured one.
fn item_data(artifact: &Artifact, id: i64) -> Option<&Value> {
    let want = format!("panel-{id}");
    artifact
        .items
        .iter()
        .find(|it| it.id == want)
        .and_then(|it| it.data.as_ref())
}

/// The most common datasource across a dashboard's panels (ties broken
/// lexicographically), for the page header line.
fn primary_ds(panels: &[PanelView]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for p in panels {
        *counts.entry(p.datasource.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(name, _)| name.to_owned())
        .unwrap_or_else(|| "—".to_owned())
}

/// Navigation links: dashlist/text panel titles + dashboard-level link
/// titles, deduped and sorted (best-effort: a `dashlist` is dynamic, so
/// we surface its presence rather than invent dashboard names).
fn nav_links(dash: Option<&Value>) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    if let Some(panels) = dash.and_then(|d| d.get("panels")).and_then(Value::as_array) {
        for p in panels {
            let ty = p.get("type").and_then(Value::as_str).unwrap_or("");
            if matches!(ty, "dashlist" | "text") {
                let t = p.get("title").and_then(Value::as_str).unwrap_or("");
                links.push(format!(
                    "{} ({ty})",
                    if t.is_empty() { "(untitled)" } else { t }
                ));
            }
        }
    }
    if let Some(ls) = dash.and_then(|d| d.get("links")).and_then(Value::as_array) {
        for l in ls {
            if let Some(t) = l.get("title").and_then(Value::as_str) {
                links.push(t.to_owned());
            }
        }
    }
    links.sort();
    links.dedup();
    links
}

fn str_array(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

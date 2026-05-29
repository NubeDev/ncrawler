//! Offline audit mode (REPORT §5).
//!
//! Audit reads the **frozen** per-dashboard artifacts straight from disk —
//! the response/error frames Api-mode always captures — plus the
//! `_instance` sidecar datasource list. It issues **no live HTTP**: there
//! is no `reqwest::Client` anywhere on this path (a test asserts the source
//! is reqwest-free), mirroring the two-phase rule that the report is a pure
//! renderer over data already on disk.
//!
//! Seven check classes ship here, each producing zero or more [`Finding`]s:
//!
//! 1. **Dead datasource references** — a panel points at a datasource that
//!    is not in the sidecar's datasource list (and is not a `$variable`).
//! 2. **Broken queries** — the stored response carries an error frame.
//! 3. **Empty panels** — a query ran but returned 0 rows.
//! 4. **Duplicate dashboards** — keyed on a `blake3` fingerprint over a
//!    normalised `(panels[], targets[], variables[])` projection that
//!    **excludes the title**, so identically-titled-but-different
//!    dashboards do NOT collide and differently-titled-but-identical ones
//!    do (mirrors the SCOPE item_id-only Asset linkage discipline).
//! 5. **Orphans** — dashboards in no folder and/or untagged.
//! 6. **Unused datasources** — configured in the sidecar but referenced by
//!    no panel across the scraped artifacts.
//! 7. **Blank/constant variables** — constants left empty, or constants
//!    that hardcode a value that arguably should be a variable.
//!
//! Output is a findings table grouped by severity with a fully
//! deterministic order (severity, then class, dashboard, panel, message).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use ncrawler_spi::Artifact;

use crate::sql::DsEntry;

/// Finding severity (REPORT §5). Ordered `error < warn < info` for grouping
/// — `error` first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warn,
    Info,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warn => "warn",
            Severity::Info => "info",
        }
    }
}

/// One audit finding (REPORT §5): a severity, the source artifact path, the
/// dashboard uid + title, an optional panel id + title, and a
/// human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// The check class (stable key, used for deterministic ordering).
    pub class: &'static str,
    /// Path to the source artifact (`artifact.json`) this finding came
    /// from; empty for instance-wide findings (e.g. unused datasources).
    pub source_artifact: String,
    pub dashboard_uid: String,
    pub dashboard_title: String,
    pub panel_id: Option<i64>,
    pub panel_title: Option<String>,
    pub message: String,
}

/// A dashboard artifact paired with the path it was read from, so findings
/// can name their source artifact (REPORT §5).
pub struct DashSource<'a> {
    pub path: PathBuf,
    pub artifact: &'a Artifact,
}

/// Run every check class over the scraped dashboards + the sidecar
/// datasource list, returning findings in deterministic sorted order.
pub fn run_audit(datasources: &[DsEntry], dashboards: &[DashSource]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut referenced_ds: BTreeSet<String> = BTreeSet::new();

    for d in dashboards {
        let ctx = DashCtx::new(d);
        dead_datasources(&ctx, datasources, &mut referenced_ds, &mut findings);
        broken_queries(&ctx, &mut findings);
        empty_panels(&ctx, &mut findings);
        orphans(&ctx, &mut findings);
        blank_constant_variables(&ctx, &mut findings);
    }

    duplicate_dashboards(dashboards, &mut findings);
    unused_datasources(datasources, &referenced_ds, &mut findings);

    sort_findings(&mut findings);
    findings
}

/// Deterministic order: severity, then class, dashboard uid, panel id,
/// message.
fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.class.cmp(b.class))
            .then_with(|| a.dashboard_uid.cmp(&b.dashboard_uid))
            .then_with(|| a.panel_id.cmp(&b.panel_id))
            .then_with(|| a.message.cmp(&b.message))
    });
}

/// Per-dashboard scratch view shared by the per-dashboard checks.
struct DashCtx<'a> {
    path: String,
    uid: String,
    title: String,
    folder: Option<String>,
    tags: Vec<String>,
    dash: Option<&'a Value>,
    artifact: &'a Artifact,
}

impl<'a> DashCtx<'a> {
    fn new(d: &'a DashSource) -> Self {
        let a = d.artifact;
        let dash = a.meta.pointer("/dashboard/dashboard");
        let uid = dash
            .and_then(|d| d.get("uid"))
            .and_then(Value::as_str)
            .unwrap_or(&a.target)
            .to_owned();
        let title = dash
            .and_then(|d| d.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(&uid)
            .to_owned();
        let folder = a
            .meta
            .pointer("/dashboard/meta/folderTitle")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let tags = dash
            .and_then(|d| d.get("tags"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        DashCtx {
            path: d.path.display().to_string(),
            uid,
            title,
            folder,
            tags,
            dash,
            artifact: a,
        }
    }

    /// Non-row panels of the dashboard, in document order.
    fn panels(&self) -> Vec<&'a Value> {
        self.dash
            .and_then(|d| d.get("panels"))
            .and_then(Value::as_array)
            .map(|ps| {
                ps.iter()
                    .filter(|p| p.get("type").and_then(Value::as_str) != Some("row"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The captured `panel-{id}` response data, if any.
    fn panel_data(&self, id: i64) -> Option<&'a Value> {
        let want = format!("panel-{id}");
        self.artifact
            .items
            .iter()
            .find(|it| it.id == want)
            .and_then(|it| it.data.as_ref())
    }

    /// A finding scaffold pre-filled with this dashboard's identity.
    fn finding(
        &self,
        severity: Severity,
        class: &'static str,
        panel_id: Option<i64>,
        panel_title: Option<String>,
        message: String,
    ) -> Finding {
        Finding {
            severity,
            class,
            source_artifact: self.path.clone(),
            dashboard_uid: self.uid.clone(),
            dashboard_title: self.title.clone(),
            panel_id,
            panel_title,
            message,
        }
    }
}

fn panel_id(panel: &Value) -> Option<i64> {
    panel.get("id").and_then(Value::as_i64)
}

fn panel_title(panel: &Value) -> Option<String> {
    panel
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// How a panel's `datasource` reference resolves against the sidecar list.
enum DsRef {
    /// Resolved to a configured datasource (display name).
    Known(String),
    /// A `$variable` reference — neither dead nor a concrete datasource.
    Variable,
    /// Referenced but not present in the sidecar (display of the ref).
    Dead(String),
}

/// Resolve a panel `datasource` reference to a [`DsRef`]. Never invents a
/// match: an unknown uid/name/type is [`DsRef::Dead`].
fn resolve_ds_ref(ds_ref: Option<&Value>, datasources: &[DsEntry]) -> DsRef {
    let default = datasources.iter().find(|d| d.is_default);
    match ds_ref {
        None | Some(Value::Null) => match default {
            Some(d) => DsRef::Known(d.name.clone()),
            None => DsRef::Dead("(default, none configured)".to_owned()),
        },
        Some(Value::String(s)) if s == "default" || s == "-- Grafana --" => match default {
            Some(d) => DsRef::Known(d.name.clone()),
            None => DsRef::Dead(s.clone()),
        },
        Some(Value::String(s)) if s.starts_with('$') => DsRef::Variable,
        Some(Value::String(s)) => datasources
            .iter()
            .find(|d| &d.name == s || d.uid.as_deref() == Some(s))
            .map(|d| DsRef::Known(d.name.clone()))
            .unwrap_or_else(|| DsRef::Dead(s.clone())),
        Some(Value::Object(o)) => resolve_ds_object(o, datasources),
        Some(other) => DsRef::Dead(other.to_string()),
    }
}

fn resolve_ds_object(obj: &serde_json::Map<String, Value>, datasources: &[DsEntry]) -> DsRef {
    if let Some(uid) = obj.get("uid").and_then(Value::as_str) {
        if uid.starts_with('$') {
            return DsRef::Variable;
        }
        if let Some(d) = datasources.iter().find(|d| d.uid.as_deref() == Some(uid)) {
            return DsRef::Known(d.name.clone());
        }
        // A concrete uid that matches nothing is dead, even if a type is
        // also present (the type alone cannot rescue a stale uid).
        return DsRef::Dead(uid.to_owned());
    }
    if let Some(ty) = obj.get("type").and_then(Value::as_str) {
        return datasources
            .iter()
            .find(|d| d.ds_type == ty)
            .map(|d| DsRef::Known(d.name.clone()))
            .unwrap_or_else(|| DsRef::Dead(ty.to_owned()));
    }
    DsRef::Dead("(unspecified)".to_owned())
}

/// Check 1: dead datasource references. Also records every *known*
/// reference so [`unused_datasources`] can find the configured-but-unused
/// ones.
fn dead_datasources(
    ctx: &DashCtx,
    datasources: &[DsEntry],
    referenced: &mut BTreeSet<String>,
    out: &mut Vec<Finding>,
) {
    for panel in ctx.panels() {
        match resolve_ds_ref(panel.get("datasource"), datasources) {
            DsRef::Known(name) => {
                referenced.insert(name);
            }
            DsRef::Variable => {}
            DsRef::Dead(reference) => out.push(ctx.finding(
                Severity::Error,
                "dead-datasource",
                panel_id(panel),
                panel_title(panel),
                format!("panel references datasource `{reference}` not present in the instance"),
            )),
        }
    }
}

/// Check 2: broken queries — stored error frames in the response.
fn broken_queries(ctx: &DashCtx, out: &mut Vec<Finding>) {
    for panel in ctx.panels() {
        let Some(id) = panel_id(panel) else { continue };
        let Some(data) = ctx.panel_data(id) else {
            continue;
        };
        for (ref_id, err) in result_errors(data) {
            out.push(ctx.finding(
                Severity::Error,
                "broken-query",
                Some(id),
                panel_title(panel),
                format!("query {ref_id} failed: {err}"),
            ));
        }
    }
}

/// Error strings per refId in a `/api/ds/query` response, sorted by refId.
fn result_errors(data: &Value) -> Vec<(String, String)> {
    let Some(results) = data.get("results").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut refs: Vec<&String> = results.keys().collect();
    refs.sort();
    let mut out = Vec::new();
    for r in refs {
        let result = &results[r];
        if let Some(err) = result.get("error").and_then(Value::as_str) {
            if !err.is_empty() {
                out.push((r.clone(), err.to_owned()));
                continue;
            }
        }
        // Grafana 8+ nests the message under `status`/`errorSource`; fall
        // back to a generic message when only an error status is present.
        if let Some(status) = result.get("status").and_then(Value::as_i64) {
            if status >= 400 {
                out.push((r.clone(), format!("status {status}")));
            }
        }
    }
    out
}

/// Check 3: empty panels — a query ran but returned 0 rows.
fn empty_panels(ctx: &DashCtx, out: &mut Vec<Finding>) {
    for panel in ctx.panels() {
        let Some(id) = panel_id(panel) else { continue };
        let Some(data) = ctx.panel_data(id) else {
            continue;
        };
        // Skip panels whose query errored — that is a broken query, not an
        // empty one.
        if !result_errors(data).is_empty() {
            continue;
        }
        if let Some(rows) = response_row_count(data) {
            if rows == 0 {
                out.push(ctx.finding(
                    Severity::Warn,
                    "empty-panel",
                    Some(id),
                    panel_title(panel),
                    "query ran but returned 0 rows".to_owned(),
                ));
            }
        }
    }
}

/// Total row count across the frames of a response, or `None` when the
/// response carries no columnar data at all (i.e. no query ran / no frames
/// — not an empty result, just absent).
fn response_row_count(data: &Value) -> Option<usize> {
    let results = data.get("results").and_then(Value::as_object)?;
    let mut saw_frame = false;
    let mut total = 0usize;
    for result in results.values() {
        let Some(frames) = result.get("frames").and_then(Value::as_array) else {
            continue;
        };
        for f in frames {
            saw_frame = true;
            if let Some(values) = f.pointer("/data/values").and_then(Value::as_array) {
                let rows = values
                    .iter()
                    .filter_map(Value::as_array)
                    .map(|c| c.len())
                    .max()
                    .unwrap_or(0);
                total += rows;
            }
        }
    }
    if saw_frame {
        Some(total)
    } else {
        None
    }
}

/// Check 4: duplicate dashboards, keyed on a blake3 fingerprint over a
/// normalised `(panels[], targets[], variables[])` projection that excludes
/// the title. Dashboards sharing a fingerprint are reported (info on the
/// first, warn on the rest pointing back at the canonical one).
fn duplicate_dashboards(dashboards: &[DashSource], out: &mut Vec<Finding>) {
    // fingerprint -> list of (uid, title, path) in input order.
    let mut groups: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for d in dashboards {
        let ctx = DashCtx::new(d);
        let fp = fingerprint(&ctx);
        groups
            .entry(fp)
            .or_default()
            .push((ctx.uid.clone(), ctx.title.clone(), ctx.path.clone()));
    }
    for (fp, mut members) in groups {
        if members.len() < 2 {
            continue;
        }
        // Canonical member: lowest uid, for a stable "duplicate of" target.
        members.sort_by(|a, b| a.0.cmp(&b.0));
        let (canon_uid, _, _) = members[0].clone();
        let short = &fp[..16.min(fp.len())];
        for (uid, title, path) in &members {
            let msg = if *uid == canon_uid {
                format!(
                    "duplicate set (fingerprint {short}): {} dashboards share this panel/query set",
                    members.len()
                )
            } else {
                format!("duplicate of `{canon_uid}` (fingerprint {short})")
            };
            out.push(Finding {
                severity: Severity::Warn,
                class: "duplicate-dashboard",
                source_artifact: path.clone(),
                dashboard_uid: uid.clone(),
                dashboard_title: title.clone(),
                panel_id: None,
                panel_title: None,
                message: msg,
            });
        }
    }
}

/// The blake3 fingerprint of a dashboard's normalised projection. The
/// projection deliberately omits the dashboard title (REPORT §5: match on
/// content, not title) — it covers the sorted panel set (id, type, title,
/// datasource ref, sorted targets) and the sorted variable set
/// (name, type). Stable JSON serialisation in, hex digest out.
fn fingerprint(ctx: &DashCtx) -> String {
    let mut panels: Vec<Value> = ctx
        .panels()
        .iter()
        .map(|p| {
            let mut targets: Vec<Value> = p
                .get("targets")
                .and_then(Value::as_array)
                .map(|ts| {
                    ts.iter()
                        .map(|t| {
                            json!({
                                "refId": t.get("refId").and_then(Value::as_str).unwrap_or(""),
                                "rawSql": t.get("rawSql").and_then(Value::as_str).unwrap_or(""),
                                "expr": t.get("expr").and_then(Value::as_str).unwrap_or(""),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            targets.sort_by_key(|t| t.to_string());
            json!({
                "id": p.get("id").and_then(Value::as_i64),
                "type": p.get("type").and_then(Value::as_str).unwrap_or(""),
                "title": p.get("title").and_then(Value::as_str).unwrap_or(""),
                "datasource": p.get("datasource").cloned().unwrap_or(Value::Null),
                "targets": targets,
            })
        })
        .collect();
    panels.sort_by_key(|p| p.to_string());

    let mut variables: Vec<Value> = ctx
        .dash
        .and_then(|d| d.pointer("/templating/list"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|v| {
                    json!({
                        "name": v.get("name").and_then(Value::as_str).unwrap_or(""),
                        "type": v.get("type").and_then(Value::as_str).unwrap_or(""),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    variables.sort_by_key(|v| v.to_string());

    let projection = json!({ "panels": panels, "variables": variables });
    let bytes = serde_json::to_vec(&projection).unwrap_or_default();
    blake3::hash(&bytes).to_hex().to_string()
}

/// Check 5: orphans — dashboards in no folder and/or untagged.
fn orphans(ctx: &DashCtx, out: &mut Vec<Finding>) {
    let no_folder = ctx.folder.is_none();
    let untagged = ctx.tags.is_empty();
    if !no_folder && !untagged {
        return;
    }
    let what = match (no_folder, untagged) {
        (true, true) => "in no folder and untagged",
        (true, false) => "in no folder",
        (false, true) => "untagged",
        (false, false) => unreachable!(),
    };
    out.push(ctx.finding(
        Severity::Info,
        "orphan",
        None,
        None,
        format!("orphan dashboard: {what}"),
    ));
}

/// Check 7: blank/constant variables. An empty constant is a warn (broken
/// config); a constant with a hardcoded value is an info (smells like it
/// should be a real variable).
fn blank_constant_variables(ctx: &DashCtx, out: &mut Vec<Finding>) {
    let list = ctx
        .dash
        .and_then(|d| d.pointer("/templating/list"))
        .and_then(Value::as_array);
    let Some(list) = list else { return };
    for v in list {
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        if ty != "constant" {
            continue;
        }
        let name = v.get("name").and_then(Value::as_str).unwrap_or("");
        let value = match v.pointer("/current/value") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        let query_empty = v
            .get("query")
            .and_then(Value::as_str)
            .map(str::is_empty)
            .unwrap_or(true);
        if value.is_empty() && query_empty {
            out.push(ctx.finding(
                Severity::Warn,
                "blank-variable",
                None,
                None,
                format!("constant variable `{name}` is left empty"),
            ));
        } else {
            out.push(ctx.finding(
                Severity::Info,
                "constant-variable",
                None,
                None,
                format!("constant variable `{name}` hardcodes a value"),
            ));
        }
    }
}

/// Check 6: unused datasources — configured in the sidecar but referenced
/// by no panel across the scraped artifacts.
fn unused_datasources(
    datasources: &[DsEntry],
    referenced: &BTreeSet<String>,
    out: &mut Vec<Finding>,
) {
    let mut names: Vec<&DsEntry> = datasources.iter().collect();
    names.sort_by(|a, b| a.name.cmp(&b.name));
    for d in names {
        if referenced.contains(&d.name) {
            continue;
        }
        out.push(Finding {
            severity: Severity::Warn,
            class: "unused-datasource",
            source_artifact: String::new(),
            dashboard_uid: String::new(),
            dashboard_title: String::new(),
            panel_id: None,
            panel_title: None,
            message: format!(
                "datasource `{}` ({}) is configured but referenced by no scraped panel",
                d.name, d.ds_type
            ),
        });
    }
}

/// Build [`DashSource`]s from parallel slices of paths + artifacts.
pub fn pair_sources<'a>(dirs: &[PathBuf], artifacts: &'a [Artifact]) -> Vec<DashSource<'a>> {
    artifacts
        .iter()
        .enumerate()
        .map(|(i, a)| DashSource {
            path: artifact_path(dirs.get(i).map(PathBuf::as_path)),
            artifact: a,
        })
        .collect()
}

fn artifact_path(dir: Option<&Path>) -> PathBuf {
    match dir {
        Some(d) => d.join("artifact.json"),
        None => PathBuf::from("artifact.json"),
    }
}

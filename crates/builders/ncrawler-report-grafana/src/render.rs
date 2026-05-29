//! Deterministic Markdown rendering of the [`RenderModel`] (REPORT §4).
//!
//! Pure: same model + options in, same bytes out — no clock, no
//! filesystem. Secret redaction (REPORT §7, default-on) is applied here as
//! the report leaves the 0700 artifact boundary; the artifacts on disk
//! stay raw.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use ncrawler_redact::Redactor;

use crate::model::{DashboardView, PanelView, RenderModel};
use crate::{Mode, ReportOptions};

/// Render the model to Markdown under `opts`.
pub fn render(model: &RenderModel, opts: &ReportOptions) -> String {
    let red = Red::new(opts.redact);
    let mut out = String::new();
    header(&mut out, model, opts);
    overview(&mut out, model);
    structure(&mut out, model);
    pages(&mut out, model, opts, &red);
    out
}

/// The metadata header (REPORT §4): mode, data on/off + window, scope,
/// on-disk-vs-inventory counts, redacted flag.
fn header(out: &mut String, model: &RenderModel, opts: &ReportOptions) {
    let _ = writeln!(out, "# Grafana Report — {}", model.instance.host);
    let mode = match opts.mode {
        Mode::Overview => "overview",
        Mode::Full => "full",
    };
    let data = if opts.data {
        format!("on ({})", opts.window)
    } else {
        "off".to_owned()
    };
    let redacted = if opts.redact {
        "redacted"
    } else {
        "not redacted"
    };
    let _ = writeln!(
        out,
        "generated {} · mode: {mode} · data: {data} · {redacted}",
        model.generated.format("%Y-%m-%d")
    );
    let _ = writeln!(
        out,
        "scope: {}  ({} on disk of {} in instance inventory)",
        model.scope, model.on_disk_total, model.inventory_total
    );
}

fn overview(out: &mut String, model: &RenderModel) {
    let inst = &model.instance;
    out.push_str("\n## 1. Overview\n");
    let _ = writeln!(out, "- URL:          https://{}", inst.host);
    let version = inst.version.as_deref().unwrap_or("unknown");
    let edition = inst.edition.as_deref().unwrap_or("unknown");
    let _ = writeln!(out, "- Version:      {version}  ({edition})");
    let _ = writeln!(out, "- Datasources:  {}", datasource_line(model));
    let _ = writeln!(
        out,
        "- Dashboards:   {} inventory / {} scraped       Folders: {}",
        inst.dashboards_inventory, inst.dashboards_on_disk, inst.folders_count
    );
    let renderer = match inst.renderer_available {
        Some(true) => "installed",
        Some(false) => "not installed  (visual/screenshot mode unavailable; chrome fallback only)",
        None => "unknown",
    };
    let _ = writeln!(out, "- Renderer plugin: {renderer}");
}

fn datasource_line(model: &RenderModel) -> String {
    if model.instance.datasources.is_empty() {
        return "none recorded".to_owned();
    }
    let mut ds: Vec<String> = model
        .instance
        .datasources
        .iter()
        .map(|d| {
            let dflt = if d.is_default { ", default" } else { "" };
            format!("{} ({}{dflt})", d.name, d.ds_type)
        })
        .collect();
    ds.sort();
    ds.join(" · ")
}

fn structure(out: &mut String, model: &RenderModel) {
    out.push_str("\n## 2. Structure / Navigation\n");
    folder_tree(out, model);
    navigation(out, model);
    tag_index(out, model);
    panel_type_distribution(out, model);
    datasource_usage(out, model);
}

fn folder_tree(out: &mut String, model: &RenderModel) {
    out.push_str("\n### Folder tree\n");
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for d in &model.dashboards {
        let key = d.folder.clone().unwrap_or_else(|| "(no folder)".to_owned());
        *counts.entry(key).or_default() += 1;
    }
    if counts.is_empty() {
        out.push_str("- (none on disk)\n");
    }
    for (folder, n) in counts {
        let _ = writeln!(out, "- {folder}: {n}");
    }
}

fn navigation(out: &mut String, model: &RenderModel) {
    let Some(nav) = model.dashboards.iter().find(|d| d.is_navigation) else {
        return;
    };
    let _ = writeln!(out, "\n### 0. Navigation ({})", nav.title);
    if nav.nav_links.is_empty() {
        out.push_str("- (no dashlist/links found)\n");
    }
    for link in &nav.nav_links {
        let _ = writeln!(out, "- {link}");
    }
}

fn tag_index(out: &mut String, model: &RenderModel) {
    out.push_str("\n### Tag index\n");
    let mut index: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for d in &model.dashboards {
        for t in &d.tags {
            index
                .entry(t.clone())
                .or_default()
                .push(format!("{} ({})", d.title, d.uid));
        }
    }
    if index.is_empty() {
        out.push_str("- (no tags)\n");
    }
    for (tag, mut dashes) in index {
        dashes.sort();
        let _ = writeln!(out, "- {tag}: {}", dashes.join(", "));
    }
}

fn panel_type_distribution(out: &mut String, model: &RenderModel) {
    out.push_str("\n### Panel-type distribution\n");
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for d in &model.dashboards {
        for p in &d.panels {
            let key = if p.ptype.is_empty() {
                "(unknown)".to_owned()
            } else {
                p.ptype.clone()
            };
            *counts.entry(key).or_default() += 1;
        }
    }
    if counts.is_empty() {
        out.push_str("- (no panels)\n");
    }
    for (ty, n) in counts {
        let _ = writeln!(out, "- {ty}: {n}");
    }
}

fn datasource_usage(out: &mut String, model: &RenderModel) {
    out.push_str("\n### Datasource usage\n");
    // ds -> (panel count, set of dashboard titles)
    let mut usage: BTreeMap<String, (usize, std::collections::BTreeSet<String>)> = BTreeMap::new();
    for d in &model.dashboards {
        for p in &d.panels {
            let e = usage.entry(p.datasource.clone()).or_default();
            e.0 += 1;
            e.1.insert(d.title.clone());
        }
    }
    if usage.is_empty() {
        out.push_str("- (none)\n");
    }
    for (ds, (panels, dashes)) in usage {
        let _ = writeln!(
            out,
            "- {ds}: {panels} panel(s) across {} dashboard(s)",
            dashes.len()
        );
    }
}

fn pages(out: &mut String, model: &RenderModel, opts: &ReportOptions, red: &Red) {
    out.push_str("\n## 3. Pages\n");
    if model.dashboards.is_empty() {
        out.push_str("\n_(no dashboards selected / on disk)_\n");
    }
    for d in &model.dashboards {
        page(out, d, opts, red);
    }
}

fn page(out: &mut String, d: &DashboardView, opts: &ReportOptions, red: &Red) {
    let folder = d.folder.as_deref().unwrap_or("(no folder)");
    let _ = writeln!(out, "\n### {} — uid {} · folder: {folder}", d.title, d.uid);
    let _ = writeln!(
        out,
        "- {} panels · {} variables · datasource: {}",
        d.panel_count(),
        d.variable_count(),
        d.primary_datasource
    );
    variables_line(out, d, red);
    panels_table(out, d, opts, red);
    if matches!(opts.mode, Mode::Full) {
        panel_details(out, d, opts, red);
    }
}

fn variables_line(out: &mut String, d: &DashboardView, red: &Red) {
    if d.variables.is_empty() {
        out.push_str("- Variables: (none)\n");
        return;
    }
    let rendered: Vec<String> = d
        .variables
        .iter()
        .map(|(k, v)| format!("{k}={}", red.var(k, v)))
        .collect();
    let _ = writeln!(out, "- Variables: {}", rendered.join(", "));
}

fn panels_table(out: &mut String, d: &DashboardView, opts: &ReportOptions, red: &Red) {
    out.push_str("- Panels (sorted by panel id):\n\n");
    out.push_str("  | panel | id | type | datasource | value |\n");
    out.push_str("  |-------|----|------|-----------|-------|\n");
    for p in &d.panels {
        let value = match (opts.data, &p.value) {
            (true, Some(v)) => red.text(v).into_owned(),
            _ => "—".to_owned(),
        };
        let _ = writeln!(
            out,
            "  | {} | {} | {} | {} | {} |",
            cell(&p.title),
            p.id,
            cell(&p.ptype),
            cell(&p.datasource),
            cell(&value)
        );
    }
}

/// Per-panel template SQL (full), plus executed SQL + sample rows
/// (full + `--data`, only where the response exposed them — never faked).
fn panel_details(out: &mut String, d: &DashboardView, opts: &ReportOptions, red: &Red) {
    for p in &d.panels {
        if p.template_sql.is_empty() && p.executed_sql.is_empty() && p.sample_rows.is_none() {
            continue;
        }
        let _ = writeln!(out, "\n#### {} (panel {})", p.title, p.id);
        for sql in &p.template_sql {
            out.push_str("\ntemplate SQL:\n```sql\n");
            out.push_str(&red.text(sql));
            out.push_str("\n```\n");
        }
        if opts.data {
            executed_and_sample(out, p, red);
        }
    }
}

fn executed_and_sample(out: &mut String, p: &PanelView, red: &Red) {
    for sql in &p.executed_sql {
        out.push_str("\nexecuted SQL (where the datasource exposes it):\n```sql\n");
        out.push_str(&red.text(sql));
        out.push_str("\n```\n");
    }
    if let Some(rows) = &p.sample_rows {
        out.push_str("\nsample returned rows:\n```json\n");
        out.push_str(&red.text(rows));
        out.push_str("\n```\n");
    }
}

/// Escape a table cell: collapse newlines and escape pipes so the Markdown
/// table stays well-formed and single-line.
fn cell(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\n', '\r'], " ")
}

/// Redaction gate: applies [`Redactor`] when on, a no-op passthrough when
/// `--no-redact` was given.
struct Red {
    on: bool,
    redactor: Redactor,
}

impl Red {
    fn new(on: bool) -> Self {
        Self {
            on,
            redactor: Redactor::new(),
        }
    }

    fn text<'a>(&self, s: &'a str) -> std::borrow::Cow<'a, str> {
        if self.on {
            self.redactor.redact(s)
        } else {
            std::borrow::Cow::Borrowed(s)
        }
    }

    /// Redact a variable value by both its key name and value shape.
    fn var(&self, key: &str, value: &str) -> String {
        if !self.on {
            return value.to_owned();
        }
        let mut map = std::collections::HashMap::new();
        map.insert(key.to_owned(), value.to_owned());
        self.redactor.redact_variables(&mut map);
        map.remove(key).unwrap_or_default()
    }
}

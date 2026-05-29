//! ncrawler — deterministic offline **Grafana instance report** builder
//! (`report-grafana`).
//!
//! A pure renderer over data already on disk (two-phase: scrape →
//! artifacts + `_instance` sidecar → build report). It reads the
//! `_instance/<host>/latest` sidecar (overview + structure) plus the
//! selected per-dashboard artifacts (pages) and writes `REPORT.md` next to
//! the sidecar's `latest`. Nothing here touches a live Grafana.
//!
//! Three sections (REPORT §4): **Overview** (instance facts), **Structure**
//! (folder tree, navigation, tag index, panel-type distribution,
//! datasource usage), **Pages** (one block per dashboard with its
//! variables — redacted by default — and panels sorted by panel id).
//!
//! Depth: `--mode overview|full`. In `full`, each panel additionally gets
//! its **template SQL** (raw `rawSql`, `$variables` verbatim). With
//! `--data`, panels that have a captured response also get **executed
//! SQL** (`results.<refId>.meta.executedQueryString`, only where the
//! datasource exposes it — never invented) and a clipped row sample.
//!
//! Stable ordering is mandatory (REPORT §6b): pages sort `(folder, title,
//! uid)`, panels by id, tags/folders lexicographically — so re-renders
//! diff cleanly.

mod model;
mod render;
mod sql;

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use ncrawler_core::{read_artifact, InstanceSidecar};
use ncrawler_spi::{Artifact, BuildCtx, BuildError, BuildOutput, Builder, Cancel};

pub use model::RenderModel;
pub use render::render;

/// Filename written next to the sidecar's `latest`.
pub const REPORT_FILENAME: &str = "REPORT.md";

/// Sidecar filename written by the scraper (`ncrawler-core`).
const INSTANCE_FILE: &str = "instance.json";

/// Default query window when `--data` is on (REPORT §3, matches the
/// scraper's `now-6h`).
pub const DEFAULT_WINDOW: &str = "now-6h";

/// Report depth (REPORT §3). Audit is a later stage; this builder ships
/// overview + full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Overview,
    Full,
}

impl Mode {
    /// Parse `--mode` (default `overview`). Unknown values fall back to
    /// `overview` rather than failing the build.
    pub fn parse(s: &str) -> Mode {
        match s {
            "full" => Mode::Full,
            _ => Mode::Overview,
        }
    }
}

/// The rendering knobs resolved from the CLI flags.
#[derive(Debug, Clone)]
pub struct ReportOptions {
    pub mode: Mode,
    /// `--data`: emit executed SQL + sample rows (default off).
    pub data: bool,
    /// `--window` used when `--data` is on (default [`DEFAULT_WINDOW`]).
    pub window: String,
    /// `--redact`/`--no-redact` (default on).
    pub redact: bool,
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self {
            mode: Mode::Overview,
            data: false,
            window: DEFAULT_WINDOW.to_owned(),
            redact: true,
        }
    }
}

impl ReportOptions {
    /// Read the options out of a [`BuildCtx::options`] JSON object.
    fn from_options(opts: &Value) -> Self {
        ReportOptions {
            mode: opts
                .get("mode")
                .and_then(Value::as_str)
                .map(Mode::parse)
                .unwrap_or(Mode::Overview),
            data: opts.get("data").and_then(Value::as_bool).unwrap_or(false),
            window: opts
                .get("window")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_WINDOW)
                .to_owned(),
            redact: opts.get("redact").and_then(Value::as_bool).unwrap_or(true),
        }
    }
}

/// The deterministic Grafana report [`Builder`].
#[derive(Default)]
pub struct GrafanaReportBuilder;

impl GrafanaReportBuilder {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Builder for GrafanaReportBuilder {
    fn name(&self) -> &str {
        "report-grafana"
    }

    /// `artifact` is unused: `report-grafana` renders over the **store**
    /// (the sidecar named by `ctx.artifact_dir` + `ctx.dashboard_dirs`),
    /// not a single artifact. The CLI prefers [`build_report`] directly;
    /// this impl keeps the builder usable through the generic seam.
    async fn build(
        &self,
        _artifact: &Artifact,
        ctx: &BuildCtx,
        cancel: &dyn Cancel,
    ) -> Result<BuildOutput, BuildError> {
        let opts = ReportOptions::from_options(&ctx.options);
        let scope = ctx
            .options
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or("(scope unknown)")
            .to_owned();
        let inventory_total = ctx
            .options
            .get("inventory_total")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let on_disk_total = ctx
            .options
            .get("on_disk_total")
            .and_then(Value::as_u64)
            .unwrap_or(ctx.dashboard_dirs.len() as u64) as usize;
        build_report(
            &ctx.artifact_dir,
            &ctx.dashboard_dirs,
            &scope,
            inventory_total,
            on_disk_total,
            &opts,
            cancel,
        )
    }
}

/// Render a Grafana report from the store and write `REPORT.md` next to
/// the sidecar's `latest`.
///
/// `sidecar_dir` is the `_instance/<host>/latest` directory (holding
/// `instance.json`); `dashboard_dirs` are the selected per-dashboard
/// artifact dirs (each holding `artifact.json`). `scope` describes the
/// selection (e.g. `--all`); the two counts populate the header's
/// on-disk-vs-inventory line (REPORT §4).
#[allow(clippy::too_many_arguments)]
pub fn build_report(
    sidecar_dir: &Path,
    dashboard_dirs: &[PathBuf],
    scope: &str,
    inventory_total: usize,
    on_disk_total: usize,
    opts: &ReportOptions,
    cancel: &dyn Cancel,
) -> Result<BuildOutput, BuildError> {
    if cancel.is_cancelled() {
        return Err(BuildError::Cancelled);
    }
    let sidecar = read_sidecar(sidecar_dir)?;
    let mut dashboards = Vec::with_capacity(dashboard_dirs.len());
    for dir in dashboard_dirs {
        if cancel.is_cancelled() {
            return Err(BuildError::Cancelled);
        }
        let artifact = read_artifact(dir).map_err(|e| BuildError::Io(e.to_string()))?;
        dashboards.push(artifact);
    }

    let facts = model::Facts {
        host: &sidecar.host,
        generated: sidecar.fetched_at,
        search: &sidecar.search,
        datasources: &sidecar.datasources,
        instance: &sidecar.instance,
        folders: &sidecar.folders,
    };
    let model = RenderModel::assemble(
        &facts,
        &dashboards,
        scope.to_owned(),
        inventory_total,
        on_disk_total,
    );
    let markdown = render::render(&model, opts);

    let rel = PathBuf::from(REPORT_FILENAME);
    let abs = sidecar_dir.join(&rel);
    std::fs::write(&abs, markdown).map_err(|e| BuildError::Io(e.to_string()))?;
    Ok(BuildOutput {
        files: vec![rel],
        summary: format!(
            "rendered {} dashboard(s) to {REPORT_FILENAME} (mode {:?}, data {}, redact {})",
            model.dashboards.len(),
            opts.mode,
            opts.data,
            opts.redact
        ),
    })
}

/// Read + deserialize the `_instance/<host>/latest/instance.json` sidecar.
fn read_sidecar(sidecar_dir: &Path) -> Result<InstanceSidecar, BuildError> {
    let path = sidecar_dir.join(INSTANCE_FILE);
    let bytes = std::fs::read(&path)
        .map_err(|e| BuildError::Io(format!("reading instance sidecar {}: {e}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| BuildError::Io(format!("parsing {}: {e}", path.display())))
}

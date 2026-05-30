//! Stage 2 — execute the plan from Stage 1, one result-per-panel
//! (SCOPE: REPORTS-UPDATE Stage 2).
//!
//! Reads `audit.json`, POSTs each queryable panel's `query_body`, and
//! writes `data/panel-<id>.json` + `data-status.json`. Selective
//! execution (`--panel`, `--only-missing`, `--only-failed`) makes a
//! re-run cheap: non-selected panels keep their on-disk result and
//! status byte-identical.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use ncrawler_spi::ScrapeError;

use crate::audit::Audit;
use crate::client::GrafanaClient;
use crate::status::{classify, DataStatus, PanelStatus, PanelStatusEntry};

/// `data/` subdirectory and `data-status.json` filenames.
pub const DATA_DIR: &str = "data";
pub const STATUS_FILENAME: &str = "data-status.json";

/// Which panels Stage 2 should (re)run. Mutually-combined precedence:
/// explicit `--panel` ids win; else `--only-missing` / `--only-failed`
/// narrow the set; else everything.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// `--panel <id>` (repeatable). Empty = no explicit restriction.
    pub panels: Vec<i64>,
    /// `--only-missing`: skip panels that already have a data file.
    pub only_missing: bool,
    /// `--only-failed`: re-run only `error` / `timeout` panels.
    pub only_failed: bool,
}

impl Selection {
    /// Should this panel be (re)run given the prior status + on-disk files?
    fn wants(&self, panel_id: i64, dir: &Path, prev: &DataStatus) -> bool {
        if !self.panels.is_empty() {
            return self.panels.contains(&panel_id);
        }
        if self.only_missing && panel_file(dir, panel_id).exists() {
            return false;
        }
        if self.only_failed {
            return matches!(
                prev.get(panel_id).map(|e| e.status),
                Some(PanelStatus::Error) | Some(PanelStatus::Timeout)
            );
        }
        true
    }
}

/// Path to a panel's result file: `<dir>/data/panel-<id>.json`.
pub fn panel_file(dir: &Path, panel_id: i64) -> PathBuf {
    dir.join(DATA_DIR).join(format!("panel-{panel_id}.json"))
}

/// Execute Stage 2 against `client` for the plan in `audit`, writing
/// results under `dir`. Returns the merged [`DataStatus`].
///
/// `timeout` is the per-panel cap (`None` = unbounded). Non-queryable
/// panels are recorded `skipped`; selected panels are queried and
/// recorded `ok`/`empty`/`error`/`timeout`; non-selected panels keep
/// their previous status + result file untouched.
pub async fn execute(
    client: &dyn GrafanaClient,
    audit: &Audit,
    dir: &Path,
    selection: &Selection,
    timeout: Option<Duration>,
) -> Result<DataStatus, ScrapeError> {
    let data_dir = dir.join(DATA_DIR);
    std::fs::create_dir_all(&data_dir).map_err(|e| ScrapeError::Other(e.to_string()))?;

    // Start from the prior status so non-selected panels are preserved.
    let mut status = read_status(dir).unwrap_or_default();
    let mut ran = 0usize;

    for panel in &audit.panels {
        let Some(body) = &panel.query_body else {
            // Non-queryable / all-hidden: record once, never query.
            status.set(
                panel.id,
                PanelStatusEntry {
                    status: PanelStatus::Skipped,
                    rows: None,
                    ms: None,
                    error: None,
                },
            );
            continue;
        };
        if !selection.wants(panel.id, dir, &status) {
            continue;
        }
        ran += 1;
        let entry = run_one(client, body, &data_dir, panel.id, timeout).await?;
        status.set(panel.id, entry);
    }

    if ran == 0 {
        tracing::warn!("stage data: selection matched no panels to run");
    }
    status.ran_at = Some(chrono::Utc::now());
    write_status(dir, &status)?;
    Ok(status)
}

/// Run a single panel query, persist its result file (on success), and
/// produce the status entry. A transport/datasource failure is recorded
/// as `error`; a timeout as `timeout`; neither aborts the sweep.
async fn run_one(
    client: &dyn GrafanaClient,
    body: &Value,
    data_dir: &Path,
    panel_id: i64,
    timeout: Option<Duration>,
) -> Result<PanelStatusEntry, ScrapeError> {
    let start = Instant::now();
    let outcome = match timeout {
        None => client.ds_query(body).await.map_err(RunErr::Failed),
        Some(dur) => match tokio::time::timeout(dur, client.ds_query(body)).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(RunErr::Failed(e)),
            Err(_) => Err(RunErr::Timeout),
        },
    };
    let ms = start.elapsed().as_millis() as u64;

    match outcome {
        Ok(resp) => {
            write_panel(data_dir, panel_id, &resp)?;
            let (status, rows, error) = classify(&resp);
            Ok(PanelStatusEntry {
                status,
                rows: Some(rows),
                ms: Some(ms),
                error,
            })
        }
        Err(RunErr::Timeout) => {
            tracing::warn!(panel_id, ms, "stage data: panel query timed out");
            Ok(PanelStatusEntry {
                status: PanelStatus::Timeout,
                rows: None,
                ms: Some(ms),
                error: None,
            })
        }
        Err(RunErr::Failed(e)) => {
            tracing::warn!(panel_id, error = %e, "stage data: panel query failed");
            Ok(PanelStatusEntry {
                status: PanelStatus::Error,
                rows: None,
                ms: Some(ms),
                error: Some(e.to_string()),
            })
        }
    }
}

/// Internal split between a timeout and a real query failure.
enum RunErr {
    Timeout,
    Failed(ScrapeError),
}

/// Write one `data/panel-<id>.json` (pretty, so it diffs cleanly).
fn write_panel(data_dir: &Path, panel_id: i64, resp: &Value) -> Result<(), ScrapeError> {
    let path = data_dir.join(format!("panel-{panel_id}.json"));
    let json = serde_json::to_string_pretty(resp)
        .map_err(|e| ScrapeError::Other(format!("serialise panel {panel_id}: {e}")))?;
    std::fs::write(&path, json).map_err(|e| ScrapeError::Other(e.to_string()))
}

/// Read `data-status.json` if present.
pub fn read_status(dir: &Path) -> Option<DataStatus> {
    let path = dir.join(STATUS_FILENAME);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Write `data-status.json` (pretty).
fn write_status(dir: &Path, status: &DataStatus) -> Result<(), ScrapeError> {
    let path = dir.join(STATUS_FILENAME);
    let json = serde_json::to_string_pretty(status)
        .map_err(|e| ScrapeError::Other(format!("serialise data-status: {e}")))?;
    std::fs::write(&path, json).map_err(|e| ScrapeError::Other(e.to_string()))
}

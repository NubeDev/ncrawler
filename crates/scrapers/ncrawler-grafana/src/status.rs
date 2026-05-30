//! Stage-2 per-panel status: the single source of truth shared with the
//! report builder (SCOPE: REPORTS-UPDATE status vocabulary).
//!
//! `data-status.json` is an *index* over `data/panel-<id>.json`: it
//! records how each panel's last `/api/ds/query` went so a selective
//! re-run (`--only-failed`) and the report's "Query status" column can
//! both read one authoritative value instead of re-deriving it.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The closed status vocabulary. Serialised lowercase so the on-disk
/// `data-status.json` reads `"ok"`, `"empty"`, … and the report builder
/// can match the same strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelStatus {
    /// Query succeeded with ≥1 row.
    Ok,
    /// Query succeeded with 0 rows (genuine "no data").
    Empty,
    /// A datasource error frame came back.
    Error,
    /// The query exceeded `--query-timeout`.
    Timeout,
    /// Non-queryable / hidden target — never sent.
    Skipped,
}

impl PanelStatus {
    /// The on-disk / report string for this status.
    pub fn as_str(self) -> &'static str {
        match self {
            PanelStatus::Ok => "ok",
            PanelStatus::Empty => "empty",
            PanelStatus::Error => "error",
            PanelStatus::Timeout => "timeout",
            PanelStatus::Skipped => "skipped",
        }
    }
}

/// One panel's entry in `data-status.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelStatusEntry {
    pub status: PanelStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The whole `data-status.json` document: an index keyed by the numeric
/// panel id (as a string, so it survives a JSON object round-trip).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DataStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ran_at: Option<DateTime<Utc>>,
    pub panels: BTreeMap<String, PanelStatusEntry>,
}

impl DataStatus {
    /// Look up a panel's last status by its numeric id.
    pub fn get(&self, panel_id: i64) -> Option<&PanelStatusEntry> {
        self.panels.get(&panel_id.to_string())
    }

    /// Record (or overwrite) a panel's status.
    pub fn set(&mut self, panel_id: i64, entry: PanelStatusEntry) {
        self.panels.insert(panel_id.to_string(), entry);
    }
}

/// Classify a `/api/ds/query` response body into `(status, rows, error)`.
///
/// The body shape is Grafana's `{ "results": { "<refId>": { … } } }`.
/// A result frame carries its rows in legacy `series` (`points`) or
/// `tables` (`rows`), or the newer `frames[].data.values`. Any frame
/// with an `error` string makes the panel [`PanelStatus::Error`];
/// otherwise ≥1 row is [`PanelStatus::Ok`] and 0 rows is
/// [`PanelStatus::Empty`].
pub fn classify(resp: &Value) -> (PanelStatus, usize, Option<String>) {
    let Some(results) = resp.get("results").and_then(Value::as_object) else {
        return (PanelStatus::Empty, 0, None);
    };
    let mut first_error: Option<String> = None;
    let mut rows = 0usize;
    for frame in results.values() {
        if let Some(err) = frame.get("error").and_then(Value::as_str) {
            if first_error.is_none() {
                first_error = Some(err.to_owned());
            }
            continue;
        }
        rows += count_rows(frame);
    }
    if let Some(err) = first_error {
        return (PanelStatus::Error, rows, Some(err));
    }
    if rows > 0 {
        (PanelStatus::Ok, rows, None)
    } else {
        (PanelStatus::Empty, 0, None)
    }
}

/// Count the rows a single result frame returned across the three shapes
/// Grafana 7.x / 8.x emit.
fn count_rows(frame: &Value) -> usize {
    let mut rows = 0usize;
    // Legacy tables: `[{ "rows": [[…], …] }]`.
    if let Some(tables) = frame.get("tables").and_then(Value::as_array) {
        for t in tables {
            if let Some(r) = t.get("rows").and_then(Value::as_array) {
                rows += r.len();
            }
        }
    }
    // Legacy time-series: `[{ "points": [[v, ts], …] }]`.
    if let Some(series) = frame.get("series").and_then(Value::as_array) {
        for s in series {
            if let Some(p) = s.get("points").and_then(Value::as_array) {
                rows += p.len();
            }
        }
    }
    // Dataframe format: `frames[].data.values[0].len()`.
    if let Some(frames) = frame.get("frames").and_then(Value::as_array) {
        for f in frames {
            if let Some(values) = f
                .get("data")
                .and_then(|d| d.get("values"))
                .and_then(Value::as_array)
            {
                if let Some(col) = values.first().and_then(Value::as_array) {
                    rows += col.len();
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn table_rows_are_ok() {
        let resp = json!({ "results": { "A": {
            "tables": [{ "columns": [{"text":"x"}], "rows": [["Total", 97.192]] }]
        }}});
        let (status, rows, err) = classify(&resp);
        assert_eq!(status, PanelStatus::Ok);
        assert_eq!(rows, 1);
        assert!(err.is_none());
    }

    #[test]
    fn empty_series_is_empty() {
        let resp = json!({ "results": { "C": { "series": [] }}});
        assert_eq!(classify(&resp).0, PanelStatus::Empty);
    }

    #[test]
    fn error_frame_is_error() {
        let resp = json!({ "results": { "A": {
            "error": "relation \"metric_table\" does not exist"
        }}});
        let (status, _, err) = classify(&resp);
        assert_eq!(status, PanelStatus::Error);
        assert_eq!(err.as_deref(), Some("relation \"metric_table\" does not exist"));
    }

    #[test]
    fn series_points_count_as_rows() {
        let resp = json!({ "results": { "C": {
            "series": [{ "name": "a", "points": [[1.0, 10], [2.0, 20]] }]
        }}});
        let (status, rows, _) = classify(&resp);
        assert_eq!(status, PanelStatus::Ok);
        assert_eq!(rows, 2);
    }

    #[test]
    fn dataframe_values_count_as_rows() {
        let resp = json!({ "results": { "A": {
            "frames": [{ "data": { "values": [[1, 2, 3], [10, 20, 30]] } }]
        }}});
        assert_eq!(classify(&resp), (PanelStatus::Ok, 3, None));
    }

    #[test]
    fn lowercase_serde() {
        assert_eq!(
            serde_json::to_string(&PanelStatus::Timeout).unwrap(),
            "\"timeout\""
        );
    }
}

//! Extraction of per-panel SQL and sample rows from scraped data.
//!
//! Two kinds of SQL, never conflated (REPORT §3):
//!
//! - **Template SQL** — the panel target's raw `rawSql` from
//!   `meta.dashboard`. Always present (no query needed), deterministic,
//!   still contains un-resolved `$variables` shown verbatim.
//! - **Executed SQL** — `results.<refId>.meta.executedQueryString` from
//!   the `/api/ds/query` response. Present ONLY where the datasource
//!   exposes it (postgres/mysql yes, `grafana-rubix-os-data-source` no);
//!   it is read from the frozen response and **never invented**.

use serde_json::{Map, Value};

/// Max rows in a sample (REPORT §3: clipped to the smaller of 10 rows /
/// 4 KiB pretty-printed JSON).
const SAMPLE_MAX_ROWS: usize = 10;
/// Max pretty-printed JSON bytes in a sample.
const SAMPLE_MAX_BYTES: usize = 4096;

/// Raw `rawSql` per non-hidden target of a panel, in target order. The
/// text is returned verbatim (un-resolved `$variables` intact); redaction
/// is a later render-time pass.
pub fn template_sql(panel: &Value) -> Vec<String> {
    panel
        .get("targets")
        .and_then(Value::as_array)
        .map(|ts| {
            ts.iter()
                .filter(|t| !t.get("hide").and_then(Value::as_bool).unwrap_or(false))
                .filter_map(|t| t.get("rawSql").and_then(Value::as_str))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Executed SQL strings read from a `/api/ds/query` response, one per
/// refId that actually exposes `executedQueryString`, in sorted refId
/// order. Empty when the datasource does not expose it (NOT faked).
pub fn executed_sql(data: &Value) -> Vec<String> {
    let Some(results) = data.get("results").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut refs: Vec<&String> = results.keys().collect();
    refs.sort();
    let mut out = Vec::new();
    for r in refs {
        if let Some(sql) = result_executed_sql(&results[r]) {
            out.push(sql);
        }
    }
    out
}

/// Pull `executedQueryString` from one result, tolerating both the
/// top-level `meta` shape and the per-frame `schema.meta` shape Grafana
/// 7.x emits.
fn result_executed_sql(result: &Value) -> Option<String> {
    if let Some(s) = result
        .pointer("/meta/executedQueryString")
        .and_then(Value::as_str)
    {
        return Some(s.to_owned());
    }
    result
        .get("frames")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|f| {
            f.pointer("/schema/meta/executedQueryString")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

/// A sample of returned rows from the first refId/frame that carries
/// data, as pretty JSON clipped to the smaller of [`SAMPLE_MAX_ROWS`]
/// rows or [`SAMPLE_MAX_BYTES`]. `None` when the response carries no
/// columnar data.
pub fn sample_rows(data: &Value) -> Option<String> {
    let frame = first_frame(data)?;
    let names = field_names(frame);
    let columns = frame.pointer("/data/values").and_then(Value::as_array)?;
    let row_count = columns
        .iter()
        .filter_map(Value::as_array)
        .map(|c| c.len())
        .max()?;
    let take = row_count.min(SAMPLE_MAX_ROWS);
    let rows: Vec<Value> = (0..take).map(|i| row_object(&names, columns, i)).collect();
    Some(clip(&Value::Array(rows)))
}

/// A single representative value for the panels table's `value` column
/// (only shown under `--data`): the last non-null cell of the last column
/// of the first frame. Deterministic; `None` when there is no data.
pub fn representative_value(data: &Value) -> Option<String> {
    let frame = first_frame(data)?;
    let columns = frame.pointer("/data/values").and_then(Value::as_array)?;
    let col = columns.last()?.as_array()?;
    col.iter().rev().find(|v| !v.is_null()).map(cell_display)
}

/// Render a JSON cell as a compact table-cell string.
fn cell_display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The first frame (sorted refId) that has a `data.values` array.
fn first_frame(data: &Value) -> Option<&Value> {
    let results = data.get("results").and_then(Value::as_object)?;
    let mut refs: Vec<&String> = results.keys().collect();
    refs.sort();
    for r in refs {
        if let Some(frames) = results[r].get("frames").and_then(Value::as_array) {
            if let Some(f) = frames.iter().find(|f| f.pointer("/data/values").is_some()) {
                return Some(f);
            }
        }
    }
    None
}

/// Field/column names of a frame, defaulting to `col{n}` when unnamed.
fn field_names(frame: &Value) -> Vec<String> {
    frame
        .pointer("/schema/fields")
        .and_then(Value::as_array)
        .map(|fs| {
            fs.iter()
                .enumerate()
                .map(|(i, f)| {
                    f.get("name")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("col{i}"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assemble row `i` as a `{ field: value }` object from column arrays.
fn row_object(names: &[String], columns: &[Value], i: usize) -> Value {
    let mut row = Map::new();
    for (c, col) in columns.iter().enumerate() {
        let name = names.get(c).cloned().unwrap_or_else(|| format!("col{c}"));
        let cell = col
            .as_array()
            .and_then(|a| a.get(i))
            .cloned()
            .unwrap_or(Value::Null);
        row.insert(name, cell);
    }
    Value::Object(row)
}

/// Pretty-print `v`, then clip to [`SAMPLE_MAX_BYTES`] on a char boundary
/// with a truncation marker so the output stays valid UTF-8 and bounded.
fn clip(v: &Value) -> String {
    let s = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
    if s.len() <= SAMPLE_MAX_BYTES {
        return s;
    }
    let mut end = SAMPLE_MAX_BYTES;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (clipped to {SAMPLE_MAX_BYTES} bytes)", &s[..end])
}

/// Resolve a panel/target `datasource` reference to a display name using
/// the sidecar datasource list `(name, type, uid, is_default)`. Never
/// invents: an unknown reference is returned verbatim (it may be a
/// `$variable`), and a null reference resolves to the default datasource.
pub fn resolve_ds_name(ds_ref: Option<&Value>, datasources: &[DsEntry]) -> String {
    match ds_ref {
        None | Some(Value::Null) => default_ds(datasources),
        Some(Value::String(s)) if s == "default" || s == "-- Grafana --" => default_ds(datasources),
        Some(Value::String(s)) => datasources
            .iter()
            .find(|d| &d.name == s || d.uid.as_deref() == Some(s))
            .map(|d| d.name.clone())
            .unwrap_or_else(|| s.clone()),
        Some(Value::Object(_)) => resolve_ds_object(ds_ref.unwrap(), datasources),
        Some(other) => other.to_string(),
    }
}

fn resolve_ds_object(obj: &Value, datasources: &[DsEntry]) -> String {
    if let Some(uid) = obj.get("uid").and_then(Value::as_str) {
        if let Some(d) = datasources.iter().find(|d| d.uid.as_deref() == Some(uid)) {
            return d.name.clone();
        }
    }
    if let Some(ty) = obj.get("type").and_then(Value::as_str) {
        if let Some(d) = datasources.iter().find(|d| d.ds_type == ty) {
            return d.name.clone();
        }
        return ty.to_owned();
    }
    default_ds(datasources)
}

fn default_ds(datasources: &[DsEntry]) -> String {
    datasources
        .iter()
        .find(|d| d.is_default)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| "default".to_owned())
}

/// One datasource as recorded in the `_instance` sidecar.
#[derive(Debug, Clone)]
pub struct DsEntry {
    pub name: String,
    pub ds_type: String,
    pub uid: Option<String>,
    pub is_default: bool,
}

impl DsEntry {
    /// Parse the sidecar's `/api/datasources` array. A non-array yields an
    /// empty list (best-effort meta).
    pub fn parse_list(datasources: &Value) -> Vec<DsEntry> {
        datasources
            .as_array()
            .map(|a| a.iter().filter_map(DsEntry::from_row).collect())
            .unwrap_or_default()
    }

    fn from_row(row: &Value) -> Option<DsEntry> {
        let name = row.get("name").and_then(Value::as_str)?.to_owned();
        Some(DsEntry {
            name,
            ds_type: row
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            uid: row.get("uid").and_then(Value::as_str).map(str::to_owned),
            is_default: row
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

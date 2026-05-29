//! Client-side template-variable interpolation.
//!
//! Grafana expands dashboard variables (`$var`, `${var}`, `${var:fmt}`,
//! `[[var]]`) and the built-in time range (`${__from}` / `${__to}`) in
//! the browser BEFORE a panel query is POSTed to `/api/ds/query`; the
//! backend never sees them. So to reproduce a panel's real query we must
//! perform that substitution ourselves.
//!
//! Datasource SQL macros (`$__timeFilter`, `$__time`, `$__timeGroup`,
//! `$__interval`, `$__unixEpoch*`, ...) are the EXCEPTION: those are
//! expanded server-side by the datasource plugin, so we deliberately
//! leave them untouched. Unknown `$name` references are likewise left
//! verbatim rather than blanked, so a miss is visible in the executed
//! query instead of silently corrupting it.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

/// A resolved dashboard variable: a scalar, or a multi-value selection.
#[derive(Debug, Clone)]
pub enum VarValue {
    Single(String),
    Multi(Vec<String>),
}

/// The interpolation context for one dashboard scrape.
pub struct Interpolator {
    vars: HashMap<String, VarValue>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
}

impl Interpolator {
    /// Build the variable map from a dashboard's `templating.list`,
    /// resolving each variable's current value:
    ///
    /// - `constant` / `textbox`: the `query` field holds the value.
    /// - everything else (`custom`, `query`, `interval`, ...): the
    ///   `current.value` (a string or an array for multi-value), falling
    ///   back to `query` when no selection is recorded.
    pub fn new(dashboard: &Value, from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        let mut vars = HashMap::new();
        let list = dashboard
            .get("dashboard")
            .and_then(|d| d.get("templating"))
            .and_then(|t| t.get("list"))
            .and_then(Value::as_array);
        if let Some(list) = list {
            for v in list {
                let Some(name) = v.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
                let value = match ty {
                    "constant" | "textbox" => v.get("query").map(value_to_var),
                    _ => v
                        .get("current")
                        .and_then(|c| c.get("value"))
                        .map(value_to_var)
                        .or_else(|| v.get("query").map(value_to_var)),
                };
                if let Some(value) = value {
                    vars.insert(name.to_owned(), value);
                }
            }
        }
        Self { vars, from, to }
    }

    /// Interpolate every string leaf of a JSON query target in place,
    /// returning a new value. Non-string leaves are untouched.
    pub fn interpolate_value(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.interpolate(s)),
            Value::Array(a) => Value::Array(a.iter().map(|v| self.interpolate_value(v)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), self.interpolate_value(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// Interpolate one string. Recognises, in scan order:
    /// `${name[.idx][:fmt]}`, `[[name[:fmt]]]`, and bare `$name`.
    pub fn interpolate(&self, input: &str) -> String {
        let bytes = input.as_bytes();
        let mut out = String::with_capacity(input.len());
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if c == b'$' && bytes.get(i + 1) == Some(&b'{') {
                if let Some((end, body)) = read_delimited(input, i + 2, '}') {
                    match self.expand_braced(body) {
                        Some(rep) => out.push_str(&rep),
                        None => out.push_str(&input[i..end]),
                    }
                    i = end;
                    continue;
                }
            } else if c == b'[' && bytes.get(i + 1) == Some(&b'[') {
                if let Some((end, body)) = read_delimited(input, i + 2, ']') {
                    // `[[name]]` and `[[name:fmt]]` — same grammar as the
                    // braced form, just a different delimiter, and the
                    // closing `]]` is two bytes.
                    if input[end..].starts_with(']') {
                        if let Some(rep) = self.expand_braced(body) {
                            out.push_str(&rep);
                            i = end + 1;
                            continue;
                        }
                    }
                }
            } else if c == b'$' {
                if let Some((end, name)) = read_ident(input, i + 1) {
                    match self.expand_name(name, None, None) {
                        Some(rep) => out.push_str(&rep),
                        None => out.push_str(&input[i..end]),
                    }
                    i = end;
                    continue;
                }
            }
            // Default: copy this byte (safe — we only branch on ASCII).
            out.push(c as char);
            i += 1;
        }
        out
    }

    /// Expand the inside of a `${...}` / `[[...]]`: `name`, `name:fmt`,
    /// `name.idx`, or `name.idx:fmt`.
    fn expand_braced(&self, body: &str) -> Option<String> {
        let (name_part, fmt) = match body.split_once(':') {
            Some((n, f)) => (n, Some(f)),
            None => (body, None),
        };
        let (name, idx) = match name_part.split_once('.') {
            Some((n, i)) => (n, i.parse::<usize>().ok()),
            None => (name_part, None),
        };
        self.expand_name(name, idx, fmt)
    }

    /// Resolve `name` (optionally an index into a multi-value, optionally
    /// formatted). Built-in `__from` / `__to` are handled first; other
    /// `__`-prefixed macros are left for the datasource backend.
    fn expand_name(&self, name: &str, idx: Option<usize>, fmt: Option<&str>) -> Option<String> {
        if name == "__from" || name == "__to" {
            let t = if name == "__from" { self.from } else { self.to };
            return Some(format_time(t, fmt));
        }
        if name.starts_with("__") {
            return None; // server-side SQL macro; leave verbatim.
        }
        let var = self.vars.get(name)?;
        let selected = match (var, idx) {
            (VarValue::Multi(vs), Some(i)) => VarValue::Single(vs.get(i)?.clone()),
            _ => var.clone(),
        };
        Some(format_var(&selected, fmt))
    }
}

/// Coerce a templating value (string, number, or array) into a [`VarValue`].
fn value_to_var(v: &Value) -> VarValue {
    match v {
        Value::Array(a) => VarValue::Multi(a.iter().map(scalar_string).collect()),
        other => VarValue::Single(scalar_string(other)),
    }
}

/// Render a JSON scalar as the bare string Grafana would substitute.
fn scalar_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Apply a Grafana format specifier to a resolved variable. The default
/// (no specifier) mirrors Grafana: a scalar substitutes raw; a
/// multi-value joins with commas.
fn format_var(value: &VarValue, fmt: Option<&str>) -> String {
    let values: Vec<&str> = match value {
        VarValue::Single(s) => vec![s.as_str()],
        VarValue::Multi(vs) => vs.iter().map(String::as_str).collect(),
    };
    match fmt {
        None | Some("raw") => values.join(","),
        Some("csv") => values.join(","),
        Some("pipe") => values.join("|"),
        Some("singlequote") | Some("sqlstring") => values
            .iter()
            .map(|v| format!("'{}'", v.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(","),
        Some("doublequote") => values
            .iter()
            .map(|v| format!("\"{}\"", v.replace('"', "\\\"")))
            .collect::<Vec<_>>()
            .join(","),
        Some("json") => Value::Array(
            values
                .iter()
                .map(|v| Value::String(v.to_string()))
                .collect(),
        )
        .to_string(),
        Some("glob") => {
            if values.len() == 1 {
                values[0].to_owned()
            } else {
                format!("{{{}}}", values.join(","))
            }
        }
        Some("regex") => {
            if values.len() == 1 {
                values[0].to_owned()
            } else {
                format!("({})", values.join("|"))
            }
        }
        // Unknown specifier: fall back to the raw join rather than drop it.
        Some(_) => values.join(","),
    }
}

/// Format a built-in time variable. `${__from}` → epoch milliseconds;
/// `${__from:date}` (and any `:date:*` form) → ISO-8601, matching
/// Grafana's `moment.toISOString()` default.
fn format_time(t: DateTime<Utc>, fmt: Option<&str>) -> String {
    match fmt {
        None => t.timestamp_millis().to_string(),
        // `date`, `date:iso`, and custom moment tokens we don't parse all
        // resolve to ISO-8601 (the panels that drive data queries use the
        // plain `:date` form; custom moment formats appear only in display
        // strings).
        Some(_) => t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
    }
}

/// Read until the unescaped `close` delimiter starting at `start`.
/// Returns `(index just past close, inner body)`.
fn read_delimited(input: &str, start: usize, close: char) -> Option<(usize, &str)> {
    let rest = &input[start..];
    let pos = rest.find(close)?;
    Some((start + pos + close.len_utf8(), &rest[..pos]))
}

/// Read a bare `$name` identifier (`[A-Za-z0-9_]+`) starting at `start`.
fn read_ident(input: &str, start: usize) -> Option<(usize, &str)> {
    let bytes = input.as_bytes();
    let mut j = start;
    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
        j += 1;
    }
    if j == start {
        None
    } else {
        Some((j, &input[start..j]))
    }
}

/// Parse a Grafana time-range bound (`now`, `now-6h`, `now-1d`, an epoch
/// millisecond string, ...) into an absolute instant relative to `anchor`.
/// Unrecognised input falls back to `anchor`.
pub fn parse_time(spec: &str, anchor: DateTime<Utc>) -> DateTime<Utc> {
    let s = spec.trim();
    if s == "now" {
        return anchor;
    }
    if let Some(rest) = s.strip_prefix("now-") {
        if let Some(d) = parse_duration(rest) {
            return anchor - d;
        }
    }
    if let Some(rest) = s.strip_prefix("now+") {
        if let Some(d) = parse_duration(rest) {
            return anchor + d;
        }
    }
    if let Ok(ms) = s.parse::<i64>() {
        if let Some(dt) = DateTime::from_timestamp_millis(ms) {
            return dt;
        }
    }
    anchor
}

/// `<n><unit>` where unit ∈ s,m,h,d,w,M,y (calendar units approximated:
/// w=7d, M=30d, y=365d — good enough for a query window bound).
fn parse_duration(s: &str) -> Option<Duration> {
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    let n: i64 = s[..split].parse().ok()?;
    let unit = &s[split..];
    let d = match unit {
        "s" => Duration::seconds(n),
        "m" => Duration::minutes(n),
        "h" => Duration::hours(n),
        "d" => Duration::days(n),
        "w" => Duration::days(n * 7),
        "M" => Duration::days(n * 30),
        "y" => Duration::days(n * 365),
        _ => return None,
    };
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn interp_with(dash: Value) -> Interpolator {
        let anchor = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        Interpolator::new(&dash, anchor - Duration::hours(6), anchor)
    }

    fn dash() -> Value {
        json!({ "dashboard": { "templating": { "list": [
            { "name": "net", "type": "constant", "query": "Wattwatchers" },
            { "name": "hosts", "type": "custom", "multi": true,
              "current": { "value": ["a", "b"] }, "query": "a,b" },
            { "name": "keys", "type": "custom", "multi": true,
              "current": { "value": ["site", "building"] } },
        ]}}})
    }

    #[test]
    fn constant_and_braced() {
        let i = interp_with(dash());
        assert_eq!(i.interpolate("net = '${net}'"), "net = 'Wattwatchers'");
        assert_eq!(i.interpolate("net = $net"), "net = Wattwatchers");
    }

    #[test]
    fn multi_value_singlequote_and_default() {
        let i = interp_with(dash());
        assert_eq!(i.interpolate("IN (${hosts:singlequote})"), "IN ('a','b')");
        assert_eq!(i.interpolate("${hosts}"), "a,b");
        assert_eq!(i.interpolate("${hosts:pipe}"), "a|b");
    }

    #[test]
    fn indexed_multi_value() {
        let i = interp_with(dash());
        assert_eq!(
            i.interpolate("${keys.0:raw} ${keys.1:raw}"),
            "site building"
        );
    }

    #[test]
    fn singlequote_escapes_quotes() {
        let dash = json!({ "dashboard": { "templating": { "list": [
            { "name": "x", "type": "constant", "query": "O'Brien" },
        ]}}});
        let i = interp_with(dash);
        assert_eq!(i.interpolate("${x:singlequote}"), "'O''Brien'");
    }

    #[test]
    fn unknown_var_and_sql_macro_left_verbatim() {
        let i = interp_with(dash());
        assert_eq!(i.interpolate("$__timeFilter(t)"), "$__timeFilter(t)");
        assert_eq!(i.interpolate("${nope}"), "${nope}");
        assert_eq!(i.interpolate("$nope"), "$nope");
    }

    #[test]
    fn builtin_time_range() {
        let i = interp_with(dash());
        // `:date` → ISO-8601; bare → epoch ms.
        assert_eq!(i.interpolate("${__to:date}"), "2023-11-14T22:13:20.000Z");
        assert_eq!(i.interpolate("$__to"), "1700000000000");
    }

    #[test]
    fn interpolate_value_recurses() {
        let i = interp_with(dash());
        let got = i.interpolate_value(&json!({ "rawSql": "x = '${net}'", "refId": "A" }));
        assert_eq!(got["rawSql"], "x = 'Wattwatchers'");
        assert_eq!(got["refId"], "A");
    }

    #[test]
    fn parse_time_relative() {
        let anchor = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        assert_eq!(parse_time("now", anchor), anchor);
        assert_eq!(parse_time("now-6h", anchor), anchor - Duration::hours(6));
        assert_eq!(parse_time("now-1d", anchor), anchor - Duration::days(1));
        assert_eq!(parse_time("garbage", anchor), anchor);
    }
}

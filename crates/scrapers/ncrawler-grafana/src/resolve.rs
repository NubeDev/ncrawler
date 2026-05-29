//! Datasource reference → numeric `datasourceId` resolution.
//!
//! Grafana 7.x's `/api/ds/query` requires each query to carry the
//! numeric `datasourceId` of its datasource; a dashboard panel only
//! records a *reference* — `null` (the org default), a name string, a
//! `$variable`, or an object `{ "uid": "..." }`. This maps the
//! `GET /api/datasources` listing into those lookups.

use serde_json::Value;

use crate::interp::Interpolator;

/// One configured datasource, reduced to the fields we resolve against.
struct Ds {
    id: i64,
    uid: Option<String>,
    name: String,
}

/// Resolver built from a `GET /api/datasources` response.
pub struct DatasourceResolver {
    sources: Vec<Ds>,
    default: Option<usize>,
}

impl DatasourceResolver {
    /// Build from the datasources array; tolerates a non-array body
    /// (treated as empty).
    pub fn new(datasources: &Value) -> Self {
        let mut sources = Vec::new();
        let mut default = None;
        if let Some(arr) = datasources.as_array() {
            for d in arr {
                let Some(id) = d.get("id").and_then(Value::as_i64) else {
                    continue;
                };
                let name = d
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let uid = d.get("uid").and_then(Value::as_str).map(str::to_owned);
                if d.get("isDefault").and_then(Value::as_bool).unwrap_or(false) {
                    default = Some(sources.len());
                }
                sources.push(Ds { id, uid, name });
            }
        }
        Self { sources, default }
    }

    /// An empty resolver — every `resolve` returns `None` (used when the
    /// datasource list could not be fetched).
    pub fn empty() -> Self {
        Self {
            sources: Vec::new(),
            default: None,
        }
    }

    /// Resolve a panel/target `datasource` reference to `(id, name)`.
    /// `None` (or `null`) selects the org default; a `$variable` name is
    /// interpolated first. Built-in pseudo-datasources (`-- Mixed --`,
    /// `-- Dashboard --`, `-- Grafana --`) and unknown references yield
    /// `None`.
    pub fn resolve(
        &self,
        reference: Option<&Value>,
        interp: &Interpolator,
    ) -> Option<(i64, String)> {
        match reference {
            None | Some(Value::Null) => self.default_ds(),
            Some(Value::String(s)) => self.resolve_str(s, interp),
            Some(Value::Object(o)) => {
                if let Some(uid) = o.get("uid").and_then(Value::as_str) {
                    // A `{ "uid": "$ds" }` reference may itself be a variable.
                    let uid = interp.interpolate(uid);
                    self.by_uid(&uid).or_else(|| self.resolve_str(&uid, interp))
                } else {
                    self.default_ds()
                }
            }
            _ => None,
        }
    }

    fn resolve_str(&self, raw: &str, interp: &Interpolator) -> Option<(i64, String)> {
        let s = interp.interpolate(raw);
        match s.as_str() {
            "default" => self.default_ds(),
            // Layout/aggregate pseudo-datasources have no single id.
            name if name.starts_with("-- ") => None,
            name => self.by_name(name).or_else(|| self.by_uid(name)),
        }
    }

    fn default_ds(&self) -> Option<(i64, String)> {
        self.default.map(|i| {
            let d = &self.sources[i];
            (d.id, d.name.clone())
        })
    }

    fn by_name(&self, name: &str) -> Option<(i64, String)> {
        self.sources
            .iter()
            .find(|d| d.name == name)
            .map(|d| (d.id, d.name.clone()))
    }

    fn by_uid(&self, uid: &str) -> Option<(i64, String)> {
        self.sources
            .iter()
            .find(|d| d.uid.as_deref() == Some(uid))
            .map(|d| (d.id, d.name.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Duration};
    use serde_json::json;

    fn interp() -> Interpolator {
        let anchor = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        Interpolator::new(
            &json!({ "dashboard": { "templating": { "list": [
                { "name": "ds", "type": "constant", "query": "PostgreSQL" }
            ]}}}),
            anchor - Duration::hours(6),
            anchor,
        )
    }

    fn resolver() -> DatasourceResolver {
        DatasourceResolver::new(&json!([
            { "id": 1, "uid": "pg-uid", "name": "PostgreSQL", "type": "postgres", "isDefault": true },
            { "id": 2, "uid": "rx-uid", "name": "Rubix", "type": "rubix", "isDefault": false },
        ]))
    }

    #[test]
    fn null_and_none_select_default() {
        let r = resolver();
        assert_eq!(r.resolve(None, &interp()), Some((1, "PostgreSQL".into())));
        assert_eq!(
            r.resolve(Some(&Value::Null), &interp()),
            Some((1, "PostgreSQL".into()))
        );
    }

    #[test]
    fn by_name_and_uid_object() {
        let r = resolver();
        assert_eq!(
            r.resolve(Some(&json!("Rubix")), &interp()),
            Some((2, "Rubix".into()))
        );
        assert_eq!(
            r.resolve(Some(&json!({ "uid": "rx-uid" })), &interp()),
            Some((2, "Rubix".into()))
        );
    }

    #[test]
    fn variable_name_is_interpolated() {
        let r = resolver();
        assert_eq!(
            r.resolve(Some(&json!("$ds")), &interp()),
            Some((1, "PostgreSQL".into()))
        );
    }

    #[test]
    fn pseudo_and_unknown_yield_none() {
        let r = resolver();
        assert_eq!(r.resolve(Some(&json!("-- Mixed --")), &interp()), None);
        assert_eq!(r.resolve(Some(&json!("Nope")), &interp()), None);
    }

    #[test]
    fn empty_resolver_resolves_nothing() {
        let r = DatasourceResolver::empty();
        assert_eq!(r.resolve(None, &interp()), None);
    }
}

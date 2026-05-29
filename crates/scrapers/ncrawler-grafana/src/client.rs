//! The single file that knows about the `grafana` crate.
//!
//! `api.rs`, `visual.rs`, and `merge.rs` see Grafana solely through the
//! [`GrafanaClient`] trait defined here, so replacing the pinned
//! `grafana = =0.1.3` dependency with hand-rolled `reqwest` later is a
//! one-file change (SCOPE: isolation).

use async_trait::async_trait;
use grafana::{Auth, Client};
use serde_json::{json, Value};
use starter_spi::secrets::{Secret, SecretStore};

use ncrawler_spi::ScrapeError;

/// The replaceable seam over the Grafana HTTP API. Everything outside
/// this file depends on this trait, never on the `grafana` crate.
#[async_trait]
pub trait GrafanaClient: Send + Sync {
    /// `GET /api/dashboards/uid/{uid}` — returns `{ "meta", "dashboard" }`.
    async fn dashboard_by_uid(&self, uid: &str) -> Result<Value, ScrapeError>;

    /// `POST /api/ds/query` — panel query, returns the raw response body.
    async fn ds_query(&self, body: &Value) -> Result<Value, ScrapeError>;

    /// `GET /api/search` — dashboard/folder discovery.
    async fn search(&self) -> Result<Value, ScrapeError>;

    /// `GET /api/annotations` — annotation list.
    async fn annotations(&self) -> Result<Value, ScrapeError>;
}

/// Resolve the Grafana bearer token, newest source first:
///
/// 1. `SecretStore` keyed `ncrawler:grafana:<host>:token`.
/// 2. `GRAFANA_TOKEN` env fallback.
///
/// The returned [`Secret`] redacts itself on `Debug`/`Display`; callers
/// MUST NOT log the exposed value (SCOPE: tokens never logged).
pub fn resolve_token(host: &str, store: Option<&dyn SecretStore>) -> Option<Secret> {
    let key = format!("ncrawler:grafana:{host}:token");
    if let Some(store) = store {
        if let Ok(Some(secret)) = store.get(&key) {
            return Some(secret);
        }
    }
    std::env::var("GRAFANA_TOKEN").ok().map(Secret::new)
}

/// The production [`GrafanaClient`], backed by the `grafana` crate.
pub struct GrafanaCrateClient {
    inner: Client,
}

impl GrafanaCrateClient {
    /// Build a client for `base_url`, authenticating with `token` when
    /// present. The token is moved straight into the `grafana` crate's
    /// auth header and never logged.
    pub fn new(base_url: &str, token: Option<&Secret>) -> Result<Self, ScrapeError> {
        let mut builder = Client::builder(base_url)
            .map_err(map_err)?
            // The `grafana` crate retries internally; we want a single
            // shot so wiremock expectations stay deterministic and a
            // 5xx/timeout surfaces promptly to the operator.
            .max_retries(0);
        if let Some(token) = token {
            builder = builder.auth(Auth::bearer(token.expose()));
        }
        Ok(Self {
            inner: builder.build().map_err(map_err)?,
        })
    }
}

#[async_trait]
impl GrafanaClient for GrafanaCrateClient {
    async fn dashboard_by_uid(&self, uid: &str) -> Result<Value, ScrapeError> {
        let resp = self
            .inner
            .dashboards()
            .get_by_uid(uid.to_owned())
            .await
            .map_err(map_err)?;
        Ok(json!({ "meta": resp.meta, "dashboard": resp.dashboard }))
    }

    async fn ds_query(&self, body: &Value) -> Result<Value, ScrapeError> {
        // Primary path: the generated OpenAPI wrapper for `POST /ds/query`.
        match self
            .inner
            .openapi()
            .query_metrics_with_expressions::<Value, Value>(body)
            .await
        {
            Ok(value) => Ok(value),
            // `client.raw()` fallback (SCOPE): a datasource-specific
            // payload the generated wrapper rejects can be hand-shaped
            // and POSTed verbatim to `/ds/query`. We only fall back on
            // API-shape errors, not auth/not-found, so a 401 still
            // surfaces as an auth error rather than being masked.
            Err(err) if is_api_shape_error(&err) => self
                .inner
                .raw()
                .request_json::<Value, (), Value>(
                    http::Method::POST,
                    &["ds", "query"],
                    None,
                    Some(body),
                )
                .await
                .map_err(map_err),
            Err(err) => Err(map_err(err)),
        }
    }

    async fn search(&self) -> Result<Value, ScrapeError> {
        self.inner
            .openapi()
            .search::<Value>(None)
            .await
            .map_err(map_err)
    }

    async fn annotations(&self) -> Result<Value, ScrapeError> {
        self.inner
            .openapi()
            .get_annotations::<Value>(None)
            .await
            .map_err(map_err)
    }
}

/// Does this error look like a payload-shape mismatch (worth the
/// `raw()` retry) rather than auth/not-found/transport?
fn is_api_shape_error(err: &grafana::Error) -> bool {
    matches!(err, grafana::Error::Api(_) | grafana::Error::Decode { .. })
}

/// Map the `grafana` crate's error onto our phase-level [`ScrapeError`].
/// `Decode` (malformed JSON) becomes `Other` so callers can distinguish
/// it from `Auth`/`NotFound`.
fn map_err(err: grafana::Error) -> ScrapeError {
    match err {
        grafana::Error::Auth(_) => ScrapeError::Auth(err.to_string()),
        grafana::Error::NotFound(_) => ScrapeError::NotFound(err.to_string()),
        grafana::Error::Transport { .. } => ScrapeError::Network(err.to_string()),
        grafana::Error::Decode { .. } => ScrapeError::Other(format!("malformed response: {err}")),
        other => ScrapeError::Other(other.to_string()),
    }
}

//! Multi-dashboard `mode = Api` scrape-loop tests (REPORT §8 step 3).
//!
//! - `multi_loop_*`: wiremock-backed fan-out over a mixed inventory
//!   (200 / 401 / 404 / malformed JSON) asserting siblings survive a
//!   per-dashboard failure and the `_instance` sidecar is written once.
//! - `concurrency_cap_limits`: a mock [`GrafanaClient`] proving the token
//!   bucket never lets more than `concurrency` dashboards run at once.
//! - `sidecar_skipped_when_fresh`: a fresh on-disk sidecar is reused, not
//!   refetched.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use ncrawler_core::{ArtifactStore, InstanceSidecar};
use ncrawler_grafana::client::{GrafanaClient, GrafanaCrateClient};
use ncrawler_grafana::{multi, scrape_selection, DashboardSelector, MultiConfig, SidecarOutcome};
use ncrawler_spi::ScrapeError;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn now() -> chrono::DateTime<chrono::Utc> {
    "2026-05-29T00:00:00Z".parse().unwrap()
}

/// A [`Cancel`] that is never cancelled.
struct NeverCancel;
impl ncrawler_spi::Cancel for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn cancelled<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(std::future::pending())
    }
}

// ---------------------------------------------------------------------------
// wiremock: mixed-status multi-dashboard loop
// ---------------------------------------------------------------------------

/// Mount the four instance-sidecar endpoints with `ok` bodies so
/// `ensure_sidecar` can write a sidecar.
async fn mount_sidecar_endpoints(server: &MockServer) {
    for (p, body) in [
        ("/api/datasources", json!([])),
        ("/api/folders", json!([])),
        ("/api/health", json!({ "version": "7.5.17" })),
        (
            "/api/frontend/settings",
            json!({ "buildInfo": { "version": "7.5.17", "edition": "OSS" }, "rendererAvailable": false }),
        ),
        ("/api/annotations", json!([])),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }
}

#[tokio::test]
async fn multi_loop_one_failure_does_not_abort_siblings() {
    let server = MockServer::start().await;

    // Inventory of four dashboards: one each of 200 / 401 / 404 / malformed.
    let inventory = json!([
        { "uid": "ok",        "title": "Healthy" },
        { "uid": "bad401",    "title": "Forbidden" },
        { "uid": "missing404","title": "Gone" },
        { "uid": "malformed", "title": "Garbled" },
    ]);
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(inventory))
        .mount(&server)
        .await;
    mount_sidecar_endpoints(&server).await;

    // Per-uid dashboard responses (no panels → no /api/ds/query needed).
    Mock::given(method("GET"))
        .and(path("/api/dashboards/uid/ok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "meta": {}, "dashboard": { "uid": "ok", "title": "Healthy", "panels": [] }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/dashboards/uid/bad401"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/dashboards/uid/missing404"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/dashboards/uid/malformed"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{ not json"))
        .mount(&server)
        .await;

    let client = GrafanaCrateClient::new(&server.uri(), None).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(tmp.path());
    let selector = DashboardSelector::from_args(&["--all".to_owned()]).unwrap();
    let options = json!({ "url": "http://grafana.local", "mode": "api" });

    let summary = scrape_selection(
        &client,
        &store,
        "grafana.local",
        &selector,
        &options,
        &[],
        now(),
        &MultiConfig::default(),
        &NeverCancel,
    )
    .await
    .expect("the loop itself succeeds even when some dashboards fail");

    // Exactly the healthy dashboard succeeded; the other three are
    // collected as failures without aborting it.
    assert_eq!(summary.succeeded, vec!["ok".to_owned()]);
    let failed: Vec<&str> = summary.failed.iter().map(|f| f.uid.as_str()).collect();
    assert_eq!(failed, vec!["bad401", "malformed", "missing404"]);

    // The successful dashboard was actually persisted to the store.
    assert!(store.latest_link("grafana", "ok").exists());
    // The sidecar was written once.
    assert_eq!(summary.sidecar, SidecarOutcome::Written);
    assert!(store
        .read_instance_sidecar("grafana", "grafana.local")
        .unwrap()
        .is_some());
}

// ---------------------------------------------------------------------------
// mock client: concurrency cap + sidecar skip
// ---------------------------------------------------------------------------

/// A mock [`GrafanaClient`] that (a) tracks the peak number of concurrent
/// `dashboard_by_uid` calls in flight and (b) counts the sidecar-only
/// endpoints so a skip can be proven.
struct MockClient {
    inventory: Value,
    in_flight: AtomicUsize,
    peak: AtomicUsize,
    folders_calls: AtomicUsize,
    health_calls: AtomicUsize,
    /// When set, `dashboard_by_uid` waits on this barrier (after bumping
    /// the in-flight counter) to force the configured concurrency to
    /// actually overlap before any task releases its permit.
    barrier: Option<Arc<tokio::sync::Barrier>>,
}

impl MockClient {
    fn new(inventory: Value) -> Self {
        Self {
            inventory,
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            folders_calls: AtomicUsize::new(0),
            health_calls: AtomicUsize::new(0),
            barrier: None,
        }
    }
}

#[async_trait]
impl GrafanaClient for MockClient {
    async fn dashboard_by_uid(&self, uid: &str) -> Result<Value, ScrapeError> {
        let cur = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(cur, Ordering::SeqCst);
        if let Some(b) = &self.barrier {
            // Block until a full wave of `concurrency` tasks has arrived,
            // guaranteeing the peak counter observes the real overlap.
            b.wait().await;
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(json!({ "meta": {}, "dashboard": { "uid": uid, "panels": [] } }))
    }

    async fn ds_query(&self, _body: &Value) -> Result<Value, ScrapeError> {
        Ok(json!({ "results": {} }))
    }

    async fn search(&self) -> Result<Value, ScrapeError> {
        Ok(self.inventory.clone())
    }

    async fn datasources(&self) -> Result<Value, ScrapeError> {
        Ok(json!([]))
    }

    async fn annotations(&self) -> Result<Value, ScrapeError> {
        Ok(json!([]))
    }

    async fn folders(&self) -> Result<Value, ScrapeError> {
        self.folders_calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!([]))
    }

    async fn health(&self) -> Result<Value, ScrapeError> {
        self.health_calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({ "version": "7.5.17" }))
    }

    async fn frontend_settings(&self) -> Result<Value, ScrapeError> {
        Ok(json!({ "rendererAvailable": false }))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrency_cap_limits() {
    const CAP: usize = 2;
    const N: usize = 6; // a multiple of CAP so every wave is full

    let inventory: Vec<Value> = (0..N)
        .map(|i| json!({ "uid": format!("d{i}"), "title": format!("Dash {i}") }))
        .collect();
    let mut client = MockClient::new(Value::Array(inventory));
    // A barrier sized to the cap: exactly `CAP` permits exist, so exactly
    // `CAP` tasks reach the barrier together and release as a wave. If the
    // limiter were broken and let more through, `peak` would exceed CAP.
    client.barrier = Some(Arc::new(tokio::sync::Barrier::new(CAP)));
    let client = Arc::new(client);

    let tmp = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(tmp.path());
    let selector = DashboardSelector::from_args(&["--all".to_owned()]).unwrap();
    let options = json!({ "url": "http://grafana.local", "mode": "api" });

    let config = MultiConfig {
        concurrency: CAP,
        ..MultiConfig::default()
    };
    let summary = scrape_selection(
        client.as_ref(),
        &store,
        "grafana.local",
        &selector,
        &options,
        &[],
        now(),
        &config,
        &NeverCancel,
    )
    .await
    .unwrap();

    assert_eq!(summary.succeeded.len(), N, "every dashboard persisted");
    let peak = client.peak.load(Ordering::SeqCst);
    assert!(peak <= CAP, "limiter let {peak} run at once, cap is {CAP}");
    assert_eq!(
        peak, CAP,
        "the limiter should saturate the cap, not serialize"
    );
}

#[tokio::test]
async fn sidecar_skipped_when_fresh() {
    let inventory = json!([{ "uid": "d0", "title": "Only" }]);
    let client = MockClient::new(inventory);

    let tmp = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(tmp.path());

    // Pre-write a sidecar that is 10 minutes old.
    let mut sidecar = InstanceSidecar::new("grafana.local", now() - chrono::Duration::minutes(10));
    sidecar.search = json!([{ "uid": "d0", "title": "Only" }]);
    store.write_instance("grafana", &sidecar).unwrap();

    let selector = DashboardSelector::from_args(&["--all".to_owned()]).unwrap();
    let options = json!({ "url": "http://grafana.local", "mode": "api" });
    // 1h freshness window → the 10-minute-old sidecar is reused.
    let config = MultiConfig {
        sidecar_max_age: chrono::Duration::seconds(multi::DEFAULT_SIDECAR_MAX_AGE_SECS),
        ..MultiConfig::default()
    };

    let summary = scrape_selection(
        &client,
        &store,
        "grafana.local",
        &selector,
        &options,
        &[],
        now(),
        &config,
        &NeverCancel,
    )
    .await
    .unwrap();

    assert_eq!(summary.sidecar, SidecarOutcome::SkippedFresh);
    // The sidecar-only endpoints (folders/health) were NOT refetched.
    assert_eq!(client.folders_calls.load(Ordering::SeqCst), 0);
    assert_eq!(client.health_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sidecar_refreshed_when_stale() {
    let inventory = json!([{ "uid": "d0", "title": "Only" }]);
    let client = MockClient::new(inventory);

    let tmp = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(tmp.path());

    // Pre-write a sidecar 2h old; the default 1h window makes it stale.
    let mut sidecar = InstanceSidecar::new("grafana.local", now() - chrono::Duration::hours(2));
    sidecar.search = json!([]);
    store.write_instance("grafana", &sidecar).unwrap();

    let selector = DashboardSelector::from_args(&["--all".to_owned()]).unwrap();
    let options = json!({ "url": "http://grafana.local", "mode": "api" });

    let summary = scrape_selection(
        &client,
        &store,
        "grafana.local",
        &selector,
        &options,
        &[],
        now(),
        &MultiConfig::default(),
        &NeverCancel,
    )
    .await
    .unwrap();

    assert_eq!(summary.sidecar, SidecarOutcome::Refreshed);
    assert_eq!(client.folders_calls.load(Ordering::SeqCst), 1);
    assert_eq!(client.health_calls.load(Ordering::SeqCst), 1);
}

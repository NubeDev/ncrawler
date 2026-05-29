//! wiremock-backed tests for the per-instance sidecar (REPORT §6a):
//! fetching the four instance endpoints into a sidecar, SSRF gating,
//! writing/reading it through the shared `ncrawler-core` store, and the
//! legacy `meta.search` fallback + mixed-store scenario.
//!
//! No live network: `wiremock` serves canned responses on localhost.

use ncrawler_core::{ArtifactStore, FactsOrigin};
use ncrawler_grafana::client::GrafanaCrateClient;
use ncrawler_grafana::instance;
use ncrawler_spi::{Artifact, ScrapeError};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> GrafanaCrateClient {
    GrafanaCrateClient::new(&server.uri(), None).expect("client builds")
}

fn now() -> chrono::DateTime<chrono::Utc> {
    "2026-05-29T14:22:01Z".parse().unwrap()
}

async fn mount_ok(server: &MockServer, p: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// Mount every instance endpoint with healthy bodies. `ds_url` is the
/// datasource `url` surfaced by `/api/datasources` (drives SSRF tests).
async fn mount_instance(server: &MockServer, ds_url: &str) {
    mount_ok(
        server,
        "/api/search",
        json!([{ "uid": "abc", "title": "Site Summary", "folderTitle": "Sites", "tags": ["site"] }]),
    )
    .await;
    mount_ok(
        server,
        "/api/datasources",
        json!([{ "id": 1, "name": "PostgreSQL", "type": "postgres", "isDefault": true, "url": ds_url }]),
    )
    .await;
    mount_ok(
        server,
        "/api/folders",
        json!([{ "uid": "f1", "title": "Sites" }]),
    )
    .await;
    mount_ok(
        server,
        "/api/health",
        json!({ "database": "ok", "version": "7.5.17" }),
    )
    .await;
    mount_ok(
        server,
        "/api/frontend/settings",
        json!({
            "buildInfo": { "version": "7.5.17", "edition": "Open Source" },
            "rendererAvailable": false
        }),
    )
    .await;
}

#[tokio::test]
async fn fetch_assembles_sidecar_from_all_endpoints() {
    let server = MockServer::start().await;
    mount_instance(&server, "http://db.grafana.local:5432").await;

    let side = instance::fetch(&client(&server), "grafana.local", now()).await;
    assert_eq!(side.schema_version, 1);
    assert_eq!(side.host, "grafana.local");
    assert_eq!(side.search[0]["uid"], "abc");
    assert_eq!(side.datasources[0]["type"], "postgres");
    assert_eq!(side.folders[0]["title"], "Sites");
    // `instance` is composed from /api/health + /api/frontend/settings.
    assert_eq!(side.instance["version"], "7.5.17");
    assert_eq!(side.instance["edition"], "Open Source");
    assert_eq!(side.instance["rendererAvailable"], false);
}

#[tokio::test]
async fn fetch_is_best_effort_nulls_failed_endpoint() {
    let server = MockServer::start().await;
    // Everything healthy except /api/folders, which 500s.
    mount_ok(&server, "/api/search", json!([{ "uid": "abc" }])).await;
    mount_ok(&server, "/api/datasources", json!([])).await;
    Mock::given(method("GET"))
        .and(path("/api/folders"))
        .respond_with(ResponseTemplate::new(500).set_body_raw("boom", "text/plain"))
        .mount(&server)
        .await;
    mount_ok(&server, "/api/health", json!({ "version": "7.5.17" })).await;
    mount_ok(
        &server,
        "/api/frontend/settings",
        json!({ "rendererAvailable": true }),
    )
    .await;

    let side = instance::fetch(&client(&server), "h", now()).await;
    // The failed endpoint leaves its field null; the scrape is not aborted.
    assert!(side.folders.is_null());
    assert_eq!(side.search[0]["uid"], "abc");
    // version falls back to /api/health when settings has no buildInfo.
    assert_eq!(side.instance["version"], "7.5.17");
    assert_eq!(side.instance["rendererAvailable"], true);
}

#[tokio::test]
async fn enforce_ssrf_blocks_disallowed_datasource_host() {
    let server = MockServer::start().await;
    mount_instance(&server, "http://evil.internal/query").await;

    let side = instance::fetch(&client(&server), "h", now()).await;
    let err = instance::enforce_ssrf(&["grafana.local".to_owned()], &side).unwrap_err();
    assert!(
        matches!(err, ScrapeError::SsrfBlocked(ref h) if h == "evil.internal"),
        "got {err:?}"
    );
}

#[tokio::test]
async fn enforce_ssrf_allows_listed_host() {
    let server = MockServer::start().await;
    mount_instance(&server, "https://db.grafana.local/q").await;

    let side = instance::fetch(&client(&server), "h", now()).await;
    instance::enforce_ssrf(&["*.grafana.local".to_owned()], &side).expect("allowed host passes");
}

#[tokio::test]
async fn fetch_write_read_round_trip_through_store() {
    let server = MockServer::start().await;
    mount_instance(&server, "http://db.grafana.local:5432").await;
    let side = instance::fetch(&client(&server), "rd-esr.nube-iiot.com", now()).await;

    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    store.write_instance("grafana", &side).unwrap();

    // The reader prefers the sidecar.
    let facts = store
        .read_instance_facts("grafana", "rd-esr.nube-iiot.com", "abc")
        .unwrap();
    assert!(matches!(facts.origin, FactsOrigin::Sidecar(_)));
    assert_eq!(facts.search[0]["uid"], "abc");
    assert_eq!(facts.datasources[0]["type"], "postgres");
}

#[tokio::test]
async fn reader_falls_back_to_legacy_meta_search() {
    // A grafana-shaped legacy artifact (meta.search embedded), no sidecar.
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    let mut legacy = Artifact::new("grafana", "legacyUid", now());
    legacy.meta = json!({
        "dashboard": { "dashboard": { "uid": "legacyUid" } },
        "search": [{ "uid": "legacyUid", "title": "Legacy" }],
        "annotations": []
    });
    store.write(&legacy).unwrap();

    let facts = store
        .read_instance_facts("grafana", "old-host", "legacyUid")
        .unwrap();
    assert!(matches!(facts.origin, FactsOrigin::LegacyMeta(_)));
    assert_eq!(facts.search[0]["uid"], "legacyUid");
}

#[tokio::test]
async fn mixed_store_new_sidecar_and_legacy_coexist() {
    let server = MockServer::start().await;
    mount_instance(&server, "http://db.grafana.local:5432").await;

    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());

    // Newly-scraped host gets a sidecar.
    let side = instance::fetch(&client(&server), "new-host", now()).await;
    store.write_instance("grafana", &side).unwrap();

    // A different host predates the sidecar — only a legacy artifact.
    let mut legacy = Artifact::new("grafana", "oldUid", now());
    legacy.meta = json!({ "search": [{ "uid": "oldUid" }] });
    store.write(&legacy).unwrap();

    let new_facts = store
        .read_instance_facts("grafana", "new-host", "abc")
        .unwrap();
    assert!(matches!(new_facts.origin, FactsOrigin::Sidecar(_)));

    let old_facts = store
        .read_instance_facts("grafana", "old-host", "oldUid")
        .unwrap();
    assert!(matches!(old_facts.origin, FactsOrigin::LegacyMeta(_)));
}

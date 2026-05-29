//! wiremock + golden tests for the Grafana renderer (`mode = Visual`)
//! and the API+pixels merge (`mode = Both`). No live network: `wiremock`
//! serves the dashboard, `/api/ds/query`, `/api/frontend/settings`
//! (the renderer probe) and `/render/d-solo/...` (canned PNG bytes).

use ncrawler_grafana::client::{GrafanaCrateClient, RendererClient};
use ncrawler_grafana::{merge, visual, VisualOpts};
use ncrawler_spi::{ScrapeError, ScrapeJob};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Minimal but valid PNG signature + IHDR-ish bytes; we persist them
/// verbatim, so any bytes round-trip.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nFAKE-PNG-PIXELS";

fn api_client(server: &MockServer) -> GrafanaCrateClient {
    GrafanaCrateClient::new(&server.uri(), None).expect("client builds")
}

fn renderer(server: &MockServer) -> RendererClient {
    RendererClient::new(&server.uri(), None).expect("renderer builds")
}

fn job(target: &str) -> ScrapeJob {
    ScrapeJob {
        source: "grafana".to_owned(),
        target: target.to_owned(),
        allow_hosts: vec![],
        options: json!({ "url": "http://grafana.local", "mode": "visual" }),
    }
}

fn now() -> chrono::DateTime<chrono::Utc> {
    "2026-05-29T00:00:00Z".parse().unwrap()
}

async fn mount_dashboard(server: &MockServer) {
    let dashboard = json!({
        "meta": {},
        "dashboard": {
            "uid": "abc",
            "title": "Prod overview",
            "panels": [
                { "id": 2, "title": "CPU", "tags": ["cpu"], "targets": [] },
                { "id": 5, "title": "Mem", "targets": [] },
                { "type": "row", "title": "a row, no id" }
            ]
        }
    });
    Mock::given(method("GET"))
        .and(path("/api/dashboards/uid/abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(dashboard))
        .mount(server)
        .await;
}

async fn mount_companions(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/ds/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": {} })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/annotations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;
}

async fn mount_renderer(server: &MockServer, available: bool) {
    Mock::given(method("GET"))
        .and(path("/api/frontend/settings"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "rendererAvailable": available })),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/render/d-solo/abc/_"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(PNG, "image/png"))
        .mount(server)
        .await;
}

#[tokio::test]
async fn visual_missing_plugin_errors_without_fallback() {
    let server = MockServer::start().await;
    mount_dashboard(&server).await;
    mount_renderer(&server, false).await;
    let tmp = tempfile::tempdir().unwrap();

    let err = visual::scrape(
        &api_client(&server),
        &renderer(&server),
        &job("abc"),
        &VisualOpts::default(),
        tmp.path(),
        now(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, ScrapeError::RendererPluginMissing),
        "got {err:?}"
    );
}

#[tokio::test]
async fn visual_writes_one_png_per_panel_linked_by_item_id() {
    let server = MockServer::start().await;
    mount_dashboard(&server).await;
    mount_renderer(&server, true).await;
    let tmp = tempfile::tempdir().unwrap();

    let artifact = visual::scrape(
        &api_client(&server),
        &renderer(&server),
        &job("abc"),
        &VisualOpts::default(),
        tmp.path(),
        now(),
    )
    .await
    .expect("visual scrape succeeds");

    // One asset per real panel (ids 2 and 5; the id-less row is skipped),
    // each linked to its panel item by item_id (never by label).
    assert_eq!(artifact.assets.len(), 2);
    for asset in &artifact.assets {
        assert_eq!(asset.mime, "image/png");
        let item_id = asset.item_id.as_deref().expect("asset links an item");
        assert!(artifact.items.iter().any(|it| it.id == item_id));
        // The PNG was written to disk under assets/.
        let bytes = std::fs::read(tmp.path().join(asset.path.file_name().unwrap())).unwrap();
        assert_eq!(bytes, PNG);
    }
    assert_eq!(artifact.assets[0].item_id.as_deref(), Some("panel-2"));
    assert_eq!(artifact.assets[1].item_id.as_deref(), Some("panel-5"));
}

#[tokio::test]
async fn both_mode_merges_data_items_with_matching_png_assets() {
    let server = MockServer::start().await;
    mount_dashboard(&server).await;
    mount_companions(&server).await;
    mount_renderer(&server, true).await;
    let tmp = tempfile::tempdir().unwrap();

    let artifact = merge::scrape(
        &api_client(&server),
        &renderer(&server),
        &job("abc"),
        &VisualOpts::default(),
        tmp.path(),
        now(),
    )
    .await
    .expect("both scrape succeeds");

    // Golden: items carry data (from /api/ds/query), each panel item has
    // exactly one matching asset by item_id.
    let golden = golden_view(&artifact);
    let expected: Value = serde_json::from_str(include_str!("fixtures/merge_golden.json")).unwrap();
    assert_eq!(golden, expected);

    // Data really came through the API path (not the Visual path, which
    // leaves data = None).
    assert!(artifact.items.iter().all(|it| it.data.is_some()));
}

/// A deterministic projection of the artifact for golden comparison:
/// items (id/kind/title/has_data) and assets (mime/item_id), dropping the
/// timestamp and the full dashboard meta.
fn golden_view(a: &ncrawler_spi::Artifact) -> Value {
    json!({
        "source": a.source,
        "items": a.items.iter().map(|it| json!({
            "id": it.id,
            "kind": it.kind,
            "title": it.title,
            "has_data": it.data.is_some(),
        })).collect::<Vec<_>>(),
        "assets": a.assets.iter().map(|asset| json!({
            "mime": asset.mime,
            "item_id": asset.item_id,
            "path": asset.path,
        })).collect::<Vec<_>>(),
    })
}

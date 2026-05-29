//! wiremock-backed unit tests for the Grafana `mode = Api` path.
//!
//! Every endpoint is exercised for success + 401 + 404 + malformed JSON,
//! plus an SSRF-reject case. No live network: `wiremock` serves canned
//! responses on localhost.

use ncrawler_grafana::api;
use ncrawler_grafana::client::{GrafanaClient, GrafanaCrateClient};
use ncrawler_spi::{ScrapeError, ScrapeJob};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> GrafanaCrateClient {
    GrafanaCrateClient::new(&server.uri(), None).expect("client builds")
}

fn job(target: &str, allow_hosts: Vec<String>) -> ScrapeJob {
    ScrapeJob {
        source: "grafana".to_owned(),
        target: target.to_owned(),
        allow_hosts,
        options: json!({ "url": "http://grafana.local", "mode": "api" }),
    }
}

fn now() -> chrono::DateTime<chrono::Utc> {
    "2026-05-29T00:00:00Z".parse().unwrap()
}

/// Mount a dashboard with one panel plus the three companion endpoints,
/// each returning `ok`. `ds_query_body` is what `/api/ds/query` returns.
async fn mount_happy(server: &MockServer, ds_query_body: Value) {
    let dashboard = json!({
        "meta": { "isStarred": false },
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
    Mock::given(method("POST"))
        .and(path("/api/ds/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ds_query_body))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "uid": "abc" }])))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/annotations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;
}

#[tokio::test]
async fn scrape_api_success_emits_one_item_per_panel() {
    let server = MockServer::start().await;
    mount_happy(&server, json!({ "results": {} })).await;

    let artifact = api::scrape(&client(&server), &job("abc", vec![]), now())
        .await
        .expect("scrape succeeds");

    // Two real panels (ids 2 and 5); the id-less row is skipped.
    assert_eq!(artifact.items.len(), 2);
    assert_eq!(artifact.items[0].id, "panel-2");
    assert_eq!(artifact.items[1].id, "panel-5");
    assert_eq!(artifact.items[0].tags, vec!["cpu".to_owned()]);
    assert_eq!(artifact.source, "grafana");
    // Dashboard JSON + companion endpoints land in meta.
    assert_eq!(artifact.meta["dashboard"]["dashboard"]["uid"], "abc");
    assert_eq!(artifact.meta["search"][0]["uid"], "abc");
    assert!(artifact.meta["annotations"].is_array());
}

#[tokio::test]
async fn dashboard_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/dashboards/uid/abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "meta": {}, "dashboard": { "uid": "abc" }
        })))
        .mount(&server)
        .await;
    let got = client(&server).dashboard_by_uid("abc").await.unwrap();
    assert_eq!(got["dashboard"]["uid"], "abc");
}

#[tokio::test]
async fn ds_query_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/ds/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": { "A": {} } })))
        .mount(&server)
        .await;
    let got = client(&server)
        .ds_query(&json!({ "queries": [] }))
        .await
        .unwrap();
    assert!(got["results"].is_object());
}

#[tokio::test]
async fn search_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "uid": "x" }])))
        .mount(&server)
        .await;
    let got = client(&server).search().await.unwrap();
    assert_eq!(got[0]["uid"], "x");
}

#[tokio::test]
async fn annotations_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/annotations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 1 }])))
        .mount(&server)
        .await;
    let got = client(&server).annotations().await.unwrap();
    assert_eq!(got[0]["id"], 1);
}

/// Mount a single endpoint with `status`/`body` and assert the mapped
/// error for each of the four API surfaces.
async fn assert_error<F>(status: u16, body: &'static str, check: F)
where
    F: Fn(ScrapeError),
{
    // dashboard
    {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/dashboards/uid/abc"))
            .respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        check(client(&server).dashboard_by_uid("abc").await.unwrap_err());
    }
    // ds/query
    {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/ds/query"))
            .respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        check(client(&server).ds_query(&json!({})).await.unwrap_err());
    }
    // search
    {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/search"))
            .respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        check(client(&server).search().await.unwrap_err());
    }
    // annotations
    {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/annotations"))
            .respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        check(client(&server).annotations().await.unwrap_err());
    }
}

#[tokio::test]
async fn every_endpoint_maps_401_to_auth() {
    assert_error(401, r#"{"message":"unauthorized"}"#, |e| {
        assert!(matches!(e, ScrapeError::Auth(_)), "got {e:?}");
    })
    .await;
}

#[tokio::test]
async fn every_endpoint_maps_404_to_not_found() {
    assert_error(404, r#"{"message":"not found"}"#, |e| {
        assert!(matches!(e, ScrapeError::NotFound(_)), "got {e:?}");
    })
    .await;
}

#[tokio::test]
async fn every_endpoint_maps_malformed_json_to_other() {
    // 200 OK but the body is not valid JSON for the expected shape.
    assert_error(200, "this is not json", |e| {
        assert!(matches!(e, ScrapeError::Other(_)), "got {e:?}");
    })
    .await;
}

#[tokio::test]
async fn scrape_rejects_disallowed_datasource_host() {
    let server = MockServer::start().await;
    // The /api/ds/query response surfaces a datasource URL whose host is
    // NOT in the allow-list → SSRF reject.
    mount_happy(
        &server,
        json!({ "results": { "A": { "meta": { "url": "http://evil.internal/query" } } } }),
    )
    .await;

    let err = api::scrape(
        &client(&server),
        &job("abc", vec!["grafana.local".to_owned()]),
        now(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, ScrapeError::SsrfBlocked(ref h) if h == "evil.internal"),
        "got {err:?}"
    );
}

#[tokio::test]
async fn scrape_allows_listed_datasource_host() {
    let server = MockServer::start().await;
    mount_happy(
        &server,
        json!({ "results": { "A": { "meta": { "url": "https://ds.grafana.local/q" } } } }),
    )
    .await;
    // `*.grafana.local` wildcard covers the datasource host.
    let artifact = api::scrape(
        &client(&server),
        &job("abc", vec!["*.grafana.local".to_owned()]),
        now(),
    )
    .await
    .expect("allowed host passes SSRF guard");
    assert_eq!(artifact.items.len(), 2);
}

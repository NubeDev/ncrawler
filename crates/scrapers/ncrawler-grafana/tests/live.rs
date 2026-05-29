//! Live renderer-plugin + headless-Chrome-fallback tests. Gated on
//! `RUN_LIVE_TESTS=1` AND `#[ignore]` so the default `cargo test` never
//! touches the network or launches a browser (SCOPE: testing). Run with:
//!
//! ```sh
//! RUN_LIVE_TESTS=1 GRAFANA_URL=... GRAFANA_TOKEN=... GRAFANA_UID=... \
//!   cargo test -p ncrawler-grafana -- --ignored
//! ```

use ncrawler_grafana::client::resolve_token;
use ncrawler_grafana::client::{GrafanaCrateClient, RendererClient};

fn live_enabled() -> bool {
    std::env::var("RUN_LIVE_TESTS").as_deref() == Ok("1")
}

/// A `Cancel` that never cancels, for driving the async fan-out.
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

/// Multi-dashboard smoke test: `--all --limit 5` against the
/// docker-compose Grafana fixture. Proves the fan-out, sidecar write, and
/// bounded concurrency end-to-end on a real instance (REPORT §8 step 3).
#[tokio::test]
#[ignore = "live network; needs RUN_LIVE_TESTS=1 + a real Grafana"]
async fn multi_scrape_all_limit_5_against_live_grafana() {
    if !live_enabled() {
        return;
    }
    let url = std::env::var("GRAFANA_URL").expect("GRAFANA_URL");
    let host = url::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
    let token = resolve_token(&host, None);
    let client = GrafanaCrateClient::new(&url, token.as_ref()).expect("client builds");

    let tmp = tempfile::tempdir().expect("tempdir");
    let store = ncrawler_core::ArtifactStore::new(tmp.path());
    let selector = ncrawler_grafana::DashboardSelector::from_args(&[
        "--all".to_owned(),
        "--limit".to_owned(),
        "5".to_owned(),
    ])
    .expect("selector parses");
    let options = serde_json::json!({ "url": url, "mode": "api" });

    let summary = ncrawler_grafana::scrape_selection(
        &client,
        &store,
        &host,
        &selector,
        &options,
        &[],
        chrono::Utc::now(),
        &ncrawler_grafana::MultiConfig::default(),
        &NeverCancel,
    )
    .await
    .expect("multi scrape succeeds");

    // At most 5 dashboards were touched (the `--limit 5` cap), and the
    // sidecar exists afterwards regardless of per-dashboard outcomes.
    assert!(summary.succeeded.len() + summary.failed.len() <= 5);
    assert!(store
        .read_instance_sidecar("grafana", &host)
        .expect("sidecar readable")
        .is_some());
}

#[tokio::test]
#[ignore = "live network; needs RUN_LIVE_TESTS=1 + a real Grafana"]
async fn renderer_probe_against_live_grafana() {
    if !live_enabled() {
        return;
    }
    let url = std::env::var("GRAFANA_URL").expect("GRAFANA_URL");
    let host = url::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
    let token = resolve_token(&host, None);
    let renderer = RendererClient::new(&url, token).expect("renderer builds");
    // Either the plugin is present (Ok) or explicitly missing — both are
    // valid live outcomes; a transport error is not.
    match renderer.probe().await {
        Ok(()) => {}
        Err(ncrawler_spi::ScrapeError::RendererPluginMissing) => {}
        Err(e) => panic!("unexpected probe error: {e:?}"),
    }
}

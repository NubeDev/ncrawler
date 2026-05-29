//! Live renderer-plugin + headless-Chrome-fallback tests. Gated on
//! `RUN_LIVE_TESTS=1` AND `#[ignore]` so the default `cargo test` never
//! touches the network or launches a browser (SCOPE: testing). Run with:
//!
//! ```sh
//! RUN_LIVE_TESTS=1 GRAFANA_URL=... GRAFANA_TOKEN=... GRAFANA_UID=... \
//!   cargo test -p ncrawler-grafana -- --ignored
//! ```

use ncrawler_grafana::client::resolve_token;
use ncrawler_grafana::client::RendererClient;

fn live_enabled() -> bool {
    std::env::var("RUN_LIVE_TESTS").as_deref() == Ok("1")
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

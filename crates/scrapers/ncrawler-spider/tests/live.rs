//! Live-network + headless-Chrome tests. Gated on `RUN_LIVE_TESTS=1`
//! AND `#[ignore]` so `cargo test` never hits the network by default
//! (SCOPE: testing). Run with:
//!
//! ```sh
//! RUN_LIVE_TESTS=1 cargo test -p ncrawler-spider -- --ignored
//! ```

use ncrawler_spi::{Cancel, ScrapeJob, Scraper};

/// A no-op cancel for live runs.
struct Never;
impl Cancel for Never {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn cancelled<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // Never cancels: a future that stays pending forever.
        Box::pin(std::future::pending())
    }
}

fn live_enabled() -> bool {
    std::env::var("RUN_LIVE_TESTS").as_deref() == Ok("1")
}

#[tokio::test]
#[ignore = "live network; needs RUN_LIVE_TESTS=1"]
async fn crawl_example_com_http_only() {
    if !live_enabled() {
        return;
    }
    let job = ScrapeJob {
        source: "spider".into(),
        target: "https://example.com".into(),
        allow_hosts: vec!["example.com".into()],
        options: serde_json::json!({ "depth": 0, "limit": 1 }),
    };
    let artifact = ncrawler_spider::SpiderScraper::new()
        .scrape(job, &Never)
        .await
        .expect("live crawl succeeds");
    assert!(!artifact.items.is_empty());
    assert!(artifact.items[0].id.starts_with("page-"));
}

#[tokio::test]
#[ignore = "headless Chrome; needs RUN_LIVE_TESTS=1 + a Chrome binary"]
async fn render_js_with_headless_chrome() {
    if !live_enabled() {
        return;
    }
    let html = ncrawler_spider::chrome::render_html("https://example.com")
        .await
        .expect("chrome renders");
    assert!(html.contains("Example Domain"));
}

//! ncrawler — HTTP-only spider scraper for arbitrary pages.
//!
//! Thin wrapper over the open-source `spider` crate, pulled
//! `default-features = false` so neither the `chrome` nor `smart`
//! feature is on and `spider_chrome` never enters the dependency tree
//! (SCOPE: one browser stack — `cargo tree -e normal` must NOT show
//! `spider_chrome`). JS rendering is delegated to our own
//! `chromiumoxide` layer in [`chrome`], opted into via `--render-js`.
//!
//! Each `spider::page::Page` maps to an [`Item::Page`] with the stable id
//! `page-{blake3(normalised_url)[..16]}`; readable text comes from
//! `dom_smoothie` and an optional Markdown rendering from `fast_html2md`.
//! Every URL followed is validated against the SSRF allow-list at scrape
//! time (SCOPE: security).

pub mod chrome;
pub mod page;

use async_trait::async_trait;
use serde_json::{json, Value};
use url::Url;

use ncrawler_spi::{Artifact, Cancel, ScrapeError, ScrapeJob, Scraper};

/// Per-job spider knobs parsed from `ScrapeJob.options`.
#[derive(Debug, Clone)]
struct SpiderOpts {
    depth: usize,
    limit: u32,
    delay_ms: u64,
    concurrency: Option<usize>,
    respect_robots: bool,
    render_js: bool,
}

impl SpiderOpts {
    fn from_job(job: &ScrapeJob) -> Self {
        let o = &job.options;
        let respect_robots = o
            .get("respect_robots")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if !respect_robots {
            tracing::warn!("robots.txt disabled for this crawl (operator override)");
        }
        Self {
            depth: o.get("depth").and_then(Value::as_u64).unwrap_or(2) as usize,
            limit: o.get("limit").and_then(Value::as_u64).unwrap_or(50) as u32,
            delay_ms: o.get("delay").and_then(Value::as_u64).unwrap_or(0),
            concurrency: o
                .get("concurrency")
                .and_then(Value::as_u64)
                .map(|c| c as usize),
            respect_robots,
            render_js: o.get("render_js").and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

/// The HTTP-only spider [`Scraper`].
#[derive(Default)]
pub struct SpiderScraper;

impl SpiderScraper {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Scraper for SpiderScraper {
    fn name(&self) -> &str {
        "spider"
    }

    async fn scrape(&self, job: ScrapeJob, cancel: &dyn Cancel) -> Result<Artifact, ScrapeError> {
        if cancel.is_cancelled() {
            return Err(ScrapeError::Cancelled);
        }
        // SSRF: the seed itself is the first URL we would follow.
        check_host(&job.target, &job.allow_hosts)?;
        let opts = SpiderOpts::from_job(&job);
        let fetched_at = chrono::Utc::now();

        let pages = crawl(&job, &opts).await?;

        let mut items = Vec::with_capacity(pages.len());
        for (url, html) in pages {
            if cancel.is_cancelled() {
                return Err(ScrapeError::Cancelled);
            }
            // Every followed URL is validated before we keep its content.
            check_host(&url, &job.allow_hosts)?;
            let html = if opts.render_js {
                chrome::render_html(&url)
                    .await
                    .map_err(ScrapeError::Other)?
            } else {
                html
            };
            items.push(page::html_to_item(&url, &html));
        }

        let mut artifact = Artifact::new("spider", job.target.clone(), fetched_at);
        artifact.items = items;
        artifact.meta = json!({
            "depth": opts.depth,
            "limit": opts.limit,
            "render_js": opts.render_js,
            "respect_robots": opts.respect_robots,
        });
        Ok(artifact)
    }
}

/// Drive a `spider::website::Website` crawl and collect `(url, html)`
/// for every page. HTTP-only; JS rendering (if requested) is applied
/// later by the caller via [`chrome`].
async fn crawl(job: &ScrapeJob, opts: &SpiderOpts) -> Result<Vec<(String, String)>, ScrapeError> {
    use spider::website::Website;

    let mut website = Website::new(&job.target);
    website
        .with_depth(opts.depth)
        .with_limit(opts.limit)
        .with_delay(opts.delay_ms)
        .with_respect_robots_txt(opts.respect_robots)
        .with_concurrency_limit(opts.concurrency);

    // Spider delivers pages via a broadcast channel; subscribe before crawl.
    let mut rx = website.subscribe(512);
    let crawl_handle = tokio::spawn(async move {
        website.crawl().await;
        website.unsubscribe();
    });

    let mut pages = Vec::new();
    while let Ok(page) = rx.recv().await {
        pages.push((page.get_url().to_string(), page.get_html()));
    }
    crawl_handle.await.map_err(|e| ScrapeError::Other(e.to_string()))?;

    Ok(pages)
}

/// Validate a URL's host against the allow-list. Empty list = operator
/// did not opt in → allow all (SCOPE: default no allow-list). Supports
/// exact and `*.suffix` wildcard patterns.
fn check_host(url: &str, allow_hosts: &[String]) -> Result<(), ScrapeError> {
    if allow_hosts.is_empty() {
        return Ok(());
    }
    let host = Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .ok_or_else(|| ScrapeError::SsrfBlocked(url.to_owned()))?;
    if host_allowed(&host, allow_hosts) {
        Ok(())
    } else {
        Err(ScrapeError::SsrfBlocked(host))
    }
}

fn host_allowed(host: &str, allow_hosts: &[String]) -> bool {
    allow_hosts.iter().any(|pat| match pat.strip_prefix("*.") {
        Some(suffix) => host == suffix || host.ends_with(&format!(".{suffix}")),
        None => host == pat,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_allow_list_allows_all() {
        assert!(check_host("https://anything.example/x", &[]).is_ok());
    }

    #[test]
    fn wildcard_and_exact_match() {
        let allow = vec!["status.example".to_owned(), "*.docs.example".to_owned()];
        assert!(check_host("https://status.example/p", &allow).is_ok());
        assert!(check_host("https://api.docs.example/p", &allow).is_ok());
        assert!(matches!(
            check_host("https://evil.example/p", &allow),
            Err(ScrapeError::SsrfBlocked(_))
        ));
    }
}

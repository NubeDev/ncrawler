//! Best-effort `--visual-fallback chrome` path for `mode = Visual`.
//!
//! This drives our own `chromiumoxide` layer (the single browser stack;
//! never `spider_chrome`) at the dashboard URL and captures ONE
//! whole-dashboard PNG. It is documented as flaky (auth-token-in-Chrome
//! vs. Bearer-on-API, template variables, lazy-loaded panels, no
//! reliable "all queries finished" signal — SCOPE: visual strategy), so
//! the asset has `item_id = None` (whole-artifact, not per-panel).
//!
//! Chrome binary discovery and the sandbox opt-out follow the documented
//! `NCRAWLER_CHROME` → `which` → well-known-paths chain.

use std::path::{Path, PathBuf};

use ncrawler_spi::{Asset, ScrapeError};

use crate::visual::VisualOpts;

/// Capture a single full-page screenshot of the dashboard URL. Returns a
/// one-element asset vector (`item_id = None`). Any browser error maps to
/// [`ScrapeError::Other`] — this path is best-effort by design.
pub(crate) async fn fallback_screenshot(
    opts: &VisualOpts,
    assets_dir: &Path,
) -> Result<Vec<Asset>, ScrapeError> {
    let chrome = discover_chrome().map_err(ScrapeError::Other)?;
    let png = screenshot(&chrome, &opts.dashboard_url)
        .await
        .map_err(ScrapeError::Other)?;
    std::fs::create_dir_all(assets_dir).map_err(|e| ScrapeError::Other(e.to_string()))?;
    let file_name = "dashboard.png";
    std::fs::write(assets_dir.join(file_name), &png)
        .map_err(|e| ScrapeError::Other(e.to_string()))?;
    Ok(vec![Asset {
        path: Path::new("assets").join(file_name),
        mime: "image/png".to_owned(),
        label: "dashboard (chrome fallback)".to_owned(),
        item_id: None,
    }])
}

async fn screenshot(chrome: &Path, url: &str) -> Result<Vec<u8>, String> {
    use chromiumoxide::browser::{Browser, BrowserConfig};
    use futures::StreamExt;

    let mut builder = BrowserConfig::builder().chrome_executable(chrome);
    // Sandbox on by default; opt out only via env, logged WARN.
    if std::env::var_os("NCRAWLER_CHROME_NO_SANDBOX").is_some() {
        tracing::warn!("NCRAWLER_CHROME_NO_SANDBOX set; launching Chrome WITHOUT the sandbox");
        builder = builder.no_sandbox();
    }
    let config = builder.build()?;

    let (mut browser, mut handler) = Browser::launch(config).await.map_err(|e| e.to_string())?;
    let drive = tokio::task::spawn(async move { while handler.next().await.is_some() {} });

    let result = capture(&browser, url).await;
    let _ = browser.close().await;
    let _ = browser.wait().await;
    drive.abort();
    result
}

async fn capture(browser: &chromiumoxide::Browser, url: &str) -> Result<Vec<u8>, String> {
    use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
    use chromiumoxide::page::ScreenshotParams;

    let page = browser.new_page(url).await.map_err(|e| e.to_string())?;
    page.wait_for_navigation()
        .await
        .map_err(|e| e.to_string())?;
    let params = ScreenshotParams::builder()
        .format(CaptureScreenshotFormat::Png)
        .full_page(true)
        .build();
    page.screenshot(params).await.map_err(|e| e.to_string())
}

/// Resolve a Chrome/Chromium binary: `NCRAWLER_CHROME` → `which` →
/// well-known platform paths → error naming all of the above
/// (SCOPE: Chrome binary discovery).
pub(crate) fn discover_chrome() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("NCRAWLER_CHROME") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!("NCRAWLER_CHROME={} is not a file", p.display()));
    }
    for name in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ] {
        if let Some(p) = which_in_path(name) {
            return Ok(p);
        }
    }
    for p in well_known_paths() {
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(
        "no Chrome/Chromium binary found; set NCRAWLER_CHROME, or install \
         chromium / chromium-browser / google-chrome on PATH, or place it \
         at a well-known location (/usr/bin/..., /Applications/...)"
            .to_owned(),
    )
}

/// Minimal `which`: scan `PATH` for an executable named `name`.
fn which_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn well_known_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
    ]
}

#[cfg(not(target_os = "macos"))]
fn well_known_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/usr/bin/chromium-browser"),
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/usr/bin/google-chrome-stable"),
        PathBuf::from("/snap/bin/chromium"),
    ]
}

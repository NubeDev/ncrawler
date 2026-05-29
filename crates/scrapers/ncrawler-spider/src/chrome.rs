//! Our own `chromiumoxide` JS-rendering layer — the future
//! `starter-headless` promotion candidate (SCOPE: promotion candidates).
//!
//! This is the ONLY CDP implementation in the build. `spider`'s vendored
//! `spider_chrome` fork is deliberately kept out of the tree (spider is
//! pulled `default-features = false`); depending on both would double the
//! most fragile part of the build (two CDP impls, two launch paths, two
//! Chrome version-skew sources).
//!
//! Opted into via `--render-js`. Chrome binary discovery follows the
//! documented `NCRAWLER_CHROME` → `which` → well-known-paths chain; the
//! sandbox is on by default and `NCRAWLER_CHROME_NO_SANDBOX` opts out,
//! logged at WARN.

use std::path::{Path, PathBuf};

/// Navigate to `url` with headless Chrome and return the rendered DOM
/// HTML (`document.documentElement.outerHTML` via CDP `getContent`).
pub async fn render_html(url: &str) -> Result<String, String> {
    let chrome = discover_chrome()?;
    fetch(&chrome, url).await
}

async fn fetch(chrome: &Path, url: &str) -> Result<String, String> {
    use chromiumoxide::browser::{Browser, BrowserConfig};
    use futures::StreamExt;

    let mut builder = BrowserConfig::builder().chrome_executable(chrome);
    if std::env::var_os("NCRAWLER_CHROME_NO_SANDBOX").is_some() {
        tracing::warn!("NCRAWLER_CHROME_NO_SANDBOX set; launching Chrome WITHOUT the sandbox");
        builder = builder.no_sandbox();
    }
    let config = builder.build()?;

    let (mut browser, mut handler) = Browser::launch(config).await.map_err(|e| e.to_string())?;
    let drive = tokio::task::spawn(async move { while handler.next().await.is_some() {} });

    let result = navigate(&browser, url).await;
    let _ = browser.close().await;
    let _ = browser.wait().await;
    drive.abort();
    result
}

async fn navigate(browser: &chromiumoxide::Browser, url: &str) -> Result<String, String> {
    let page = browser.new_page(url).await.map_err(|e| e.to_string())?;
    page.wait_for_navigation()
        .await
        .map_err(|e| e.to_string())?;
    page.content().await.map_err(|e| e.to_string())
}

/// Resolve a Chrome/Chromium binary: `NCRAWLER_CHROME` → `which` →
/// well-known platform paths → error naming all of the above
/// (SCOPE: Chrome binary discovery).
pub fn discover_chrome() -> Result<PathBuf, String> {
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

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

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ncrawler_spi::{Asset, Item, ScrapeError};
use starter_spi::secrets::Secret;

use crate::visual::VisualOpts;

/// A Grafana `grafana_session` cookie obtained via password login, used
/// to authenticate the headless-Chrome browser session (the web UI does
/// not honour an API-key `Authorization: Bearer` header — only a session
/// cookie — so without this the capture lands on the anonymous home page).
struct SessionCookie {
    name: String,
    value: String,
}

/// Log in to Grafana with `GRAFANA_USER` / `GRAFANA_PASSWORD` and return
/// the resulting `grafana_session` cookie. Returns `None` when the env
/// vars are unset or login fails (the caller logs a WARN and proceeds
/// best-effort). Never logs the password.
async fn login_cookie(base_url: &str) -> Option<SessionCookie> {
    let user = std::env::var("GRAFANA_USER").ok().filter(|s| !s.is_empty())?;
    let pass = std::env::var("GRAFANA_PASSWORD")
        .ok()
        .filter(|s| !s.is_empty())?;
    let base = base_url.trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let resp = client
        .post(format!("{base}/login"))
        .json(&serde_json::json!({ "user": user, "password": pass }))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "grafana /login did not succeed");
        return None;
    }
    for hv in resp.headers().get_all(reqwest::header::SET_COOKIE) {
        let Ok(s) = hv.to_str() else { continue };
        if let Some(rest) = s.strip_prefix("grafana_session=") {
            let value = rest.split(';').next().unwrap_or("").to_owned();
            if !value.is_empty() {
                return Some(SessionCookie {
                    name: "grafana_session".to_owned(),
                    value,
                });
            }
        }
    }
    tracing::warn!("grafana /login returned no grafana_session cookie");
    None
}

/// Inject a session cookie into a Chrome page via CDP `Network.setCookie`
/// scoped to the page URL. Best-effort: a failure is logged and ignored.
async fn apply_cookie(page: &chromiumoxide::Page, url: &str, cookie: Option<&SessionCookie>) {
    use chromiumoxide::cdp::browser_protocol::network::CookieParam;
    let Some(c) = cookie else { return };
    let param = match CookieParam::builder()
        .name(c.name.clone())
        .value(c.value.clone())
        .url(url.to_owned())
        .build()
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("building session cookie failed: {e}");
            return;
        }
    };
    if let Err(e) = page.set_cookie(param).await {
        tracing::warn!("setting session cookie failed: {e}");
    }
}


/// Capture a single full-page screenshot of the dashboard URL. Returns a
/// one-element asset vector (`item_id = None`). Any browser error maps to
/// [`ScrapeError::Other`] — this path is best-effort by design.
pub(crate) async fn fallback_screenshot(
    opts: &VisualOpts,
    assets_dir: &Path,
) -> Result<Vec<Asset>, ScrapeError> {
    let chrome = discover_chrome().map_err(ScrapeError::Other)?;
    let mut headers = HashMap::new();
    if let Some(t) = opts.token.as_ref() {
        headers.insert("Authorization".to_owned(), format!("Bearer {}", t.expose()));
    }
    let cookie = login_cookie(&opts.base_url).await;
    if cookie.is_none() {
        tracing::warn!(
            "no grafana_session cookie (set GRAFANA_USER + GRAFANA_PASSWORD); \
             the Chrome capture will likely render the anonymous login/home page"
        );
    }
    let viewport_w = if opts.width >= 1280 { opts.width } else { 1600 };
    let viewport_h = if opts.height >= 720 { opts.height } else { 1200 };
    let png = screenshot(
        &chrome,
        &opts.dashboard_url,
        &headers,
        cookie.as_ref(),
        viewport_w,
        viewport_h,
    )
    .await
    .map_err(ScrapeError::Other)?;
    std::fs::create_dir_all(assets_dir).map_err(|e| ScrapeError::Other(e.to_string()))?;
    let file_name = "dashboard.png";
    std::fs::write(assets_dir.join(file_name), &png)
        .map_err(|e| ScrapeError::Other(e.to_string()))?;
    Ok(vec![Asset {
        path: Path::new("assets").join(file_name),
        mime: "image/png".to_owned(),
        label: "whole dashboard (chrome)".to_owned(),
        item_id: None,
    }])
}

async fn screenshot(
    chrome: &Path,
    url: &str,
    headers: &HashMap<String, String>,
    cookie: Option<&SessionCookie>,
    viewport_w: u32,
    viewport_h: u32,
) -> Result<Vec<u8>, String> {
    use chromiumoxide::browser::{Browser, BrowserConfig};
    use futures::StreamExt;

    let profile_dir = std::env::temp_dir().join(format!(
        "ncrawler-chrome-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let mut builder = BrowserConfig::builder()
        .chrome_executable(chrome)
        .user_data_dir(&profile_dir)
        .window_size(viewport_w, viewport_h);
    if std::env::var_os("NCRAWLER_CHROME_NO_SANDBOX").is_some() {
        tracing::warn!("NCRAWLER_CHROME_NO_SANDBOX set; launching Chrome WITHOUT the sandbox");
        builder = builder.no_sandbox();
    }
    let config = builder.build()?;
    let (mut browser, mut handler) = Browser::launch(config).await.map_err(|e| e.to_string())?;
    let drive = tokio::task::spawn(async move { while handler.next().await.is_some() {} });
    let result = capture(&browser, url, headers, cookie, viewport_w, viewport_h).await;
    let _ = browser.close().await;
    let _ = browser.wait().await;
    drive.abort();
    result
}

/// Per-panel screenshots via `/d-solo/<uid>/dashboard?panelId=<id>` with
/// the `Authorization: Bearer` header set on every request. Returns one
/// [`Asset`] per [`Item::Panel`], linked via `item_id`. Best-effort: a
/// single failing panel is logged as WARN and skipped (the rest still
/// land on disk).
pub(crate) async fn fallback_per_panel_screenshots(
    opts: &VisualOpts,
    items: &[Item],
    token: Option<&Secret>,
    base_url: &str,
    uid: &str,
    assets_dir: &Path,
) -> Result<Vec<Asset>, ScrapeError> {
    let chrome = discover_chrome().map_err(ScrapeError::Other)?;
    std::fs::create_dir_all(assets_dir).map_err(|e| ScrapeError::Other(e.to_string()))?;

    let mut headers = HashMap::new();
    if let Some(t) = token {
        headers.insert("Authorization".to_owned(), format!("Bearer {}", t.expose()));
    }
    let base = base_url.trim_end_matches('/').to_owned();
    let cookie = login_cookie(&base).await;
    if cookie.is_none() {
        tracing::warn!(
            "no grafana_session cookie (set GRAFANA_USER + GRAFANA_PASSWORD); \
             per-panel Chrome captures will likely render the anonymous home page"
        );
    }
    let width = opts.width;
    let height = opts.height;
    let from = opts.from.clone();
    let to = opts.to.clone();

    use chromiumoxide::browser::{Browser, BrowserConfig};
    use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
    use chromiumoxide::page::ScreenshotParams;
    use futures::StreamExt;

    let profile_dir = std::env::temp_dir().join(format!(
        "ncrawler-chrome-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let mut builder = BrowserConfig::builder()
        .chrome_executable(&chrome)
        .user_data_dir(&profile_dir)
        .window_size(width, height);
    if std::env::var_os("NCRAWLER_CHROME_NO_SANDBOX").is_some() {
        tracing::warn!("NCRAWLER_CHROME_NO_SANDBOX set; launching Chrome WITHOUT the sandbox");
        builder = builder.no_sandbox();
    }
    let config = builder
        .build()
        .map_err(|e| ScrapeError::Other(e.to_string()))?;
    let (mut browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| ScrapeError::Other(e.to_string()))?;
    let drive = tokio::task::spawn(async move { while handler.next().await.is_some() {} });

    let mut assets = Vec::new();
    for item in items {
        let Some(panel_id) = item
            .id
            .strip_prefix("panel-")
            .and_then(|s| s.parse::<i64>().ok())
        else {
            continue;
        };
        let url = format!(
            "{base}/d-solo/{uid}/dashboard?panelId={panel_id}&from={from}&to={to}&kiosk=tv"
        );
        let res = render_one(&browser, &url, &headers, cookie.as_ref(), width, height).await;
        match res {
            Ok(png) => {
                let file_name = format!("{}.png", item.id);
                if let Err(e) = std::fs::write(assets_dir.join(&file_name), &png) {
                    tracing::warn!(panel_id, "writing panel PNG failed: {e}");
                    continue;
                }
                assets.push(Asset {
                    path: Path::new("assets").join(&file_name),
                    mime: "image/png".to_owned(),
                    label: item.title.clone().unwrap_or_else(|| item.id.clone()),
                    item_id: Some(item.id.clone()),
                });
            }
            Err(e) => {
                tracing::warn!(panel_id, "chrome per-panel screenshot failed: {e}");
            }
        }
    }

    let _ = browser.close().await;
    let _ = browser.wait().await;
    drive.abort();
    let _ = (CaptureScreenshotFormat::Png, ScreenshotParams::builder());
    Ok(assets)
}

async fn render_one(
    browser: &chromiumoxide::Browser,
    url: &str,
    headers: &HashMap<String, String>,
    cookie: Option<&SessionCookie>,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
    use chromiumoxide::cdp::browser_protocol::network::SetExtraHttpHeadersParams;
    use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
    use chromiumoxide::page::ScreenshotParams;

    let page = browser.new_page("about:blank").await.map_err(|e| e.to_string())?;
    if !headers.is_empty() {
        let hv = serde_json::to_value(headers).map_err(|e| e.to_string())?;
        page.execute(SetExtraHttpHeadersParams::new(
            chromiumoxide::cdp::browser_protocol::network::Headers::new(hv),
        ))
        .await
        .map_err(|e| e.to_string())?;
    }
    page.execute(
        SetDeviceMetricsOverrideParams::builder()
            .width(width as i64)
            .height(height as i64)
            .device_scale_factor(1.0)
            .mobile(false)
            .build()
            .map_err(|e| e.to_string())?,
    )
    .await
    .map_err(|e| e.to_string())?;
    apply_cookie(&page, url, cookie).await;
    page.goto(url).await.map_err(|e| e.to_string())?;
    page.wait_for_navigation()
        .await
        .map_err(|e| e.to_string())?;
    // Give Grafana a moment to run its panel query and paint.
    tokio::time::sleep(std::time::Duration::from_millis(3500)).await;
    let params = ScreenshotParams::builder()
        .format(CaptureScreenshotFormat::Png)
        .full_page(false)
        .build();
    let png = page.screenshot(params).await.map_err(|e| e.to_string())?;
    let _ = page.close().await;
    Ok(png)
}


async fn capture(
    browser: &chromiumoxide::Browser,
    url: &str,
    headers: &HashMap<String, String>,
    cookie: Option<&SessionCookie>,
    viewport_w: u32,
    viewport_h: u32,
) -> Result<Vec<u8>, String> {
    use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
    use chromiumoxide::cdp::browser_protocol::network::SetExtraHttpHeadersParams;
    use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
    use chromiumoxide::page::ScreenshotParams;

    let page = browser.new_page("about:blank").await.map_err(|e| e.to_string())?;
    page.execute(
        SetDeviceMetricsOverrideParams::builder()
            .width(viewport_w as i64)
            .height(viewport_h as i64)
            .device_scale_factor(1.0)
            .mobile(false)
            .build()
            .map_err(|e| e.to_string())?,
    )
    .await
    .map_err(|e| e.to_string())?;
    if !headers.is_empty() {
        let hv = serde_json::to_value(headers).map_err(|e| e.to_string())?;
        page.execute(SetExtraHttpHeadersParams::new(
            chromiumoxide::cdp::browser_protocol::network::Headers::new(hv),
        ))
        .await
        .map_err(|e| e.to_string())?;
    }
    apply_cookie(&page, url, cookie).await;
    page.goto(url).await.map_err(|e| e.to_string())?;
    page.wait_for_navigation()
        .await
        .map_err(|e| e.to_string())?;
    // Give Grafana time to paint all panels on a whole-dashboard capture.
    let wait_ms = std::env::var("NCRAWLER_CHROME_WAIT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8000);
    tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
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

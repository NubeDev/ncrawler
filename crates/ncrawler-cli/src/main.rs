//! `ncrawler` — CLI entry point.
//!
//! `ls` and `show` are end-to-end against the on-disk store. `scrape`
//! and `build` print a not-yet-implemented stub keyed by the
//! source/builder name; the scraper and builder registries land in
//! later milestones.

mod cli;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::Parser;
use ncrawler_core::{parse_since, read_artifact, ArtifactStore};

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Scrape { source, out, rest } => run_scrape(&source, out, &rest).await?,
        Command::Build {
            builder,
            artifact_dir,
            rest,
        } => run_build(&builder, artifact_dir, &rest).await?,
        Command::Ls { source, since, out } => run_ls(source, since, out)?,
        Command::Show { artifact_dir } => run_show(artifact_dir)?,
    }
    Ok(())
}

/// Resolve the on-disk skills directory: `NCRAWLER_SKILLS_DIR` or
/// `./skills` relative to the working directory.
fn skills_dir() -> std::path::PathBuf {
    std::env::var_os("NCRAWLER_SKILLS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("./skills"))
}

/// Pull `--flag value` out of the trailing args; returns the value.
fn flag_value(rest: &[String], flag: &str) -> Option<String> {
    rest.iter()
        .position(|a| a == flag)
        .and_then(|i| rest.get(i + 1))
        .cloned()
}

/// Collect every `--flag value` occurrence (repeatable flags like
/// `--panel` / `--allow-host`).
fn flag_values(rest: &[String], flag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == flag {
            if let Some(v) = rest.get(i + 1) {
                out.push(v.clone());
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Does a bare boolean flag appear?
fn flag_present(rest: &[String], flag: &str) -> bool {
    rest.iter().any(|a| a == flag)
}

/// Scrape a source into a fresh on-disk artifact. Grafana visual/both
/// modes pre-write panel PNGs into the artifact's `assets/` dir
/// (computed from `--out`); the store then writes `artifact.json` into
/// the same timestamped directory.
async fn run_scrape(source: &str, out: std::path::PathBuf, rest: &[String]) -> Result<()> {
    use ncrawler_core::ArtifactStore;
    use ncrawler_spi::{ScrapeJob, Scraper};

    let allow_hosts = flag_values(rest, "--allow-host");
    let (job, scraper): (ScrapeJob, Box<dyn Scraper>) = match source {
        "grafana" => (
            grafana_job(&out, rest, allow_hosts)?,
            Box::new(ncrawler_grafana::GrafanaScraper::new()),
        ),
        "spider" => (
            spider_job(rest, allow_hosts)?,
            Box::new(ncrawler_spider::SpiderScraper::new()),
        ),
        other => anyhow::bail!("scrape: unknown source `{other}` (expected grafana | spider)"),
    };

    let cancel = starter_ai::TokenCancel::new();
    let cancel_signal = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::warn!("interrupt received; cancelling scrape");
            cancel_signal.cancel();
        }
    });

    let artifact = scraper
        .scrape(job, &cancel)
        .await
        .map_err(|e| anyhow::anyhow!("scrape failed: {e}"))?;
    let dir = ArtifactStore::new(&out).write(&artifact)?;
    println!(
        "wrote {} ({} items, {} assets) to {}",
        artifact.source,
        artifact.items.len(),
        artifact.assets.len(),
        dir.display()
    );
    Ok(())
}

/// Build a Grafana [`ScrapeJob`] from the trailing flags.
fn grafana_job(
    out: &std::path::Path,
    rest: &[String],
    allow_hosts: Vec<String>,
) -> Result<ncrawler_spi::ScrapeJob> {
    use ncrawler_spi::ScrapeJob;
    let url = flag_value(rest, "--url").context("grafana scrape needs --url")?;
    let uid = flag_value(rest, "--uid").context("grafana scrape needs --uid")?;
    let mode = flag_value(rest, "--mode").unwrap_or_else(|| "both".to_owned());
    let mut options = serde_json::Map::new();
    options.insert("url".into(), url.into());
    options.insert("mode".into(), mode.into());
    options.insert("out".into(), out.display().to_string().into());
    if let Some(f) = flag_value(rest, "--from") {
        options.insert("from".into(), f.into());
    }
    if let Some(t) = flag_value(rest, "--to") {
        options.insert("to".into(), t.into());
    }
    if let Some(vf) = flag_value(rest, "--visual-fallback") {
        options.insert("visual_fallback".into(), vf.into());
    }
    let panels: Vec<serde_json::Value> = flag_values(rest, "--panel")
        .iter()
        .filter_map(|p| p.parse::<i64>().ok())
        .map(serde_json::Value::from)
        .collect();
    if !panels.is_empty() {
        options.insert("panels".into(), panels.into());
    }
    Ok(ScrapeJob {
        source: "grafana".into(),
        target: uid,
        allow_hosts,
        options: serde_json::Value::Object(options),
    })
}

/// Build a spider [`ScrapeJob`] from the trailing flags.
fn spider_job(rest: &[String], allow_hosts: Vec<String>) -> Result<ncrawler_spi::ScrapeJob> {
    use ncrawler_spi::ScrapeJob;
    let url = flag_value(rest, "--url").context("spider scrape needs --url")?;
    let mut options = serde_json::Map::new();
    if let Some(d) = flag_value(rest, "--depth").and_then(|s| s.parse::<u64>().ok()) {
        options.insert("depth".into(), d.into());
    }
    if let Some(l) = flag_value(rest, "--limit").and_then(|s| s.parse::<u64>().ok()) {
        options.insert("limit".into(), l.into());
    }
    if let Some(ms) = flag_value(rest, "--delay").and_then(|s| s.parse::<u64>().ok()) {
        options.insert("delay".into(), ms.into());
    }
    if flag_present(rest, "--render-js") {
        options.insert("render_js".into(), true.into());
    }
    if flag_present(rest, "--no-robots") {
        options.insert("respect_robots".into(), false.into());
    }
    Ok(ScrapeJob {
        source: "spider".into(),
        target: url,
        allow_hosts,
        options: serde_json::Value::Object(options),
    })
}

/// Build a derived artifact. `report-md` is deterministic + offline;
/// `report-ai` streams a Claude run, wiring Ctrl-C through
/// `starter_spi::ai::Cancel` (via `TokenCancel`) so a long run is
/// cancellable mid-stream (SCOPE: cancellation).
async fn run_build(builder: &str, artifact_dir: std::path::PathBuf, rest: &[String]) -> Result<()> {
    use ncrawler_spi::{BuildCtx, Builder};

    let artifact = read_artifact(&artifact_dir)
        .with_context(|| format!("reading artifact at {}", artifact_dir.display()))?;
    let mut options = serde_json::Map::new();
    if let Some(m) = flag_value(rest, "--model") {
        options.insert("model".into(), serde_json::Value::String(m));
    }
    let ctx = BuildCtx {
        artifact_dir: artifact_dir.clone(),
        options: serde_json::Value::Object(options),
    };

    // Ctrl-C flips the cancellation token the builder polls + selects on.
    let cancel = starter_ai::TokenCancel::new();
    let cancel_signal = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::warn!("interrupt received; cancelling build");
            cancel_signal.cancel();
        }
    });

    let output = match builder {
        "report-md" => {
            let b = ncrawler_report_md::MarkdownBuilder::new();
            b.build(&artifact, &ctx, &cancel).await
        }
        "report-ai" => {
            let dir = skills_dir();
            let b = ncrawler_report_ai::AiReportBuilder::with_defaults(dir)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            b.build(&artifact, &ctx, &cancel).await
        }
        other => {
            anyhow::bail!("build: unknown builder `{other}` (expected report-md | report-ai)")
        }
    }
    .map_err(|e| anyhow::anyhow!("build failed: {e}"))?;

    println!("{}", output.summary);
    for f in &output.files {
        println!("  wrote {}", f.display());
    }
    Ok(())
}

fn run_ls(source: Option<String>, since: Option<String>, out: std::path::PathBuf) -> Result<()> {
    let cutoff = match since {
        Some(s) => {
            Some(Utc::now() - parse_since(&s).with_context(|| format!("bad --since `{s}`"))?)
        }
        None => None,
    };
    let store = ArtifactStore::new(out);
    let entries = store.list(source.as_deref(), cutoff)?;
    if entries.is_empty() {
        println!("no artifacts found");
        return Ok(());
    }
    for e in entries {
        println!(
            "{}  {:<10}  {}",
            e.fetched_at.format("%Y-%m-%dT%H:%M:%SZ"),
            e.source,
            e.target
        );
    }
    Ok(())
}

fn run_show(artifact_dir: std::path::PathBuf) -> Result<()> {
    let a = read_artifact(&artifact_dir)
        .with_context(|| format!("reading artifact at {}", artifact_dir.display()))?;
    println!("source:        {}", a.source);
    println!("target:        {}", a.target);
    println!("fetched_at:    {}", a.fetched_at.to_rfc3339());
    println!("schema:        v{}", a.schema_version);
    println!("items:         {}", a.items.len());
    println!("assets:        {}", a.assets.len());
    for it in &a.items {
        let title = it.title.as_deref().unwrap_or("");
        println!("  - [{:?}] {} {}", it.kind, it.id, title);
    }
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt().with_env_filter(filter).try_init();
}

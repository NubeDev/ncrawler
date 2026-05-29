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
        Command::Scrape { source, out, .. } => {
            println!(
                "scrape: source `{source}` is not yet implemented (out: {})",
                out.display()
            );
        }
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

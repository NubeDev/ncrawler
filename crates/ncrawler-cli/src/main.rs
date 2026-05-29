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
            ..
        } => {
            println!(
                "build: builder `{builder}` is not yet implemented (artifact: {})",
                artifact_dir.display()
            );
        }
        Command::Ls { source, since, out } => run_ls(source, since, out)?,
        Command::Show { artifact_dir } => run_show(artifact_dir)?,
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

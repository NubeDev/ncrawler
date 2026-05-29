//! Clap-derive command surface (SCOPE: CLI surface).

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ncrawler",
    version,
    about = "Two-phase scrape -> build toolkit for observability surfaces."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scrape a source into an on-disk artifact.
    Scrape {
        /// Source name, e.g. `grafana` | `spider`.
        source: String,
        /// Artifact root (default `./artifacts`).
        #[arg(long, default_value = "./artifacts")]
        out: PathBuf,
        /// Source-specific flags, forwarded verbatim once implemented.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },
    /// Build a derived artifact from an existing scrape.
    Build {
        /// Builder name, e.g. `report-md` | `report-ai` | `vector`.
        builder: String,
        /// Artifact directory (accepts a `latest` symlink).
        artifact_dir: PathBuf,
        /// Builder-specific flags, forwarded verbatim once implemented.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },
    /// List artifacts by parsing directory names.
    Ls {
        #[arg(long)]
        source: Option<String>,
        /// Compact duration window, e.g. `24h`, `7d`.
        #[arg(long)]
        since: Option<String>,
        #[arg(long, default_value = "./artifacts")]
        out: PathBuf,
    },
    /// Show a one-line-per-item summary of an artifact (no build).
    Show {
        /// Artifact directory (accepts a `latest` symlink).
        artifact_dir: PathBuf,
    },
}

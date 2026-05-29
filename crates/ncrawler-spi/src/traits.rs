//! The `Scraper` / `Builder` seams and their call-time inputs.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::artifact::{Artifact, BuildOutput};
use crate::error::{BuildError, ScrapeError};
use crate::Cancel;

/// A unit of work for a [`Scraper`].
///
/// `options` is source-specific (mode, panels, depth, …) and parsed by
/// the scraper itself; the typed fields are common to all sources.
#[derive(Debug, Clone)]
pub struct ScrapeJob {
    /// Logical source name, matched against the registry.
    pub source: String,
    /// What to scrape: dashboard uid, URL, query, …
    pub target: String,
    /// SSRF allow-list; empty means "operator did not opt in"
    /// (SCOPE: security). Patterns are matched by each scraper.
    pub allow_hosts: Vec<String>,
    /// Source-specific knobs, best-effort.
    pub options: serde_json::Value,
}

/// Context handed to a [`Builder`]: where the artifact lives on disk and
/// any builder-specific options.
#[derive(Debug, Clone)]
pub struct BuildCtx {
    /// Absolute path to the artifact directory the build writes into.
    pub artifact_dir: PathBuf,
    /// Builder-specific knobs (skill id, model, store URL, …).
    pub options: serde_json::Value,
}

/// Produces an [`Artifact`] from a source.
#[async_trait]
pub trait Scraper: Send + Sync {
    fn name(&self) -> &str;

    async fn scrape(&self, job: ScrapeJob, cancel: &dyn Cancel) -> Result<Artifact, ScrapeError>;
}

/// Derives output (Markdown, AI summary, embeddings) from an artifact.
#[async_trait]
pub trait Builder: Send + Sync {
    fn name(&self) -> &str;

    async fn build(
        &self,
        artifact: &Artifact,
        ctx: &BuildCtx,
        cancel: &dyn Cancel,
    ) -> Result<BuildOutput, BuildError>;
}

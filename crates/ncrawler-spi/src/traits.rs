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
///
/// `schema_version` note: `dashboard_dirs` was added so a multi-artifact
/// renderer (`report-grafana`) can consume the `_instance/<host>/latest`
/// sidecar (named by `artifact_dir`) **plus** a list of per-dashboard
/// artifact directories in one build. Single-artifact builders leave it
/// empty; the field is additive and does not change the on-disk
/// [`Artifact`] schema, so `ARTIFACT_SCHEMA_VERSION` is unchanged.
#[derive(Debug, Clone)]
pub struct BuildCtx {
    /// Absolute path to the directory the build writes into. For
    /// single-artifact builders this is the artifact dir; for
    /// `report-grafana` it is the `_instance/<host>/latest` sidecar dir
    /// (where `REPORT.md` is written next to `instance.json`).
    pub artifact_dir: PathBuf,
    /// Additional per-dashboard artifact directories a multi-artifact
    /// builder renders over (REPORT §6b). Empty for single-artifact
    /// builders.
    pub dashboard_dirs: Vec<PathBuf>,
    /// Builder-specific knobs (skill id, model, store URL, …).
    pub options: serde_json::Value,
}

impl BuildCtx {
    /// Construct a single-artifact build context (no extra dashboard
    /// dirs). Convenience for the common case.
    pub fn new(artifact_dir: impl Into<PathBuf>, options: serde_json::Value) -> Self {
        Self {
            artifact_dir: artifact_dir.into(),
            dashboard_dirs: Vec::new(),
            options,
        }
    }
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

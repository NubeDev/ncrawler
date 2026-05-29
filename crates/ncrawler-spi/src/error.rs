//! Error types for the two phases.

use thiserror::Error;

/// Failure modes of the scrape phase.
#[derive(Debug, Error)]
pub enum ScrapeError {
    #[error("network error: {0}")]
    Network(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("target not found: {0}")]
    NotFound(String),
    /// `Visual` mode requires the `grafana-image-renderer` plugin; it is
    /// absent (SCOPE: visual strategy).
    #[error("grafana renderer plugin missing; install grafana-image-renderer or pass --visual-fallback chrome")]
    RendererPluginMissing,
    /// A URL was rejected by the SSRF allow-list (SCOPE: security).
    #[error("host blocked by SSRF allow-list: {0}")]
    SsrfBlocked(String),
    /// The requested scrape mode is not implemented yet (e.g. the
    /// Grafana `Visual` / `Both` paths land in a later milestone).
    #[error("scrape mode unsupported: {0}")]
    ModeUnsupported(String),
    #[error("scrape cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

/// Failure modes of the build phase.
#[derive(Debug, Error)]
pub enum BuildError {
    #[error("io error: {0}")]
    Io(String),
    #[error("no skill resolved for this artifact: {0}")]
    MissingSkill(String),
    #[error("ai runner error: {0}")]
    Ai(String),
    #[error("build cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

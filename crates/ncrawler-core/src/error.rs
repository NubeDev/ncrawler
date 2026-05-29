//! Errors surfaced by the artifact store.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// The artifact on disk was written with a newer major schema than
    /// this build understands (SCOPE: readers reject unknown majors).
    #[error("unsupported artifact schema version {found}; this build supports up to {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("not an artifact directory (missing artifact.json): {0}")]
    NotAnArtifact(String),
    #[error("malformed artifact directory name: {0}")]
    BadDirName(String),
    #[error("invalid --since duration: {0}")]
    BadSince(String),
}

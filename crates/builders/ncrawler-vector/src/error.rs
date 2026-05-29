use thiserror::Error;

/// Errors surfaced by the vector builder, its embedders, and its stores.
#[derive(Debug, Error)]
pub enum VectorError {
    #[error("embedding failed: {0}")]
    Embed(String),

    #[error("vector store error: {0}")]
    Store(String),

    #[error("unsupported or malformed store uri: {0}")]
    BadUri(String),

    #[error("operation cancelled")]
    Cancelled,

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

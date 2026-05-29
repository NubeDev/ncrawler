//! High-level entry point used by the CLI.

use ncrawler_spi::{Artifact, Cancel};

use crate::error::VectorError;
use crate::factory::{connect_store, default_embedder};
use crate::pipeline::{BuildSummary, VectorPipeline};
use crate::uri::StoreUri;

/// Chunk + embed `artifact` into the store named by `store_uri`.
///
/// `store_uri` is `lance://<path>` (default) or
/// `qdrant://<host:port>[/<collection>]`. The embedder is the crate's
/// configured default ([`FastEmbedEmbedder`](crate::embed::FastEmbedEmbedder)
/// when the `embed-fastembed` feature is on, otherwise the dependency-free
/// [`HashEmbedder`](crate::HashEmbedder)).
pub async fn build_vector(
    artifact: &Artifact,
    store_uri: &str,
    cancel: &dyn Cancel,
) -> Result<BuildSummary, VectorError> {
    let uri = StoreUri::parse(store_uri)?;
    let embedder = default_embedder().await?;
    let dim = embedder.dim();
    let store = connect_store(&uri, dim).await?;
    VectorPipeline::new(embedder, store)
        .build(artifact, cancel)
        .await
}

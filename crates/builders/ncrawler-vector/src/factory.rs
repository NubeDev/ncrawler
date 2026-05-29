//! Construct embedders and stores from runtime configuration (used by the CLI).

use crate::embed::Embedder;
use crate::error::VectorError;
use crate::store::VectorStore;
use crate::uri::StoreUri;

/// The default embedder for the running build of the crate: `fastembed` when
/// the `embed-fastembed` feature is on, otherwise the dependency-free
/// [`HashEmbedder`] fallback.
pub async fn default_embedder() -> Result<Box<dyn Embedder>, VectorError> {
    #[cfg(feature = "embed-fastembed")]
    {
        let e = crate::embed::FastEmbedEmbedder::new()?;
        Ok(Box::new(e))
    }
    #[cfg(not(feature = "embed-fastembed"))]
    {
        Ok(Box::new(crate::embed::HashEmbedder::default()))
    }
}

/// Connect to the store named by `uri`, creating it if necessary, sized for
/// `dim`-width vectors.
pub async fn connect_store(
    uri: &StoreUri,
    dim: usize,
) -> Result<Box<dyn VectorStore>, VectorError> {
    match uri {
        StoreUri::Lance { path } => {
            #[cfg(feature = "store-lance")]
            {
                let s = crate::store::lance::LanceStore::open(path, dim).await?;
                Ok(Box::new(s))
            }
            #[cfg(not(feature = "store-lance"))]
            {
                let _ = (path, dim);
                Err(VectorError::BadUri(
                    "lance:// requires the `store-lance` feature".into(),
                ))
            }
        }
        StoreUri::Qdrant { url, collection } => {
            #[cfg(feature = "store-qdrant")]
            {
                let s = crate::store::qdrant::QdrantStore::open(url, collection, dim).await?;
                Ok(Box::new(s))
            }
            #[cfg(not(feature = "store-qdrant"))]
            {
                let _ = (url, collection, dim);
                Err(VectorError::BadUri(
                    "qdrant:// requires the `store-qdrant` feature".into(),
                ))
            }
        }
    }
}

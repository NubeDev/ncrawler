//! Embedders: turn chunk text into fixed-width float vectors.

use async_trait::async_trait;

use crate::error::VectorError;

/// Produces dense embeddings for a batch of texts.
///
/// Implementations must return one vector per input text, each of length
/// [`Embedder::dim`].
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Dimensionality of the vectors this embedder produces.
    fn dim(&self) -> usize;

    /// Embed a batch. Returns `texts.len()` vectors, each of length `dim()`.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, VectorError>;
}

/// Forward `Embedder` through a boxed trait object so callers can hold a
/// `Box<dyn Embedder>` and still drive [`VectorBuilder`](crate::VectorBuilder).
#[async_trait]
impl Embedder for Box<dyn Embedder> {
    fn dim(&self) -> usize {
        (**self).dim()
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, VectorError> {
        (**self).embed(texts).await
    }
}

/// Deterministic, dependency-free embedder.
///
/// Not semantically meaningful — it hashes token bytes into a bag-of-words
/// frequency vector — but it is fast, offline, and stable, which makes it the
/// embedder used by unit tests and the documented in-memory mock path. The
/// real default embedder is [`FastEmbedEmbedder`].
#[derive(Debug, Clone)]
pub struct HashEmbedder {
    dim: usize,
}

impl HashEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim: dim.max(1) }
    }
}

impl Default for HashEmbedder {
    fn default() -> Self {
        Self::new(64)
    }
}

#[async_trait]
impl Embedder for HashEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, VectorError> {
        Ok(texts.iter().map(|t| embed_one(t, self.dim)).collect())
    }
}

fn embed_one(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for token in text.split_whitespace() {
        let h = blake3::hash(token.to_lowercase().as_bytes());
        let idx = (u32::from_le_bytes(h.as_bytes()[..4].try_into().unwrap()) as usize) % dim;
        v[idx] += 1.0;
    }
    // L2-normalise so cosine similarity behaves.
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// Local ONNX embedder backed by `fastembed-rs` (Apache-2.0). No network at
/// inference time once the model is cached. Uses the library's default model.
#[cfg(feature = "embed-fastembed")]
pub struct FastEmbedEmbedder {
    model: tokio::sync::Mutex<fastembed::TextEmbedding>,
    dim: usize,
}

#[cfg(feature = "embed-fastembed")]
impl FastEmbedEmbedder {
    /// Build with the library default model (all-MiniLM-L6-v2, dim 384).
    pub fn new() -> Result<Self, VectorError> {
        let model = fastembed::TextEmbedding::try_new(Default::default())
            .map_err(|e| VectorError::Embed(e.to_string()))?;
        Ok(Self {
            model: tokio::sync::Mutex::new(model),
            dim: 384,
        })
    }
}

#[cfg(feature = "embed-fastembed")]
#[async_trait]
impl Embedder for FastEmbedEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, VectorError> {
        let docs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let guard = self.model.lock().await;
        guard
            .embed(docs, None)
            .map_err(|e| VectorError::Embed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_embedder_is_deterministic_and_normalised() {
        let e = HashEmbedder::new(32);
        let a = e.embed(&["cpu usage high".into()]).await.unwrap();
        let b = e.embed(&["cpu usage high".into()]).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(a[0].len(), 32);
        let norm: f32 = a[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[tokio::test]
    async fn batch_returns_one_vector_per_input() {
        let e = HashEmbedder::new(16);
        let out = e
            .embed(&["a".into(), "b".into(), "c".into()])
            .await
            .unwrap();
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|v| v.len() == 16));
    }
}

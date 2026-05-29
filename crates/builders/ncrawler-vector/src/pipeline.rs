//! The vector build pipeline: chunk → embed → upsert.

use ncrawler_spi::{Artifact, Cancel};

use crate::chunk::{chunk_artifact, ChunkConfig};
use crate::embed::Embedder;
use crate::error::VectorError;
use crate::store::{VectorRecord, VectorStore};

/// What a build produced, for logging / CLI summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildSummary {
    pub items: usize,
    pub chunks: usize,
    pub dim: usize,
}

/// Wires an [`Embedder`] and a [`VectorStore`] into a runnable pipeline.
pub struct VectorPipeline<E, S> {
    embedder: E,
    store: S,
    chunk: ChunkConfig,
}

impl<E: Embedder, S: VectorStore> VectorPipeline<E, S> {
    pub fn new(embedder: E, store: S) -> Self {
        Self {
            embedder,
            store,
            chunk: ChunkConfig::default(),
        }
    }

    /// Override the chunking configuration.
    pub fn with_chunk_config(mut self, chunk: ChunkConfig) -> Self {
        self.chunk = chunk;
        self
    }

    /// Chunk, embed, and upsert every item in `artifact`.
    pub async fn build(
        &self,
        artifact: &Artifact,
        cancel: &dyn Cancel,
    ) -> Result<BuildSummary, VectorError> {
        let chunks = chunk_artifact(artifact, self.chunk);
        if cancel.is_cancelled() {
            return Err(VectorError::Cancelled);
        }
        if chunks.is_empty() {
            return Ok(BuildSummary {
                items: 0,
                chunks: 0,
                dim: self.embedder.dim(),
            });
        }

        let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        let vectors = self.embedder.embed(&texts).await?;
        if vectors.len() != chunks.len() {
            return Err(VectorError::Embed(format!(
                "embedder returned {} vectors for {} chunks",
                vectors.len(),
                chunks.len()
            )));
        }
        if cancel.is_cancelled() {
            return Err(VectorError::Cancelled);
        }

        let records: Vec<VectorRecord> = chunks
            .into_iter()
            .zip(vectors)
            .map(|(c, vector)| VectorRecord {
                source: artifact.source.clone(),
                target: artifact.target.clone(),
                item_id: c.item_id,
                seq: c.seq,
                text: c.text,
                vector,
            })
            .collect();

        let summary = BuildSummary {
            items: artifact.items.len(),
            chunks: records.len(),
            dim: self.embedder.dim(),
        };
        self.store.upsert(&records).await?;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::HashEmbedder;
    use crate::store::memory::MemoryStore;
    use chrono::Utc;
    use ncrawler_spi::{Item, ItemKind};

    /// A no-op [`Cancel`] for tests (the real trait has no built-in one).
    struct NoCancel;
    impl Cancel for NoCancel {
        fn is_cancelled(&self) -> bool {
            false
        }
        fn cancelled(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(std::future::pending())
        }
    }

    fn artifact(items: Vec<Item>) -> Artifact {
        let mut a = Artifact::new("grafana", "abc123", Utc::now());
        a.items = items;
        a
    }

    fn item(id: &str, text: &str) -> Item {
        Item {
            id: id.into(),
            kind: ItemKind::Panel,
            title: Some(id.into()),
            text: text.into(),
            data: None,
            tags: vec![],
        }
    }

    #[tokio::test]
    async fn build_then_rebuild_same_target_is_idempotent() {
        let pipeline = VectorPipeline::new(HashEmbedder::new(16), MemoryStore::new());
        let art = artifact(vec![item("panel-1", "cpu high"), item("panel-2", "mem ok")]);

        let s1 = pipeline.build(&art, &NoCancel).await.unwrap();
        let count1 = pipeline_store(&pipeline).count().await.unwrap();
        let s2 = pipeline.build(&art, &NoCancel).await.unwrap();
        let count2 = pipeline_store(&pipeline).count().await.unwrap();

        assert_eq!(s1, s2);
        assert_eq!(
            count1, count2,
            "re-build of same target must not grow the store"
        );
        assert_eq!(s1.items, 2);
    }

    #[tokio::test]
    async fn empty_artifact_builds_nothing() {
        let pipeline = VectorPipeline::new(HashEmbedder::new(8), MemoryStore::new());
        let summary = pipeline.build(&artifact(vec![]), &NoCancel).await.unwrap();
        assert_eq!(summary.chunks, 0);
        assert_eq!(pipeline_store(&pipeline).count().await.unwrap(), 0);
    }

    fn pipeline_store(p: &VectorPipeline<HashEmbedder, MemoryStore>) -> &MemoryStore {
        &p.store
    }
}

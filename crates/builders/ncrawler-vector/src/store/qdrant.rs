//! Optional [`VectorStore`]: Qdrant (Apache-2.0), feature `store-qdrant`.
//!
//! NOT a default backend: Qdrant is a server, which conflicts with the
//! no-long-lived-process default path. Provided for operators who already run
//! one. Points are keyed by a deterministic UUID derived from the chunk key,
//! and the `(source, target, item_id)` triple is stored as a payload field so
//! re-scrapes delete-by-filter before re-inserting.

use async_trait::async_trait;
use qdrant_client::qdrant::{
    Condition, CreateCollectionBuilder, DeletePointsBuilder, Distance, Filter, PointStruct,
    UpsertPointsBuilder, VectorParamsBuilder,
};
use qdrant_client::{Payload, Qdrant};

use super::{distinct_triples, VectorRecord, VectorStore};
use crate::error::VectorError;

/// Qdrant-backed store targeting one collection.
pub struct QdrantStore {
    client: Qdrant,
    collection: String,
    dim: u64,
}

impl QdrantStore {
    /// Connect to `url` (e.g. `http://localhost:6334`) and ensure `collection`
    /// exists with the given vector width.
    pub async fn open(url: &str, collection: &str, dim: usize) -> Result<Self, VectorError> {
        let client = Qdrant::from_url(url).build().map_err(store_err)?;
        let store = Self {
            client,
            collection: collection.to_string(),
            dim: dim as u64,
        };
        store.ensure_collection().await?;
        Ok(store)
    }

    async fn ensure_collection(&self) -> Result<(), VectorError> {
        let exists = self
            .client
            .collection_exists(&self.collection)
            .await
            .map_err(store_err)?;
        if !exists {
            self.client
                .create_collection(
                    CreateCollectionBuilder::new(&self.collection)
                        .vectors_config(VectorParamsBuilder::new(self.dim, Distance::Cosine)),
                )
                .await
                .map_err(store_err)?;
        }
        Ok(())
    }
}

#[async_trait]
impl VectorStore for QdrantStore {
    async fn upsert(&self, records: &[VectorRecord]) -> Result<(), VectorError> {
        if records.is_empty() {
            return Ok(());
        }
        // Delete every existing point for the triples in this batch.
        let conditions: Vec<Condition> = distinct_triples(records)
            .into_iter()
            .map(|t| Condition::matches("triple", t))
            .collect();
        let filter = Filter::should(conditions);
        self.client
            .delete_points(
                DeletePointsBuilder::new(&self.collection)
                    .points(filter)
                    .wait(true),
            )
            .await
            .map_err(store_err)?;

        let points: Vec<PointStruct> = records.iter().map(point_of).collect();
        self.client
            .upsert_points(UpsertPointsBuilder::new(&self.collection, points).wait(true))
            .await
            .map_err(store_err)?;
        Ok(())
    }

    async fn count(&self) -> Result<usize, VectorError> {
        let info = self
            .client
            .collection_info(&self.collection)
            .await
            .map_err(store_err)?;
        Ok(info.result.and_then(|r| r.points_count).unwrap_or(0) as usize)
    }
}

fn point_of(r: &VectorRecord) -> PointStruct {
    let id = uuid_from_key(&r.key());
    let payload: Payload = serde_json::json!({
        "triple": r.triple(),
        "source": r.source,
        "target": r.target,
        "item_id": r.item_id,
        "seq": r.seq as u64,
        "text": r.text,
    })
    .try_into()
    .unwrap_or_default();
    PointStruct::new(id, r.vector.clone(), payload)
}

/// Deterministic UUID (v-agnostic, formatted) from a chunk key, so the same
/// chunk always maps to the same point id.
fn uuid_from_key(key: &str) -> String {
    let h = blake3::hash(key.as_bytes());
    let b = h.as_bytes();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15]
    )
}

fn store_err<E: std::fmt::Display>(e: E) -> VectorError {
    VectorError::Store(e.to_string())
}

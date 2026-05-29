//! Vector stores: where embedded chunks live.
//!
//! The central contract is *upsert idempotency*. [`VectorStore::upsert`]
//! takes a batch of [`VectorRecord`]s and, for every distinct
//! `(source, target, item_id)` triple present in the batch, MUST first remove
//! any existing rows for that triple and then insert the new ones. This makes
//! re-scrapes of the same target overwrite rather than duplicate, even when a
//! re-scrape produces a different number of chunks.

use async_trait::async_trait;

use crate::error::VectorError;

pub mod memory;

#[cfg(feature = "store-lance")]
pub mod lance;
#[cfg(feature = "store-qdrant")]
pub mod qdrant;

const SEP: char = '\u{1f}';

/// One embedded chunk, ready to persist.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorRecord {
    pub source: String,
    pub target: String,
    pub item_id: String,
    pub seq: usize,
    pub text: String,
    pub vector: Vec<f32>,
}

impl VectorRecord {
    /// The upsert grouping key: `(source, target, item_id)`.
    pub fn triple(&self) -> String {
        triple_key(&self.source, &self.target, &self.item_id)
    }

    /// The per-chunk primary key: triple plus chunk seq.
    pub fn key(&self) -> String {
        record_key(&self.source, &self.target, &self.item_id, self.seq)
    }
}

/// Stable grouping key for the `(source, target, item_id)` triple.
pub fn triple_key(source: &str, target: &str, item_id: &str) -> String {
    format!("{source}{SEP}{target}{SEP}{item_id}")
}

/// Stable per-chunk primary key.
pub fn record_key(source: &str, target: &str, item_id: &str, seq: usize) -> String {
    format!("{}{SEP}{seq}", triple_key(source, target, item_id))
}

/// A pluggable destination for embedded chunks.
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Upsert a batch with delete-then-insert semantics per `(source,
    /// target, item_id)` triple. See the module docs for the full contract.
    async fn upsert(&self, records: &[VectorRecord]) -> Result<(), VectorError>;

    /// Total number of chunk rows currently stored.
    async fn count(&self) -> Result<usize, VectorError>;
}

/// Forward `VectorStore` through a boxed trait object.
#[async_trait]
impl VectorStore for Box<dyn VectorStore> {
    async fn upsert(&self, records: &[VectorRecord]) -> Result<(), VectorError> {
        (**self).upsert(records).await
    }

    async fn count(&self) -> Result<usize, VectorError> {
        (**self).count().await
    }
}

/// Distinct triples present in a batch, preserving first-seen order.
pub(crate) fn distinct_triples(records: &[VectorRecord]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for r in records {
        let t = r.triple();
        if seen.insert(t.clone()) {
            out.push(t);
        }
    }
    out
}

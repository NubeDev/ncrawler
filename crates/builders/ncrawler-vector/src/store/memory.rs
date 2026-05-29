//! In-memory [`VectorStore`] — the documented mock used by unit tests and a
//! reference implementation of the upsert-idempotency contract.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;

use super::{distinct_triples, VectorRecord, VectorStore};
use crate::error::VectorError;

/// A simple, thread-safe, process-local vector store.
///
/// Rows are keyed by [`VectorRecord::key`]; the delete-then-insert step keys
/// by [`VectorRecord::triple`] so stale chunks from a previous, longer scrape
/// are removed.
#[derive(Debug, Default)]
pub struct MemoryStore {
    rows: Mutex<BTreeMap<String, VectorRecord>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of all stored records (test helper), ordered by key.
    pub fn rows(&self) -> Vec<VectorRecord> {
        self.rows.lock().unwrap().values().cloned().collect()
    }
}

#[async_trait]
impl VectorStore for MemoryStore {
    async fn upsert(&self, records: &[VectorRecord]) -> Result<(), VectorError> {
        let triples = distinct_triples(records);
        let mut rows = self.rows.lock().unwrap();
        // Delete every existing row belonging to a triple in this batch.
        rows.retain(|_, r| !triples.contains(&r.triple()));
        // Insert (last write wins on duplicate per-chunk keys within a batch).
        for r in records {
            rows.insert(r.key(), r.clone());
        }
        Ok(())
    }

    async fn count(&self) -> Result<usize, VectorError> {
        Ok(self.rows.lock().unwrap().len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(item_id: &str, seq: usize, text: &str) -> VectorRecord {
        VectorRecord {
            source: "grafana".into(),
            target: "abc123".into(),
            item_id: item_id.into(),
            seq,
            text: text.into(),
            vector: vec![0.1, 0.2, 0.3],
        }
    }

    #[tokio::test]
    async fn upsert_is_idempotent_on_repeat() {
        let s = MemoryStore::new();
        let batch = vec![rec("panel-1", 0, "a"), rec("panel-1", 1, "b")];
        s.upsert(&batch).await.unwrap();
        s.upsert(&batch).await.unwrap();
        assert_eq!(s.count().await.unwrap(), 2, "re-upsert must not duplicate");
    }

    #[tokio::test]
    async fn rescrape_overwrites_content_for_same_triple() {
        let s = MemoryStore::new();
        s.upsert(&[rec("panel-1", 0, "old")]).await.unwrap();
        s.upsert(&[rec("panel-1", 0, "new")]).await.unwrap();
        let rows = s.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "new");
    }

    #[tokio::test]
    async fn rescrape_with_fewer_chunks_drops_stale() {
        let s = MemoryStore::new();
        s.upsert(&[
            rec("panel-1", 0, "a"),
            rec("panel-1", 1, "b"),
            rec("panel-1", 2, "c"),
        ])
        .await
        .unwrap();
        assert_eq!(s.count().await.unwrap(), 3);
        // Re-scrape now only yields one chunk for the same panel.
        s.upsert(&[rec("panel-1", 0, "a-merged")]).await.unwrap();
        assert_eq!(s.count().await.unwrap(), 1, "stale chunks must be removed");
    }

    #[tokio::test]
    async fn distinct_triples_are_independent() {
        let s = MemoryStore::new();
        s.upsert(&[rec("panel-1", 0, "a"), rec("panel-2", 0, "b")])
            .await
            .unwrap();
        // Re-scrape panel-1 only; panel-2 must survive untouched.
        s.upsert(&[rec("panel-1", 0, "a2")]).await.unwrap();
        assert_eq!(s.count().await.unwrap(), 2);
    }
}

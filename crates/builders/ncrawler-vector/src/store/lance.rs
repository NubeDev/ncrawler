//! Default [`VectorStore`]: LanceDB (Apache-2.0) — in-process, a single
//! directory on disk, no server. Consistent with the daemon-free goal.

use std::sync::Arc;

use arrow_array::{
    FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use lancedb::{connect, Connection};
use tokio::sync::Mutex;

use super::{distinct_triples, VectorRecord, VectorStore};
use crate::error::VectorError;

const TABLE: &str = "chunks";

/// LanceDB-backed store rooted at a single directory.
pub struct LanceStore {
    conn: Connection,
    dim: usize,
    /// Serialises create/add/delete so concurrent upserts don't race on table
    /// creation. LanceDB itself is multi-writer, but first-create is not.
    write_lock: Mutex<()>,
}

impl LanceStore {
    /// Open (or lazily create on first upsert) a store at `path`, expecting
    /// vectors of width `dim`.
    pub async fn open(path: &str, dim: usize) -> Result<Self, VectorError> {
        let conn = connect(path).execute().await.map_err(store_err)?;
        Ok(Self {
            conn,
            dim,
            write_lock: Mutex::new(()),
        })
    }

    fn schema(&self) -> Arc<Schema> {
        let item = Arc::new(Field::new("item", DataType::Float32, true));
        Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("triple", DataType::Utf8, false),
            Field::new("source", DataType::Utf8, false),
            Field::new("target", DataType::Utf8, false),
            Field::new("item_id", DataType::Utf8, false),
            Field::new("seq", DataType::UInt32, false),
            Field::new("text", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(item, self.dim as i32),
                false,
            ),
        ]))
    }

    fn batch(&self, records: &[VectorRecord]) -> Result<RecordBatch, VectorError> {
        let keys = StringArray::from_iter_values(records.iter().map(|r| r.key()));
        let triples = StringArray::from_iter_values(records.iter().map(|r| r.triple()));
        let sources = StringArray::from_iter_values(records.iter().map(|r| r.source.clone()));
        let targets = StringArray::from_iter_values(records.iter().map(|r| r.target.clone()));
        let item_ids = StringArray::from_iter_values(records.iter().map(|r| r.item_id.clone()));
        let seqs = UInt32Array::from_iter_values(records.iter().map(|r| r.seq as u32));
        let texts = StringArray::from_iter_values(records.iter().map(|r| r.text.clone()));

        let flat: Vec<f32> = records
            .iter()
            .flat_map(|r| r.vector.iter().copied())
            .collect();
        let values = Float32Array::from(flat);
        let item = Arc::new(Field::new("item", DataType::Float32, true));
        let vectors = FixedSizeListArray::try_new(item, self.dim as i32, Arc::new(values), None)
            .map_err(|e| VectorError::Store(e.to_string()))?;

        RecordBatch::try_new(
            self.schema(),
            vec![
                Arc::new(keys),
                Arc::new(triples),
                Arc::new(sources),
                Arc::new(targets),
                Arc::new(item_ids),
                Arc::new(seqs),
                Arc::new(texts),
                Arc::new(vectors),
            ],
        )
        .map_err(|e| VectorError::Store(e.to_string()))
    }

    async fn table_exists(&self) -> Result<bool, VectorError> {
        let names = self.conn.table_names().execute().await.map_err(store_err)?;
        Ok(names.iter().any(|n| n == TABLE))
    }
}

#[async_trait]
impl VectorStore for LanceStore {
    async fn upsert(&self, records: &[VectorRecord]) -> Result<(), VectorError> {
        if records.is_empty() {
            return Ok(());
        }
        for r in records {
            if r.vector.len() != self.dim {
                return Err(VectorError::Store(format!(
                    "vector width {} != store dim {}",
                    r.vector.len(),
                    self.dim
                )));
            }
        }

        let _guard = self.write_lock.lock().await;
        let batch = self.batch(records)?;
        let schema = self.schema();

        if self.table_exists().await? {
            let tbl = self
                .conn
                .open_table(TABLE)
                .execute()
                .await
                .map_err(store_err)?;
            // Delete-then-insert per triple so re-scrapes overwrite.
            let predicate = triple_in_predicate(records);
            tbl.delete(&predicate).await.map_err(store_err)?;
            let reader = RecordBatchIterator::new([Ok(batch)], schema);
            tbl.add(Box::new(reader))
                .execute()
                .await
                .map_err(store_err)?;
        } else {
            let reader = RecordBatchIterator::new([Ok(batch)], schema);
            self.conn
                .create_table(TABLE, Box::new(reader))
                .execute()
                .await
                .map_err(store_err)?;
        }
        Ok(())
    }

    async fn count(&self) -> Result<usize, VectorError> {
        if !self.table_exists().await? {
            return Ok(0);
        }
        let tbl = self
            .conn
            .open_table(TABLE)
            .execute()
            .await
            .map_err(store_err)?;
        tbl.count_rows(None).await.map_err(store_err)
    }
}

fn store_err<E: std::fmt::Display>(e: E) -> VectorError {
    VectorError::Store(e.to_string())
}

/// `triple IN ('a','b',...)` with single quotes doubled for SQL safety.
fn triple_in_predicate(records: &[VectorRecord]) -> String {
    let list = distinct_triples(records)
        .iter()
        .map(|t| format!("'{}'", t.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    format!("triple IN ({list})")
}

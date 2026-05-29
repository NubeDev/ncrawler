//! `ncrawler-vector` — the (stretch) vector builder.
//!
//! Pipeline: chunk `Item`s into embedding-sized pieces → embed each chunk
//! via a pluggable [`Embedder`] → upsert into a pluggable [`VectorStore`]
//! keyed on `(source, target, item_id)`.
//!
//! Because `Item.id` is stable across re-scrapes of the same target (see
//! `SCOPE.md`), upserts *overwrite* prior chunks for the same panel/page
//! rather than duplicating them. The default backends are
//! [`FastEmbedEmbedder`](embed::FastEmbedEmbedder) (ONNX, no network) and
//! [`LanceStore`](store::lance::LanceStore) (in-process, on-disk) — both
//! daemon-free, consistent with the no-long-lived-process goal.
//!
//! The CLI calls [`build_vector`], which resolves the embedder + store from
//! a `--store` URI (default [`DEFAULT_STORE_URI`]) and runs the pipeline.

mod api;
mod chunk;
mod embed;
mod error;
mod factory;
mod pipeline;
mod store;
mod uri;

pub use api::build_vector;
pub use chunk::{chunk_artifact, chunk_item, Chunk, ChunkConfig};
pub use embed::{Embedder, HashEmbedder};
pub use error::VectorError;
pub use factory::{connect_store, default_embedder};
pub use pipeline::{BuildSummary, VectorPipeline};
pub use store::memory::MemoryStore;
pub use store::{record_key, triple_key, VectorRecord, VectorStore};
pub use uri::{StoreUri, DEFAULT_STORE_URI};

#[cfg(feature = "embed-fastembed")]
pub use embed::FastEmbedEmbedder;
#[cfg(feature = "store-lance")]
pub use store::lance::LanceStore;
#[cfg(feature = "store-qdrant")]
pub use store::qdrant::QdrantStore;

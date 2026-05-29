//! `ncrawler-core` — the on-disk artifact store and dirname index.
//!
//! The store is the contract joining the scrape and build phases:
//! timestamped directories, a per-`(source, target)` `latest` symlink,
//! `0700` permissions, and JSON round-trips that reject unknown major
//! schema versions.

mod error;
mod sidecar;
mod since;
mod store;

pub use error::StoreError;
pub use sidecar::{FactsOrigin, InstanceFacts, InstanceSidecar, INSTANCE_SCHEMA_VERSION};
pub use since::parse_since;
pub use store::{dir_name, read_artifact, safe, ArtifactStore, ListEntry};

#[cfg(test)]
mod tests;

//! `ncrawler-spi` — the contract shared by scrapers and builders.
//!
//! Types only; no implementation dependencies beyond serde/chrono and
//! the `Cancel` seam re-exported from `starter-spi`. The on-disk
//! [`Artifact`] is the boundary between the scrape and build phases.

mod artifact;
mod error;
mod traits;

/// Major version of the [`Artifact`] schema. Bumped on any breaking
/// change to the typed fields; readers reject unknown majors.
pub const ARTIFACT_SCHEMA_VERSION: u32 = 1;

pub use artifact::{Artifact, Asset, BuildOutput, Item, ItemKind};
pub use error::{BuildError, ScrapeError};
pub use traits::{BuildCtx, Builder, ScrapeJob, Scraper};

/// Cancellation handle, re-exported from `starter_spi::ai` so
/// cancellation composes with the existing `ClaudeRunner` (SCOPE).
pub use starter_spi::ai::Cancel;

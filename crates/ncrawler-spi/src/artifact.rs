//! The on-disk contract: `Artifact` and its parts.
//!
//! These typed fields are the seam between the scrape and build phases.
//! `schema_version` is bumped on any breaking change to them; readers
//! reject unknown majors (see [`crate::ARTIFACT_SCHEMA_VERSION`]).

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single scraped snapshot of one `target` from one `source`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Major schema version this artifact was written with.
    pub schema_version: u32,
    /// Logical source, e.g. `"grafana"` | `"spider"`.
    pub source: String,
    /// What was scraped: a dashboard uid, URL, query, …
    pub target: String,
    /// When the scrape completed.
    pub fetched_at: DateTime<Utc>,
    /// Structured, human-readable items.
    pub items: Vec<Item>,
    /// Binary/extra payloads; paths are relative to the artifact dir.
    pub assets: Vec<Asset>,
    /// Source-specific, best-effort metadata. Builders MUST NOT depend
    /// on `meta` keys for correctness (SCOPE: versioning rules).
    pub meta: serde_json::Value,
}

impl Artifact {
    /// Construct an artifact stamped with the current schema version.
    pub fn new(
        source: impl Into<String>,
        target: impl Into<String>,
        fetched_at: DateTime<Utc>,
    ) -> Self {
        Self {
            schema_version: crate::ARTIFACT_SCHEMA_VERSION,
            source: source.into(),
            target: target.into(),
            fetched_at,
            items: Vec::new(),
            assets: Vec::new(),
            meta: serde_json::Value::Null,
        }
    }
}

/// One logical thing within an artifact (a panel, a page, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// STABLE across re-scrapes of the same target (SCOPE: id stability).
    pub id: String,
    pub kind: ItemKind,
    pub title: Option<String>,
    /// Human-readable rendering.
    pub text: String,
    /// Structured payload, when available.
    pub data: Option<serde_json::Value>,
    pub tags: Vec<String>,
}

/// The closed set of item shapes v1 understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Panel,
    Page,
    ApiResponse,
    Annotation,
    Alert,
    Log,
}

/// A binary or extra file living under the artifact's `assets/` dir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    /// Path relative to the artifact directory.
    pub path: PathBuf,
    pub mime: String,
    pub label: String,
    /// Links to [`Item::id`] when the asset belongs to a specific item
    /// (e.g. a panel screenshot); `None` for whole-artifact assets
    /// (SCOPE: Asset ↔ Item linkage). Builders merge by `item_id`;
    /// string-matching on `label` is forbidden.
    pub item_id: Option<String>,
}

/// What a [`crate::Builder`] produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildOutput {
    /// Files written, relative to the artifact directory.
    pub files: Vec<PathBuf>,
    /// Short human-readable summary of the build.
    pub summary: String,
}

//! The per-instance sidecar artifact and its resolved view.
//!
//! Per-dashboard artifacts used to embed the full `/api/search` inventory
//! in `meta.search` — at `--all` scale that copies the whole inventory
//! once per dashboard (REPORT §6a). The fix is to persist instance-wide
//! data **once** in a sidecar artifact written to
//! `<root>/<source>/_instance/<host>/<rfc3339-utc>__instance/instance.json`,
//! with a per-host `latest` symlink, and have the reader fall back to a
//! legacy artifact's `meta.search` only during the migration grace.
//!
//! The sidecar reuses the on-disk store machinery in [`crate::store`] (the
//! same `0700` dirs + `latest` symlink discipline as the main store); it
//! does NOT fork a second store.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Major schema version of the instance sidecar on disk. Bumped on any
/// breaking change to the typed fields below; readers reject unknown
/// majors (mirrors the artifact `schema_version` discipline).
pub const INSTANCE_SCHEMA_VERSION: u32 = 1;

/// The on-disk instance sidecar: instance-wide Grafana facts persisted
/// once per scrape run.
///
/// The four payload fields are intentionally untyped [`Value`]s: they
/// carry whatever the corresponding endpoint returned (best-effort, like
/// `Artifact::meta`), so a future Grafana version growing a field does not
/// require a schema bump. The *shape* (these four keys + the envelope) is
/// what `INSTANCE_SCHEMA_VERSION` versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceSidecar {
    /// Major schema version this sidecar was written with.
    pub schema_version: u32,
    /// The Grafana host this sidecar describes (e.g. `rd-esr.nube-iiot.com`).
    pub host: String,
    /// When the sidecar was written.
    pub fetched_at: DateTime<Utc>,
    /// `/api/search` inventory: title / uid / folder / tags per dashboard.
    pub search: Value,
    /// `/api/datasources`: type, default flag, … (persisted for query
    /// resolution at report time; previously fetched then discarded).
    pub datasources: Value,
    /// `/api/health` + `/api/frontend/settings`: version, edition,
    /// rendererAvailable.
    pub instance: Value,
    /// `/api/folders`: the sidebar/nav tree.
    pub folders: Value,
}

impl InstanceSidecar {
    /// Construct an empty sidecar stamped with the current schema version.
    /// Payload fields default to JSON `null` and are filled by the caller.
    pub fn new(host: impl Into<String>, fetched_at: DateTime<Utc>) -> Self {
        Self {
            schema_version: INSTANCE_SCHEMA_VERSION,
            host: host.into(),
            fetched_at,
            search: Value::Null,
            datasources: Value::Null,
            instance: Value::Null,
            folders: Value::Null,
        }
    }
}

/// Where a resolved [`InstanceFacts`] came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactsOrigin {
    /// Read from the `_instance/<host>/latest` sidecar (the preferred
    /// path). Carries the resolved `instance.json` path.
    Sidecar(PathBuf),
    /// Migration grace: no sidecar was present, so the inventory was
    /// recovered from a legacy per-dashboard artifact's `meta.search`.
    /// Carries the legacy `artifact.json` path (named in a load-time
    /// `tracing::warn`).
    LegacyMeta(PathBuf),
}

/// The reader's resolved view of one Grafana instance, produced by
/// [`crate::ArtifactStore::read_instance_facts`].
///
/// When [`FactsOrigin::LegacyMeta`], only `search` is populated (the
/// legacy artifact never carried datasources / instance / folders); the
/// other fields are JSON `null`.
#[derive(Debug, Clone)]
pub struct InstanceFacts {
    pub host: String,
    pub search: Value,
    pub datasources: Value,
    pub instance: Value,
    pub folders: Value,
    pub origin: FactsOrigin,
}

impl InstanceFacts {
    pub(crate) fn from_sidecar(sidecar: InstanceSidecar, path: PathBuf) -> Self {
        Self {
            host: sidecar.host,
            search: sidecar.search,
            datasources: sidecar.datasources,
            instance: sidecar.instance,
            folders: sidecar.folders,
            origin: FactsOrigin::Sidecar(path),
        }
    }

    pub(crate) fn from_legacy_meta(host: String, search: Value, path: PathBuf) -> Self {
        Self {
            host,
            search,
            datasources: Value::Null,
            instance: Value::Null,
            folders: Value::Null,
            origin: FactsOrigin::LegacyMeta(path),
        }
    }
}

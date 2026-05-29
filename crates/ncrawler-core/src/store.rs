//! On-disk artifact store.
//!
//! Layout (SCOPE: on-disk artifact layout):
//! ```text
//! <root>/
//! ├── <source>/<safe(target)>/latest -> ../../<dirname>
//! └── <dirname>/
//!     ├── artifact.json
//!     └── assets/
//! ```
//! where `<dirname>` is `<rfc3339-utc>__<source>__<safe(target)>`. The
//! dirname is the only index — `ls --since` parses it, there is no
//! separate manifest.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use ncrawler_spi::{Artifact, ARTIFACT_SCHEMA_VERSION};

use crate::error::StoreError;
use crate::sidecar::{InstanceFacts, InstanceSidecar, INSTANCE_SCHEMA_VERSION};

/// Timestamp format used in directory names. Colons are illegal/awkward
/// in paths, so the RFC3339 `:` separators become `-` and we keep the
/// trailing `Z` to mark UTC.
const DIR_TS_FMT: &str = "%Y-%m-%dT%H-%M-%SZ";

const ARTIFACT_FILE: &str = "artifact.json";
const ASSETS_DIR: &str = "assets";
const LATEST: &str = "latest";

/// Directory under `<root>/<source>/` that holds per-host instance
/// sidecars (REPORT §6a). The leading underscore keeps it from clashing
/// with a `safe(target)` (targets never start with `_instance` for a real
/// dashboard uid, and even if one did it would live under a different
/// host-nested layout).
const INSTANCE_DIR: &str = "_instance";
const INSTANCE_FILE: &str = "instance.json";
/// Suffix of an instance sidecar's timestamped directory: the dirname is
/// `<rfc3339-utc>__instance`.
const INSTANCE_SUFFIX: &str = "instance";

/// One entry discovered by scanning the store root, parsed entirely
/// from the directory name.
#[derive(Debug, Clone)]
pub struct ListEntry {
    pub dir: PathBuf,
    pub fetched_at: DateTime<Utc>,
    pub source: String,
    pub target: String,
}

/// The artifact store rooted at a single directory (default
/// `./artifacts`).
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to the per-`(source, target)` `latest` symlink.
    pub fn latest_link(&self, source: &str, target: &str) -> PathBuf {
        self.root.join(source).join(safe(target)).join(LATEST)
    }

    /// Write an artifact into a fresh timestamped directory, create its
    /// `assets/` dir, and rewrite the `latest` symlink. Returns the new
    /// artifact directory.
    pub fn write(&self, artifact: &Artifact) -> Result<PathBuf, StoreError> {
        let name = dir_name(artifact.fetched_at, &artifact.source, &artifact.target);
        let dir = self.root.join(&name);
        create_dir_secure(&dir)?;
        create_dir_secure(&dir.join(ASSETS_DIR))?;

        let file = std::fs::File::create(dir.join(ARTIFACT_FILE))?;
        serde_json::to_writer_pretty(std::io::BufWriter::new(file), artifact)?;

        self.rewrite_latest(&artifact.source, &artifact.target, &name)?;
        Ok(dir)
    }

    /// Read and validate an artifact from its directory (the `latest`
    /// symlink is accepted because `std::fs` follows it). Rejects
    /// unknown major schema versions.
    pub fn read(&self, dir: &Path) -> Result<Artifact, StoreError> {
        read_artifact(dir)
    }

    /// Scan the root for artifact directories, optionally filtered by
    /// `source` and by a `since` cutoff (entries at or after it).
    pub fn list(
        &self,
        source: Option<&str>,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ListEntry>, StoreError> {
        let mut out = Vec::new();
        let rd = match std::fs::read_dir(&self.root) {
            Ok(rd) => rd,
            // An empty / not-yet-created store lists as nothing.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for ent in rd {
            let ent = ent?;
            if !ent.file_type()?.is_dir() {
                continue;
            }
            let name = ent.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(entry) = parse_dir_name(name, &ent.path()) else {
                continue;
            };
            if let Some(src) = source {
                if entry.source != src {
                    continue;
                }
            }
            if let Some(cut) = since {
                if entry.fetched_at < cut {
                    continue;
                }
            }
            out.push(entry);
        }
        out.sort_by(|a, b| b.fetched_at.cmp(&a.fetched_at));
        Ok(out)
    }

    fn rewrite_latest(&self, source: &str, target: &str, dirname: &str) -> Result<(), StoreError> {
        let link = self.latest_link(source, target);
        // The timestamped dir is two levels up from <source>/<safe(target)>/.
        let rel = Path::new("..").join("..").join(dirname);
        replace_symlink(&link, &rel)
    }

    /// Base directory holding a host's instance sidecars:
    /// `<root>/<source>/_instance/<safe(host)>`.
    fn instance_host_dir(&self, source: &str, host: &str) -> PathBuf {
        self.root.join(source).join(INSTANCE_DIR).join(safe(host))
    }

    /// Path to the per-`(source, host)` instance `latest` symlink:
    /// `<root>/<source>/_instance/<safe(host)>/latest`.
    pub fn instance_latest_link(&self, source: &str, host: &str) -> PathBuf {
        self.instance_host_dir(source, host).join(LATEST)
    }

    /// Write an [`InstanceSidecar`] into a fresh timestamped directory
    /// under `<root>/<source>/_instance/<safe(host)>/` and rewrite that
    /// host's `latest` symlink. Reuses the same `0700` + symlink
    /// machinery as [`Self::write`] (one store, REPORT §6a). Returns the
    /// new sidecar directory.
    pub fn write_instance(
        &self,
        source: &str,
        sidecar: &InstanceSidecar,
    ) -> Result<PathBuf, StoreError> {
        let base = self.instance_host_dir(source, &sidecar.host);
        let dirname = format!(
            "{}__{}",
            sidecar.fetched_at.format(DIR_TS_FMT),
            INSTANCE_SUFFIX
        );
        let dir = base.join(&dirname);
        create_dir_secure(&dir)?;

        let file = std::fs::File::create(dir.join(INSTANCE_FILE))?;
        serde_json::to_writer_pretty(std::io::BufWriter::new(file), sidecar)?;

        // The `latest` symlink is a sibling of the timestamped dir, so the
        // relative target is just the dirname (no `../..`).
        let link = base.join(LATEST);
        replace_symlink(&link, Path::new(&dirname))?;
        Ok(dir)
    }

    /// Read the `_instance/<host>/latest` sidecar if one exists. Returns
    /// `Ok(None)` when no sidecar is present (the caller may then fall
    /// back to a legacy per-dashboard artifact). Rejects unknown major
    /// schema versions.
    pub fn read_instance_sidecar(
        &self,
        source: &str,
        host: &str,
    ) -> Result<Option<InstanceSidecar>, StoreError> {
        let path = self.instance_latest_link(source, host).join(INSTANCE_FILE);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let sidecar: InstanceSidecar = serde_json::from_slice(&bytes)?;
        if sidecar.schema_version > INSTANCE_SCHEMA_VERSION {
            return Err(StoreError::UnsupportedInstanceSchema {
                found: sidecar.schema_version,
                supported: INSTANCE_SCHEMA_VERSION,
            });
        }
        Ok(Some(sidecar))
    }

    /// Resolve instance facts for `(host, uid)`.
    ///
    /// Reads the `_instance/<host>/latest` sidecar first. ONLY when no
    /// sidecar is present does it fall back to the legacy per-dashboard
    /// artifact's `meta.search` (migration grace), emitting a
    /// `tracing::warn` at load time that names the legacy artifact. Errors
    /// with [`StoreError::InstanceFactsUnavailable`] when neither exists.
    pub fn read_instance_facts(
        &self,
        source: &str,
        host: &str,
        uid: &str,
    ) -> Result<InstanceFacts, StoreError> {
        if let Some(sidecar) = self.read_instance_sidecar(source, host)? {
            let path = self.instance_latest_link(source, host).join(INSTANCE_FILE);
            return Ok(InstanceFacts::from_sidecar(sidecar, path));
        }
        // Migration fallback: recover the inventory from the legacy
        // per-dashboard artifact's `meta.search`.
        let link = self.latest_link(source, uid);
        match read_artifact(&link) {
            Ok(artifact) => {
                let search = artifact.meta.get("search").cloned();
                match search {
                    Some(search) if !search.is_null() => {
                        let path = link.join(ARTIFACT_FILE);
                        tracing::warn!(
                            artifact = %path.display(),
                            host,
                            uid,
                            "no instance sidecar; falling back to legacy meta.search \
                             (re-scrape to write an _instance sidecar)"
                        );
                        Ok(InstanceFacts::from_legacy_meta(
                            host.to_owned(),
                            search,
                            path,
                        ))
                    }
                    _ => Err(StoreError::InstanceFactsUnavailable {
                        host: host.to_owned(),
                        uid: uid.to_owned(),
                    }),
                }
            }
            Err(StoreError::NotAnArtifact(_)) => Err(StoreError::InstanceFactsUnavailable {
                host: host.to_owned(),
                uid: uid.to_owned(),
            }),
            Err(e) => Err(e),
        }
    }
}

/// Remove any existing `link` and recreate it pointing at `target`
/// (a path relative to the link's own directory). Creates the link's
/// parent `0700` if missing. Shared by the artifact + sidecar writers.
fn replace_symlink(link: &Path, target: &Path) -> Result<(), StoreError> {
    if let Some(parent) = link.parent() {
        create_dir_secure(parent)?;
    }
    match std::fs::symlink_metadata(link) {
        Ok(_) => std::fs::remove_file(link)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    symlink(target, link)?;
    Ok(())
}

/// Read + validate an artifact directory. Free function so the CLI can
/// `show` a path without constructing a store.
pub fn read_artifact(dir: &Path) -> Result<Artifact, StoreError> {
    let path = dir.join(ARTIFACT_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(StoreError::NotAnArtifact(dir.display().to_string()))
        }
        Err(e) => return Err(e.into()),
    };
    let artifact: Artifact = serde_json::from_slice(&bytes)?;
    if artifact.schema_version > ARTIFACT_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema {
            found: artifact.schema_version,
            supported: ARTIFACT_SCHEMA_VERSION,
        });
    }
    Ok(artifact)
}

/// Build the index directory name for an artifact.
pub fn dir_name(fetched_at: DateTime<Utc>, source: &str, target: &str) -> String {
    format!(
        "{}__{}__{}",
        fetched_at.format(DIR_TS_FMT),
        source,
        safe(target)
    )
}

/// Parse an index directory name back into a [`ListEntry`]. Returns
/// `None` for names that do not match the `ts__source__target` shape so
/// stray directories are skipped rather than erroring the whole scan.
fn parse_dir_name(name: &str, dir: &Path) -> Option<ListEntry> {
    let mut parts = name.splitn(3, "__");
    let ts = parts.next()?;
    let source = parts.next()?;
    let target = parts.next()?;
    if source.is_empty() || target.is_empty() {
        return None;
    }
    let naive = chrono::NaiveDateTime::parse_from_str(ts, DIR_TS_FMT).ok()?;
    Some(ListEntry {
        dir: dir.to_path_buf(),
        fetched_at: DateTime::from_naive_utc_and_offset(naive, Utc),
        source: source.to_string(),
        target: target.to_string(),
    })
}

/// Sanitise a target into a filesystem-safe single path segment: keep
/// `[A-Za-z0-9._-]`, replace everything else with `-`.
pub fn safe(target: &str) -> String {
    let mut s: String = target
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-' => c,
            _ => '-',
        })
        .collect();
    if s.is_empty() {
        s.push('-');
    }
    s
}

/// Create a directory (and parents) `0700` on unix (SCOPE: artifact
/// directories written operator-only). The mode is best-effort on
/// non-unix targets.
fn create_dir_secure(dir: &Path) -> Result<(), StoreError> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

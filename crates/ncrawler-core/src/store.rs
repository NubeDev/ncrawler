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

/// Timestamp format used in directory names. Colons are illegal/awkward
/// in paths, so the RFC3339 `:` separators become `-` and we keep the
/// trailing `Z` to mark UTC.
const DIR_TS_FMT: &str = "%Y-%m-%dT%H-%M-%SZ";

const ARTIFACT_FILE: &str = "artifact.json";
const ASSETS_DIR: &str = "assets";
const LATEST: &str = "latest";

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
        if let Some(parent) = link.parent() {
            create_dir_secure(parent)?;
        }
        // The target is two levels up from <source>/<safe(target)>/.
        let rel = Path::new("..").join("..").join(dirname);
        match std::fs::symlink_metadata(&link) {
            Ok(_) => std::fs::remove_file(&link)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        symlink(&rel, &link)?;
        Ok(())
    }
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

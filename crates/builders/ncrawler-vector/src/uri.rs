//! Parsing of `--store` URIs: `lance://<path>` (default) and
//! `qdrant://<host:port>[/<collection>]`.

use crate::error::VectorError;

/// The default store URI when `--store` is omitted: an on-disk LanceDB dir.
pub const DEFAULT_STORE_URI: &str = "lance://./vec";

/// A parsed store target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreUri {
    /// LanceDB at a filesystem path.
    Lance { path: String },
    /// Qdrant server at `url` writing into `collection`.
    Qdrant { url: String, collection: String },
}

impl StoreUri {
    /// Parse a `--store` value. Empty input falls back to [`DEFAULT_STORE_URI`].
    pub fn parse(s: &str) -> Result<Self, VectorError> {
        let s = if s.is_empty() { DEFAULT_STORE_URI } else { s };
        if let Some(path) = s.strip_prefix("lance://") {
            if path.is_empty() {
                return Err(VectorError::BadUri(s.into()));
            }
            return Ok(StoreUri::Lance {
                path: path.to_string(),
            });
        }
        if let Some(rest) = s.strip_prefix("qdrant://") {
            let (authority, collection) = match rest.split_once('/') {
                Some((a, c)) if !c.is_empty() => (a, c),
                _ => (rest.trim_end_matches('/'), "ncrawler"),
            };
            if authority.is_empty() {
                return Err(VectorError::BadUri(s.into()));
            }
            return Ok(StoreUri::Qdrant {
                url: format!("http://{authority}"),
                collection: collection.to_string(),
            });
        }
        Err(VectorError::BadUri(s.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uri_is_lance_dir() {
        assert_eq!(
            StoreUri::parse("").unwrap(),
            StoreUri::Lance {
                path: "./vec".into()
            }
        );
        assert_eq!(
            StoreUri::parse(DEFAULT_STORE_URI).unwrap(),
            StoreUri::parse("").unwrap()
        );
    }

    #[test]
    fn lance_path_is_preserved() {
        assert_eq!(
            StoreUri::parse("lance:///abs/path").unwrap(),
            StoreUri::Lance {
                path: "/abs/path".into()
            }
        );
    }

    #[test]
    fn qdrant_with_and_without_collection() {
        assert_eq!(
            StoreUri::parse("qdrant://localhost:6334/panels").unwrap(),
            StoreUri::Qdrant {
                url: "http://localhost:6334".into(),
                collection: "panels".into()
            }
        );
        assert_eq!(
            StoreUri::parse("qdrant://localhost:6334").unwrap(),
            StoreUri::Qdrant {
                url: "http://localhost:6334".into(),
                collection: "ncrawler".into()
            }
        );
    }

    #[test]
    fn unknown_scheme_is_rejected() {
        assert!(StoreUri::parse("redis://x").is_err());
        assert!(StoreUri::parse("lance://").is_err());
    }
}

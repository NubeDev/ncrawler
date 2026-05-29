//! `--since` duration parsing for `ncrawler ls`.

use chrono::Duration;

use crate::error::StoreError;

/// Parse a compact duration like `30s`, `45m`, `24h`, `7d` into a
/// [`chrono::Duration`]. Used by `ncrawler ls --since`.
pub fn parse_since(s: &str) -> Result<Duration, StoreError> {
    let s = s.trim();
    let (num, unit) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| StoreError::BadSince(s.to_string()))?,
    );
    let n: i64 = num
        .parse()
        .map_err(|_| StoreError::BadSince(s.to_string()))?;
    match unit {
        "s" => Ok(Duration::seconds(n)),
        "m" => Ok(Duration::minutes(n)),
        "h" => Ok(Duration::hours(n)),
        "d" => Ok(Duration::days(n)),
        _ => Err(StoreError::BadSince(s.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units() {
        assert_eq!(parse_since("30s").unwrap(), Duration::seconds(30));
        assert_eq!(parse_since("45m").unwrap(), Duration::minutes(45));
        assert_eq!(parse_since("24h").unwrap(), Duration::hours(24));
        assert_eq!(parse_since("7d").unwrap(), Duration::days(7));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_since("h").is_err());
        assert!(parse_since("12").is_err());
        assert!(parse_since("12w").is_err());
        assert!(parse_since("").is_err());
    }
}

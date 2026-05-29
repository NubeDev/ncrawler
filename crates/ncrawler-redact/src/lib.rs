//! Shared, default-on secret redaction for ncrawler report builders.
//!
//! This crate provides a single [`Redactor`] that masks secret-ish values in
//! free text (e.g. SQL literals) and in variable maps. It is invoked as a
//! **builder-side pass** by the renderers — never at scrape time. The artifact
//! on disk stays raw for audit/forensics, protected by the SCOPE-mandated 0700
//! permissions; redaction is the defence applied when data leaves that boundary
//! (rendered reports).
//!
//! Placement (REPORT §7 / stage): this lives in its own `ncrawler-redact`
//! crate rather than in `ncrawler-spi` so the pattern matcher (`regex`) and the
//! property-test harness (`quickcheck`) do not leak into the dependency-light
//! contract crate. `ncrawler-report-md` applies it as a builder-side pass; the
//! CLI exposes it as `--redact` (default) / `--no-redact` (logged opt-out).
//!
//! # API
//!
//! - [`Redactor::redact`] — `redact(text) -> Cow<str>`, no allocation when there
//!   is nothing to mask.
//! - [`Redactor::redact_variables`] — masks the values of a variable map in
//!   place, also masking values whose *key name* is itself secret-ish.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

use regex::{Captures, Regex};

/// The token substituted for any masked secret. Chosen so that it never matches
/// any redaction pattern itself, making [`Redactor::redact`] idempotent.
pub const MASK: &str = "***REDACTED***";

/// Masks secret-ish values in text and variable maps.
///
/// Construct once and reuse; the underlying regex is compiled lazily and shared
/// across all instances.
#[derive(Debug, Clone, Copy, Default)]
pub struct Redactor {
    _private: (),
}

/// The combined alternation. Order is significant: the `password=`/`secret=`/
/// `key=` key-value form is tried first so its value (which may itself be hex)
/// is masked as a unit with the assignment prefix preserved.
fn pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?x)
            # password='...' | secret='...' | key='...'   (SQL string literals)
            (?P<kv>
                (?P<kvkey>(?i:password|secret|key)\s*=\s*)
                '(?P<kvval>[^']*)'
            )
            |
            # bearer <token>  (auth-header / token shaped)
            (?P<bearer>
                (?i:bearer)\s+
                (?P<bearertok>[A-Za-z0-9\-._~+/]{12,}={0,2})
            )
            |
            # UUID v4
            (?P<uuid>
                \b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-4[0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}\b
            )
            |
            # long hex run (32+), e.g. API keys / digests
            (?P<hex>\b[0-9a-fA-F]{32,}\b)
            "#,
        )
        .expect("redaction pattern is a valid regex")
    })
}

/// Variable-key names that are known-secret by name regardless of their value's
/// shape. Compared case-insensitively as a whole-name match.
fn key_is_secret(name: &str) -> bool {
    const SECRET_KEYS: &[&str] = &[
        "password",
        "passwd",
        "pwd",
        "secret",
        "key",
        "apikey",
        "api_key",
        "token",
        "access_token",
        "refresh_token",
        "private_key",
        "client_secret",
        "auth",
        "authorization",
        "credential",
        "credentials",
    ];
    let lower = name.trim().to_ascii_lowercase();
    SECRET_KEYS.iter().any(|k| *k == lower)
}

impl Redactor {
    /// Create a new redactor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mask every secret-ish substring in `text`.
    ///
    /// Returns [`Cow::Borrowed`] (no allocation) when nothing matches, so the
    /// common no-secret path is zero-copy.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        pattern().replace_all(text, |caps: &Captures| Self::replacement(caps))
    }

    /// Mask the values of a variable map in place.
    ///
    /// A value is masked either because it contains a secret-ish substring
    /// ([`Redactor::redact`]) or because its *key name* is itself secret-ish
    /// (e.g. `password`), in which case the whole value is replaced — this
    /// catches secrets that do not match any textual pattern.
    pub fn redact_variables(&self, vars: &mut HashMap<String, String>) {
        for (k, v) in vars.iter_mut() {
            if key_is_secret(k) {
                if v != MASK {
                    v.clear();
                    v.push_str(MASK);
                }
                continue;
            }
            if let Cow::Owned(masked) = self.redact(v) {
                *v = masked;
            }
        }
    }

    /// Build the replacement string for a single match, dispatching on which
    /// named alternative fired.
    fn replacement(caps: &Captures) -> String {
        if caps.name("kv").is_some() {
            // Preserve the assignment prefix and quoting; mask only the literal.
            let key = caps.name("kvkey").map(|m| m.as_str()).unwrap_or_default();
            return format!("{key}'{MASK}'");
        }
        if let Some(full) = caps.name("bearer") {
            // Preserve everything up to the token (the `Bearer ` prefix), mask
            // the token itself.
            let tok = caps
                .name("bearertok")
                .map(|m| m.as_str())
                .unwrap_or_default();
            let full = full.as_str();
            let prefix = &full[..full.len() - tok.len()];
            return format!("{prefix}{MASK}");
        }
        // uuid / hex: the whole match is the secret.
        MASK.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r() -> Redactor {
        Redactor::new()
    }

    #[test]
    fn long_hex_is_masked() {
        let s = "digest deadbeefdeadbeefdeadbeefdeadbeef done";
        assert_eq!(r().redact(s), "digest ***REDACTED*** done");
    }

    #[test]
    fn short_hex_is_not_masked() {
        // 31 hex chars: below the 32 threshold.
        let s = "x deadbeefdeadbeefdeadbeefdeadbee y"; // 31 chars
        assert_eq!(r().redact(s), s);
    }

    #[test]
    fn uuid_v4_is_masked() {
        let s = "id=550e8400-e29b-41d4-a716-446655440000;";
        assert_eq!(r().redact(s), "id=***REDACTED***;");
    }

    #[test]
    fn non_v4_uuid_is_not_masked() {
        // version nibble is 1, not 4 -> not a v4 uuid (and not 32-run hex due to dashes).
        let s = "550e8400-e29b-11d4-a716-446655440000";
        assert_eq!(r().redact(s), s);
    }

    #[test]
    fn sql_password_literal_is_masked() {
        let s = "WHERE password = 'hunter2' AND active";
        assert_eq!(
            r().redact(s),
            "WHERE password = '***REDACTED***' AND active"
        );
    }

    #[test]
    fn sql_secret_and_key_literals_are_masked() {
        assert_eq!(r().redact("secret='abc'"), "secret='***REDACTED***'");
        assert_eq!(r().redact("KEY = 'xyz'"), "KEY = '***REDACTED***'");
    }

    #[test]
    fn bearer_token_is_masked() {
        let s = "Authorization: Bearer abcdef0123456789ABCDEF";
        assert_eq!(r().redact(s), "Authorization: Bearer ***REDACTED***");
    }

    #[test]
    fn bearer_prose_is_not_masked() {
        // "Smith" is too short to be a token.
        let s = "the bearer Smith arrived";
        assert_eq!(r().redact(s), s);
    }

    #[test]
    fn common_sql_identifiers_round_trip() {
        let s = "SELECT host, instance, datasource_uid, panel_id, time_range \
                 FROM dashboards WHERE folder = 'Production' ORDER BY created_at";
        assert_eq!(r().redact(s), s);
        assert!(matches!(r().redact(s), Cow::Borrowed(_)));
    }

    #[test]
    fn no_match_does_not_allocate() {
        let s = "perfectly ordinary text with no secrets";
        assert!(matches!(r().redact(s), Cow::Borrowed(_)));
    }

    #[test]
    fn idempotent_no_double_masking() {
        let s = "key='deadbeefdeadbeefdeadbeefdeadbeef' and \
                 id=550e8400-e29b-41d4-a716-446655440000";
        let once = r().redact(s).into_owned();
        let twice = r().redact(&once).into_owned();
        assert_eq!(once, twice);
        // The mask token itself is never re-masked.
        assert!(matches!(r().redact(MASK), Cow::Borrowed(_)));
    }

    #[test]
    fn redact_variables_masks_by_value_and_by_key_name() {
        let mut vars = HashMap::new();
        vars.insert("host".to_string(), "db.internal".to_string());
        vars.insert("token".to_string(), "anything-at-all".to_string());
        vars.insert(
            "note".to_string(),
            "digest deadbeefdeadbeefdeadbeefdeadbeef".to_string(),
        );
        r().redact_variables(&mut vars);
        assert_eq!(vars["host"], "db.internal"); // untouched
        assert_eq!(vars["token"], MASK); // masked by key name
        assert_eq!(vars["note"], "digest ***REDACTED***"); // masked by value
    }

    #[test]
    fn redact_variables_is_idempotent() {
        let mut vars = HashMap::new();
        vars.insert("password".to_string(), "p".to_string());
        r().redact_variables(&mut vars);
        let first = vars["password"].clone();
        r().redact_variables(&mut vars);
        assert_eq!(vars["password"], first);
    }
}

#[cfg(test)]
mod prop {
    //! Property tests (quickcheck) for the three locked obligations:
    //! (a) every documented secret pattern is masked,
    //! (b) non-secret text round-trips byte-identically,
    //! (c) redaction is a true, idempotent bypass-friendly operation
    //!     (applying it twice equals applying it once — no double-masking).

    use super::*;
    use quickcheck::{Arbitrary, Gen, TestResult};
    use quickcheck_macros::quickcheck;

    fn hex_of(g: &mut Gen, len: usize) -> String {
        const HEX: &[char] = &[
            '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
        ];
        (0..len).map(|_| *g.choose(HEX).unwrap()).collect()
    }

    /// A generated value known to contain a secret, in one of the documented shapes.
    #[derive(Clone, Debug)]
    struct SecretBearing(String);

    impl Arbitrary for SecretBearing {
        fn arbitrary(g: &mut Gen) -> Self {
            let kind = g.choose(&[0u8, 1, 2, 3, 4]).copied().unwrap();
            let s = match kind {
                // long hex (32..=48)
                0 => {
                    let len = 32 + (usize::arbitrary(g) % 17);
                    hex_of(g, len)
                }
                // uuid v4
                1 => format!(
                    "{}-{}-4{}-{}{}-{}",
                    hex_of(g, 8),
                    hex_of(g, 4),
                    hex_of(g, 3),
                    g.choose(&['8', '9', 'a', 'b']).unwrap(),
                    hex_of(g, 3),
                    hex_of(g, 12),
                ),
                // password/secret/key = '...'
                2 => {
                    let key = g.choose(&["password", "secret", "key"]).unwrap();
                    format!("{key} = 'whatever-value'")
                }
                // bearer token
                3 => format!("Bearer {}", hex_of(g, 24)),
                // embedded in surrounding prose
                _ => format!("prefix secret='{}' suffix", hex_of(g, 8)),
            };
            SecretBearing(s)
        }
    }

    /// (a) every documented secret pattern is masked.
    #[quickcheck]
    fn secrets_are_always_masked(s: SecretBearing) -> bool {
        let out = Redactor::new().redact(&s.0);
        out.contains(MASK)
    }

    /// (b) non-secret text round-trips byte-identically.
    ///
    /// We restrict the generator to short alphanumeric "identifier" words joined
    /// by SQL punctuation — the realistic non-secret surface (column names,
    /// table names, keywords) — and assert byte-identical, borrowed output.
    #[derive(Clone, Debug)]
    struct Identifierish(String);

    impl Arbitrary for Identifierish {
        fn arbitrary(g: &mut Gen) -> Self {
            const WORDS: &[&str] = &[
                "SELECT",
                "FROM",
                "WHERE",
                "host",
                "instance",
                "datasource",
                "panel_id",
                "dashboard",
                "uid",
                "time",
                "value",
                "count",
                "avg",
                "folder",
                "title",
                "created_at",
                "updated_at",
                "label",
                "metric",
                "node",
                "status",
                "region",
                "zone",
                "and",
                "or",
                "not",
                "null",
            ];
            const SEP: &[&str] = &[" ", ", ", ".", " = ", "(", ") ", " > ", "; "];
            let n = 1 + (usize::arbitrary(g) % 12);
            let mut out = String::new();
            for i in 0..n {
                if i > 0 {
                    out.push_str(g.choose(SEP).unwrap());
                }
                out.push_str(g.choose(WORDS).unwrap());
            }
            Identifierish(out)
        }
    }

    #[quickcheck]
    fn non_secret_round_trips(s: Identifierish) -> TestResult {
        let out = Redactor::new().redact(&s.0);
        // Must be byte-identical and (since nothing matched) borrowed.
        if out != s.0 {
            return TestResult::failed();
        }
        TestResult::from_bool(matches!(out, Cow::Borrowed(_)))
    }

    /// (c) idempotence: applying redaction twice equals applying it once.
    /// This is what makes a `--no-redact` bypass a true bypass — re-running the
    /// pass never double-masks already-masked content.
    #[quickcheck]
    fn idempotent(s: SecretBearing) -> bool {
        let r = Redactor::new();
        let once = r.redact(&s.0).into_owned();
        let twice = r.redact(&once).into_owned();
        once == twice
    }
}

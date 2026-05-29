## Done

- Added `Mode::Audit` to `ncrawler-report-grafana` (parses `--mode audit`; CLI already routes through `Mode::parse`, so `ncrawler build report-grafana --mode audit` works).
- New `src/audit.rs`: offline audit over frozen artifacts (no live HTTP / no `reqwest::Client`). Implements all seven REPORT §5 check classes — dead datasource refs, broken queries (stored error frames), empty panels (0-row results), duplicate dashboards via `blake3` over a normalised `(panels[], targets[], variables[])` projection that **excludes the title**, orphans (no folder and/or untagged), unused datasources, and blank/constant variables.
- Each `Finding` carries `severity (Error|Warn|Info)`, check class, source artifact path, dashboard uid+title, optional panel id+title, and a human-readable message.
- `render::render_audit` emits the §4 header + instance facts + a findings table grouped by severity with deterministic ordering (severity → class → uid → panel id → message). Messages are redacted under the default-on policy.
- Added `blake3` dep to the crate's Cargo.toml.
- `tests/audit.rs`: fixture-driven, present + absent path per check class, plus a fingerprint-vs-title test (two same-title/different-content dashboards must NOT match; two different-title/same-content ones must), a determinism test, and a test asserting the audit source + manifest are reqwest-free.
- Fixed a pre-existing temp-dir race in `tests/golden.rs` (two tests shared a fixture dir) with a per-call atomic counter.

## Next

- Stage 7 (optional, REPORT §8 step 7): persist interpolated request body per item for build-deterministic interpolated SQL without `--data`. Not started.

## What you need to know

- `cargo build/clippy --all-targets -D warnings/fmt --check/test --workspace` all green.
- `cargo tree -e normal`: single `chromiumoxide v0.7.0` stack, zero `spider_chrome`.
- Audit reaches the live store path via `build_report` branching on `Mode::Audit`; it reads dashboards into `(dir, Artifact)` pairs via `audit::pair_sources` so findings can name their source `artifact.json`.
- Severity→table mapping: dead-datasource/broken-query = error; empty-panel/duplicate-dashboard/unused-datasource/blank-variable = warn; orphan/constant-variable = info.
- Duplicate detection: canonical member of a fingerprint group is the lowest uid; others are reported as "duplicate of `<uid>`".

## Open questions

- "Hardcoded values that should be variables" (REPORT §5) is implemented narrowly as constant-type template variables; detecting hardcoded literals inside `rawSql` was deemed out of scope/too noisy. Flag at next REVIEW gate if broader coverage is wanted.
- Orphan check treats no-folder and untagged as one combined finding per dashboard (message states which conditions hold); ratify if REPORT intended two separate findings.

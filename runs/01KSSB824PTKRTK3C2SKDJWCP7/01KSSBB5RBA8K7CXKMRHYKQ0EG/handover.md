## Done

- Added the per-instance sidecar to ncrawler-core (sidecar.rs: `InstanceSidecar`/`InstanceFacts`/`FactsOrigin`, `INSTANCE_SCHEMA_VERSION=1`) and store methods `write_instance`/`read_instance_sidecar`/`instance_latest_link`/`read_instance_facts`, reusing the existing 0700 + latest-symlink machinery (no second store). Layout: `<root>/grafana/_instance/<host>/<rfc3339-utc>__instance/instance.json`.
- Reader resolves `(host, uid) -> InstanceFacts` from the sidecar, falling back to legacy `meta.search` only when no sidecar exists, with a load-time `tracing::warn` naming the legacy artifact. New `StoreError::{UnsupportedInstanceSchema, InstanceFactsUnavailable}`.
- ncrawler-grafana: `GrafanaClient` gained `folders()`/`health()`/`frontend_settings()` (raw() in client.rs); new `instance.rs` fetches+composes the sidecar best-effort with SSRF gating; `api.rs` dropped `meta.search`; `lib.rs` writes the sidecar once per scrape run.
- Tests: core write/read/schema-reject/precedence/fallback/unavailable/mixed-store + golden-file shape; wiremock grafana endpoint-assembly/best-effort/SSRF/round-trip/fallback/mixed. `cargo build/clippy/fmt/test --workspace` green; live tests gated on RUN_LIVE_TESTS=1.
- Updated REPORT.md §6a path sketch to the impl-accurate host-nested layout.

## Next

- Stage 2 (shared selector `--all/--uid/--name/--folder/--tag/--limit` for scrape + report) — a fresh session picks this up.

## What you need to know

- The sidecar carries `instance` as an extracted `{version, edition, rendererAvailable}` object (composed from /api/health + /api/frontend/settings), not the raw responses — chosen for stable golden output.
- serde_json serializes object keys sorted (no `preserve_order` feature), so sidecar JSON is deterministic; the golden lives at `crates/ncrawler-core/tests/fixtures/instance.golden.json` and regenerates with `UPDATE_GOLDEN=1 cargo test -p ncrawler-core`.
- The sidecar host falls back to `"unknown-host"` if the URL has no parseable host. SSRF over the sidecar payloads reuses `api::enforce_allow_hosts`.
- `cargo fmt --all` also reflowed one pre-existing line in `ncrawler-spider/src/lib.rs`; included in the commit to keep the tree fmt-clean.

## Open questions

- (none)

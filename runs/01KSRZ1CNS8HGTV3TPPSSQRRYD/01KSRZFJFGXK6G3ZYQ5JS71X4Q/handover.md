## Done

- Added `ScrapeError::ModeUnsupported` variant in crates/ncrawler-spi/src/error.rs.
- Implemented `crates/scrapers/ncrawler-grafana`: `client.rs` isolates the pinned `grafana = =0.1.3` dep behind a `GrafanaClient` trait (token resolution via SecretStore key `ncrawler:grafana:<host>:token` + `GRAFANA_TOKEN` env fallback, never logged; error mapping 401→Auth/404→NotFound/decode→Other); `api.rs` for mode=Api (GET dashboards/uid via `client.dashboards()`, POST /api/ds/query via `openapi().query_metrics_with_expressions` with documented `client.raw()` fallback, GET /api/search + /api/annotations via `openapi()`), one `Item::Panel` per panel with id `panel-{panelId}`, dashboard JSON in meta, SSRF allow-host validation over surfaced datasource URLs; `visual.rs`/`merge.rs` placeholders returning `ScrapeError::ModeUnsupported`; `lib.rs` `GrafanaScraper` impl dispatching on mode.
- wiremock tests (tests/api_tests.rs, 10 tests): success + 401 + 404 + malformed-JSON across all four endpoints, SSRF reject + wildcard allow.
- Implemented `crates/builders/ncrawler-report-md`: deterministic `render()` + `MarkdownBuilder` (per-Item title/tags/fenced JSON, image embeds matched by `item_id` only); golden fixtures + test, plus a test giving two assets identical labels to prove label-matching is dead.
- Recorded the grafana/reqwest deviation in SCOPE.md.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all green. Committed as 036c336.

## Next

- M3: wire `crates/builders/ncrawler-report-ai` to `starter-ai::ClaudeRunner` + `starter-skills` (the `skills/grafana-triage` bundle already exists from stage 1).
- Later (M4): implement `visual.rs`/`merge.rs` (renderer plugin), which adds the reqwest 0.12 visual path.

## What you need to know

- DEVIATION (needs REVIEW ratification): `grafana 0.1.3` hard-requires `reqwest 0.13.1` and `rust-version 1.92`, NOT reqwest 0.12. `cargo tree` now shows BOTH reqwest 0.12 and 0.13, breaking SCOPE's single-reqwest-version invariant. Contained: the grafana crate is reachable only via client.rs behind the `GrafanaClient` trait. Documented in SCOPE.md "Deviations"; toolchain is 1.94.1 so the 1.92 requirement is satisfied.
- The grafana crate auto-appends `/api` to the base URL; client builds with `max_retries(0)` for deterministic wiremock tests.
- ds/query falls back to `client.raw()` only on Api/Decode-shape errors, so 401/404 still surface correctly.

## Open questions

- REVIEW must decide: accept the dual reqwest version, or replace the grafana crate with hand-rolled reqwest 0.12 in client.rs.

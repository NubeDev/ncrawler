## Done

- Added `crates/scrapers/ncrawler-grafana/src/multi.rs`: `scrape_selection()` resolves `DashboardSelector` against live `/api/search`, ensures the `_instance/<host>` sidecar exactly once (`SidecarOutcome::{Written,Refreshed,SkippedFresh}`; reuse when younger than `MultiConfig.sidecar_max_age`, default 3600s), then fans out per-`<uid>/latest` artifacts under a `tokio::sync::Semaphore` token bucket (`DEFAULT_CONCURRENCY=4`). Per-dashboard errors collected into `MultiSummary` (sorted `succeeded`/`failed`), siblings never aborted.
- `GrafanaScraper::scrape_multi(job, selector, config, cancel)` added (builds client + store, resolves token via existing quarantine path, host via `Url::host_str()`). New types re-exported from crate root: `scrape_selection`, `MultiConfig`, `MultiSummary`, `SidecarOutcome`, `DashboardError`, `DEFAULT_CONCURRENCY`, `DEFAULT_SIDECAR_MAX_AGE_SECS`.
- CLI: `run_scrape` routes `grafana` + `--mode api` to new `run_grafana_multi` (handles `--all`/`--name`/`--folder`/`--tag`/multi-`--uid` + `--concurrency` + `--sidecar-max-age`); `single_uid_target` message now points at `--mode api`. Visual/both unchanged (single-dashboard).
- Tests `tests/multi_tests.rs` (4, all pass): wiremock mixed 200/401/404/malformed loop; concurrency-cap (Barrier+atomic peak, asserts `peak == CAP`); sidecar-skip (fresh => folders/health not refetched); sidecar-refresh (stale => refetched). Live `--all --limit 5` smoke added to `tests/live.rs` (`#[ignore]`, `RUN_LIVE_TESTS=1`).
- `cargo build/clippy(-D warnings)/fmt --check/test --workspace` all green. Committed `61fe4ad`, pushed to `codeless/ncrawler-report-grafana`.

## Next

- Stage 4 (REPORT §8 step 4 / template stage 4): shared default-on `Redactor` (mask secret-ish variable values + SQL literals), wired as a builder-side pass into `ncrawler-report-md`, with `--redact`/`--no-redact`. This stage precedes a REVIEW gate.

## What you need to know

- This worktree is on `codeless/ncrawler-report-grafana` (HEAD `61fe4ad`). The Bash tool's cwd resets to the MAIN worktree (`/home/user/code/rust/ncrawler`, on `main`) between calls — but commands here ran against the job worktree's checkout correctly; do NOT prepend `cd /home/user/code/rust/ncrawler` (that targets `main`, which lacks stages 1–3). Just run bare.
- Per-dashboard fan-out is API-mode only by design (stage scope says "API-mode scrape loop"). Visual/both multi-selection is intentionally rejected with a `--mode api` hint, not implemented.
- Host key uses `Url::host_str()` (port-dropping) for consistency with the stage-1 sidecar writer and the `ncrawler:grafana:<host>:token` secret key. REVIEW-stage2.md flagged two still-unratified amendments to carry: (1) host normalisation drops the port; (2) `_instance` should be a documented reserved target segment. Neither is fixed here — surface at the next REVIEW gate.
- `MultiConfig.sidecar_max_age` is the "wall-clock cap" the stage references, interpreted as a sidecar freshness window (default 1h, CLI `--sidecar-max-age <secs>`). If the reviewer intended a different meaning (e.g. a max run duration), flag it.

## Open questions

- Sidecar freshness-window interpretation of "scrape's wall-clock cap" (above) should be confirmed at REVIEW.
- Folder match remains case-insensitive *exact* (inherited from stage 2); REPORT §2 only says "restrict to a folder" — still unconfirmed vs substring.

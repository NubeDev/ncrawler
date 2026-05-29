## Done

- Added `crates/scrapers/ncrawler-grafana/src/selector.rs`: `DashboardSelector` parsing `--all`, `--uid a,b,c` (comma-list, repeatable, dedup + order-preserving), `--name` (case-insensitive substring), `--folder` (case-insensitive exact), `--tag` (case-insensitive membership), `--limit N`. `--limit 0` and `--limit > 10_000` (`MAX_LIMIT`) rejected at parse; empty selection rejected.
- `parse_inventory` normalises both live `/api/search` and the on-disk sidecar `search` array (same wire shape; skips `dash-folder` + uid-less rows) into `DashboardEntry`.
- Dual-`--all` (REPORT §2): `resolve_live` (scrape, full live inventory) vs `resolve_on_disk` (report, only what's on disk + the sidecar inventory total). `resolve_on_disk` emits `tracing::warn` when on-disk coverage is narrower, and `Resolution` records both counts (`inventory_total`, `on_disk_total`, `header_scope()`, `coverage_is_narrow()`). Stable `(folder,title,uid)` sort before `--limit`.
- Wired selector into `ncrawler scrape grafana` (replaces bare `--uid` parse). Single `--uid x` still works as the singleton; multi-uid/`--all`/`--name`/`--folder`/`--tag` bail with a message pointing at the next-stage live fan-out.
- Exported `DashboardSelector/DashboardEntry/Resolution/SelectorError/parse_inventory/MAX_LIMIT` from the crate so the future report builder consumes them without a cycle.
- 20 selector unit tests (every flag permutation, AND-combination, dedup/trim, all safety bounds, live-vs-on-disk divergence, coverage warn). `cargo build/clippy(-D warnings)/fmt --check/test --workspace` all green.
- Committed as `13b0ddf` on branch `codeless/ncrawler-report-grafana`.

## Next

- Stage 3 (REPORT §8 step 3): multi-dashboard scrape — loop `resolve_live` over live `/api/search` in the grafana scraper and replace the CLI `single_uid_target` bail so `--all`/`--name`/etc. actually fan out per-dashboard artifacts.

## What you need to know

- This worktree is on branch `codeless/ncrawler-report-grafana` (HEAD now `13b0ddf`), NOT `main`. The Bash tool resets cwd to the main worktree (`/home/user/code/rust/ncrawler`, on `main` @ a70e348) between calls — run with no `cd` to stay in the job worktree, or the sidecar/selector code won't be present.
- Stage 1 (instance sidecar + reader fallback) is already landed on this branch; `InstanceSidecar.search` is the report-time inventory source for the selector.
- `REVIEW-stage2.md` (sidecar gate, PASS) flags two unratified amendments for the selector to respect at fan-out: (1) host normalisation currently drops the port (`Url::host_str()`), so same-host/different-port instances collide in `_instance/<host>`; (2) `_instance` should be documented as a reserved target-namespace segment. Resolve host keying before the stage-3 fan-out.
- `single_uid_target` in `crates/ncrawler-cli/src/main.rs` is the deliberate temporary gate; remove it when the loop lands.

## Open questions

- Folder match is case-insensitive *exact*; REPORT §2 only says "restrict to a folder". Confirm exact (vs substring) is the intended semantics at the next REVIEW gate.

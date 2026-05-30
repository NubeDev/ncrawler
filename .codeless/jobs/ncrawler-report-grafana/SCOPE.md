# Scope - ncrawler-report-grafana

The authoritative design lives at
[/home/user/code/rust/ncrawler/REPORT.md](../../../REPORT.md). The v1
artifact / Item / Asset / store contract lives at
[/home/user/code/rust/ncrawler/SCOPE.md](../../../SCOPE.md) and is
unchanged by this job. This file is the trimmed per-job brief. Where it
disagrees with the deep docs, **the deep docs win** - fix this file
rather than diverge. Where REPORT.md and SCOPE.md disagree, **SCOPE.md
wins** (artifact contract is older and more load-bearing); flag the
divergence in the same stage's handover.

## Goal

Land the Grafana Report Builder: a new deterministic offline builder
`report-grafana` rendering one or more on-disk artifacts to
`REPORT.md`, plus the artifact-side changes it requires (per-instance
sidecar, shared dashboard selector, multi-dashboard scrape loop,
default-on redaction). After this job:

1. `grafana/_instance/<host>/<rfc3339>__instance/instance.json` exists
   as a per-instance sidecar (`search` + `datasources` + `instance` +
   `folders`) with a per-(source,target=`_instance:<host>`) `latest`
   symlink, written once per scrape run. Per-dashboard artifacts no
   longer embed `meta.search` going forward; the reader falls back to
   legacy `meta.search` only when the sidecar is absent (migration
   grace path, logs WARN).
2. A shared `DashboardSelector` (`--all` / `--uid` / `--name` /
   `--folder` / `--tag` / `--limit`) drives BOTH `scrape grafana`
   (resolves live against `/api/search`) and `build report-grafana`
   (resolves offline against the sidecar). The dual `--all` semantics
   from REPORT.md \u00a72 are explicit and warned.
3. `ncrawler scrape grafana` loops the resolved selector with bounded
   concurrency (default 4), refreshing the sidecar at the start of
   each run, and collects per-dashboard errors into a final summary
   without aborting siblings.
4. A shared `Redactor` (default-on) masks secret-shaped values in
   variables and SQL at builder render time. Artifacts on disk stay
   raw under 0700 perms; the masker is a builder pass, not a scrape
   pass. `--no-redact` opts out and logs WARN.
5. `crates/builders/ncrawler-report-grafana` ships `--mode overview |
   full`, `--data`, `--window` (default `now-6h`), the shared
   selector, and the three-section output (overview / structure /
   pages) from REPORT.md \u00a74 with stable ordering. `full` emits
   template SQL deterministically; `--data` adds executed SQL **only
   where the datasource exposes it** (postgres/mysql yes, Rubix OS
   no, never invented).
6. `--mode audit` reads frozen response / error frames offline and
   ships the seven check classes from REPORT.md \u00a75; duplicate
   detection is `blake3` over a normalised `(panels, targets,
   variables)` projection - **never title similarity**.

## In scope (six stages with two REVIEW gates)

- **Stage 1 - Instance sidecar + reader fallback.** Write the sidecar
  artifact once per scrape; reader prefers sidecar, falls back to
  legacy `meta.search` only when absent (WARN). Per-dashboard
  artifacts stop embedding `meta.search` going forward.
  **REVIEW gate.**
- **Stage 2 - Shared selector.** `DashboardSelector` parser for
  scrape (live) + report (on-disk), dual-`--all` semantics, safety
  bounds, no break to the existing single-`--uid` CLI flag.
- **Stage 3 - Multi-dashboard scrape.** Iterate the selector with
  bounded concurrency; sidecar refresh policy; per-dashboard error
  collection.
- **Stage 4 - Shared redaction pass.** Builder-side `Redactor`,
  default-on, `--no-redact` opt-out with WARN, property-tested
  against false positives on plain SQL identifiers.
  **REVIEW gate.**
- **Stage 5 - `report-grafana` builder (overview + full).** Three
  sections from REPORT.md \u00a74, stable ordering, template SQL
  always, executed SQL only where the datasource exposes it.
- **Stage 6 - Audit mode + duplicate fingerprinting.** Seven check
  classes from REPORT.md \u00a75 reading frozen artifacts offline;
  blake3 panel-set fingerprint duplicate detection (no title
  matching).

## Out of scope

- **Live HTTP at build time.** `report-grafana` is a pure renderer
  over on-disk artifacts; no `reqwest::Client` reachable from the
  builder's call graph. Audit mode in particular is offline by
  design (REPORT.md \u00a75) and a test asserts it.
- **Embedding PNG screenshots in the report.** The `rd-esr` instance
  has no renderer plugin (`rendererAvailable=false`); the only path
  to images is the documented-flaky `--visual-fallback chrome`. Kept
  out of the deterministic report.
- **Persisting the interpolated request body per Item** (REPORT.md
  \u00a78 step 7, optional). Future stage; this job does not ship it.
- **Title-similarity duplicate detection.** Fingerprint-only; tests
  must give two dashboards identical titles to prove this.
- **Redaction at scrape time.** Artifacts stay raw on disk under
  0700; the masker is builder-side. Reusing the existing
  `starter_ai::secret` machinery is preferred over a new ad-hoc
  matcher wherever a value is known-secret at the type level.
- **Promoting `ncrawler-redact` (if introduced) to a starter crate.**
  The promotion rule from SCOPE.md ("lift when a second consumer
  materialises") is not met by this job alone.
- **A second engine for the artifact store, the selector, or the
  report.** Reuse `ncrawler-core`'s on-disk store; reuse the
  existing CLI clap shell; reuse `ncrawler-report-md` patterns
  rather than duplicating them.

## Constraints (unchanged from v1 unless noted)

- **OSS-only deps**, MIT / Apache-2.0 / BSD. No GPL/AGPL.
- **Dep versions stay aligned with starter** so `cargo tree -e
  normal` continues to show single versions of `tokio`, `reqwest`,
  `serde`, `chrono`, `clap`. The `reqwest 0.13` duplication via the
  `grafana` crate documented in the v1 *Deviations* section is
  inherited - do not regress further. No new transitives.
- **No `spider_chrome` anywhere.** `cargo tree -e normal` continues
  to show a single browser stack.
- **No `time` crate.** `chrono` only.
- **Starter HOW-TO-CODE.md budgets**: <=400 lines per file, <=50
  lines per function. The selector and the redactor are likely to
  approach the file budget - split modules early rather than at the
  end.
- **`schema_version=1` policy.** The sidecar is a new artifact kind
  but the same schema version. If any stage promotes a `meta` key
  to a typed field, bump `ARTIFACT_SCHEMA_VERSION` in the same
  stage (SCOPE.md rule).
- **`Asset.item_id` remains the only Item<->Asset link.** Unchanged
  from v1; the duplicate-fingerprint code in stage 6 explicitly
  mirrors the same discipline.
- **Tokens never logged.** Bearer tokens flow through
  `starter_ai::secret`-style wrappers; no `tracing` field, log
  line, test fixture, or persisted Event ever carries one. Stage 4
  introduces a *value-based* masker for the builder side - it does
  NOT replace the type-based discipline for known-secret values.
- **SSRF allow-list is enforced at scrape time**, not build time
  (build phase makes no outbound calls in v1 or here).
- **No `--force`, no `--no-verify`.** If a hook fails, fix the
  cause.

## Deliverables (what "done" looks like)

1. Branch `codeless/ncrawler-report-grafana` with one commit per
   stage (six stages = six commits), pushed.
2. `cargo build --workspace` green at every stage boundary.
3. `cargo clippy --workspace --all-targets -- -D warnings` green at
   every stage boundary for crates touched in that stage.
4. `cargo fmt --check` green at every stage boundary.
5. `cargo test --workspace` green at every stage boundary; live-net
   and live-Claude tests gated on `RUN_LIVE_TESTS=1` and NOT run by
   default.
6. `cargo tree -e normal` recorded in the stage-1 and stage-6
   handovers showing (a) no new top-level deps beyond what this job
   needs (lancedb / fastembed surfaces from v1 stay where they are);
   (b) no `spider_chrome` anywhere; (c) no new `reqwest` major
   versions beyond the `0.12` / `0.13` pair already documented.
7. End-to-end smoke under `RUN_LIVE_TESTS=1`: `ncrawler scrape
  grafana --url https://grafana.example.com --all --limit 5`
   followed by `ncrawler build report-grafana --all --mode full
   --data --window now-30d`, both succeed and produce a
   `REPORT.md` with (a) the metadata header from REPORT.md \u00a74;
   (b) deterministic ordering verified by running twice and
   diffing; (c) no token / host-uuid / tenant-id from the live
   instance present in the redacted-default output.

## Open questions - RESOLVED (2026-05-29, before start)

The deep REPORT.md is the authoritative resolution; three job-specific
notes follow.

### Q1 - Single job or two jobs (artifact changes vs report)?

**Answer: single job, six stages, two REVIEW gates.**

The report depends on the sidecar (overview comes from it), the
selector (the report's `--all` resolves against on-disk artifacts),
the multi-dashboard scrape (no real-world artifact set exists
without it), and the redactor (the report defaults redaction on
and tests must prove no secret-shaped value leaks). Splitting
would push the report into a second job that cannot meaningfully
test end-to-end on its own. The REVIEW gates sit at the two
highest-leverage seams: after the sidecar (artifact format change,
migration policy, reader fallback - touches every later stage) and
after the redactor (security invariant before the report ships
redacted output to disk by default).

### Q2 - Where does `report-grafana` live (new crate vs extend `report-md`)?

**Answer: new crate `crates/builders/ncrawler-report-grafana`.**

`ncrawler-report-md` is artifact-agnostic by design; the
Grafana-specific report consumes the per-instance sidecar plus
multiple per-dashboard artifacts (vs single-artifact today) and
ships Grafana-specific section schemas (datasources, panels,
folders, audit checks). Keeping the two builders separate
preserves `report-md`'s single-artifact contract and keeps the
Grafana-specific surface in a single source-of-truth crate. The
SPI extension for multi-artifact build context lands as its own
commit inside stage 5 with a `schema_version` note if it touches
the SPI.

### Q3 - Sidecar layout (`_instance/<host>` directory vs single artifact key)?

**Answer: `grafana/_instance/<host>/<rfc3339>__instance/` with a
`latest` symlink, reusing `ncrawler-core`'s existing store
machinery.**

Forking a second store layout for the sidecar would duplicate the
`latest`-symlink / 0700-perms / `ls --since` machinery; using a
synthetic `target=`_instance:<host>`` keeps a single store
implementation and lets the existing CLI `ls` / `show` paths see
the sidecar without changes. The host normalisation rule
(lowercase, port included, scheme stripped) is documented in
stage 1 and round-tripped through the existing `safe()` filename
sanitiser.

## References

- Deep design (authoritative):
  [/home/user/code/rust/ncrawler/REPORT.md](../../../REPORT.md).
- v1 contract (artifact / Item / Asset / store rules, OSS / no-daemon
  / no-`spider_chrome` invariants):
  [/home/user/code/rust/ncrawler/SCOPE.md](../../../SCOPE.md).
- v1 job (reference convention - fat-prose stages, RESOLVED
  open-questions, closing trio):
  [../ncrawler-v1/](../ncrawler-v1/).
- Grafana SDK (pinned `=0.1.3` per v1): https://docs.rs/grafana/0.1.3/grafana/.
- Live instance grounding the design: `grafana.example.com`
  (Grafana 7.5.17 OSS, PostgreSQL + grafana-rubix-os-data-source,
  no renderer plugin).

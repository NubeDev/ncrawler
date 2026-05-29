## Done

- Merged `main` into the job branch to pick up the stage-4 `ncrawler-redact` crate (the worktree branch predated it).
- Implemented `ncrawler-report-grafana` (overview + full, no audit): three REPORT.md sections, template/executed SQL split, clipped row samples, default-on redaction, stable ordering.
- Extended `BuildCtx` with `dashboard_dirs` (additive; documented, no Artifact schema bump) and added `ArtifactStore::list_instance_hosts`.
- Wired `ncrawler build report-grafana` with `--store/--mode/--data/--window/--redact|--no-redact/--host` + the stage-2 shared selector; made `build`'s positional artifact_dir optional.
- Golden fixtures + tests for (a) overview single, (b) full/no-data multi w/ template SQL, (c) full+data SQL (executed SQL present) AND non-SQL Rubix (executed SQL absent, not faked); plus redaction + trait-path tests.
- cargo build/clippy/fmt/test --workspace green. Committed as `fde50da`.

## Next

- Stage 6 / next: `--mode audit` (REPORT §5) — dead datasource refs, broken queries, empty panels, blake3 panel-fingerprint duplicate detection, orphans, unused datasources, blank/constant variables — reading frozen response/error frames offline.
- Optional REPORT §8 step 7: persist interpolated request body per item for build-deterministic interpolated SQL without `--data`.

## What you need to know

- Branch topology was split: stages 2–3 on `codeless/ncrawler-report-grafana`, stage-4 redaction on `main`. I merged main in. If a canonical checkout still sits on `main`, this branch now contains both lineages.
- The builder renders over the **store**, not a single artifact, so the CLI dispatches it specially (like `vector`) via the `build_report` free fn; `GrafanaReportBuilder` implements the `Builder` trait too (ignores its `artifact` arg, reads sidecar dir + `dashboard_dirs` + options from `BuildCtx`) and is covered by a parity test.
- Determinism: the header "generated" date comes from the sidecar `fetched_at` (not wall-clock) so golden output is stable. Regenerate goldens with `UPDATE_GOLDEN=1 cargo test -p ncrawler-report-grafana --test golden`.
- Executed-SQL extraction tolerates both `results.<refId>.meta.executedQueryString` and the Grafana 7.x per-frame `schema.meta.executedQueryString`.

## Open questions

- Deviation to ratify at REVIEW: REPORT §3 matrix marks per-panel **variables** as full-only, but this stage's prose bundles the variables table into Pages for both modes — I followed the stage prose (variables shown, redacted, in overview + full).
- The redactor masks any `key = '...'` SQL literal, which fires on a column literally named `key` (seen in the full_data fixture). Documented redactor behavior, but worth a REVIEW note if false positives on `key`-named columns matter.
- Artifact-layer plaintext exposure (scrape persists `rawSql`/variables/data under 0700) remains an open follow-up from stage 4, unchanged here.

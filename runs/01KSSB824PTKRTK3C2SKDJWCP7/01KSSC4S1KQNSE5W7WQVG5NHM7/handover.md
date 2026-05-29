## Done

- Reviewed the stage-1 WORK diff (`6f83e24 stage 1 - instance sidecar + reader fallback`) on branch `codeless/ncrawler-report-grafana` (worktree HEAD now `17ec534`).
- Verified all four Layer-1 invariants hold: R1 dep direction (`ncrawler-grafana → ncrawler-core`, no inversion), R2 single transport (sidecar fetched via existing `GrafanaClient` trait; no `spider_chrome`/daemon), R4/R5 trust boundary (shared `enforce_allow_hosts` SSRF over sidecar payloads before write; no secrets in sidecar), wire-formats untouched (`ARTIFACT_SCHEMA_VERSION` still 1; new sidecar is a distinct kind with its own `INSTANCE_SCHEMA_VERSION=1` + reject-on-read).
- Confirmed stage-specific checks: sidecar-wins precedence + reachable-only-when-absent legacy fallback (both tested), schema reject-on-read for the new sidecar kind (`UnsupportedInstanceSchema`, tested), `safe(host)` round-trips, report-md goldens byte-identical (report-md never read `meta.search`).
- Ran `cargo test` (core 20 / grafana 13+7 / report-md goldens), `cargo fmt --check`, workspace `cargo clippy` — all green.
- Recorded verdict + amendments in `.codeless/jobs/ncrawler-report-grafana/REVIEW-stage2.md` and committed.
- PASS: the sidecar contract holds all Layer-1 invariants and the stage's correctness checks; only two non-blocking doc amendments need ratifying before stage 3.

## Next

- (none — stage 3 picks up in a fresh session; it must ratify the two amendments before fanning out the shared dashboard selector)

## What you need to know

- I initially read/tested the wrong checkout (`main`, HEAD `a70e348`, which lacks the sidecar) before noticing the stage-1 code lives only on `codeless/ncrawler-report-grafana`. All final verification was redone in the correct worktree.
- The `<<run git from the worktree, not /home/user/code/rust/ncrawler>>` — that path is the `main` checkout and does not have stage 1.

## Open questions

- Amendment 1 (must ratify before stage 3): host normalisation drops the port. REPORT §6a specifies "port included" but `lib.rs` uses `Url::host_str()`, which omits the port — same-host/different-port instances collide in `_instance/<host>` and share the `ncrawler:grafana:<host>:token` key. Decide: amend docs to "port excluded" or include `:port`.
- Amendment 2 (must ratify before stage 3): document `_instance` as a reserved target-namespace segment so a dashboard uid sanitising to `_instance` cannot collide with the sidecar dir.
- PASS: All Layer-1 invariants (R1/R2/R4/R5, wire-formats) hold over the stage-1 sidecar diff; report-md goldens are byte-identical and sidecar schema-reject/precedence/fallback are tested green.

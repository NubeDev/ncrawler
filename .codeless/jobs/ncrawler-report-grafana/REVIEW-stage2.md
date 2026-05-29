# Stage 2 REVIEW — sidecar contract gate

Gate over the stage-1 WORK diff (`6f83e24 stage 1 - instance sidecar + reader fallback`).

## Verdict: PASS

Layer-1 invariants hold:

- **R1 (crate dependency direction):** `ncrawler-grafana → ncrawler-core` is the
  pre-existing, correct direction. `ncrawler-core` gained `sidecar.rs` + store
  methods with no new outward dependencies. No inversion introduced.
- **R2 (single transport):** the sidecar is fetched through the existing
  `GrafanaClient` trait (the pinned `grafana` crate / workspace reqwest). No new
  transport, no `spider_chrome`, no daemon.
- **R4/R5 (trust boundary):** SSRF `enforce_allow_hosts` is now shared between the
  per-dashboard scrape and the sidecar, and runs over the sidecar payloads
  *before* write. Token resolution/logging unchanged; the sidecar carries no
  secret material.
- **Wire formats untouched:** `ARTIFACT_SCHEMA_VERSION` stays 1. Dropping
  `meta.search` from per-dashboard `meta` is a best-effort-meta change, not a
  schema bump. The new sidecar is a distinct on-disk *kind* with its own
  `INSTANCE_SCHEMA_VERSION = 1` and major-version reject-on-read
  (`UnsupportedInstanceSchema`, tested). `ncrawler-report-md` golden output is
  byte-identical (it never read `meta.search`; goldens pass).

## Stage-specific checks

- On-disk layout `<root>/grafana/_instance/<safe(host)>/<ts>__instance/instance.json`
  with a sibling-relative per-host `latest` symlink reuses `create_dir_secure` +
  `replace_symlink` (no second store forked). Correct.
- `read_instance_facts` reads the sidecar first; the legacy `meta.search` path is
  reachable ONLY when the sidecar is absent and emits a `tracing::warn` naming the
  legacy artifact. `instance_facts_prefers_sidecar_over_legacy_meta` proves
  sidecar wins; `instance_facts_falls_back_to_legacy_meta` proves the fallback is
  reachable. No silent disagreement.
- `safe(host)` round-trips the chosen host form (dots/dashes preserved).
- `cargo test` (core 20 / grafana 13 + 7 / report-md goldens), `fmt --check`, and
  workspace `clippy` all green.

## Amendments to ratify before stage 3 fans out the selector (NOT patched here)

1. **Host normalisation drops the port.** REPORT §6a's chosen form is
   "lowercase, port included, scheme stripped", but `lib.rs` derives the host via
   `Url::host_str()`, which lowercases + strips scheme but **omits the port**. Two
   Grafana instances on the same hostname / different ports collide in
   `_instance/<host>` (and share the `ncrawler:grafana:<host>:token` secret key).
   Either amend REPORT/SCOPE to "port excluded" or include `:port` in the host key.
   The selector (stage 3) keys on host, so resolve before fan-out.
2. **`_instance` reserved segment.** The layout assumes no real dashboard `uid`
   sanitises to `_instance`. REPORT/SCOPE should document `_instance` as a reserved
   target-namespace segment so a pathological uid cannot collide with the sidecar dir.

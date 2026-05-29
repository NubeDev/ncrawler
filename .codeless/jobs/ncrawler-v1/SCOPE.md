# Scope - ncrawler-v1

The authoritative design lives at
[/home/user/code/rust/ncrawler/SCOPE.md](../../../SCOPE.md). This
brief is the trimmed per-job scope. Where this disagrees with the
deep doc, **the deep doc wins** - fix this file rather than diverge.

## Goal

Land ncrawler v1: a modular two-phase data-collection toolkit where
**Scraper -> Artifact (on-disk, versioned) -> Builder** is the only
shape that exists. After this job:

1. The `ncrawler-spi` contract (`Artifact` / `Item` / `Asset` with
   `item_id` linkage, per-source stable `Item.id`, `schema_version=1`,
   best-effort `meta`) is the single seam between scrapers and
   builders.
2. `ncrawler-grafana` scrapes dashboards in three modes (`Api` via
   the pinned `grafana = =0.1.3` crate; `Visual` via the renderer
   plugin; `Both` merged) and writes a real artifact end-to-end.
3. `ncrawler-spider` scrapes arbitrary web pages HTTP-only (no
   `spider_chrome`), with `dom_smoothie` readable extraction and
   our own `chromiumoxide` layer for JS rendering.
4. `ncrawler-report-md` renders any artifact deterministically.
5. `ncrawler-report-ai` summarises any artifact via
   `starter-ai::ClaudeRunner` and a `starter-skills` bundle, with one
   bundled skill at `skills/grafana-triage/`.
6. `ncrawler-vector` (stretch) embeds + upserts artifacts into a
   default LanceDB directory or, opt-in, a Qdrant server.

## In scope (five stages with two REVIEW gates)

- **Stage 1 - Skeleton + SPI contract.** Workspace, three core
  crates wired, `ncrawler-spi` types with `Asset.item_id` linkage and
  documented Item-id stability rules, on-disk artifact store with
  `latest` symlinks and 0700 perms, CLI shell where `ls` / `show`
  work against an empty store. **REVIEW gate.**
- **Stage 2 - Grafana API scraper + Markdown builder.**
  `ncrawler-grafana` mode=`Api` end-to-end via a `GrafanaClient` trait
  in `client.rs` that wraps the pinned `grafana` crate; deterministic
  `panel-{panelId}` IDs; SSRF allow-host validation at scrape time;
  wiremock unit tests. `ncrawler-report-md` with golden-file tests
  exercising the `item_id` merge path.
- **Stage 3 - AI report builder + first skill.**
  `ncrawler-report-ai` pipeline (md prompt -> `SkillSelector` ->
  `ClaudeRunner`) wired to `starter-ai` (only `provider-claude`) and
  `starter-skills` with the `grafana-triage` bundle on disk; mock-runner
  unit tests + `RUN_LIVE_TESTS=1` live-Claude integration test.
  **REVIEW gate.**
- **Stage 4 - Visual/Both modes + Spider scraper.** Grafana
  `Visual`/`Both` via `/render/d-solo/...` (renderer plugin primary,
  `--visual-fallback chrome` opt-in via our `chromiumoxide` layer
  documented as flaky); `ncrawler-spider` HTTP-only with
  `dom_smoothie` + `fast_html2md`, SSRF allow-host enforced at
  scrape time, `--render-js` opting into Chrome.
- **Stage 5 (stretch) - Vector builder.** `ncrawler-vector` with
  `fastembed` + `lancedb` default; `store-qdrant` feature optional;
  upserts keyed on `(source, target, item_id)`.

## Out of scope

- **Spider Cloud, hosted Anthropic, OTLP exporter, qdrant-as-default.**
  All four would break the daemon-free / OSS-only / no-managed-services
  goals.
- **`spider`'s `chrome` and `smart` features.** Single browser stack
  via upstream `chromiumoxide` only; `cargo tree -e normal` must NOT
  show `spider_chrome`.
- **The `time` crate.** `chrono` is pinned to match starter; mixing
  forks the datetime type at the seam.
- **A second engine behind any starter promotion.** `starter-headless`,
  `starter-vector`, `starter-artifact` are promotion *candidates* in
  the deep SCOPE; this job does not lift any of them.
- **Long-lived daemon, scheduler, web UI, write-back to Grafana,
  multi-tenant auth, real-time alerting.** All explicitly out of v1.
- **Dependence on `meta` keys.** Builders may read `meta` best-effort;
  the moment any builder *requires* a `meta` key, that key gets
  promoted to a typed `Item`/`Artifact` field with a `schema_version`
  bump in the same stage.

## Constraints

- **OSS-only deps**, MIT / Apache-2.0 / BSD. No GPL/AGPL.
- **Dep versions aligned with starter** so `cargo tree -e normal`
  shows a single version of `tokio`, `reqwest`, `serde`, `chrono`,
  `clap` after wiring path deps to `starter-spi` / `starter-ai` /
  `starter-skills` / `starter-secrets-*` / `starter-observability`.
- **Starter HOW-TO-CODE.md budgets**: <=400 lines per file, <=50
  lines per function. The Grafana crate is pre-split into
  `client.rs` / `api.rs` / `visual.rs` / `merge.rs` for this reason;
  preserve the split.
- **Per-source `Item.id` stability is a hard contract** documented
  in the deep SCOPE. Adding a new source means adding its stability
  rule to SCOPE.md *in the same stage*.
- **`Asset.item_id` is the only Item<->Asset linkage.** Builders MUST
  NOT label-match; tests must assert this by giving two assets
  identical labels.
- **`meta` is best-effort.** Builders that read it treat missing keys
  as "feature off", never as an error.
- **Tokens never logged.** Bearer tokens flow through
  `starter_ai::secret`-style wrappers; no `tracing` field, log line,
  test fixture, or persisted Event ever carries one.
- **SSRF allow-list is enforced at scrape time**, not build time
  (build phase makes no outbound calls in v1).
- **Headless Chrome sandboxed by default**; `NCRAWLER_CHROME_NO_SANDBOX=1`
  opts out and logs WARN.
- **No `--force`, no `--no-verify`.** If a hook fails, fix the cause.

## Deliverables (what "done" looks like)

1. Branch `codeless/ncrawler-v1` with one commit per stage (five
   stages = five commits), pushed.
2. `cargo build --workspace` green at every stage boundary.
3. `cargo clippy --workspace --all-targets -- -D warnings` green at
   every stage boundary for crates touched in that stage.
4. `cargo fmt --check` green at every stage boundary.
5. `cargo test --workspace` green at every stage boundary; live-net
   and live-Claude tests gated on `RUN_LIVE_TESTS=1` and NOT run by
   default.
6. `cargo tree -e normal` recorded in the stage-1 handover and
   updated in the stage-4 handover showing (a) single versions of
   `tokio`, `reqwest`, `serde`, `chrono`, `clap`; (b) no
   `spider_chrome` anywhere in the graph.
7. `skills/grafana-triage/SKILL.md` lands as a real bundle that
   `starter-skills` accepts under its blake3 content-hash
   quarantine.
8. End-to-end smoke under `RUN_LIVE_TESTS=1`: `ncrawler scrape
   grafana --mode both` against the docker-compose Grafana fixture
   in `tests/fixtures/`, followed by `ncrawler build report-ai`
   against the resulting artifact, both succeed and produce
   `build-report-ai.md` + `build-report-ai.jsonl` next to the
   artifact with no token strings in either.

## Open questions - RESOLVED (2026-05-29, before start)

The deep SCOPE.md is the authoritative resolution. Three job-specific
notes follow.

### Q1 - Single job vs split per scraper / builder?

**Answer: single job, five stages, two REVIEW gates.**

The stages are vertical slices that depend on each other (every
later stage consumes the SPI fixed in stage 1; the AI builder in
stage 3 needs the artifact format from stage 2 to test against;
the visual + spider work in stage 4 fans out only after the
end-to-end Grafana-API -> Claude pipeline has shipped). The two
REVIEW gates sit at the two cheapest-to-amend / most-expensive-to-
get-wrong boundaries: after the SPI contract (touching every
later stage) and after the AI builder (before fanning out into
visual + spider complexity).

### Q2 - Grafana API: hand-rolled reqwest vs the `grafana` crate?

**Answer: the pinned `grafana = =0.1.3` crate, isolated behind a
`GrafanaClient` trait in `client.rs`.**

The crate is async + tokio + reqwest + rustls (matches our stack),
MIT, ships dashboard/folder wrappers + a generated OpenAPI layer +
a `raw()` escape hatch. Risks (single maintainer, v0.1.x churn, low
download count) are mitigated by exact pinning + single-file
isolation behind the trait. The renderer endpoint sits outside
`/api/` so `client.rs` also keeps a plain `reqwest::Client` for the
visual path - the crate does not own that surface.

### Q3 - LanceDB vs sqlite-vec for the default vector store?

**Answer: LanceDB.**

LanceDB ships proper ANN indexes and hybrid (vector + full-text)
search; sqlite-vec is brute-force KNN only (no HNSW/IVF as of late
2025). LanceDB also has a first-class native async Rust client and
a single-directory on-disk store, both of which fit the
artifact-local / daemon-free goal. sqlite-vec's advantage is
re-using an existing SQLite, which ncrawler does not have.

## References

- Deep design (authoritative):
  [/home/user/code/rust/ncrawler/SCOPE.md](../../../SCOPE.md).
- Reference Claude runner consumed via `starter-ai`:
  [/home/user/code/rust/starter/crates/starter-ai/src/runners/claude.rs](/home/user/code/rust/starter/crates/starter-ai/src/runners/claude.rs).
- Skill bundle conventions:
  `starter-skills` crate root - parser, blake3 hash quarantine,
  `SkillSelector` implementations.
- Spider crate (HTTP-only usage): https://github.com/spider-rs/spider.
- Grafana SDK (pinned `=0.1.3`): https://docs.rs/grafana/0.1.3/grafana/.
- LanceDB Rust client: https://lancedb.github.io/lancedb/.

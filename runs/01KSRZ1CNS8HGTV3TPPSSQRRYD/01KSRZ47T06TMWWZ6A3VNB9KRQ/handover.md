## Done

- Created the `ncrawler` virtual Cargo workspace (`Cargo.toml`) with members for ncrawler-spi/core/cli plus five empty placeholder crates under `crates/scrapers/` and `crates/builders/`, and the top-level `skills/grafana-triage/`.
- `ncrawler-spi`: `ARTIFACT_SCHEMA_VERSION = 1`; `Artifact` / `Item` / `ItemKind` / `Asset` (`item_id: Option<String>`) / `BuildOutput`; `Scraper` / `Builder` traits; `ScrapeJob` / `BuildCtx`; `ScrapeError` (incl. `RendererPluginMissing`, `SsrfBlocked`) / `BuildError`; `Cancel` re-exported from `starter_spi::ai::Cancel`.
- `ncrawler-core`: `ArtifactStore` with timestamped dirs `<rfc3339-utc>__<source>__<safe(target)>` (format `%Y-%m-%dT%H-%M-%SZ`), per-`(source,target)` `latest` symlink rewrite (`../../<dirname>`), `0700` dir perms (unix), dirname-as-index `list()` with `--since` duration parsing, and JSON read/write round-trip rejecting `schema_version > 1`.
- `ncrawler-cli`: clap-derive shell with `scrape`/`build`/`ls`/`show`; `ls`+`show` work end-to-end (empty store prints "no artifacts found"), `scrape`/`build` print not-yet-implemented stubs keyed by source/builder name. tracing-subscriber init.
- Pinned tokio/reqwest/serde/chrono/clap to sibling `starter` versions. `cargo tree -e normal` shows single versions: tokio v1.52.3, reqwest v0.12.28, serde v1.0.228, chrono v0.4.44, clap v4.6.1.
- Committed on branch `codeless/ncrawler-v1` (commit `0ff76b5`).

## Next

- Stage 2 (M2 — Grafana API): implement `ncrawler-grafana` mode=`Api` with `panel-{panelId}` IDs and wire `scrape grafana` end-to-end; add the scraper registry to core/cli; implement `ncrawler-report-md`.

## What you need to know

- Working in git worktree `/home/user/.codeless/worktrees/job-01KSRZ1CNS8HGTV3TPPSSQRRYD`; canonical checkout is `/home/user/code/rust/ncrawler`.
- `starter-spi` is a path dep via an **absolute** path (`/home/user/code/rust/starter/crates/starter-spi`) in the root `[workspace.dependencies]` — relative `../starter` does not resolve from the worktree; absolute resolves from both locations. Revisit if the repo ever moves machines.
- `Cargo.lock` is gitignored (matches starter's library convention); reconsider committing it once the binary stabilises.
- `ScrapeJob.allow_hosts` is `Vec<String>` for now (SCOPE names a `HostPattern` type — define it when the SSRF guard lands).
- SCOPE.md is copied into the worktree root and is the locked authoritative brief.

## Open questions

- (none)

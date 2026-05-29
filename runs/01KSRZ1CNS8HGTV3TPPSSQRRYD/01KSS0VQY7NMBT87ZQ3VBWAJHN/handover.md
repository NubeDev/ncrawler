## Done

- Grafana mode=Visual (visual.rs) and mode=Both (merge.rs) implemented: renderer-plugin PNGs via hand-rolled reqwest RendererClient in client.rs (probe via /api/frontend/settings, render via /render/d-solo/{uid}/_), one PNG per panel written to assets/ with Asset.item_id linked to its panel-{id} Item; RendererPluginMissing returned on probe failure.
- --visual-fallback chrome gated behind the flag with a flaky-path WARN at startup; grafana chrome.rs drives the shared chromiumoxide layer (whole-dashboard PNG, item_id=None) with NCRAWLER_CHROME->which->well-known discovery + NCRAWLER_CHROME_NO_SANDBOX WARN opt-out.
- ncrawler-spider: spider crate default-features=false (features sync + reqwest_rustls_tls only; no chrome/smart, no spider_chrome). Page->Item::Page with id page-{blake3(normalised_url)[..16]} (lowercase host, sorted query, no fragment), dom_smoothie readable text + fast_html2md (lib name `html2md`) markdown in data, SSRF allow-host validation on seed + every followed URL.
- spider chrome.rs: own chromiumoxide JS layer opted into via --render-js, same Chrome discovery chain + sandbox WARN.
- CLI scrape wired for grafana/spider exposing --mode/--visual-fallback/--from/--to/--panel/--allow-host and --depth/--limit/--delay/--render-js/--no-robots; writes artifact via ArtifactStore.
- Tests: wiremock + golden (tests/fixtures/merge_golden.json) for renderer Visual + Both/merge; spider unit tests for url-normalisation/id/SSRF/extraction; live-net + headless-Chrome tests gated on RUN_LIVE_TESTS=1 and #[ignore].
- cargo build/clippy(-D warnings)/fmt --check/test --workspace all green. Committed: fc03861.

## Next

- Stage 7 (REVIEW gate / final stage of 7): a fresh session picks it up. Decide the reqwest dual-version deviation (accept grafana 0.1.3 + reqwest 0.13, or hand-roll client.rs on reqwest 0.12) per SCOPE Deviations §.
- Optional: M5 ncrawler-vector (stretch, not v1-blocking) remains a placeholder crate.

## What you need to know

- cargo tree -e normal: SINGLE browser stack — chromiumoxide v0.7.0 only; spider_chrome and chromey absent (verified). reqwest shows BOTH 0.12.28 (workspace/visual/spider) and 0.13.4 (transitive via grafana 0.1.3) — the pre-existing documented deviation in SCOPE.md "Deviations".
- Asset-write flow: scrapers write PNGs themselves into out/<dirname>/assets/ where dirname = ncrawler_core::dir_name(fetched_at, source, target); the store then writes artifact.json into the same dir (recomputes the same name from artifact.fetched_at). lib.rs assets_dir_for() does this; module fns (visual::scrape/merge::scrape) take assets_dir explicitly so tests pass a tempdir.
- ncrawler-grafana now depends on ncrawler-core (for dir_name), reqwest, futures, chromiumoxide, tokio. Workspace gained url/spider/dom_smoothie/fast_html2md/chromiumoxide/futures deps.
- Renderer probe key is `rendererAvailable` in /api/frontend/settings; render path slug is `_` (/render/d-solo/{uid}/_). Auth 401/403 from renderer maps to ScrapeError::Auth, other non-2xx to RendererPluginMissing.
- Cargo.lock is gitignored in this repo (consistent with stages 1-3), so it is not part of the commit.

## Open questions

- SSRF for spider is enforced by validating the seed host and each resulting Page URL (spider stays same-domain by default); there is no per-link pre-follow hook used. If REVIEW wants hard pre-follow blocking, add a spider whitelist/blacklist or custom RemoteFetcher.
- The spider crate, even HTTP-only, pulls a large transitive tree (utoipa, spider_fingerprint, llm_models_spider, etc.). SCOPE already flags spider as "minimal maintenance"; REVIEW may weigh replacing it with reqwest + dom_smoothie directly behind the unchanged Scraper trait.

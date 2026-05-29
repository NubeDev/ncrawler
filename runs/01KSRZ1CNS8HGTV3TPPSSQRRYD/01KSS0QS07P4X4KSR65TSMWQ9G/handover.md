## Done

- REVIEW gate for the AI builder + skill plumbing stage. Examined the stage-3 diff (`ncrawler-report-ai`, `skills/grafana-triage/`, CLI dispatch) against the Layer-1 rulebook.
- Verified R1 (dep direction): `ncrawler-spi` has no impl deps (serde/serde_json/thiserror/async-trait/chrono + `starter-spi` for the `Cancel` re-export only); `report-ai` → `report-md`/`spi`/`starter-ai`/`starter-skills`/`starter-flow-spi`; `cli` → builders. No cycles.
- Verified R2 (single transport): the AI path runs solely through `starter-ai` `Registry::with_defaults()` → `Provider::Claude` → `ClaudeRunner::run`; events arrive via one `mpsc::Sender<Event>` drained concurrently with the run future. No second AI runner/transport.
- Verified R4/R5 (trust boundary): secrets routed through `starter_ai::api_key_for`, placed only in runner input, never in Events/logs (redaction test `pipeline_streams_persists_and_redacts` passes — fake marker handed to runner, absent from `.jsonl`/`.md`); blake3 hash quarantine enforced (`list_quarantined()` empty assertion, quarantined bundles never reach `SkillSelector`); SKILL.md treats snapshot content as untrusted data.
- Verified wire-formats untouched: `ARTIFACT_SCHEMA_VERSION = 1` unchanged; `build-report-ai.md` + `build-report-ai.jsonl` are additive outputs.
- Confirmed `SkillSelector` matches `ncrawler.skills.grafana-triage` for a Grafana artifact (source=grafana + tags).
- Ran `cargo build --workspace` (green), `cargo test -p ncrawler-report-ai` (4 pass, 1 `#[ignore]` live), `cargo clippy -p ncrawler-report-ai --all-targets` (clean); grepped fixtures/sources — no token literals anywhere except the deliberate fake test marker.
- Recorded the verdict in an empty commit on `codeless/ncrawler-v1` (`182c74f`).

## Next

- Stage 4 / M4: Grafana `Visual`/`Both` via renderer plugin + `--visual-fallback chrome`, and `ncrawler-spider` HTTP-only. Not started (separate session).
- Before stage 4 fans out: amend SCOPE.md re the reqwest version deviation (see below), since SCOPE locks deviations behind a doc update.

## What you need to know

- PASS verdict. The one open SCOPE deviation is dependency-policy, not a Layer-1 break, and is pre-existing (introduced by the `grafana = =0.1.3` pin in stage 2, untouched here): the dep-policy invariant "`cargo tree -e normal` MUST show a single version of reqwest" is violated — `reqwest v0.12.28` (via grafana) coexists with `reqwest v0.13.4` (via the starter workspace). SCOPE.md must be amended (accept the duplicate with rationale, or repin grafana/starter to converge) before stage 4 adds the visual + spider HTTP stacks that also pull reqwest.
- `Cargo.lock` is gitignored in this repo (prior-stage decision), so dep changes are not captured in the lockfile.
- Skill ids must be reverse-DNS (≥2 dot segments); the bundle uses `ncrawler.skills.grafana-triage` while the directory is `grafana-triage`. Default selector is `KeywordSkillSelector` (description contains "grafana"), with single-bundle first-candidate fallback.
- `ClaudeRunner::run` returns `RunResult` (not a stream); the pipeline prefers `RunResult.text` and falls back to accumulated `Text` events.

## Open questions

- (none)

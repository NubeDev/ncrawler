## Done

- Implemented `ncrawler-report-ai`: `AiReportBuilder` (`Builder` impl) composing the pipeline — render via `ncrawler-report-md` → resolve skill via `starter-skills` `SkillSelector` against artifact `source`+`tags` → `Registry::with_defaults()` → `Provider::Claude` → `ClaudeRunner::run` streaming `Event`s.
- Persists assistant text to `build-report-ai.md` and the streamed Event log (one JSON object/line) to `build-report-ai.jsonl` next to the artifact.
- `SkillResolver` seam (`skills.rs`) backed by the real `SkillRegistry`; `RegistrySkillResolver` builds a `SlotMap` from source+tags and a read-only operator `Principal`. Quarantined bundles never reach the selector.
- Cancellation: CLI Ctrl-C → `starter_ai::TokenCancel` (`starter_spi::ai::Cancel`), checked pre-run and passed into `ClaudeRunner`.
- Secrets routed through `starter_ai::api_key_for`; only placed in runner input, never in events/logs (test asserts redaction).
- `skills/grafana-triage/SKILL.md`: `deny_unknown_fields`-clean frontmatter (id `ncrawler.skills.grafana-triage`, reverse-DNS), anomaly+threshold+correlation system prompt, explicit `file://thresholds.md` resource. blake3 quarantine succeeds on load (lands approved).
- CLI `build` now dispatches `report-md` and `report-ai`.
- Tests: selector-match against a real loaded registry + scripted mock `ClaudeRunner`; streaming/persistence/secret-redaction; missing-skill + pre-run cancellation; `#[ignore]` `live_end_to_end_claude` gated on `RUN_LIVE_TESTS=1`. `cargo build/clippy/fmt/test --workspace` all green.

## Next

- Stage 5 (M4): Grafana `Visual`/`Both` via renderer plugin + `--visual-fallback chrome`, and `ncrawler-spider` HTTP-only. Not started.

## What you need to know

- Skill ids MUST be reverse-DNS (`KindId` rule: ≥2 dot segments) — `grafana-triage` alone is invalid; the bundle uses `ncrawler.skills.grafana-triage`. The directory name is independent.
- Default selector is `KeywordSkillSelector` (no AiRunner passed to the skill registry); the skill `description` includes "grafana" so it matches `source=grafana`. With a single bundle it also falls back to first-candidate.
- New workspace path deps added: `starter-ai` (feature `provider-claude` only), `starter-skills`, `starter-flow-spi`. `starter-ai` declared `default-features=false` at workspace level; feature selected at consuming crates.
- `ClaudeRunner::run` returns `RunResult` (not a stream); events arrive via an `mpsc::Sender<Event>`. The pipeline drains the channel concurrently with the run future via `tokio::join!`; assistant text uses `RunResult.text`, falling back to accumulated `Text` events.
- CLI resolves skills dir from `NCRAWLER_SKILLS_DIR` or `./skills`.
- `Cargo.lock` is gitignored in this repo (prior-stage decision), so dep changes aren't committed in the lockfile.

## Open questions

- SCOPE pre-existing deviation (unchanged this stage): `grafana=0.1.3` forces `reqwest 0.13` alongside `0.12`, so the "single reqwest version" invariant still doesn't hold — surface at the REVIEW gate.

## Done

- Added `crates/ncrawler-redact` — a standalone crate (kept out of `ncrawler-spi` per the stage's explicit allowance so `regex`/`quickcheck` don't leak into the contract crate) with `Redactor::redact(text) -> Cow<str>` (zero-copy no-match path) and `Redactor::redact_variables(&mut HashMap<String,String>)`. Masks: long hex (32+), UUID v4, `password|secret|key = '...'` SQL literals, `Bearer <token>`; mask token `***REDACTED***` is itself unmaskable (idempotent). `redact_variables` also masks by secret-ish key name.
- Wired it into workspace: added the crate to `members` and a `[workspace.dependencies]` entry, plus `regex`/`quickcheck`/`quickcheck_macros` deps.
- `ncrawler-report-md`: applies the redactor as a builder-side pass before writing `report.md`. `render()` stays pure/raw (golden tests + report-ai prompt body unchanged). Toggle via `BuildCtx` option `redact` (default true); `redact_markdown()` and `REDACT_OPTION` exported.
- `ncrawler-cli`: `build` gains `--redact` (default) / `--no-redact`; opt-out logs a WARN (`resolve_redact`), threaded through `ctx.options.redact`.
- Tests: quickcheck property tests for (a) all documented patterns masked, (b) non-secret SQL identifiers round-trip byte-identically + borrowed, (c) idempotence/true bypass; plus builder-level tests for default-mask and `--no-redact` verbatim bypass.
- `cargo build / clippy -D warnings / fmt --check / test --workspace` all green. Committed as `59bd909`.

## Next

- Stage 5 (report-grafana builder): use `Redactor::redact_variables` on the per-panel variables section, and apply `redact_markdown`/`Redactor::redact` before emitting the Grafana report.
- Consider applying the same redaction pass in `ncrawler-report-ai` before persisting `build-report-ai.md` (out of this stage's scope; report-ai currently consumes raw `render()`).

## What you need to know

- Two recovery fixes are folded into this commit: an early scratch command of mine had clobbered `.gitignore` (restored from `a70e348` — note `Cargo.lock` and `artifacts/` are intentionally git-ignored, so the lockfile is NOT committed), and `cargo fmt --all` corrected a pre-existing formatting nit in `ncrawler-spider/src/lib.rs`.
- DEVIATION for the REVIEW gate: SCOPE §Security points at `starter_ai::secret`-style wrappers for type-level known secrets. At the report-md layer no string is known-secret at the type level, so masking is pattern-based on rendered text; the wrapper reuse belongs where report-grafana surfaces variables. Recorded here for ratification.
- The commit message (59bd909) lost two backtick-quoted words (`redact`, `ncrawler-cli`) to shell backtick-substitution — content is still accurate; not amended.

## Open questions

- Should `report-ai` also redact its persisted `build-report-ai.md`? Left raw for now since the stage scoped only report-md + CLI.

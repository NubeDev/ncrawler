# Workflow - ncrawler-v1

How to drive the stages in `template.yaml`. Read this before every
stage alongside `SCOPE.md` (the trimmed per-job brief) and the deep
design at [/home/user/code/rust/ncrawler/SCOPE.md](../../../SCOPE.md).

## Sequencing

Five stages, two REVIEW gates:

- **REVIEW after stage 1.** The SPI contract is the single seam
  every later stage builds on; getting `Artifact` / `Item` /
  `Asset.item_id` / per-source ID stability / `schema_version`
  wrong here costs work in every subsequent stage.
- **REVIEW after stage 3.** The AI builder + skill plumbing is the
  first time `starter-ai` + `starter-skills` are exercised
  end-to-end against a live Claude CLI; confirm streaming +
  cancellation + secret handling + hash quarantine all behave
  before fanning out into the visual + spider work in stage 4.

Stages otherwise run linearly. A REVIEW gate pauses the *next*
stage; the work that *led* to the gate completes its closing trio
(checks + docs + git) normally and pushes.

Within a stage, prefer one crate at a time. Cross-crate refactors
belong in their own stage with their own commit, not bundled into
the stage that motivated them.

## Per-stage discipline

Before writing any code or docs in a stage:

1. Re-read the deep SCOPE.md end-to-end. It is locked. If you find
   a real ambiguity, stop the stage, propose a SCOPE.md diff in
   the handover, and surface it at the next REVIEW gate. Do not
   silently reinterpret it.
2. Re-read the previous stage's `handover.md` in the worktree.
   Anything that needs to survive a stage boundary lives there,
   not in the agent's head.
3. Skim the matching crates in the sibling `starter` workspace
   before adding a path dep. Match the version pins exactly so
   `cargo tree -e normal` continues to show a single version of
   `tokio`, `reqwest`, `serde`, `chrono`, `clap`.
4. For stage 1: design the SPI types on paper (or in a scratch
   doc) before writing them. Once shipped they are the contract
   every later stage depends on.
5. For stage 2: enumerate the Grafana endpoints the API mode
   needs *before* picking which to reach through `client.dashboards()`,
   which through `client.openapi()`, and which through `client.raw()`.
6. For stage 3: read `starter_ai::runners::claude::ClaudeRunner`
   and `starter_skills::registry` end-to-end before wiring; they
   define the streaming + cancellation + hash-quarantine semantics
   the AI builder must preserve.
7. For stage 4: run `cargo tree -e normal | grep -i chrome`
   BEFORE adding any crate that mentions Chrome. If `spider_chrome`
   ever appears, back out the `spider` feature - do not "fix
   forward".

Before claiming the stage done:

1. `cargo build --workspace` green from the repo root.
2. `cargo clippy --workspace --all-targets -- -D warnings` green
   for crates touched this stage.
3. `cargo fmt --check` green.
4. `cargo test --workspace` green; live-net (`RUN_LIVE_TESTS=1`)
   and live-Claude tests stay gated and are NOT run by default.
5. For stages 1 and 4: `cargo tree -e normal` captured in the
   handover.
6. Tokens never appear in any captured log, test fixture, or
   persisted Event stream. Grep the worktree for the value of
   `GRAFANA_TOKEN` (if set) before committing.

Commit + push at stage end:

```
git add -A
git commit -m "stage N: <one-line title from template.yaml>"
git push origin codeless/ncrawler-v1
```

No `--force`, no `--no-verify`. If a hook fails, fix the cause.

## REVIEW gate behaviour

At a REVIEW gate, the stage that *led* to the gate completes the
closing trio normally (checks + docs + git all green, pushed).
The handover for the *next* stage answers:

- What in the deep SCOPE.md does this work confirm, and what
  surprises surfaced?
- What new invariants does the runtime now rely on that aren't in
  SCOPE.md yet? If any: propose the SCOPE.md diff in the same
  handover.
- What deferred items - if any - did this stage punt to a later
  stage, and which stage owns them?

The reviewer can: approve and start the next stage, request a
follow-up commit on the just-finished stage's branch, or amend
SCOPE.md before unblocking.

## Closing trio - the last three todos of every stage

Every stage's todo checklist ends with the same three items, in
order. The user watches these tick over in the `Stages` overview;
they are how the user confirms a long-running stage actually
landed instead of just looking like it did. Do **not** rename or
reorder them.

1. `checks` - run the stage's `verify:` list (or `verify_cmd`).
   Every step must pass. On failure: stop, fix, re-run; do not
   advance to `docs`.
2. `docs` - update `handover.md` for the next stage and the
   active session doc, in the same worktree, so the fresh agent
   that opens the next stage has the context it needs.
3. `git` - stage the changes (`git add -A` from the worktree
   root, or specific paths if the stage was surgical), commit
   with the message `stage N: <one-line title from
   template.yaml>` so the history mirrors the template stages
   one-for-one, and push to `codeless/ncrawler-v1` so the work
   is recoverable even if the worktree is wiped.

A stage is not "done" until all three todos are green and the
push succeeds. If `checks` or `git` fails, fix the cause and
retry - do not mark the stage `[x]`, do not advance, and never
`--force` or `--no-verify`. If a stage genuinely produced no
change (e.g. an investigation stage that only updated SCOPE.md
and that doc was already current), say so in the handover and
mark `git` as `skipped - no diff`, but the next stage's commit
must include any side-effect files the investigation touched.

## Anti-patterns specific to this job

- **Do not** pull in both upstream `chromiumoxide` AND `spider`'s
  `chrome` feature. The dep graph must show one CDP impl, not two.
  If `cargo tree -e normal` ever shows `spider_chrome`, the build
  is wrong - back out the `spider` feature.
- **Do not** reach for the `time` crate. Starter pins `chrono`;
  mixing forks the datetime type at the seam.
- **Do not** hand-roll Grafana `/api/...` calls instead of going
  through the `grafana` crate behind the `GrafanaClient` trait.
  The pinned dep is the abstraction; the trait is the swap-out
  seam. Bypassing both means we have neither.
- **Do not** use `unwrap()` / `expect()` in library crates. The
  CLI binary may use `anyhow` and `expect("invariant: ...")` for
  genuine impossibilities only.
- **Do not** add a builder that requires `meta` keys. If it does,
  the keys get promoted to typed `Item` / `Artifact` fields with
  a `schema_version` bump in the same stage.
- **Do not** start a long-lived process in any default code path.
  LanceDB embedded only; `qdrant-client` is feature-gated.
- **Do not** commit a flaky test for the Chrome fallback path.
  That path is documented as best-effort - its tests live behind
  `RUN_LIVE_TESTS=1` and are not part of the default suite.
- **Do not** label-match Assets to Items. Tests must give two
  Assets identical labels to prove `item_id`-only matching is
  enforced.
- **Do not** persist or log bearer tokens. Wrap them in
  `starter_ai::secret`-style types; redact in any `tracing` field
  that touches user-visible output.
- **Do not** promote any of the `starter-headless` /
  `starter-vector` / `starter-artifact` candidates to the
  `starter` workspace inside this job. The promotion rule
  ("lift when a second consumer materialises") is intentionally
  not met by stage 5 alone.

## When to halt

- Stage 1 SPI design surfaces a fourth dimension the brief
  doesn't cover (e.g. per-Item provenance, per-Asset checksum,
  artifact-level signature). Halt and amend SCOPE.md - adding
  it after the contract ships costs every later stage.
- Stage 2's `grafana` crate cannot express a required call even
  via `client.raw()`. Halt; the resolution is either to drop the
  dep (cheap because of the trait isolation) or to upstream a
  fix - either way a design call, not a workaround.
- Stage 3's `ClaudeRunner` Event stream does not carry the
  fields the AI builder needs (cost, tokens, tool-call inputs).
  Halt at the REVIEW gate; the resolution is in `starter-ai`
  (starter side), not in ncrawler.
- Stage 4 `spider_chrome` appears in `cargo tree -e normal`
  despite default-features-off. Halt and isolate the source -
  do not paper over.
- Any stage's `verify:` finds a token string in a log, fixture,
  or persisted Event. Halt at `checks`, fix the redaction, and
  rerun the stage from the top - this is a security invariant,
  not a polish step.

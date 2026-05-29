# Workflow - ncrawler-report-grafana

How to drive the stages in `template.yaml`. Read this before every
stage alongside `SCOPE.md` (the trimmed per-job brief), the deep
design at [/home/user/code/rust/ncrawler/REPORT.md](../../../REPORT.md),
and the v1 artifact contract at
[/home/user/code/rust/ncrawler/SCOPE.md](../../../SCOPE.md).

## Sequencing

Six stages, two REVIEW gates:

- **REVIEW after stage 1.** The sidecar artifact is a new on-disk
  shape every later stage reads from; the reader's legacy-meta
  fallback is the migration story for existing artifacts under
  `runs/`. Getting either wrong costs work in every later stage and
  silently corrupts the report.
- **REVIEW after stage 4.** The redactor is the security invariant
  the report relies on to default-redact secrets before writing
  `REPORT.md` to disk. Confirm coverage + no false positives on a
  representative fixture set drawn from the live `rd-esr` artifacts
  BEFORE stage 5 enables it by default.

Stages otherwise run linearly. A REVIEW gate pauses the *next* stage;
the work that *led* to the gate completes its closing trio (checks +
docs + git) normally and pushes.

Within a stage, prefer one crate at a time. Cross-crate refactors
belong in their own stage with their own commit, not bundled into the
stage that motivated them.

## Per-stage discipline

Before writing any code or docs in a stage:

1. Re-read both REPORT.md and SCOPE.md end-to-end. Both are locked.
   If you find a real ambiguity, stop the stage, propose a diff in
   the handover, and surface it at the next REVIEW gate. Do not
   silently reinterpret either.
2. Re-read the previous stage's `handover.md` in the worktree.
   Anything that needs to survive a stage boundary lives there, not
   in the agent's head.
3. Skim the matching crates in the sibling `starter` workspace and
   the existing ncrawler-v1 crates BEFORE adding a new dep. Match
   the version pins exactly. The v1 *Deviations* `reqwest 0.13`
   carve-out is inherited; do not introduce any new transitives.
4. For stage 1: design the sidecar JSON on paper (or in a scratch
   doc) BEFORE writing it. Once shipped it is the contract every
   later stage reads. The reader's legacy-meta fallback path must
   be explicit and tested (sidecar present => fallback unreachable;
   sidecar absent + legacy `meta.search` present => fallback hit and
   WARN logged; both absent => InstanceFactsMissing error).
5. For stage 2: enumerate the selector permutations BEFORE coding,
   and write the dual-`--all` divergence-warning test FIRST so the
   semantics are documented in code.
6. For stage 3: run the multi-dashboard scrape against the
   `tests/fixtures/` Grafana fixture under RUN_LIVE_TESTS=1 BEFORE
   declaring stage done; a passing wiremock suite is necessary but
   not sufficient for a stage that introduces real concurrency.
7. For stage 4: draft the redactor patterns AND the false-positive
   corpus BEFORE writing the matcher. The property test that
   asserts no false-positives on a sample of real Grafana variable
   names + SQL column names is non-negotiable.
8. For stage 5: read REPORT.md \u00a74 end-to-end and lay out the
   three sections in the order shown there. The metadata header in
   \u00a74 is verbatim; do not "improve" it.
9. For stage 6: read REPORT.md \u00a75 end-to-end and enumerate the
   seven check classes BEFORE coding any of them. Each check has its
   own fixture exercising both present and absent paths.

Before claiming the stage done:

1. `cargo build --workspace` green from the repo root.
2. `cargo clippy --workspace --all-targets -- -D warnings` green for
   crates touched this stage.
3. `cargo fmt --check` green.
4. `cargo test --workspace` green; live-net (`RUN_LIVE_TESTS=1`) and
   live-Claude tests stay gated and are NOT run by default.
5. For stages 1 and 6: `cargo tree -e normal` captured in the
   handover.
6. Tokens never appear in any captured log, test fixture, or
   persisted Event stream. Grep the worktree for the value of
   `GRAFANA_TOKEN` (if set) before committing.
7. For stages 4 and 5: grep the rendered `REPORT.md` fixture outputs
   for known-secret patterns (long hex, uuid v4, `password=`); none
   may appear in default-redact-on output.

Commit + push at stage end:

```
git add -A
git commit -m "stage N: <one-line title from template.yaml>"
git push origin codeless/ncrawler-report-grafana
```

No `--force`, no `--no-verify`. If a hook fails, fix the cause.

## REVIEW gate behaviour

At a REVIEW gate, the stage that *led* to the gate completes the
closing trio normally (checks + docs + git all green, pushed). The
handover for the *next* stage answers:

- What in REPORT.md / SCOPE.md does this work confirm, and what
  surprises surfaced?
- What new invariants does the runtime now rely on that aren't in
  REPORT.md or SCOPE.md yet? If any: propose the diff in the same
  handover.
- What deferred items - if any - did this stage punt to a later
  stage, and which stage owns them?

The reviewer can: approve and start the next stage, request a
follow-up commit on the just-finished stage's branch, or amend
REPORT.md / SCOPE.md before unblocking.

## Closing trio - the last three todos of every stage

Every stage's todo checklist ends with the same three items, in
order. The user watches these tick over in the `Stages` overview;
they are how the user confirms a long-running stage actually landed
instead of just looking like it did. Do **not** rename or reorder
them.

1. `checks` - run the stage's `verify:` list (or `verify_cmd`).
   Every step must pass. On failure: stop, fix, re-run; do not
   advance to `docs`.
2. `docs` - update `handover.md` for the next stage and the active
   session doc, in the same worktree, so the fresh agent that opens
   the next stage has the context it needs.
3. `git` - stage the changes (`git add -A` from the worktree root,
   or specific paths if the stage was surgical), commit with the
   message `stage N: <one-line title from template.yaml>` so the
   history mirrors the template stages one-for-one, and push to
   `codeless/ncrawler-report-grafana` so the work is recoverable
   even if the worktree is wiped.

A stage is not "done" until all three todos are green and the push
succeeds. If `checks` or `git` fails, fix the cause and retry - do
not mark the stage `[x]`, do not advance, and never `--force` or
`--no-verify`.

## Anti-patterns specific to this job

- **Do not** let the report builder reach a `reqwest::Client`. The
  report is a pure renderer over on-disk artifacts; live HTTP in
  the build phase is forbidden. A test asserts the audit path
  carries no `reqwest::Client` in its call graph.
- **Do not** match dashboards by title for the duplicate detector.
  Fingerprint over `(panels, targets, variables)` ONLY. Mirror the
  v1 `Asset.item_id`-only discipline: a test that gives two
  dashboards identical titles must NOT mark them duplicates.
- **Do not** invent an executed SQL string when the datasource
  does not expose `executedQueryString`. The renderer omits the
  field cleanly; faking it would silently lie about what ran.
- **Do not** redact at scrape time. Artifacts stay raw on disk
  (under 0700) so audit / forensic re-renders remain possible.
  The masker runs at the renderer.
- **Do not** introduce false positives in the redactor. Grafana
  variable names like `${__from}` / `${__to}`, plain SQL column
  names, common identifiers (`host`, `key`, `password_hash` as a
  column NAME not a value assignment) must round-trip
  byte-identically through `redact()`.
- **Do not** silently drop `meta.search` from existing artifacts
  under `runs/`. The reader's legacy fallback is the migration
  story; the cut-over is at the writer side only.
- **Do not** fork a second on-disk store for the sidecar. The
  existing `ncrawler-core` store handles it via a synthetic
  `target=`_instance:<host>``.
- **Do not** introduce the `time` crate, the `spider` `chrome` /
  `smart` features, hosted Anthropic, Spider Cloud, OTLP, or
  qdrant-as-default. All v1 invariants still hold.
- **Do not** `--force` or `--no-verify` to push past a failing
  hook. Fix the cause.
- **Do not** persist or log bearer tokens. The
  `starter_ai::secret`-style wrappers remain the type-level
  discipline for known-secret values; the new value-based redactor
  is additional, not a replacement.
- **Do not** promote a new `ncrawler-redact` crate (if introduced)
  to the `starter` workspace inside this job.

## When to halt

- Stage 1's sidecar reader cannot agree with the legacy `meta.search`
  shape on a representative fixture under `runs/`. Halt and reconcile
  before fanning out the selector in stage 2.
- Stage 2's selector cannot express a real selection from the live
  inventory (e.g. an edge case in title substring matching against
  Unicode). Halt and amend REPORT.md \u00a72 before coding around it.
- Stage 3's multi-dashboard scrape produces flaky tests under
  bounded concurrency. Halt; the resolution is in the limiter, not
  in `#[ignore]`-ing the test.
- Stage 4's redactor surfaces a false positive on a real-world
  Grafana panel name from the `rd-esr` artifacts. Halt and refine
  patterns before stage 5 ships them by default.
- Stage 5's report contains any executedQueryString value when the
  underlying datasource is the Rubix OS plugin. Halt; this is a
  fidelity invariant, not a polish step.
- Stage 6's duplicate detector flags two dashboards with identical
  titles but distinct panel sets as duplicates. Halt; the
  fingerprint discipline is the whole point of stage 6.
- Any stage's `verify:` finds a token string, a host UUID, or a
  tenant id from the live `rd-esr` instance in a redacted-default
  output. Halt at `checks`, fix the redaction, and rerun the stage
  from the top - this is a security invariant.

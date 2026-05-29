# Stage 6 REVIEW — redaction-pass gate

Gate over the redaction WORK diff
(`59bd909 stage 4 - shared redaction pass (default-on): ncrawler-redact + wire report-md & CLI`).

## Verdict: PASS

The Layer-1 rulebook invariants hold; the gate is about those invariants, and
nothing in the redaction work violates them.

- **R1 (crate dependency direction):** `ncrawler-redact` is a new *leaf* crate
  depending only on `regex` (`cargo tree -e normal -p ncrawler-redact` shows
  regex/aho-corasick/memchr and nothing inward). `ncrawler-report-md →
  ncrawler-redact` is downward (builder → utility). `ncrawler-spi` is untouched —
  the pattern matcher and `quickcheck` deliberately stay out of the
  dependency-light contract crate. No inversion.
- **R2 (single transport):** redaction adds no transport — no reqwest/http, no
  `spider_chrome`, no daemon. It is pure in-memory text/`HashMap` masking.
- **R4/R5 (trust boundary):** redaction is correctly placed as a **builder-side
  egress** pass. The artifact on disk stays raw (defended by the SCOPE-mandated
  0700 perms); masking happens only when a renderer emits a report — i.e. exactly
  at the point data crosses the disk→shareable boundary. Token resolution/logging
  is unchanged.
- **Wire formats untouched:** `ARTIFACT_SCHEMA_VERSION` stays 1. `render()`
  remains pure/raw, so the report-md golden output and the report-ai prompt body
  are byte-identical; redaction is applied only to the emitted `report.md` via a
  `BuildCtx.options["redact"]` toggle (default-on). No schema bump, no
  serialisation change.

## Stage-specific checks (what I could verify here)

- **§7 patterns present:** long hex (32+), UUID v4, `password|secret|key='...'`
  SQL literals, and `Bearer <token>` are all masked; `redact_variables` also masks
  by secret-ish *key name* (token/password/secret/apikey/…). `cargo test -p
  ncrawler-redact` green (16 unit + property tests).
- **No false positives:** Grafana macros like `${__from}` and plain
  column/identifier names round-trip byte-identically and *borrowed* (the
  `non_secret_round_trips` quickcheck + `common_sql_identifiers_round_trip` assert
  `Cow::Borrowed`). Confirmed no masking of common SQL keywords/identifiers.
- **Idempotence:** `***REDACTED***` matches no pattern, so re-running the pass
  never double-masks — `--no-redact` is a true verbatim bypass.
- **`--no-redact` WARN:** `resolve_redact` emits a clearly-attributable
  `tracing::warn!` naming the cleartext exposure. See follow-up #2 — it lands in
  the tracing sink, **not** the persisted artifact Event stream.

## Could NOT be fully verified in this worktree (scope note, not a gate failure)

The stage's fixture-driven verification ("representative fixture set drawn from
the `rd-esr` artifacts under runs/ — variables, `rawSql`, returned data frames")
could not be executed: **no `rd-esr` Grafana artifacts exist under `runs/`** here
(only nested `handover.md` files), and the **`report-grafana` builder does not
exist yet** (only `report-md` is wired to the redactor). So §7 coverage was
confirmed against the redactor's own unit/property fixtures and the report-md
wiring, not against live `rd-esr` variables/rawSql/data frames. The full
fixture-driven pass is owed to the stage that lands `report-grafana`.

## Follow-ups to ratify (NOT patched here, per stage instruction)

1. **UUID matcher is v4-only.** `non_v4_uuid_is_not_masked` is an asserted
   behaviour: a host UUID that is not version-4 (or a tenant id that is neither
   hex-32+ nor UUID-shaped) passes through *free text* unmasked — it is only
   caught inside a variable map by secret-ish key name. REPORT §7 says "host
   UUIDs / tenant identifiers"; the live-instance secret values named in this
   stage may not all be v4. Widen the UUID pattern (any version/variant) and/or
   add the instance-observed host-UUID/tenant-id to a value denylist before
   `report-grafana` renders real variables.
2. **`--no-redact` WARN does not survive in the persisted Event stream.** It is a
   `tracing::warn!` (stderr/log sink). The stage requires the opt-out to "survive
   in the persisted Event stream for forensics"; report-md does not emit an Event
   stream, and the CLI does not route this WARN into any `build-*.jsonl`. Owe a
   persisted, attributable opt-out Event when `report-grafana` (which has the
   Event plumbing) wires redaction.
3. **Artifact-layer plaintext exposure (REPORT §7).** The scrape still persists
   `rawSql`, variables, and returned data to `artifact.json` in plaintext under
   0700. This is documented as a deliberate follow-up (redaction is an egress
   pass, the raw artifact is retained for audit/forensics), **not** silently fixed
   in this stage — as the stage requires. A future "redact-at-rest" / sealed-field
   option is the candidate fix if the 0700 boundary is judged insufficient.
4. **SCOPE deviation already logged in `59bd909`:** SCOPE §Security points at
   `starter_ai::secret` type-level wrappers; report-md masks rendered text by
   pattern because no string is a known-secret at the type level at this layer.
   Reuse of the wrapper applies where `report-grafana` surfaces variables. Carry
   forward to ratify.

# ncrawler — REPORTS-UPDATE (staged Grafana reporting)

A focused scope to replace the single, monolithic `scrape grafana --mode both`
+ `build report-grafana` flow with a **three-stage pipeline** whose stages are
independently re-runnable. Extends [SCOPE.md](SCOPE.md); does not supersede it.

---

## Why

The current flow has two concrete defects, both observed on
`rd-esr.nube-iiot.com` (Grafana 7.5.17, Postgres/TimescaleDB backend):

1. **Returned data is captured but never shown.** `mode=both` *does* store
   real query results — e.g. panel `290` (Water MTD) returns
   `total_mtd_usage = 97.192` — but `report-grafana` only renders template
   SQL, executed SQL, and error frames. The actual result rows/values appear
   nowhere, so a populated dashboard reads as "no data" in the report. This
   is the #1 user-visible bug.

2. **Monolithic, all-or-nothing, slow.** A single `both` run does dashboard
   fetch + 35 sequential `/api/ds/query` calls + a Chrome screenshot in one
   process (~4.5 min, dominated by a handful of slow hypertable queries that
   burn the full `--query-timeout` each). If anything fails or a few panels
   time out, the whole thing is re-run from zero. There is no way to re-fetch
   just the slow panels, or to rebuild the report without re-querying.

The fix is to split the work into stages joined by on-disk state, so each
stage is cheap to re-run in isolation.

---

## Goals

1. **Stage 1 — Audit.** Fetch dashboard structure once: panels, types,
   datasources, variables (resolved), and the *template* + *interpolated*
   SQL per target. No data queries. Fast, deterministic, offline-replayable.
2. **Stage 2 — Data.** Execute panel queries against the resolved plan from
   Stage 1, writing one result-per-panel. Per-panel timeout, per-panel
   retry, and selective re-run (`--panel`, `--only-missing`, `--only-failed`).
3. **Stage 3 — Report.** Build the Markdown report from Stage 1 + Stage 2
   on disk. No network. Must render **returned values**, not just SQL.
4. **Independent re-runs.** Re-running any stage must not require re-running
   the earlier ones, and must not clobber unrelated prior output.
5. **Backwards compatible.** The existing `scrape grafana --mode …` and
   `build report-grafana` paths keep working; staging is additive.

## Non-goals (this scope)

- No scheduler/daemon/web UI (per [SCOPE.md](SCOPE.md) non-goals).
- No query parallelism in v1 — stages make re-runs cheap; concurrency is a
  later optimisation (noted under *Follow-ups*).
- No write-back to Grafana.
- No change to the `Artifact` on-disk contract for other sources; staging
  state lives *inside* the existing grafana artifact directory.

---

## Stage model

```
                 stage 1: audit                stage 2: data                stage 3: report
  Grafana ──▶  dashboard + plan   ──▶   per-panel query results   ──▶   REPORT-out.md
   (API)        audit.json                 data/panel-<id>.json            (+ screenshot)
                (+ assets/ shot)           data-status.json
```

All three stages operate on **one artifact directory**:

```
artifacts/<ts>__grafana__<uid>/
├── artifact.json            # existing top-level artifact (source of truth for items/meta)
├── audit.json               # STAGE 1 output: plan + structure (no data)
├── data/
│   ├── panel-259.json       # STAGE 2 output: one ds/query response per panel
│   ├── panel-290.json
│   └── …
├── data-status.json         # STAGE 2 index: per-panel ok/empty/error/timeout + timing
├── assets/
│   └── dashboard.png        # screenshot (whichever stage captured it)
└── REPORT-out.md            # STAGE 3 output
```

Rationale: splitting `data/` into one file per panel is what makes
selective re-run trivial — Stage 2 can overwrite a single `panel-<id>.json`
without touching the rest, and Stage 3 just reads whatever is present.

---

## Stage 1 — Audit (`scrape grafana --stage audit`)

**Input:** `--url`, `--uid`, `--from/--to`, auth token.
**Does:** `GET /api/dashboards/uid/<uid>`, `GET /api/datasources`, optional
`GET /api/search` + `/api/annotations`. Resolves variables and datasource
ids exactly as `api.rs` does today, then emits a **query plan** without
executing any `/api/ds/query`.

**Writes `audit.json`:**

```jsonc
{
  "dashboard_uid": "vijaYkWvz",
  "title": "1 Site Summary - Warehouse A",
  "folder": "VIC - Lot 5 - Cranbourne West",
  "version": 16,
  "from": "now-1h", "to": "now",
  "variables": [ { "name": "buildingRef", "value": "WH-A" }, … ],
  "datasources": [ { "id": 1, "uid": "6K9hER7Sk", "name": "PostgreSQL", "default": true }, … ],
  "panels": [
    {
      "id": 290, "title": "Water - MTD Usage", "type": "stat",
      "queryable": true,
      "datasource_id": 1,
      "targets": [
        { "refId": "A",
          "template_sql": "WITH meta_tags AS ( … ${buildingRef} … )",
          "interpolated_sql": "WITH meta_tags AS ( … 'WH-A' … )",
          "query_body": { … exact /api/ds/query payload Stage 2 will POST … } }
      ]
    },
    { "id": 304, "type": "row", "queryable": false }, …
  ]
}
```

Key point: Stage 1 produces the **exact `query_body`** Stage 2 will send, so
Stage 2 is a dumb executor and the plan is auditable/diffable before any
expensive query runs.

- Fast (no data queries) and safe to re-run anytime (e.g. after a dashboard
  edit). Overwrites `audit.json` only.
- Screenshot is optional here (`--with-shot`) for an early visual.

## Stage 2 — Data (`scrape grafana --stage data`)

**Input:** the artifact dir (reads `audit.json`).
**Does:** for each `queryable` panel target, POST its `query_body` to
`/api/ds/query`, bounded by `--query-timeout` (default 30s, `0`=off — the
mechanism already added to `api.rs`). Writes one `data/panel-<id>.json` per
panel and updates `data-status.json`.

**Selective execution flags:**

| Flag | Effect |
|---|---|
| `--panel <id>` (repeatable) | Only (re)run these panels. |
| `--only-missing` | Skip panels that already have a `data/panel-<id>.json`. |
| `--only-failed` | Re-run only panels whose last status was `error`/`timeout`. |
| `--query-timeout <secs>` | Per-panel cap (existing). |

**`data-status.json`:**

```jsonc
{
  "ran_at": "2026-05-30T00:00:00Z",
  "panels": {
    "290": { "status": "ok",      "rows": 1, "ms": 850 },
    "326": { "status": "empty",   "rows": 0, "ms": 1200 },
    "308": { "status": "timeout", "ms": 30000 },
    "257": { "status": "error",   "error": "relation \"metric_table\" does not exist" }
  }
}
```

Status vocabulary (single source of truth, shared with Stage 3):
`ok` (≥1 row), `empty` (query succeeded, 0 rows), `error` (datasource
error frame), `timeout` (exceeded `--query-timeout`), `skipped`
(non-queryable / hidden targets).

This is the loop the user re-runs: `--only-failed` after a backend hiccup,
or `--panel 290 --query-timeout 60` to chase one slow panel, without
re-querying the other 34.

## Stage 3 — Report (`build report-grafana`, stage-aware)

**Input:** the artifact dir. Reads `audit.json` + `data/*.json` +
`data-status.json` + `assets/`. **No network.**

Must fix the rendering gap. In addition to today's template/executed SQL,
each panel detail block renders **the returned data**:

- **Stat/table panels** → a small Markdown table of returned columns/rows
  (e.g. `Total Month-To-Date Usage | 97.192`). Truncate to N rows with a
  `… M more rows` footer.
- **Time-series panels** → a one-line summary: series count, point count,
  first/last timestamp, min/max/last value per series (full frames stay in
  `data/panel-<id>.json`, not inlined).
- **Summary table "Query status" column** → driven by `data-status.json`
  (`ok`/`empty`/`error`/`timeout`/`skipped`), so a panel that returned
  `97.192` reads `ok`, never `no data`. The literal string "no data" is
  reserved for genuine `empty` results and is never the default fallback.
- Error/timeout panels show the reason inline (already partially done).

Re-runnable freely: editing the builder and rebuilding never touches the
data on disk.

---

## CLI surface

```bash
# Stage 1 — structure + plan, fast, no data
ncrawler scrape grafana --url … --uid vijaYkWvz --from now-1h --to now \
    --stage audit [--with-shot]

# Stage 2 — execute the plan (re-runnable subsets)
ncrawler scrape grafana --uid vijaYkWvz --stage data --query-timeout 30
ncrawler scrape grafana --uid vijaYkWvz --stage data --only-failed
ncrawler scrape grafana --uid vijaYkWvz --stage data --panel 290 --query-timeout 60

# Stage 3 — build report from disk
ncrawler build report-grafana ./artifacts/…__grafana__vijaYkWvz

# Convenience: run all three (today's one-shot behaviour, staged underneath)
ncrawler scrape grafana --url … --uid vijaYkWvz --stage all --visual-fallback chrome
```

`--stage {audit|data|report|all}`; default `all` preserves current UX.
Stage 2/3 locate the artifact dir from `--uid` (newest matching) or an
explicit `--artifact <dir>`.

---

## Acceptance criteria

1. `--stage audit` writes `audit.json` with resolved variables, datasource
   ids, and an interpolated `query_body` per target; runs with **zero**
   `/api/ds/query` calls.
2. `--stage data` executes only what the flags select; re-running
   `--only-failed` re-queries solely the previously failed/timed-out panels
   and leaves the rest of `data/` byte-identical.
3. `data-status.json` distinguishes `ok` / `empty` / `error` / `timeout`.
4. The report renders the returned value for panel `290`
   (`97.192`) and marks its status `ok`; a genuinely empty graph panel
   shows `empty`, and a timed-out panel shows `timeout` with the cap.
5. Re-running `build report-grafana` performs no network I/O.
6. The legacy `--mode api|visual|both` paths still function unchanged.

---

## Implementation sketch (non-binding)

- `crates/scrapers/ncrawler-grafana/src/`: add `stage.rs` with
  `Stage { Audit, Data, Report, All }`; factor today's `api::scrape`
  query loop into `audit::plan()` (builds bodies) + `data::execute()`
  (runs them). `merge.rs`/`visual.rs` unchanged; screenshot becomes an
  opt-in step callable from any stage.
- New small types `audit.rs` (`Audit`, `PanelPlan`, `TargetPlan`) and
  `status.rs` (`PanelStatus`) serialised with serde.
- `crates/ncrawler-cli/src/main.rs`: parse `--stage`, `--only-missing`,
  `--only-failed`, `--with-shot`, `--artifact`; dispatch per stage.
- `crates/builders/ncrawler-report-grafana/src/lib.rs`: read staged files
  when present (fall back to `artifact.json` items for legacy artifacts);
  add `render_returned_data(panel, data)` and switch `query_status` to read
  `data-status.json`.

## Follow-ups (out of this scope)

- Bounded-concurrency Stage 2 (e.g. N panels in flight) once staging lands.
- `report-ai` consuming `audit.json` + `data-status.json` to triage which
  panels are broken vs. genuinely empty.
- Promote the stage-runner pattern to `starter` if a second scraper needs it.

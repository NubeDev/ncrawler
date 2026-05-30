# ncrawler — Grafana Report Builder (design)

Status: **proposal (rev 2 — peer review incorporated)** · grounded against
the live `grafana.example.com` instance (Grafana 7.5.17 OSS).

The goal: turn scraped Grafana artifacts into a human-readable **report**
about a Grafana instance — an overview, its structure/navigation, and a
per-page breakdown — at a chosen depth (overview / full / audit).

> **rev 2 changes (from review):** split *template SQL* (deterministic,
> always present) from *executed SQL* (response-only, time-dependent,
> SQL-datasource-only); promote instance meta to a **sidecar artifact**
> instead of duplicating it per dashboard; clarify `--all` means different
> things at scrape vs report time; add a **redaction** story; commit to
> **stable ordering** for deterministic output; fingerprint-based
> duplicate detection. See §9 for the review log.

---

## 1. Concept

A new deterministic, offline builder **`report-grafana`** (same family as
`report-md`), rendered over one or more scraped artifacts. Two-phase stays
the same as the rest of ncrawler:

```
scrape  →  artifact.json (+ _instance sidecar)   →  build report-grafana  →  REPORT.md
```

The report is a pure renderer over data already on disk. Nothing in the
report needs a live Grafana at build time.

---

## 2. Selection — "ALL, or these pages by name / uuid"

One selector surface, **shared by both the scrape and the report**:

| flag | meaning |
|------|---------|
| `--all` | every dashboard (see semantics note below) |
| `--uid a,b,c` | explicit UIDs (comma list, repeatable) |
| `--name "site summary"` | title substring match (case-insensitive) |
| `--folder <name>` | restrict to a folder |
| `--tag <t>` | restrict to a tag |
| `--limit N` | cap the count (safety for `--all`; this instance has 1000+) |

> **`--all` is not the same in both phases (review #4).**
> At **scrape** time `--all` queries `/api/search` live (the true 1000+).
> At **report** time `--all` can only mean "every artifact on disk for
> this instance" — i.e. whatever was actually scraped. A `report --all`
> after a `scrape --all --limit 50` reports **50**, not 1000+. The report
> records both numbers in the header and `log()`s a warning when on-disk
> coverage is narrower than the instance inventory.

Examples:

```bash
# scrape
ncrawler scrape grafana --url https://grafana.example.com --all --limit 50
ncrawler scrape grafana --url https://grafana.example.com --uid vijaYkWvz,Iw0GqiJSk
ncrawler scrape grafana --url https://grafana.example.com --name "Warehouse"

# report
ncrawler build report-grafana --all --mode overview
ncrawler build report-grafana --uid vijaYkWvz --mode full --data --window now-30d
```

---

## 3. Depth — "full report or an overview"

`--mode overview | full | audit` (default `overview`). `--data` is a
separate switch; `--redact/--no-redact` controls secret masking (§7).

| section | overview | full | audit |
|---|:--:|:--:|:--:|
| Instance facts (url, version, db types) | ✅ | ✅ | ✅ (header) |
| Structure / nav (folders, tags, counts) | ✅ | ✅ | — |
| Per-page panel list (title, type, datasource) | ✅ | ✅ | — |
| Per-panel **template SQL** (raw, from dashboard JSON) | — | ✅ | — |
| Per-panel **variables** (redacted by default) | — | ✅ | — |
| Per-panel **executed SQL** (interpolated) | — | ✅ *(needs `--data`; SQL datasources only)* | — |
| Per-panel **sample returned data** | — | ✅ *(needs `--data`)* | — |
| Issues / findings | — | — | ✅ |

**Two kinds of SQL — do not conflate (review #1, #2):**

- **Template SQL** — the panel target's raw `rawSql` from
  `meta.dashboard`. Always present (no query needed), **deterministic**
  (no `${__from}` resolution), still contains `$variables`. This is the
  default `full`-mode SQL.
- **Executed SQL** — `results.<refId>.meta.executedQueryString` from the
  query **response**. Only exists if a query ran (`--data`), is
  **time-dependent** (`${__from}`/`${__to}` resolved to epoch-ms off
  `now` at scrape time, so it moves every scrape), and is
  **datasource-specific** — emitted by SQL backends (postgres/mysql),
  **not** by the Rubix OS plugin. The report labels it "executed SQL
  (where the datasource exposes it)" and omits it cleanly where absent.

> Optional: to get interpolated SQL *without* a live data dependency at
> render time, persist the interpolated **request body** as a per-item
> field at scrape time (see §8). That buys *build*-determinism (re-rendering
> one artifact is stable) but not *scrape*-determinism (timestamps still
> move run-to-run, correctly).

- `--window now-30d..now` (default **`now-6h`**, matching the scraper):
  time range used when `--data` is on. Many panels (MTD/YTD) compute their
  own window in SQL via `date_trunc(...)`, so they return totals
  regardless of window.

---

## 4. Report structure (the three sections)

Grounded in real values pulled from `example` (example shown with
`--mode full --data --window now-30d`; default window is `now-6h`):

```markdown
# Grafana Report — grafana.example.com
generated 2026-05-29 · mode: full · data: on (now-30d..now) · redacted
scope: --uid vijaYkWvz  (1 scraped of 1000+ in instance inventory)

## 1. Overview
- URL:          https://grafana.example.com
- Version:      7.5.17  (OSS, production)
- Datasources:  PostgreSQL (postgres, default) · Rubix OS (grafana-rubix-os-data-source)
- Dashboards:   1000+ inventory / N scraped       Folders: M
- Renderer plugin: not installed  (visual/screenshot mode unavailable; chrome fallback only)

## 2. Structure / Navigation
- Folder tree (sidebar), with dashboard counts per folder
- "0. Navigation" dashboard — its dashlist/link map
- Tag index
- Panel-type distribution (graph, stat, table, text, row, …)
- Datasource usage (which dashboards/panels hit which datasource)

## 3. Pages  (one block per selected dashboard)
### Site Summary — Warehouse A   (uid vijaYkWvz · folder: …)
- 35 panels · 11 variables · datasource: PostgreSQL
- Variables: buildingRef=WH-A, site=Lot 5 Cranbourne West, hostUUID=***  [full, redacted]
- Panels (sorted by panel id for stable diffs):
  | panel | type | datasource | value (--data) |
  |-------|------|-----------|----------------|
  | Electrical - MTD Usage | graph | PostgreSQL | 1762.1 kWh |
  | Water - MTD Usage      | graph | PostgreSQL | 91.732 kL  |
  | Electrical YTD Usage   | graph | PostgreSQL | 8765.72    |
  | Water - Variation MoM  | graph | PostgreSQL | -51.5%     |
  | …                      |       |           |            |
  ▸ [full]        template SQL per panel (raw rawSql, with $variables)
  ▸ [full+data]   executed SQL per panel (where the datasource exposes it)
  ▸ [full+data]   sample returned rows
```

---

## 5. Audit mode (future `--mode audit`)

**Audit is offline (review note).** Api-mode *always* queries and stores
the response — including **error frames** — so audit reads failures
straight from the frozen artifact; it does not need a fresh `--data` run.
That's why the §3 matrix marks audit data-free.

Checks — several already visible in this instance's stored data:

- **Dead datasource references** — panels pointing at a datasource that no
  longer exists (e.g. `InfluxDB - Fujitsu` on the Navigation dashboard).
- **Broken queries** — stored error frames
  (`relation "metric_table" does not exist`).
- **Empty panels** — queries that ran but returned 0 rows.
- **Duplicate dashboards** — keyed on a **blake3 fingerprint of the
  normalized panel/query set**, not title similarity (review). Catches
  `Building A - Summary v2/v3/v4` and `Site Summary ×N` properly, and
  dovetails with the repo's existing blake3-id discipline.
- **Orphans** — dashboards in no folder; untagged dashboards.
- **Unused datasources** — configured but referenced by no panel.
- **Blank/constant variables** — constants left empty (e.g. `host_uuid=''`
  seen in Warehouse A SQL), hardcoded values that should be variables.

Output: findings grouped by severity, each tagged with its
dashboard/panel, so it's actionable.

---

## 6. How it fits ncrawler (architecture)

### 6a. Scrape side — instance sidecar (review #3)
Per-dashboard artifacts today embed `meta.search` (the full 1000+
inventory) in **every** artifact — at `--all` scale that copies the whole
inventory 1000×. Fix: write instance-wide data **once** to a sidecar
artifact and have per-dashboard artifacts stop duplicating it.

```
# sidecar — one per (instance, run); `latest` -> newest timestamped dir
artifacts/grafana/_instance/<host>/<rfc3339-utc>__instance/instance.json   # search, datasources, instance, folders
artifacts/grafana/_instance/<host>/latest -> <rfc3339-utc>__instance
artifacts/grafana/<uid>/latest/artifact.json                               # this dashboard only (panels + data)
```

> **Stage-1 layout note (impl-accurate).** The sidecar is nested per
> `<host>` and timestamped like every other artifact dir, with the same
> `0700` + `latest`-symlink discipline as the main store (one store, not a
> fork): `_instance/<host>/<rfc3339-utc>__instance/instance.json`. The
> reader resolves `(host, uid) -> InstanceFacts` from
> `_instance/<host>/latest`, falling back to a legacy artifact's
> `meta.search` (with a load-time `tracing::warn`) only when no sidecar is
> present. `schema_version=1`.

Sidecar contents (some already fetched during scrape, just not persisted):
- `search` — dashboard inventory (titles, uids, folders, tags).
- `datasources` — db types / default (already fetched for query
  resolution; today discarded — persist it).
- `instance` — version + edition from `/api/health` +
  `/api/frontend/settings`.
- `folders` — `/api/folders` for the sidebar/nav tree.

Per-dashboard `meta` keeps only `dashboard` + `annotations` (drop the
embedded `search`). Migration note: existing artifacts still carry
`meta.search`; the reader falls back to it when no sidecar is present.

### 6b. Build side — the renderer
`report-grafana` reads the **store**: the `_instance` sidecar for
overview/nav + the selected `grafana/<uid>/latest` artifacts for pages.

- **Stable ordering (review).** `/api/search` and `panels[]` order are not
  guaranteed stable, so the renderer sorts deterministically: pages by
  (folder, title, uid), panels by panel id, tags/folders lexicographically.
  Required for a renderer whose output is diffed run-to-run.
- Needs a small extension to the builder entry point so a report can
  consume *multiple* artifacts + the sidecar, not just one `artifact_dir`
  (today's `report-md` is single-artifact).

---

## 7. Redaction & secrets (review #5)

Variables and SQL routinely carry tenant identifiers, host UUIDs, and
sometimes credentials. The report writes these to `REPORT.md` on disk and
is the artifact most likely to be shared, so redaction is **on by
default**:

- `--redact` (default) — mask variable values and literals in SQL that
  match secret-ish patterns (tokens, long hex/uuids, `password`/`secret`/
  `key` assignments). Show variable *names* and structure, mask values.
- `--no-redact` — opt out explicitly for a fully-detailed internal report.

Note the underlying exposure predates the report: the **scrape already**
persists `rawSql`, variables, and returned data to `artifact.json` in
plaintext. So redaction is ideally a shared pass usable at the artifact
layer too, not bolted only onto the report. Reuse the codebase's existing
secret-handling discipline (token quarantine in scope) rather than a new
ad-hoc matcher.

---

## 8. Proposed implementation order

1. **Instance sidecar** — write `_instance/latest` (search, datasources,
   instance, folders) once; reader falls back to legacy `meta.search`.
   Unlocks the overview without N× duplication.
2. **Selector** — shared parser (`--all/--uid/--name/--folder/--tag/--limit`)
   for scrape and report, with the dual-`--all` semantics + divergence
   warning of §2.
3. **Multi-dashboard scrape** — loop the selector over `/api/search`.
4. **Redaction pass** — shared masker (§7), default-on.
5. **`report-grafana` builder** — overview + structure + pages;
   `--mode overview|full`, `--data`, `--window`; **template SQL** default,
   **executed SQL** under `--data`; stable ordering.
6. **Audit mode** — `--mode audit` reading frozen response/error frames;
   blake3 panel-fingerprint duplicate detection.
7. *(optional)* persist interpolated **request body** per item for
   build-deterministic interpolated SQL without `--data`.

Each step is independently shippable and testable (unit tests on the
renderer with fixture artifacts; live tests gated on `RUN_LIVE_TESTS=1`).

---

## 9. Open questions / notes

- **Screenshots**: visual/PNG panels need the renderer plugin, which is
  **not installed** on `rd-esr` (`rendererAvailable=false`). Only the
  best-effort `--visual-fallback chrome` path can produce images here. A
  future report mode could embed those PNGs per panel, but it depends on a
  local Chrome and is flaky — kept out of the deterministic report for now.
- **Scale**: `--all` is 1000+ dashboards; `--limit` and folder/tag filters
  keep a run bounded. The report `log()`s when coverage is truncated.
- **Duplicates**: this instance has heavy dashboard duplication; the audit
  and the overview inventory should surface that (via fingerprint) rather
  than drown in it.

### Review log (rev 1 → rev 2)
- #1 template vs executed SQL — **accepted**; split into two features (§3).
- #2 executedQueryString datasource-specific — **accepted** (§3).
- #3 instance-meta duplication — **accepted**; sidecar artifact (§6a).
- #4 `--all` dual meaning — **accepted**; semantics note + warning (§2).
- #5 redaction — **accepted**; default-on, shared pass (§7). Nuance:
  exposure already exists at the artifact layer (scrape persists plaintext).
- window default mismatch — **fixed** (default `now-6h`, example labeled).
- stable ordering — **accepted** (§6b).
- audit is offline — **accepted**; clarified it reads frozen frames (§5).
- duplicate detection by fingerprint — **accepted** (§5).
- persist request body — **accepted as optional** (§3 note, §8 step 7);
  buys build- not scrape-determinism.

# ncrawler — Usage

Two-phase toolkit: **scrape** a source into an on-disk artifact, then
**build** one or more derived reports / indexes from it.

```
scrape  →  artifacts/<ts>__<source>__<target>/artifact.json (+ assets/)
                                                │
                                                ▼
                                build  report-md      → report.md
                                build  report-grafana → REPORT-out.md
                                build  report-ai      → build-report-ai.md
                                build  vector         → LanceDB / Qdrant
```

Every scrape directory is also reachable via the stable symlink
`artifacts/<source>/<target>/latest` — point builders at that.

---

## Setup

Rust toolchain pinned in [rust-toolchain.toml](rust-toolchain.toml)
(1.92). Just `cargo build --workspace`.

For Grafana scrapes that need authentication, export a Grafana API token:

```bash
export GRAFANA_TOKEN='<paste-api-token-here>'
# or, if a SecretStore is wired, key it as `ncrawler:grafana:<host>:token`
```

> **Auth gap — basic-auth instances.** ncrawler currently only speaks
> Grafana **bearer tokens**, not HTTP basic auth. If your instance is
> behind `admin:password`-style basic auth, mint a short-lived API key
> first and export that as `GRAFANA_TOKEN`:
>
> ```bash
> curl -s -u admin:<password> \
>     -X POST -H 'Content-Type: application/json' \
>     https://grafana.example.com/api/auth/keys \
>     -d '{"name":"ncrawler","role":"Admin","secondsToLive":3600}'
> # → {"id":N,"name":"...","key":"<paste this as GRAFANA_TOKEN>"}
> ```
>
> Revoke when done:
>
> ```bash
> curl -s -u admin:<password> -X DELETE \
>     https://grafana.example.com/api/auth/keys/<id>
> ```
>
> Native basic-auth support is not yet implemented — see the open
> question in the report builder scope.

For chrome-based screenshots: a Chromium/Chrome binary on `PATH`
(`/usr/bin/google-chrome` works), or set `NCRAWLER_CHROME=/path/to/chrome`.

---

## `ncrawler scrape grafana`

```
ncrawler scrape grafana \
    --url   https://grafana.example.com \
    --uid   <dashboard-uid> \
    --mode  api | visual | both \
    [--from now-6h]  [--to now] \
    [--panel <id>]   [--panel <id>] ... \
    [--visual-fallback chrome] \
    [--visual-whole] \
    [--allow-host <host>] ... \
    [--out ./artifacts]
```

### Modes

| `--mode` | What it produces | Needs |
|---|---|---|
| `api` (default) | One `Item::Panel` per panel with the `/api/ds/query` response stored in `data`. Template SQL, executed SQL, returned series, error frames. **No images.** | API token. |
| `visual` | One PNG per panel. Items are panel metadata only (no query data). | The Grafana `grafana-image-renderer` plugin **or** `--visual-fallback chrome`. |
| `both` | API mode plus matching PNGs attached by `item_id`. | Renderer plugin or chrome fallback. |

### Visual fallback (no renderer plugin)

When the renderer plugin isn't installed (the default on most Grafana
instances), add `--visual-fallback chrome`. The scraper drives headless
Chromium against `/d-solo/<uid>/dashboard?panelId=<id>` with the
`Authorization: Bearer <token>` header set, capturing one PNG per panel:

```bash
ncrawler scrape grafana \
    --url https://grafana.example.com \
    --uid abc123 \
    --mode visual \
    --visual-fallback chrome
```

### Whole-dashboard screenshot

For a single "presentation" PNG of the entire dashboard instead of
per-panel tiles, add `--visual-whole`:

```bash
ncrawler scrape grafana \
    --url https://grafana.example.com \
    --uid abc123 \
    --mode visual \
    --visual-fallback chrome \
    --visual-whole
```

This grabs one `assets/dashboard.png` (1600×1200 by default, full page),
linked to the artifact with `item_id = None`. The report builders embed
it once at the top of the report.

#### Knobs

- `--from`, `--to` — Grafana time range. Default `now-6h..now`. Used by
  both the renderer plugin path and the chrome path.
- `--panel <id>` — restrict to specific panel IDs (repeatable). Empty =
  all panels.
- `--query-timeout <secs>` — per-panel `/api/ds/query` timeout for
  `api` / `both` modes. Default `30`; `0` disables it. A slow datasource
  (e.g. a large TimescaleDB hypertable) can otherwise hang a single
  panel query indefinitely and stall the whole dashboard sweep; on
  timeout the panel is kept as a metadata-only item and the scrape
  continues.
- `NCRAWLER_CHROME_WAIT_MS` — ms to wait after navigation before
  capturing on the whole-dashboard path. Default `8000`. Raise this if
  some panels are blank in the screenshot (slow queries / lazy panels).
- `NCRAWLER_CHROME=/path/to/chrome` — explicit Chrome binary.
- `NCRAWLER_CHROME_NO_SANDBOX=1` — disable the Chrome sandbox (logged
  WARN, only use in CI/containers).

---

## `ncrawler build`

```
ncrawler build <builder> <artifact_dir> [flags]
```

`<artifact_dir>` can be either the timestamped scrape directory or its
`latest` symlink.

### `report-md` — deterministic per-item Markdown

Cheap, no AI, no network. One `## heading` per item, JSON-fenced `data`
block, plus image embeds for any linked assets (by `item_id`).

```bash
ncrawler build report-md ./artifacts/grafana/<uid>/latest
# → writes report.md alongside artifact.json
```

### `report-grafana` — presentation-style report

The three-section layout from [REPORT.md](REPORT.md) §4: overview,
structure (folders / tags / panel-type counts), and per-page panel
tables. Embeds screenshots when assets are linked.

```bash
ncrawler build report-grafana ./artifacts/grafana/<uid>/latest
# → writes REPORT-out.md
```

Section 1 includes a whole-dashboard screenshot when the artifact has
one (asset with `item_id = None`). Section 3's "Panel details" embeds
the matching per-panel PNG above the type/datasource line. Template SQL
comes from the panel `targets[].rawSql`; executed SQL comes from
`data.results.<refId>.meta.executedQueryString` (only emitted by SQL
datasources — postgres/mysql yes, Rubix OS no, never invented).

**v0 limits (see REPORT.md / SCOPE.md):** single-artifact only, no
redaction pass, no audit mode, no shared selector. The on-disk
artifact is plaintext under 0700 perms; the report header carries a
warning.

### `report-ai` — Claude-driven triage

Picks a skill (`ncrawler.skills.*`) by content match and runs it
against the artifact. Streams output and is cancellable mid-run via
Ctrl-C. Writes `build-report-ai.md` plus a structured
`build-report-ai.jsonl` of the run.

```bash
ncrawler build report-ai ./artifacts/grafana/<uid>/latest \
    [--model claude-sonnet-4-6]
```

Skills are loaded from `./skills` by default; override with
`NCRAWLER_SKILLS_DIR=/some/path`.

### `vector` — embeddings index

Chunks each item, embeds with `fastembed-rs` (ONNX, 384-dim, no
network), and upserts into a vector store keyed by
`(source, target, item_id)` so re-scrapes overwrite rather than
duplicate.

```bash
ncrawler build vector ./artifacts/grafana/<uid>/latest \
    [--store lance://./vec]          # default: LanceDB on disk
# build with `--features store-qdrant` and use:
#   --store qdrant://host:6334
```

LanceDB is the default (in-process, no daemon). Qdrant is opt-in
because it needs a running server:

```bash
cargo build --workspace --features store-qdrant
```

---

## `ncrawler ls` / `ncrawler show`

```bash
ncrawler ls                              # all scraped artifacts
ncrawler ls --source grafana             # filter by source
ncrawler ls --since 24h                  # only recent ones
ncrawler show ./artifacts/grafana/<uid>/latest
```

`show` prints a one-line-per-item summary of the artifact without
running a builder.

---

## End-to-end example

Capture a Grafana dashboard with both data + a whole-dashboard
screenshot, then render a presentation report and an AI triage:

```bash
export GRAFANA_TOKEN=...

# 1. Scrape — API data + chrome whole-dashboard PNG.
ncrawler scrape grafana \
    --url https://grafana.example.com \
    --uid abc123 \
    --mode both \
    --visual-fallback chrome \
    --visual-whole

# 2. Presentation report.
ncrawler build report-grafana ./artifacts/grafana/abc123/latest

# 3. AI triage.
ncrawler build report-ai     ./artifacts/grafana/abc123/latest

# 4. Vector index for later semantic search.
ncrawler build vector        ./artifacts/grafana/abc123/latest
```

Outputs land alongside `artifact.json`:

```
artifacts/grafana/abc123/latest/
├── artifact.json
├── assets/
│   └── dashboard.png          # whole-dashboard chrome capture
├── report.md                  # report-md
├── REPORT-out.md              # report-grafana (with embedded PNG)
├── build-report-ai.md         # report-ai narrative
└── build-report-ai.jsonl      # structured AI run log
```

---

## Caveats

- **Per-panel chrome screenshots** wait a fixed 3.5 s per panel before
  capturing; whole-dashboard waits `NCRAWLER_CHROME_WAIT_MS` (default
  8 s) once. Grafana 7 exposes no "all queries finished" signal, so
  slow panels may appear blank in the PNG. Bump the wait if needed.
- **chromiumoxide WS deserialization errors** in the logs
  (`data did not match any variant of untagged enum Message`) are
  spurious CDP-event noise from chromium 142 — they do **not** mean a
  failed capture; the PNG still writes.
- **Parallel chrome scrapes** each get their own
  `/tmp/ncrawler-chrome-<pid>-<ns>/` profile dir to avoid the
  `SingletonLock` collision. They're cleaned by tmpfs, but
  `rm -rf /tmp/ncrawler-chrome-*` is safe between runs.
- **Plaintext on disk.** The current builders do **not** run the
  default-on redaction pass scoped in
  [.codeless/jobs/ncrawler-report-grafana/SCOPE.md](.codeless/jobs/ncrawler-report-grafana/SCOPE.md)
  (Stage 4). Variables, hostnames, and SQL literals are written as
  scraped. Treat artifacts as sensitive.

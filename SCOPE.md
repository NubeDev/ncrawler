# ncrawler — SCOPE

A modular, two-phase data-collection toolkit: **scrape** structured + visual
snapshots from observability surfaces (Grafana first, more later), then
**build** derived artifacts (Markdown reports, AI summaries, vector
embeddings) from those snapshots. Scrape and build are decoupled phases
joined by an on-disk `Artifact` format, so a single scrape can feed many
builders and runs are replayable without re-hitting the source.

Repository: `/home/user/code/rust/ncrawler` (standalone Cargo workspace,
remote `github.com/NubeDev/ncrawler`).
Sibling repo `/home/user/code/rust/starter` is consumed as a path dependency
for cross-cutting concerns (AI runners, skills, secrets, observability).
Anything we build here that turns out to be domain-agnostic gets promoted
into `starter` as a new `starter-*` crate (see *Promotion candidates*).

---

## Goals

1. Scrape Grafana dashboards via both the HTTP API (authoritative data)
   and rendered visuals (panel PNGs), merged into one artifact.
2. Scrape arbitrary web pages (status pages, wikis, Kibana share links)
   via `spider` for HTTP crawling, with our own headless-Chrome layer
   for JS-rendered pages.
3. Run an AI builder over any artifact using the existing
   `starter-ai::ClaudeRunner` (CLI-backed, no API key needed).
4. Keep the source/builder seams narrow enough that adding Prometheus,
   Loki, Datadog, or PagerDuty as new scrapers is a single crate with
   no changes to core.
5. Prefer open-source dependencies. Managed services (Spider Cloud,
   hosted vector DBs) stay optional and feature-gated.
6. No long-lived processes in the default path. The vector phase must
   work against a directory on disk, not a server.

## Non-goals (this scope)

- No long-lived daemon, scheduler, or web UI. Triggering is out of band
  (cron, CI, `starter-cron` later if needed).
- No real-time alerting or incident routing — `ncrawler` produces
  artifacts; downstream systems decide what to do with them.
- No write-back to Grafana (no dashboard mutation, no annotation
  creation in v1).
- No multi-tenant auth model; single operator, secrets from env or
  `starter-secrets-file` / `starter-secrets-keyring`.
- No Spider Cloud integration in v1 (self-hosted only).
- No OTLP exporter in v1. `starter-observability` ships
  tracing-subscriber + Prometheus + tokio-console only; OTLP is a
  follow-up scope, not a feature flag we pretend exists today.

---

## Architecture

```
┌─────────────┐   Artifact (JSON + assets/)   ┌─────────────┐
│   Scraper   │ ────────────────────────────▶ │   Builder   │
│ (grafana,   │     on-disk, versioned         │ (report-md, │
│  spider,…)  │                                │  report-ai, │
└─────────────┘                                │  vector,…)  │
```

Two distinct phases, never combined in a single call. The artifact
directory is the contract.

### Workspace layout

```
ncrawler/
├── Cargo.toml                       # virtual workspace
├── SCOPE.md
├── skills/                          # SKILL.md bundles loaded by starter-skills
│   └── grafana-triage/
├── crates/
│   ├── ncrawler-spi/                # traits + Artifact types only, no impl deps
│   ├── ncrawler-core/               # registry, artifact store (fs), job runner
│   ├── ncrawler-cli/                # `ncrawler scrape …` / `ncrawler build …`
│   ├── scrapers/
│   │   ├── ncrawler-grafana/        # client.rs / api.rs / visual.rs / merge.rs
│   │   └── ncrawler-spider/         # HTTP-only spider wrapper + own chrome layer
│   └── builders/
│       ├── ncrawler-report-md/      # deterministic, no AI
│       ├── ncrawler-report-ai/      # uses starter-ai + starter-skills
│       └── ncrawler-vector/         # chunk + embed, pluggable store
└── examples/                        # one-shot scripts per scraper/builder
```

File-size discipline: every crate follows starter's HOW-TO-CODE.md
budget (≤ 400 lines per file, ≤ 50 lines per function). The Grafana
crate is pre-split (`client.rs` shared HTTP + auth, `api.rs` mode-Api,
`visual.rs` mode-Visual, `merge.rs` mode-Both) so no single file
grows unbounded.

### `ncrawler-spi` — the contract

```rust
pub const ARTIFACT_SCHEMA_VERSION: u32 = 1;

pub struct Artifact {
    pub schema_version: u32,
    pub source: String,                 // "grafana" | "spider" | ...
    pub target: String,                 // uid, URL, query, …
    pub fetched_at: DateTime<Utc>,
    pub items: Vec<Item>,
    pub assets: Vec<Asset>,             // paths relative to artifact dir
    pub meta: serde_json::Value,        // source-specific, best-effort
}

pub struct Item {
    pub id: String,                     // STABLE across re-scrapes (see below)
    pub kind: ItemKind,
    pub title: Option<String>,
    pub text: String,                   // human-readable rendering
    pub data: Option<serde_json::Value>,// structured payload
    pub tags: Vec<String>,
}
pub enum ItemKind { Panel, Page, ApiResponse, Annotation, Alert, Log }

pub struct Asset {
    pub path: PathBuf,
    pub mime: String,
    pub label: String,
    pub item_id: Option<String>,        // links to Item.id when applicable
}

#[async_trait] pub trait Scraper: Send + Sync {
    fn name(&self) -> &str;
    async fn scrape(&self, job: ScrapeJob, cancel: &dyn Cancel)
        -> Result<Artifact, ScrapeError>;
}

#[async_trait] pub trait Builder: Send + Sync {
    fn name(&self) -> &str;
    async fn build(&self, artifact: &Artifact, ctx: &BuildCtx, cancel: &dyn Cancel)
        -> Result<BuildOutput, BuildError>;
}
```

`Cancel` is re-exported from `starter_spi::ai::Cancel` so cancellation
composes with the existing `ClaudeRunner`.

#### Versioning rules

- `schema_version` is bumped on any breaking change to the typed
  fields of `Artifact` / `Item` / `Asset`. Readers reject unknown
  majors.
- `meta` is intentionally untyped and **best-effort**. Builders MUST
  NOT depend on `meta` keys for correctness; treat missing/changed
  keys as "feature off", never as an error. Anything a builder needs
  to function lives in the typed fields above, and gets a version
  bump when it changes.

#### Item ID stability (per source)

`Item.id` MUST be deterministic for the same logical thing across
re-scrapes of the same target. This is what makes diffing artifacts
in PRs and re-embedding into vector stores work without duplicating
every nightly run.

| Source    | `Item.id` formula                                              |
| --------- | -------------------------------------------------------------- |
| `grafana` | `panel-{panelId}` (Grafana's own panel id is stable per dash)  |
| `spider`  | `page-{blake3(normalised_url)[..16]}` (lowercase host, sorted query, no fragment) |

New sources MUST define their own stability rule before merging.

#### Asset ↔ Item linkage

`Asset.item_id = Some(Item.id)` when an asset belongs to a specific
item (e.g. a panel screenshot). `None` only for whole-artifact
assets (e.g. full-page screenshot, raw dashboard JSON dump).
Builders merge by `item_id`; string-matching on `label` is forbidden.

### On-disk artifact layout

```
artifacts/
├── grafana/
│   └── abc123/
│       └── latest -> ../../2026-05-29T14-22-01Z__grafana__abc123
└── 2026-05-29T14-22-01Z__grafana__abc123/
    ├── artifact.json                   # serde_json::to_writer_pretty(Artifact)
    └── assets/
        ├── panel-2.png
        ├── panel-5.png
        └── raw-dashboard.json
```

- Top-level dirname is the index: `<rfc3339-utc>__<source>__<safe(target)>`.
  No separate manifest; `ncrawler ls --since 24h` parses dirnames.
- A per-`(source, target)` `latest` symlink is rewritten after every
  successful scrape so `ncrawler build … grafana/abc123/latest`
  always points at the most recent run.
- Retention is operator policy, not enforced: documented default is
  "keep last 30 days per target, delete the rest". No automatic
  cleanup in v1.

---

## Scrapers (v1)

### `ncrawler-grafana`

Single crate, three modes; one shared `client.rs` for auth + base URL.

The `/api/...` surface is reached via the open-source [`grafana`](https://docs.rs/grafana)
crate (v0.1.3, MIT, async + `reqwest` + rustls — same stack we're already
on). It ships hand-written wrappers (`client.dashboards()`,
`client.folders()`, …) plus a generated OpenAPI layer
(`client.openapi()`) plus a `client.raw()` escape hatch for anything
uncovered.

Isolation: the dependency is pinned exactly (`grafana = "=0.1.3"`,
single maintainer, low download count, v0.1.x churn risk) and lives
only in `client.rs`. `api.rs`, `visual.rs`, and `merge.rs` see the
crate solely through a `GrafanaClient` wrapper trait we define
ourselves, so replacing it with hand-rolled `reqwest` later is a
one-file change.

The renderer plugin endpoint (`/render/d-solo/...`) sits **outside**
`/api/`, so `client.rs` also keeps a plain `reqwest::Client` for the
visual path. The `POST /api/ds/query` panel-query call uses the
generated wrapper by default and falls back to `client.raw()` if a
datasource-specific payload needs hand-shaping.

| Mode     | Transport                              | Produces                                  |
| -------- | -------------------------------------- | ----------------------------------------- |
| `Api`    | `reqwest`                              | `Item::Panel` per panel with `data` from `/api/ds/query`; dashboard JSON in `meta` |
| `Visual` | `/render/d-solo/...` (renderer plugin) | `Asset` PNG per panel, `item_id` linked   |
| `Both`   | API for data + renderer for pixels      | merged artifact, one `Item` per panel with a matching `Asset` |

API endpoints used (via the `grafana` crate unless noted):

- `GET /api/dashboards/uid/{uid}` — `client.dashboards()`
- `POST /api/ds/query` (panel queries) — generated wrapper, `client.raw()` fallback
- `GET /api/search` (discovery) — `client.openapi()`
- `GET /api/annotations` — `client.openapi()`
- `GET /render/d-solo/{uid}/...?panelId=N&width=…&from=…&to=…` — hand-rolled `reqwest` (outside `/api/`)

**Visual strategy — important.** The primary visual path is the
`grafana-image-renderer` plugin (`/render/d-solo/...`). When the
plugin is present, no local browser is needed and screenshots are
authoritative. When absent, `Visual` mode returns an explicit
`ScrapeError::RendererPluginMissing`; the operator can opt into a
best-effort fallback (`--visual-fallback chrome`) that drives our
own `chromiumoxide` layer at the dashboard URL, but that path is
documented as flaky (auth-token-in-Chrome vs. Bearer-on-API, template
variables, lazy-loaded panels, no reliable "all queries finished"
signal). Default: plugin-only.

Auth: `Authorization: Bearer <service-account-token>`, sourced via
`starter-secrets-file` / `starter-secrets-keyring` keyed
`ncrawler:grafana:<host>:token`. Env fallback `GRAFANA_TOKEN`.
Tokens never logged.

### `ncrawler-spider`

Thin wrapper over the open-source `spider` crate, **HTTP-only**
(`default-features = false`, no `chrome` / `smart` features). Maps
each `spider::page::Page` to an `Item::Page` with `text` = readable
content via `dom_smoothie` (Mozilla-readability port) and an
optional Markdown rendering via `fast_html2md` (Cloudflare
`lol_html`-based).

JS-rendered pages go through our own `chromiumoxide` layer
(`scrapers/ncrawler-spider/src/chrome.rs`, promotion candidate
`starter-headless`) — never through `spider`'s vendored
`spider_chrome` fork. Rationale: depending on both upstream
`chromiumoxide` and `spider_chrome` doubles the most fragile part
of the build (two CDP impls, two browser launch paths, two Chrome
version-skew sources).

Per-job config (depth, concurrency, delay, stealth, robots.txt) is
forwarded to `spider::website::Website` builders.

> Note: `spider` is marked "minimal maintenance" upstream as of
> late 2025. HTTP-only usage keeps the surface small; if it stalls
> further we replace it with `reqwest` + `dom_smoothie` directly
> without touching the `Scraper` trait.

### Chrome binary discovery

Both `ncrawler-grafana` (fallback only) and `ncrawler-spider` need
a Chrome/Chromium binary when JS rendering kicks in. Resolution
order:

1. `NCRAWLER_CHROME` env (absolute path)
2. `which chromium` / `which chromium-browser` / `which google-chrome`
3. Platform-specific well-known paths (`/Applications/...` on macOS,
   `/usr/bin/...` on Linux)
4. Fail with a clear error naming all of the above

CI uses the docker-compose fixture under `tests/fixtures/chrome/`
to pin a known browser version. Sandbox: on by default;
`NCRAWLER_CHROME_NO_SANDBOX=1` opts out, logged at WARN.

---

## Builders (v1)

### `ncrawler-report-md`

Deterministic Markdown: title + per-item section with title, tags,
fenced JSON for `data`, image embeds for the `Asset`s whose
`item_id` matches (no label-matching). Zero network, zero AI —
useful for diffing artifacts in PRs and as the input the AI builder
summarises.

### `ncrawler-report-ai`

Pipeline:

1. Run the Markdown builder to get a single prompt-ready document.
2. Resolve a skill from `starter-skills::SkillRegistry` using the
   artifact's `source` + `tags` (e.g. `grafana-triage`,
   `status-page-summary`). Skills live in `skills/` at the repo
   root, are content-hash quarantined (blake3), and supply the
   system prompt + resource files.
3. Hand off to `starter_ai::Registry::with_defaults()` →
   `Provider::Claude` → `ClaudeRunner::run()`. Stream `Event`s to
   stdout / a log file.
4. Persist the assistant's text plus the per-tool-call log alongside
   the artifact as `build-report-ai.md` + `build-report-ai.jsonl`.

Other providers (Codex, OpenAI, Anthropic REST) are reachable by
changing one enum — `starter-ai` already gates them behind features.
v1 enables only `provider-claude`.

### `ncrawler-vector` (stretch, not v1-blocking)

Chunk artifact items, embed via a pluggable `Embedder` trait, write
to a pluggable `VectorStore` trait. v1 ships:

- `Embedder`: local OSS model via `fastembed-rs` (Apache-2.0, ONNX,
  no network).
- `VectorStore` (default): **LanceDB** — in-process, single
  directory on disk, Apache-2.0. Keeps the vector phase
  artifact-local and daemon-free, consistent with the
  no-long-lived-process goal.
- `VectorStore` (optional, feature `store-qdrant`): `qdrant-client`
  for users who already run Qdrant. Not the default precisely
  because it requires a server.

Upserts key on `(source, target, item_id)` — possible because item
IDs are stable across re-scrapes. Re-embedding the same panel
overwrites rather than duplicating.

---

## CLI surface (`ncrawler-cli`)

```
ncrawler scrape grafana
    --url https://grafana.example
    --uid abc123
    [--mode api|visual|both]              # default: both
    [--visual-fallback chrome]            # opt-in to flaky path
    [--from <rfc3339>] [--to <rfc3339>]
    [--out ./artifacts]                   # default: ./artifacts
    [--panel <id>] ...                    # repeatable; default: all
    [--allow-host <pattern>] ...          # SSRF guard, repeatable

ncrawler scrape spider
    --url https://status.example
    [--depth N] [--limit N] [--delay ms]
    [--render-js] [--out ./artifacts]
    [--allow-host <pattern>] ...

ncrawler build report-md  <artifact-dir>
ncrawler build report-ai  <artifact-dir> [--skill <id>] [--model <name>]
ncrawler build vector     <artifact-dir> [--store lance://./vec | qdrant://...]

ncrawler ls   [--source grafana] [--since 24h]   # parses dirnames
ncrawler show <artifact-dir>                     # summary, no build
```

`<artifact-dir>` accepts the `latest` symlink so operators don't
have to memorise timestamps.

Built on `clap` (derive). Logs via `tracing` + `tracing-subscriber`
(initialised through `starter-observability`). Prometheus metrics
exposed when run under a long-lived command (none in v1 — listed
for future scrapers that grow a watch mode).

---

## Dependency policy (open-source first)

Pinned to OSS, permissive licences (MIT / Apache-2.0 / BSD), versions
matched to starter's root manifest to avoid duplicate crate versions
in `cargo tree`:

| Concern                 | Crate                                      | Licence    |
| ----------------------- | ------------------------------------------ | ---------- |
| Async runtime           | `tokio = "1"`                              | MIT        |
| HTTP client             | `reqwest = "0.12"` (rustls)                | MIT/Apache |
| Grafana API client      | `grafana = "=0.1.3"` (pinned; `client.raw()` escape hatch) | MIT |
| Crawling                | `spider` HTTP-only (`default-features = false`) | MIT   |
| Headless Chrome         | `chromiumoxide` (single stack)             | MIT/Apache |
| Readable extraction     | `dom_smoothie`                             | MIT        |
| HTML→Markdown           | `fast_html2md` (`lol_html`-based)          | MIT/Apache |
| CLI parsing             | `clap = "4"` (derive)                      | MIT/Apache |
| Serde                   | `serde = "1"`, `serde_json = "1"`          | MIT/Apache |
| Time                    | `chrono = "0.4"` (features `serde`, `clock`) — matches starter | MIT/Apache |
| Errors                  | `thiserror`, `anyhow` (cli only)           | MIT/Apache |
| Logging                 | `tracing`, `tracing-subscriber`            | MIT        |
| Hashing                 | `blake3`                                   | CC0/Apache |
| Embeddings (stretch)    | `fastembed`                                | Apache-2.0 |
| Vector store (default, stretch)  | `lancedb`                         | Apache-2.0 |
| Vector store (optional) | `qdrant-client` (feature `store-qdrant`)   | Apache-2.0 |

From `starter` (path deps):

- `starter-spi` — `Cancel`, shared error helpers
- `starter-ai` — `Registry`, `ClaudeRunner` (feature `provider-claude`)
- `starter-skills` — `SkillRegistry`, `SkillSelector`, blake3 hash quarantine
- `starter-secrets-file` and/or `starter-secrets-keyring` — token storage
- `starter-observability` — `tracing-subscriber` init + Prometheus registry

Explicitly **not** used in v1: Spider Cloud, hosted Anthropic API
(only `provider-claude`), `spider_chrome` / `spider`'s `chrome` and
`smart` features, OTLP exporter, any GPL/AGPL dependency.

After wiring path deps, `cargo tree -e normal` MUST show a single
version of `tokio`, `reqwest`, `serde`, `chrono`, and `clap`.

---

## Promotion candidates (move to `starter` if they generalise)

Built here first, lifted later once a second consumer materialises:

- **`starter-headless`** — thin `chromiumoxide` ergonomic layer
  (navigate, wait-for-selector, screenshot-element, PDF, binary
  discovery). The first consumer is `ncrawler-spider`'s
  `chrome.rs`; the moment a second starter app needs server-side
  rendering, lift this.
- **`starter-vector`** — `Embedder` + `VectorStore` traits,
  `fastembed` + `lancedb` impls, optional `qdrant`. Nothing about
  it is ncrawler-specific.
- **`starter-artifact`** — the on-disk artifact directory layout
  (`schema_version`, timestamped folders, `latest` symlinks,
  `assets/`), if a second domain (logs, traces) reuses the same
  shape.

**Stays in ncrawler**: anything Grafana- or spider-specific
(`ncrawler-grafana`, `ncrawler-spider`, the CLI, the report builders).

---

## Testing

- Per-crate unit tests using `tokio::test`.
- `ncrawler-grafana` ships a `wiremock`-backed integration suite for
  the API surface; renderer-plugin and Chrome-fallback visual paths
  tested behind `RUN_LIVE_TESTS=1` against a docker-compose Grafana
  fixture under `tests/fixtures/`.
- `ncrawler-spider` reuses `spider`'s upstream test patterns; live
  tests gated on `RUN_LIVE_TESTS=1`.
- Builders are tested against checked-in golden artifacts under
  `crates/builders/*/tests/fixtures/`, including a multi-panel
  Grafana artifact with item↔asset links.
- No CI defined in this scope; assume the repo follows the same
  `cargo test --workspace` discipline as `starter`.

## Security

- Tokens never logged. `tracing` fields holding secrets use
  `starter_ai::secret`-style wrappers.
- **SSRF guard at scrape phase.** `ScrapeJob` carries an optional
  `allow_hosts: Vec<HostPattern>`. `ncrawler-grafana` validates
  every datasource URL surfaced via `/api/ds/query` against it
  before issuing; `ncrawler-spider` validates every URL it would
  follow. Default: no allow-list (operator opts in). The build
  phase makes no outbound calls, so the guard belongs at scrape time.
- Headless Chrome runs sandboxed by default;
  `NCRAWLER_CHROME_NO_SANDBOX=1` opts out, logged at WARN.
- Artifact directories are written `0700` (operator-only); assets
  inherit. Scraped HTML/PNG can contain sensitive observability data.
- robots.txt respected by default in `ncrawler-spider`; override is
  an explicit per-job flag, logged at WARN.

---

## Milestones

1. **M1 — Skeleton & contract.** Workspace, `ncrawler-spi` with
   versioned `Artifact` + `Item` + `Asset` (item_id link, stable
   IDs documented), `ncrawler-core` artifact store with `latest`
   symlinks, `ncrawler-cli` shell (`ls`/`show` work against an
   empty store).
2. **M2 — Grafana API.** `ncrawler-grafana` mode=`Api`, deterministic
   `panel-<panelId>` IDs, end-to-end `scrape grafana` writes a real
   artifact. `ncrawler-report-md` renders it with item↔asset
   linkage exercised (no assets yet, but the merge code paths exist).
3. **M3 — Claude builder.** `ncrawler-report-ai` wired to
   `starter-ai` + `starter-skills` with one bundled skill
   (`grafana-triage`).
4. **M4 — Grafana visual + Spider.** Grafana mode=`Visual`/`Both`
   via the renderer plugin (primary); `--visual-fallback chrome`
   via our `chromiumoxide` layer (best-effort, documented as flaky).
   `ncrawler-spider` HTTP-only for generic pages, with
   `dom_smoothie` extraction.
5. **M5 — Vector (stretch).** `ncrawler-vector` with `fastembed` +
   `lancedb` default, `store-qdrant` feature optional. Promotion to
   `starter-vector` is governed by the rule in *Promotion candidates*
   (lift when a second consumer materialises), not decided here.

Each milestone is mergeable on its own and leaves the previous
milestones green.

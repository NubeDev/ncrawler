## Done

- Implemented the `ncrawler-vector` (stretch) vector builder, integrated into the EXISTING stage-1..4 workspace (not a parallel re-scaffold). Committed at HEAD `e60f4ba` "stage 5 (stretch) - Vector builder"; working tree clean.
- `Embedder` + `VectorStore` traits. Default `Embedder` = `FastEmbedEmbedder` (fastembed-rs, ONNX, no network) behind feature `embed-fastembed`; dependency-free `HashEmbedder` fallback used by unit tests / the in-memory mock.
- Default `VectorStore` = `LanceStore` (lancedb, in-process, single on-disk dir) behind feature `store-lance`; feature-gated `QdrantStore` (`store-qdrant`, NOT default — needs a server); `MemoryStore` mock = reference impl of the upsert contract.
- Deterministic char-based chunking with overlap (`chunk.rs`). Upsert keyed on `(source,target,item_id)` via delete-then-insert per triple, so re-scrapes overwrite rather than duplicate (uses the stage-1 stable `Item.id`). URI parsing: `lance://<path>` (default `lance://./vec`) | `qdrant://host:port[/coll]`.
- CLI: `ncrawler build vector <artifact-dir> [--store …]` wired into the existing `run_build` dispatch in `crates/ncrawler-cli/src/main.rs` (early-return branch calling `ncrawler_vector::build_vector`, reads `--store`, default `DEFAULT_STORE_URI`). CLI gained a `store-qdrant` passthrough feature → `ncrawler-vector/store-qdrant`.
- Tests: chunking + upsert idempotency vs `MemoryStore` (unit); LanceDB integration test in `tests/lance_live.rs` gated on `RUN_LIVE_TESTS=1` + `store-lance`.
- Full verify GREEN: `cargo build --workspace`, `cargo build --workspace --features store-qdrant`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (vector lib 16 + doc 1 pass; live LanceDB test ignored without the env var).

## Next

- (none) — stage 5 complete. (This was the final stage of the job.)
- Optional follow-up only: if a second consumer ever appears, promote the `Embedder`/`VectorStore` traits + impls to `starter-vector` per SCOPE's promotion rule (explicitly NOT done now).

## What you need to know

- DEPENDENCY DEVIATION (toolchain-forced, within SCOPE): SCOPE names `lancedb` without pinning a version. lancedb `0.10`/arrow `52` (what an earlier session used under cargo 1.91) FAILS to compile under this worktree's cargo 1.94.1 — `arrow-arith` hits `E0034`. Bumped to `lancedb = "0.17"` + `arrow-array`/`arrow-schema = "53"` (resolves to arrow `53.4.1`, single major, contains the fix; `lance 0.23.2`). `cargo tree` confirms one arrow version. No SCOPE.md edit needed (no version was pinned there). qdrant-client = "1", fastembed = "4". These vector deps are crate-local (not in the root `[workspace.dependencies]`) since the sibling `starter` workspace doesn't carry them.
- `ncrawler-spi` `Cancel` is re-exported from `starter_spi::ai::Cancel` and requires both `is_cancelled()` and `cancelled()`; the vector pipeline only polls `is_cancelled()`. Tests use a local no-op `NoCancel`. The CLI bridges Ctrl-C via `starter_ai::TokenCancel` (same as the other builders).
- The vector builder does NOT implement `spi::Builder` (its output is an external store dir/server, not files in the artifact dir), so it is invoked directly from `run_build` rather than through the `BuildOutput` path used by report-md/report-ai.
- ENVIRONMENT WARNING for the next session: this session suffered a severe tool-output rendering blackout — Bash/Read results were frequently delivered empty and only flushed (in bulk) several turns later when background-task notifications arrived. If you hit it, write command output to a file and read it after triggering a background task; don't trust an empty result as "no output". Also note the worktree was re-synced to branch HEAD mid-session once, which silently reverted in-progress edits to already-tracked files — re-verify edits to tracked files landed.

## Open questions

- Confirm whether pinning the vector stack to newer lancedb (0.17/arrow 53) is acceptable long-term vs. the (now uncompilable on rustc 1.94) 0.10; if SCOPE should record a minimum lancedb version, add it at the next REVIEW gate.

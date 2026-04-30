# CLAUDE.md

Local context server. Indexes files into an embedded LanceDB, exposes them to Claude Code over MCP. Single static Rust binary; OpenRouter for embeddings.

## Build & run

```bash
cargo check                # fastest iteration loop
cargo build --release
cargo run -- init          # write starter config
cargo run -- index         # bulk index paths from config
cargo run -- watch         # auto-reindex on file change
cargo run -- serve         # MCP stdio server (what Claude Code spawns)
cargo run -- search "..."  # one-off CLI sanity check
```

No test suite yet. When adding tests prefer `cargo test --lib` and put fixtures under `tests/fixtures/`.

## Module map

| file        | responsibility                                              |
| ----------- | ----------------------------------------------------------- |
| `main.rs`   | entrypoint; wires `tracing` to stderr (don't change this)   |
| `cli.rs`    | clap subcommand dispatch                                    |
| `config.rs` | TOML load/init at `$XDG_CONFIG_HOME/lilctx/config.toml`     |
| `chunk.rs`  | markdown-by-heading + line-window chunkers; `sha256_hex`    |
| `embed.rs`  | OpenRouter HTTP client (OpenAI-compatible `/v1/embeddings`) |
| `index.rs`  | bulk `run()` + single-file `reindex_file()`                 |
| `store.rs`  | LanceDB + Arrow plumbing                                    |
| `mcp.rs`    | `rmcp` server exposing `search` and `list_paths` tools      |
| `watch.rs`  | `notify`-based watcher with debounce                        |

The canonical indexing flow lives once, in `index::reindex_file`:

```
read file → file_hash = sha256(content)
         → store.file_already_indexed(path, file_hash)? → skip if yes
         → chunk → embed (batched) → store.delete_path → store.upsert
```

When adding any new ingestion path (e.g. URL fetcher, paste buffer), call into the same flow rather than reimplementing dedup.

## Hard rules

**Never `println!` / `print!` in code reachable from `serve`.** The MCP stdio transport owns stdout for JSON-RPC; any stray byte breaks the protocol. Use `eprintln!` or `tracing::{info, warn, error}`. The `search` and `status` subcommands intentionally use `println!` because they're CLI-only and never reached from `serve`.

**Every `Chunk` must carry `file_hash`.** Same value across all chunks of one file. `Store::file_already_indexed` depends on it; the dedup story collapses without it.

**Always `delete_path` before `upsert` when replacing a file's chunks.** LanceDB `add` is append-only — without the delete you accumulate duplicate `(path, chunk_index)` rows on every reindex.

**Schema changes break existing data dirs.** Any add/remove/rename in `store::make_schema` forces users to delete `data_dir`. Bump the version in `Cargo.toml` and call it out in the README's known-rough-edges section.

**Never leak task-organization details into the code.** No identifiers like `taskFourFunction`, `step3Helper`, `phase2Chunker`. No comments like `// ticket-1.3`, `// part of task 4`, `// from sprint planning doc`. Name things by what they do (`reindex_file`, `chunk_by_heading`); reference the work in commit messages or PR descriptions, never in source. The codebase outlives the plan.

## LanceDB + Arrow gotchas

The `lancedb` and `arrow-array` crates are tightly coupled and move fast. Compile errors mentioning `FixedSizeListBuilder`, `vector_search`, `RecordBatchIterator`, `Select`, or `QueryBase` are almost always version skew — bump them together, not independently.

The vector column is `FixedSizeList<Float32, dim>` where `dim` comes from config. If a user's config `dim` disagrees with the stored table's dim, search returns vague errors. The fix is always: delete `data_dir`, fix config, reindex.

## Adding an MCP tool

In `mcp.rs`:

1. Define an args struct with `#[derive(Deserialize, JsonSchema)]`
2. Add `async fn name(&self, Parameters(args): Parameters<Args>) -> Result<CallToolResult, ErrorData>` inside the `#[tool_router] impl` block
3. Annotate with `#[tool(description = "...")]` — the description is what the agent sees when deciding to call it. Be specific about what it returns.

`search` is the reference example. Errors from anyhow get bridged with `.map_err(|e| ErrorData::internal_error(e.to_string(), None))`.

## Watch debounce

The loop is "wait for first event, then drain until 500ms of silence, then flush" — not a fixed polling interval. One vim save fires 5–20 fs events; the silence-window collapses them. If you see duplicate reindexes, the editor's save burst exceeds 500ms — bump `DEBOUNCE_MS` in `watch.rs` rather than adding new dedup logic.

`should_index` in `watch.rs` is a cheap segment-based filter (excludes `.git/`, `node_modules/`, swap files, etc.). For real `.gitignore` semantics, build an `ignore::gitignore::Gitignore` per root at startup and consult it there.

## Where things commonly need fixing

- **OpenRouter rate limits** → add retry with exponential backoff in `embed::embed_batch`. Currently fails the whole batch on a single 429.
- **Large files OOM the chunker** → `chunk::chunk_lines` reads `lines: Vec<&str>` upfront. Streaming version would be a non-trivial refactor; usually easier to add a max-file-size skip in `index::collect_files`.
- **Slow cold reindex** → embedding is the bottleneck, not chunking or storage. Parallelize `embed_batch` calls (currently serial across files in `index::run`) before optimizing anything else.

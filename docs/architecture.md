# Architecture

A short tour of the codebase, the indexing flow, and the invariants the
binary relies on.

## Module map

| file        | responsibility                                              |
| ----------- | ----------------------------------------------------------- |
| `main.rs`   | Entrypoint. Wires `tracing` to stderr.                      |
| `cli.rs`    | clap subcommand dispatch.                                   |
| `config.rs` | TOML load / `init` at `$XDG_CONFIG_HOME/lilctx/config.toml`.|
| `chunk.rs`  | Markdown-by-heading and line-window chunkers; `sha256_hex`. |
| `embed.rs`  | OpenAI-compatible `/v1/embeddings` HTTP client.             |
| `index.rs`  | Bulk `run()` and single-file `reindex_file()`.              |
| `store.rs`  | LanceDB + Arrow plumbing.                                   |
| `mcp.rs`    | `rmcp` server exposing `search` and `list_paths`.           |
| `watch.rs`  | `notify`-based watcher with debounce.                       |

## The canonical indexing flow

The flow lives once, in `index::reindex_file`:

```text
read file
  → file_hash = sha256(content)
  → store.file_already_indexed(path, file_hash)?  → skip if yes
  → chunk
  → embed (batched)
  → store.delete_path
  → store.upsert
```

Both `index::run` (bulk) and `watch` route into this same shape.
**Any new ingestion path — URL fetcher, paste buffer, whatever — must call
into the same flow** rather than reinventing the dedup story.

## Hard rules

These are load-bearing. Breaking one breaks the binary in a way that is
hard to debug from the symptom.

### Never `println!` from code reachable by `serve`

The MCP stdio transport owns stdout for JSON-RPC framing. A stray byte from
an errant `println!` corrupts the next frame and the protocol falls over.

- Use `eprintln!` or `tracing::{info, warn, error}` instead.
- The `clippy.toml` lint `print_stdout = "deny"` enforces this at compile
  time.
- `cli.rs` opts out of the lint with `#![allow(clippy::print_stdout)]`
  because the `search` and `status` subcommands intentionally write
  user-visible output, and they are never reached from `serve`. Don't
  copy that allow into other modules.

### Every `Chunk` carries `file_hash`

Same value across all chunks of one file (it is a hash of the entire
source). `Store::file_already_indexed` depends on this — drop it and
the dedup story collapses, which means every reindex re-bills the
embeddings provider.

### Always `delete_path` before `upsert` when replacing a file's chunks

LanceDB `add` is append-only. Without a delete, you accumulate duplicate
`(path, chunk_index)` rows on every reindex.

### Schema changes break existing data dirs

Any add / remove / rename in `store::make_schema` invalidates every existing
`data_dir`. When you change the schema:

1. Bump the version in `Cargo.toml`.
2. Note the break in the README so users know to delete `data_dir`
   before upgrading.

The `file_hash` field was added in v0.2 and is the most recent example.

## LanceDB + Arrow gotchas

The `lancedb` and `arrow-array` crates are tightly coupled and move fast.
Compile errors mentioning `FixedSizeListBuilder`, `vector_search`,
`RecordBatchIterator`, `Select`, or `QueryBase` are almost always version
skew — bump them together, not independently.

The vector column is `FixedSizeList<Float32, dim>` where `dim` comes from
config. If the user's `embedding.dim` disagrees with the dim of the
already-stored table, search returns vague errors. The fix is always:
delete `data_dir`, fix the config, reindex.

## Watch debounce

The loop is "wait for the first event, then drain until 500ms of silence,
then flush" — not a fixed polling interval. One vim save fires 5–20 fs
events; the silence window collapses them.

If you see duplicate reindexes, the editor's save burst is exceeding 500ms.
Bump `DEBOUNCE_MS` in `watch.rs` rather than adding new dedup logic.

`should_index` in `watch.rs` is a cheap segment-based filter that excludes
`.git/`, `node_modules/`, swap files, and the binary extensions list. For
real `.gitignore` semantics, build an `ignore::gitignore::Gitignore` per
root at startup and consult it there.

## Where things commonly need fixing

- **OpenRouter rate limits** → `embed::embed_batch` fails the whole batch on
  a single 4xx/5xx. Adding retry with exponential backoff is the obvious
  next step.
- **Large files OOM the chunker** → `chunk::chunk_lines` reads
  `lines: Vec<&str>` upfront. A streaming version would be a non-trivial
  refactor; usually easier to add a max-file-size skip in
  `index::collect_files`.
- **Slow cold reindex** → embedding is the bottleneck, not chunking or
  storage. Parallelize `embed_batch` calls (currently serial across files
  in `index::run`) before optimizing anything else.

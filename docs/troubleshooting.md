# Troubleshooting

Common failure modes and the fixes that actually work.

## `env var "LILCTX_OPENROUTER_API_KEY" is unset` (or your configured `api_key_env`)

The env var that `embedding.api_key_env` points at has to be set in the
environment of whatever process is running `lilctx`. The shipped default is
`LILCTX_OPENROUTER_API_KEY`; if you set `LILCTX_OPENAI_API_KEY` instead, the
override layer flips `api_key_env` to that name automatically.

For an interactive shell, `export LILCTX_OPENROUTER_API_KEY=...`. For Claude
Code, put it in the `env` block of the MCP server entry — the spawned
process does not inherit your shell environment.

## `both LILCTX_OPENROUTER_API_KEY and LILCTX_OPENAI_API_KEY are set`

You exported both. Pick one. They're mutually exclusive because each one
controls which `base_url` the embeddings client points at, and we'd rather
hard-fail than silently pick a winner.

## `embedding dim mismatch from API: got X, configured dim = Y`

The model the API actually used returned a different vector size than your
config promised. Either:

- You changed `embedding.model` without updating `embedding.dim`. Update it
  to match what the model produces.
- The provider silently routed you to a different model. Check their docs.

After fixing the config, **delete `data_dir` and reindex**. LanceDB fixes
the vector column type at table-creation time, so a stored table with a
different `dim` cannot be reused.

## Search returns vague errors / nothing matches

Almost always a `dim` mismatch between config and the stored table. Same
fix: delete `data_dir`, confirm `embedding.dim` matches the model,
reindex.

## Compile errors mentioning `FixedSizeListBuilder`, `vector_search`, `RecordBatchIterator`

The `lancedb` and `arrow-array` crates are version-locked to each other.
If you bumped one and not the other, you get errors like this. Bump them
together — see the pin notes in `Cargo.toml`.

## Indexing is silent / stuck

`lilctx index` logs to stderr at info level by default. Crank verbosity:

```bash
RUST_LOG=lilctx=debug lilctx index
```

If embedding is slow on a cold reindex, that's expected — embedding is the
bottleneck and the calls are serial across files today. See the "where
things commonly need fixing" section in
[Architecture](./architecture.md).

## Watch reindexes the same file twice on every save

Your editor's save burst is exceeding the 500ms debounce window. Bump
`DEBOUNCE_MS` in `src/watch.rs` and rebuild. Don't add a second layer of
dedup — the existing `file_hash` skip will already make the second pass
cheap, but a longer debounce is cleaner.

## Claude Code can't see the `search` tool

In rough order of likelihood:

1. The MCP server entry has the wrong `command` or `args`. Run
   `lilctx serve` directly from a terminal and check it doesn't error out
   immediately. (It will sit there waiting for JSON on stdin — that's
   fine.)
2. The `env` block in the MCP config is missing the API key, so the
   process is exiting at startup with `env var ... is unset`.
3. The path to the binary is wrong / not on `PATH`. Use an absolute path.
4. You added the entry but didn't restart Claude Code.

Check Claude Code's MCP logs — failures during server spawn show up there.

## `config already exists` from `lilctx init`

`init` deliberately refuses to overwrite. Delete the file first (or pass
`--path` to write somewhere else).

## I changed `embedding.dim` and now nothing works

Right — the vector column type is fixed at table-creation time. There is
no migration path. Delete `data_dir`, then `lilctx index` again. Yes, this
re-bills your embeddings provider for the whole corpus.

## Stray bytes appearing on stdout when running `serve`

You added a `println!` somewhere reachable from `serve`. That breaks the
MCP protocol. Replace it with `eprintln!` or `tracing::info!`. The
`print_stdout = "deny"` clippy lint is supposed to catch this — if it
didn't, something is `#[allow]`ing it.

# CLI reference

All commands accept a global `--config <path>` flag. If omitted, the config is
read from `$XDG_CONFIG_HOME/lilctx/config.toml`.

```text
lilctx [--config <path>] <command>
```

## `init`

Write a starter config file.

```bash
lilctx init                  # writes to the default path
lilctx init --path ./foo.toml
```

Refuses to overwrite an existing file. Delete it first if you want to
regenerate.

## `index`

Walk paths, chunk files, embed, and upsert into the store.

```bash
lilctx index                       # uses `paths` from config
lilctx index ~/notes ~/projects    # one-off override
lilctx index --force               # re-embed every file regardless of hash
```

Skips files whose `file_hash` matches the stored version. With `--force`,
every file is reindexed (which means re-billing your embeddings provider —
use sparingly).

The walker honors `.gitignore`, hidden files included. Common binary
extensions (`png`, `pdf`, `zip`, `wasm`, …) are filtered out before chunking.

## `search`

One-off semantic search from the terminal. Mostly useful for sanity-checking
the index without spinning up an MCP client.

```bash
lilctx search "deploy runbook" -k 8
```

Flags:

- `-k`, `--limit <n>` — max hits, default 5.

Output is one block per hit: score, path, chunk index, and the first eight
lines of the chunk content.

## `status`

Print the data directory and the chunk count.

```bash
lilctx status
```

## `serve`

Run as an MCP server over stdio. This is what Claude Code spawns — you
generally do not run it interactively. If you do, it will sit there waiting
for JSON-RPC frames on stdin.

```bash
lilctx serve
```

All logging goes to stderr — stdout is owned by the JSON-RPC framing
channel. Set `RUST_LOG=lilctx=debug` (or `tracing-subscriber`'s
`EnvFilter` syntax) to crank verbosity.

## `watch`

Watch every configured path and reindex on change.

```bash
lilctx watch
```

The loop is "wait for the first event, then drain until 500ms of silence,
then flush." A vim save (which fires 5–20 fs events) collapses into one
reindex. Removed files are deleted from the index; renamed files surface as
a remove + a create.

A small built-in path filter excludes `.git/`, `node_modules/`, `target/`,
editor swap files, and the binary extensions listed under `index`. For full
`.gitignore` semantics, see the note in [Architecture](./architecture.md).

# lilctx

Local context server for AI coding agents. Walks a set of paths, embeds the
files into an embedded LanceDB vector store, and exposes them to Claude Code
(or any other MCP client) over an MCP stdio transport.

Single static Rust binary. Embeddings come from any OpenAI-compatible
`/v1/embeddings` endpoint — defaults to OpenRouter, but you can point it at
OpenAI direct, Together, a local Ollama gateway, etc.

## What it is good for

- Giving an agent semantic search over a local notes vault, a private codebase,
  or any directory of plain-text / markdown files.
- Running entirely on your machine: the index lives in a local directory, the
  MCP server is spawned over stdio by Claude Code itself, and the only
  outbound traffic is to the embeddings provider.
- Staying out of the way: `lilctx watch` keeps the index in sync as you edit,
  and `file_hash`-based dedup means re-running `index` is cheap.

## What it is not

- Not a code-aware indexer. It treats source files as text. Markdown gets a
  heading-aware splitter; everything else gets a line-window splitter with
  overlap.
- Not a hosted service. There is no server to deploy, no auth, no
  multi-tenant story. One user, one machine, one config.

## Install

Pre-built binaries for macOS (Intel + Apple Silicon) and Linux x86_64 are
attached to every tagged release. See [Getting started](./getting-started.md)
for the one-liner downloads. Or `cargo build --release` from source.

## Where to go next

- [Getting started](./getting-started.md) — install, init, first index, wire
  it into Claude Code.
- [CLI reference](./cli.md) — every subcommand and flag.
- [Configuration](./configuration.md) — every field in `config.toml`.
- [MCP tools](./mcp.md) — what the agent sees: `search`, `list_paths`.
- [Architecture](./architecture.md) — module map, indexing flow, the
  invariants you must not break.
- [Troubleshooting](./troubleshooting.md) — common failure modes and fixes.

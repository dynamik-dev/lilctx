# Getting started

A walkthrough from zero to "Claude Code can search my notes."

## 1. Install

Pre-built binaries are attached to every tagged release on GitHub. Pick the
right archive for your machine, extract it, and move `lilctx` onto your
`PATH`.

```bash
# macOS Apple Silicon
curl -L https://github.com/<owner>/lilctx/releases/latest/download/lilctx-aarch64-apple-darwin.tar.gz | tar xz

# macOS Intel
curl -L https://github.com/<owner>/lilctx/releases/latest/download/lilctx-x86_64-apple-darwin.tar.gz | tar xz

# Linux x86_64 (glibc)
curl -L https://github.com/<owner>/lilctx/releases/latest/download/lilctx-x86_64-unknown-linux-gnu.tar.gz | tar xz

# Then put it somewhere on PATH:
mv lilctx ~/.local/bin/   # or /usr/local/bin, ~/bin, etc.
```

Or build from source if you have a Rust toolchain:

```bash
cargo build --release
# binary lands at target/release/lilctx
```

`lilctx` is a single statically-linked binary — there is no runtime to
install, no daemon to manage.

## 2. Get an embeddings API key

The default provider is [OpenRouter](https://openrouter.ai). The fastest way
to make `lilctx` see your key is to drop it in `~/.lilctx.json`:

```json
{
  "LILCTX_OPENROUTER_API_KEY": "sk-or-..."
}
```

`lilctx` reads that file at startup and injects every entry into its process
environment, so the same value is visible whether you run `lilctx` from a
shell or whether Claude Code spawns it. Real env vars (shell exports, the
MCP `env` block) always win, so the file is a fallback rather than an
override. **Treat the file like an SSH key** — `chmod 600 ~/.lilctx.json` is
a good idea.

If you'd rather not use a file, exporting the same vars in your shell works
identically:

```bash
export LILCTX_OPENROUTER_API_KEY=sk-or-...
```

If you'd rather use OpenAI (or any other OpenAI-compatible `/v1/embeddings`
endpoint — Together, a local Ollama gateway, etc.), use the OpenAI-compatible
mode instead:

```json
{
  "LILCTX_OPENAI_API_KEY": "sk-...",
  "LILCTX_OPENAI_BASE_URL": "https://my-proxy.example/v1"
}
```

`LILCTX_OPENROUTER_API_KEY` and `LILCTX_OPENAI_API_KEY` are mutually
exclusive — set exactly one. They override whatever `embedding.base_url` and
`embedding.api_key_env` are written in the config file, so you can flip
providers without editing TOML.

## 3. Write a starter config

```bash
lilctx init
```

This writes a config to `$XDG_CONFIG_HOME/lilctx/config.toml` (typically
`~/.config/lilctx/config.toml` on Linux, `~/Library/Application
Support/lilctx/config.toml` on macOS). Pass `--path <file>` to write it
somewhere else.

Open the file and at minimum edit `paths` to point at the directories you
want indexed. See [Configuration](./configuration.md) for every field — and
for the `LILCTX_EMBEDDING_MODEL` / `LILCTX_EMBEDDING_DIM` env overrides if
you want to swap models without touching the file.

## 4. Build the index

```bash
lilctx index
```

This walks every path in your config, chunks each file, embeds the chunks in
batches against the configured provider, and writes them to LanceDB at
`data_dir`. Files unchanged since the last index are skipped (the dedup is
keyed on a SHA-256 of the file contents).

You can also pass paths explicitly:

```bash
lilctx index ~/notes ~/work/runbook.md
```

Pass `--force` to re-embed even if `file_hash` matches the stored version.

## 5. Sanity-check from the CLI

```bash
lilctx search "how do I rotate the prod database password" -k 5
lilctx status
```

`search` runs the full pipeline (embed the query, k-NN against the table,
print results). `status` prints the data directory and the row count.

## 6. Wire it into Claude Code

Add an MCP server entry that runs `lilctx serve`. In your Claude Code
config (typically `~/.claude.json` or whatever your client uses), add
something like:

```json
{
  "mcpServers": {
    "lilctx": {
      "command": "lilctx",
      "args": ["serve"],
      "env": {
        "LILCTX_OPENROUTER_API_KEY": "sk-or-..."
      }
    }
  }
}
```

If `lilctx` is not on `PATH`, use the absolute path to the binary. The `env`
block is how the server gets its API key — Claude Code spawns the process
fresh, so your shell exports do not propagate.

Restart Claude Code. The agent should now have `search` and `list_paths`
tools available. See [MCP tools](./mcp.md) for the schema.

## 7. Keep the index live

In a separate terminal:

```bash
lilctx watch
```

This watches every configured path, debounces editor save-bursts, and
reindexes only the changed files. Leave it running while you work.

If you'd rather not run a watcher, just re-run `lilctx index` whenever you
remember — the `file_hash` skip-check makes it cheap on a no-op run.

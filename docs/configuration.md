# Configuration

`lilctx init` writes a starter file to `$XDG_CONFIG_HOME/lilctx/config.toml`.
This page documents every field.

`~` is expanded to the user's home directory at load time, both in `paths`
and in `data_dir`. Tildes in the middle of a path are not expanded — only a
leading `~` or `~/`.

## Top-level

```toml
paths = ["~/notes", "~/work/runbooks"]
data_dir = "~/.local/share/lilctx"
```

| field      | type           | meaning                                                  |
| ---------- | -------------- | -------------------------------------------------------- |
| `paths`    | list of paths  | Roots walked by `index` and `watch`. Required.           |
| `data_dir` | path           | LanceDB data directory. Created on first run. Required.  |

## `[chunk]`

```toml
[chunk]
size = 1500
overlap = 200
```

| field     | type | meaning                                                      |
| --------- | ---- | ------------------------------------------------------------ |
| `size`    | int  | Soft target chunk size in bytes. Line-aware, so chunks may overshoot when a single line exceeds the target. |
| `overlap` | int  | Bytes of overlap between successive chunks. Helps preserve context across boundaries. |

Markdown files (`.md`, `.markdown`) split on `# `, `## `, and `### ` headings
first; sections larger than `2 * size` then fall through to the line
chunker. Other files use the line chunker directly.

## `[embedding]`

```toml
[embedding]
dim = 768
model = "baai/bge-base-en-v1.5"
api_key_env = "LILCTX_OPENROUTER_API_KEY"
batch_size = 32
base_url = "https://openrouter.ai/api/v1"
```

| field         | type   | meaning                                                                                  |
| ------------- | ------ | ---------------------------------------------------------------------------------------- |
| `dim`         | int    | Vector dimension. **Must** match the model's output. Changing it requires deleting `data_dir`. |
| `model`       | string | Model identifier as accepted by the provider's `/v1/embeddings` endpoint.                |
| `api_key_env` | string | Name of the env var holding the API key. Read at runtime, never persisted to disk.       |
| `batch_size`  | int    | Number of texts per HTTP request. Higher = fewer round-trips but bigger blast radius on failure. |
| `base_url`    | string | OpenAI-compatible `/v1` base URL (without the trailing `/embeddings`). Defaults to OpenRouter. |

### Secrets file: `~/.lilctx.json`

Any of the env vars below can also live in `~/.lilctx.json`, a flat JSON
object of `string -> string`:

```json
{
  "LILCTX_OPENROUTER_API_KEY": "sk-or-...",
  "RUST_LOG": "lilctx=info"
}
```

`lilctx` injects every entry into its process environment at startup. Real
env vars (shell exports, MCP-config `env` blocks) always take precedence —
the file is the fallback, not the override. Use it so the API key follows
the binary regardless of where it's spawned from. `chmod 600` it.

### Env overrides

These env vars override fields without editing the file. Useful for one-shot
runs and for the MCP server entry, which spawns fresh and does not see your
shell exports.

| env var                     | effect                                                                                       |
| --------------------------- | -------------------------------------------------------------------------------------------- |
| `LILCTX_OPENROUTER_API_KEY` | Force OpenRouter mode. Sets `base_url` to `https://openrouter.ai/api/v1` and `api_key_env` to itself. |
| `LILCTX_OPENAI_API_KEY`     | Force OpenAI-compatible mode. Sets `api_key_env` to itself; `base_url` defaults to `https://api.openai.com/v1`. |
| `LILCTX_OPENAI_BASE_URL`    | Override the base URL for OpenAI-compatible mode (any OpenAI-shape endpoint).                |
| `LILCTX_EMBEDDING_MODEL`    | Override `model`.                                                                            |
| `LILCTX_EMBEDDING_DIM`      | Override `dim`. Must match the model's output dimension; mismatches force a `data_dir` wipe. |

`LILCTX_OPENROUTER_API_KEY` and `LILCTX_OPENAI_API_KEY` are mutually
exclusive — setting both is a hard error.

### Picking a model

The default (`baai/bge-base-en-v1.5`, 768 dims) is cheap on OpenRouter and
strong on English text and code. If you change to a model with a different
output dimension, you **must** update `dim` to match and delete `data_dir` —
LanceDB fixes the vector column type at table-creation time, so an existing
table with the wrong `dim` will produce vague errors at search time.

### Pointing at a different provider

Anything that speaks the OpenAI `/v1/embeddings` shape works. Two ways:

**Via env (preferred for ad-hoc switching):**

```bash
export LILCTX_OPENAI_API_KEY=sk-...
export LILCTX_OPENAI_BASE_URL=http://localhost:11434/v1   # e.g. an Ollama gateway
export LILCTX_EMBEDDING_MODEL=nomic-embed-text
export LILCTX_EMBEDDING_DIM=768
```

**Via the config file (for a persistent default):**

```toml
# OpenAI direct
api_key_env = "OPENAI_API_KEY"
model = "openai/text-embedding-3-small"
dim = 1536
base_url = "https://api.openai.com/v1"

# Local Ollama gateway (with an OpenAI-compat shim)
api_key_env = "OLLAMA_API_KEY"   # often a placeholder, but the env var must exist
model = "nomic-embed-text"
dim = 768
base_url = "http://localhost:11434/v1"
```

The `dim` field still has to match what the model returns.

## Schema changes

The on-disk TOML schema is the public surface of this binary. A field
rename or removal is a breaking change for every existing user. If you
add a field, give it a `serde(default = ...)` so older configs continue to
load.

The LanceDB schema is a separate concern — see
[Architecture](./architecture.md) for the rules around modifying it.

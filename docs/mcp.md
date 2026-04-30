# MCP tools

`lilctx serve` speaks MCP over stdio. Two tools are exposed.

## `search`

Semantic search over the indexed chunks.

**Arguments**

| name    | type   | required | default | meaning                              |
| ------- | ------ | -------- | ------- | ------------------------------------ |
| `query` | string | yes      | —       | Free-form natural-language query.    |
| `limit` | int    | no       | 8       | Maximum number of hits to return.    |

A `limit` of `0` is treated as "use the default."

**Returns**

A single text block. If there are no hits, it reads `(no results)`.
Otherwise, one entry per hit:

```text
score=0.842  /Users/me/notes/runbooks/db.md  (chunk 3)
---
<chunk content>
---
```

Scores are `1.0 - distance` from the LanceDB cosine search — higher is
closer. The chunk content is returned verbatim, no truncation, so longer
chunks produce longer responses.

## `list_paths`

Returns the configured root paths, one per line. No arguments.

Useful as an introspection call when an agent wants to know what corpus it
is searching against. The response prints `(no paths configured)` when the
config has an empty list.

## Wiring it into a client

For Claude Code, add an entry like this to your MCP config:

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

The `env` block is how the spawned process gets its API key — Claude Code
does not inherit your shell environment. If `lilctx` is not on `PATH`, use
the absolute path to the binary.

For other MCP clients, the contract is the same: stdio transport, JSON-RPC
framing on stdout, all logs to stderr.

## Adding a new tool

If you are extending the server, see the "Adding an MCP tool" section in
the project's `CLAUDE.md`. The summary:

1. Define an args struct with `#[derive(Deserialize, JsonSchema)]`.
2. Add an `async fn` inside the `#[tool_router] impl LilCtxServer` block.
3. Annotate it with `#[tool(description = "...")]` — the description is
   what the agent sees when picking which tool to call. Be specific about
   what the tool returns; vague descriptions lead to vague tool use.

`search` in `src/mcp.rs` is the reference example.

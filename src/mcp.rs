//! MCP server over stdio.
//!
//! Exposes `search` (semantic query against the indexed chunks) and `list_paths`
//! (the configured roots). All logging goes to stderr -- stdout is the JSON-RPC
//! framing channel and a stray byte breaks the protocol.

use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content},
    schemars::{self, JsonSchema},
    service::ServiceExt,
    tool, tool_handler, tool_router,
    transport::io::stdio,
    ErrorData, ServerHandler,
};
use serde::Deserialize;

use crate::{config::Config, embed::OpenRouterEmbedder, store::Store};

#[derive(Clone)]
pub(crate) struct LilCtxServer {
    cfg: Arc<Config>,
    store: Arc<Store>,
    embedder: Arc<OpenRouterEmbedder>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchArgs {
    /// Free-form natural-language query.
    query: String,
    /// Max number of hits to return.
    #[serde(default = "default_limit")]
    limit: usize,
}

const DEFAULT_LIMIT: usize = 8;
fn default_limit() -> usize {
    DEFAULT_LIMIT
}

#[tool_router]
impl LilCtxServer {
    fn new(cfg: Config, store: Store, embedder: OpenRouterEmbedder) -> Self {
        Self {
            cfg: Arc::new(cfg),
            store: Arc::new(store),
            embedder: Arc::new(embedder),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Semantic search over locally indexed files. Returns ranked chunks with file path, chunk index, and the matched content."
    )]
    async fn search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let limit = if args.limit == 0 {
            DEFAULT_LIMIT
        } else {
            args.limit
        };
        let vec = self
            .embedder
            .embed_one(&args.query)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let hits = self
            .store
            .search(&vec, limit)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let body = if hits.is_empty() {
            "(no results)".to_string()
        } else {
            hits.iter()
                .map(|h| {
                    format!(
                        "score={:.3}  {}  (chunk {})\n---\n{}\n---",
                        h.score, h.path, h.chunk_index, h.content
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    #[tool(description = "List the configured root paths that this lilctx server indexes.")]
    async fn list_paths(&self) -> Result<CallToolResult, ErrorData> {
        let body = if self.cfg.paths.is_empty() {
            "(no paths configured)".to_string()
        } else {
            self.cfg
                .paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }
}

#[tool_handler]
impl ServerHandler for LilCtxServer {}

pub(crate) async fn serve(cfg: Config) -> Result<()> {
    let store = Store::open(&cfg).await?;
    let embedder = OpenRouterEmbedder::new(&cfg)?;
    let server = LilCtxServer::new(cfg, store, embedder);
    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}

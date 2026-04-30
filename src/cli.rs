// CLI subcommands (search, status) print results to the user's terminal.
// The MCP-stdout invariant only applies to `Command::Serve`, which routes
// through `mcp::serve` and never through this dispatcher's println! calls.
#![allow(clippy::print_stdout)]

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "lilctx",
    version,
    about = "Local context server for AI coding agents"
)]
pub(crate) struct Cli {
    /// Path to config (defaults to $XDG_CONFIG_HOME/lilctx/config.toml)
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Write a starter config file
    Init {
        #[arg(short, long)]
        path: Option<PathBuf>,
    },
    /// Index files. With no args, uses paths from config. Skips files unchanged since last index.
    Index {
        paths: Vec<PathBuf>,
        /// Re-embed even if file_hash matches the stored version
        #[arg(long)]
        force: bool,
    },
    /// One-off semantic search from the CLI
    Search {
        query: String,
        #[arg(short = 'k', long, default_value = "5")]
        limit: usize,
    },
    /// Show index stats
    Status,
    /// Run as an MCP server over stdio (this is what Claude Code spawns)
    Serve,
    /// Watch configured paths and reindex on change
    Watch,
}

pub(crate) async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Init { path } => crate::config::init(path),
        other => {
            let cfg = crate::config::load(cli.config.as_deref())?;
            match other {
                Command::Init { .. } => unreachable!(),
                Command::Index { paths, force } => {
                    let targets = if paths.is_empty() {
                        cfg.paths.clone()
                    } else {
                        paths
                    };
                    if targets.is_empty() {
                        anyhow::bail!(
                            "no paths to index — pass them as args or set `paths` in config"
                        );
                    }
                    crate::index::run(&cfg, &targets, force).await
                }
                Command::Search { query, limit } => {
                    let store = crate::store::Store::open(&cfg).await?;
                    let embedder = crate::embed::OpenRouterEmbedder::new(&cfg)?;
                    let vec = embedder.embed_one(&query).await?;
                    let hits = store.search(&vec, limit).await?;
                    for h in hits {
                        println!("{:.3}  {}  (chunk {})", h.score, h.path, h.chunk_index);
                        let preview: String =
                            h.content.lines().take(8).collect::<Vec<_>>().join("\n");
                        println!("---\n{preview}\n---\n");
                    }
                    Ok(())
                }
                Command::Status => {
                    let store = crate::store::Store::open(&cfg).await?;
                    let stats = store.stats().await?;
                    println!("data dir:       {}", cfg.data_dir.display());
                    println!("indexed chunks: {}", stats.chunk_count);
                    Ok(())
                }
                Command::Serve => crate::mcp::serve(cfg).await,
                Command::Watch => crate::watch::run(cfg).await,
            }
        }
    }
}

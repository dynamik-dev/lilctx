use anyhow::Result;
use clap::Parser;

mod chunk;
mod cli;
mod config;
mod embed;
mod index;
mod mcp;
mod store;
mod watch;

#[tokio::main]
async fn main() -> Result<()> {
    // Pull `~/.lilctx.json` into the env before anything else reads from it,
    // so RUST_LOG / API keys defined there take effect for this run.
    config::load_secrets_into_env()?;

    // CRITICAL: log to stderr — stdio MCP transport owns stdout for JSON-RPC.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = cli::Cli::parse();
    cli::run(cli).await
}

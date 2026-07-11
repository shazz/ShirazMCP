use anyhow::Result;
use shiraz_mcp::session::SessionConfig;
use shiraz_mcp::tools::HatariServer;
use rmcp::{transport::io::stdio, ServiceExt};

#[tokio::main]
async fn main() -> Result<()> {
    // stdout belongs to the MCP protocol — tracing goes to stderr only.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let running = HatariServer::new(SessionConfig::default())
        .serve(stdio())
        .await?;
    running.waiting().await?;
    Ok(())
}

use anyhow::Result;
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

mod server;

#[derive(Parser)]
#[command(name = "herdr-mcp", version)]
struct Args {
    /// Start an HTTP server on the given port for the web playground
    #[arg(long)]
    http: Option<u16>,

    /// Run HTTP server only (skip MCP stdio transport)
    #[arg(long)]
    http_only: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("herdr_mcp=info".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    if args.http.is_some() {
        let server = server::HerdrMcpServer::new();
        let port = args.http.unwrap();
        tokio::spawn(async move {
            if let Err(e) = server::start_http(server, port).await {
                tracing::error!("HTTP server failed: {e}");
            }
        });
        tracing::info!("HTTP playground listening on http://localhost:{port}");
    }

    if args.http_only {
        tracing::info!("HTTP-only mode — waiting for shutdown signal");
        tokio::signal::ctrl_c().await?;
        tracing::info!("Shutting down");
        return Ok(());
    }

    tracing::info!("Starting herdr-mcp MCP server");

    let server = server::HerdrMcpServer::new();
    let service = server.serve(stdio()).await?;

    tracing::info!("herdr-mcp server initialized, waiting for requests");

    service.waiting().await?;

    tracing::info!("herdr-mcp server stopped");

    Ok(())
}

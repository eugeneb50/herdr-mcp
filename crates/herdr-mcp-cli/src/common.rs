//! Shared startup for the `herdr-mcp` (daemon) and `herdr-trim` (tools) bins.
//!
//! Single source of truth for: tracing init, the full serve stack
//! (`serve_core`), the MCP stdio server (`run_serve`), and the headless stack
//! used under dashboards (`run_serve_headless`). Both bins delegate here —
//! neither duplicates construction. (Kept in the CLI crate rather than moved
//! into `herdr-mcp-server` so the server crate gains no CLI deps.)
//
// NOTE: included by two bins that each use a subset (`herdr-mcp` never calls
// `run_serve_headless`, `herdr-trim` never calls `run_serve`). Each bin marks
// the module `#[allow(dead_code)]` — see the `mod common` declaration.

use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};

use herdr_mcp_core::Config;
use herdr_mcp_server::{
    HerdrClient, HerdrMcpServer, Persistence,
    server::{spawn_trim_poller, start_http},
    triggers,
};

/// Tracing to stderr. Never stdout: stdout is MCP/ACP JSON-RPC on the daemon.
pub fn init_tracing() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("herdr_mcp=info".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();
    Ok(())
}

/// Build the full serve stack (persistence, herdr subscriber, registry, HTTP
/// bridge, recipe triggers) and spawn its background tasks. Does NOT block —
/// the spawned tasks live on the Tokio runtime until it shuts down. Returns
/// the one `HerdrMcpServer` every surface shares (HTTP, stdio, triggers, trim
/// poller): a single scheduler and one set of live runs. Shared by both
/// `serve` and `dashboard`: the former awaits the MCP server afterwards, the
/// latter runs the TUI on the main task.
pub async fn serve_core(config: &Config) -> Result<HerdrMcpServer> {
    let data_dir = config.data_dir.clone();
    let persistence = Persistence::new(data_dir.clone());
    persistence.init().await?;
    let persistence = std::sync::Arc::new(persistence);

    let herdr_client = build_herdr_client(
        &data_dir,
        config.herdr_socket_path(),
        config.herdr.reconnect_attempts,
        config.herdr.reconnect_backoff_ms,
    );
    herdr_client.spawn_subscriber();
    let registry = herdr_client.registry.clone();

    let server = HerdrMcpServer::with_config((*persistence).clone(), registry, config)
        .with_herdr_client(herdr_client.clone());
    // Periodic trim-badge refresh so savings badges survive server restarts.
    spawn_trim_poller(std::sync::Arc::new(server.clone()));

    let scheduler = server.scheduler.clone();
    tokio::spawn(async move {
        if let Err(e) = scheduler.load_existing().await {
            tracing::warn!("failed to load existing schedules: {e}");
        }
    });
    triggers::spawn(server.clone(), herdr_client.events());

    if let Some(port) = config.http.port {
        let server = server.clone();
        let bind = config
            .http
            .bind_addr
            .clone()
            .unwrap_or_else(|| "127.0.0.1".into());
        let web_dir = config.http.resolved_web_dir();
        tracing::info!("HTTP playground listening on http://{bind}:{port}");
        tokio::spawn(async move {
            if let Err(e) = start_http(server, &bind, port, &web_dir).await {
                tracing::error!("HTTP server failed: {e}");
            }
        });
    }

    Ok(server)
}

/// `serve` — full MCP server (stdio + optional HTTP).
pub async fn run_serve(config: &Config) -> Result<()> {
    let server = serve_core(config).await?;

    if config.http.http_only {
        tracing::info!("HTTP-only mode — waiting for shutdown signal");
        tokio::signal::ctrl_c().await?;
        tracing::info!("Shutting down");
        return Ok(());
    }

    tracing::info!("Starting herdr-mcp MCP server");
    let service = server.serve(stdio()).await?;
    tracing::info!("herdr-mcp server initialized, waiting for requests");
    service.waiting().await?;
    tracing::info!("herdr-mcp server stopped");
    Ok(())
}

/// Serve stack headless (MCP stdio disabled) for the dashboard: brings up the
/// HTTP bridge + herdr event subscriber; the caller runs the UI afterwards.
pub async fn run_serve_headless(config: &Config) -> Result<()> {
    serve_core(config).await?;
    Ok(())
}

pub fn build_herdr_client(
    data_dir: &std::path::Path,
    socket_path: std::path::PathBuf,
    reconnect_attempts: u32,
    reconnect_backoff_ms: u64,
) -> std::sync::Arc<HerdrClient> {
    let persistence = Persistence::new(data_dir.to_path_buf());
    let client = HerdrClient::new(
        std::sync::Arc::new(persistence),
        socket_path,
        reconnect_attempts,
        reconnect_backoff_ms,
    );
    std::sync::Arc::new(client)
}

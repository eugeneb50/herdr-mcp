//! Shared startup for the `herdr-mcp` (daemon) and `herdr-trim` (tools) bins.
//!
//! Single source of truth for: tracing init, the full serve stack
//! (`serve_core`), the MCP stdio server (`run_serve`), and the headless stack
//! used under dashboards (`run_serve_headless`). Both bins delegate here —
//! neither duplicates construction. (Kept in the CLI crate rather than moved
//! into `herdr-mcp-server` so the server crate gains no proxy/trim CLI deps.)
//
// NOTE: included by two bins that each use a subset (`herdr-mcp` never calls
// `run_serve_headless`, `herdr-trim` never calls `run_serve`). Each bin marks
// the module `#[allow(dead_code)]` — see the `mod common` declaration.

use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};

use herdr_mcp_core::Config;
use herdr_mcp_server::{
    AgentRegistry, HerdrClient, HerdrMcpServer, Persistence,
    server::{spawn_trim_poller, start_http},
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
/// bridge) and spawn its background tasks. Does NOT block — the spawned tasks
/// live on the Tokio runtime until it shuts down. Returns the shared
/// `AgentRegistry` + `Persistence` + `HerdrClient` so callers can build a stdio server against
/// the exact same registry the HTTP bridge uses. Shared by both `serve` and
/// `dashboard`: the former awaits the MCP/HTTP server afterwards, the latter
/// runs the TUI on the main task against the same registry.
pub async fn serve_core(
    config: &Config,
) -> Result<(
    AgentRegistry,
    std::sync::Arc<Persistence>,
    std::sync::Arc<HerdrClient>,
)> {
    let data_dir = config.data_dir.clone();
    let persistence = Persistence::new(data_dir.clone());
    persistence.init().await?;
    let persistence = std::sync::Arc::new(persistence);

    let herdr_client = build_herdr_client(
        &data_dir,
        config.herdr.socket_path.clone(),
        config.herdr.reconnect_attempts,
        config.herdr.reconnect_backoff_ms,
    );
    herdr_client.spawn_subscriber();
    let registry = herdr_client.registry.clone();

    // Periodic trim-badge refresh so savings badges survive server restarts.
    let trim_server = HerdrMcpServer::with_config((*persistence).clone(), registry.clone(), config)
        .with_herdr_client(herdr_client.clone());
    spawn_trim_poller(std::sync::Arc::new(trim_server));

    if let Some(port) = config.http.port {
        let server = HerdrMcpServer::with_config((*persistence).clone(), registry.clone(), config)
            .with_herdr_client(herdr_client.clone());
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

    // Auto-start the HTTPS intercepting proxy if `--proxy-port` was set.
    if let Some(proxy_port) = config.proxy.port {
        let data_dir = persistence.data_dir().to_path_buf();
        let bind = config
            .proxy
            .bind_addr
            .clone()
            .unwrap_or_else(|| "127.0.0.1".into());
        let target_hosts = config.proxy.target_hosts.clone();
        let policy = herdr_mcp_proxy::ProxyPolicy {
            trim_outbound: config.proxy.trim_outbound,
            trim_inbound: config.proxy.trim_inbound,
            stages: config.proxy.trim_stages.clone(),
        };
        let bind_for_log = bind.clone();
        tokio::spawn(async move {
            if let Err(e) = herdr_mcp_proxy::run_proxy_listener(
                proxy_port,
                &bind,
                &data_dir,
                target_hosts,
                policy,
            )
            .await
            {
                tracing::error!("proxy listener exited: {e}");
            }
        });
        tracing::info!("Proxy listener auto-started on {bind_for_log}:{proxy_port}");
    }

    Ok((registry, persistence, herdr_client))
}

/// `serve` — full MCP server (stdio + optional HTTP).
pub async fn run_serve(config: &Config) -> Result<()> {
    let (registry, persistence, herdr_client) = serve_core(config).await?;

    if config.http.http_only {
        tracing::info!("HTTP-only mode — waiting for shutdown signal");
        tokio::signal::ctrl_c().await?;
        tracing::info!("Shutting down");
        return Ok(());
    }

    tracing::info!("Starting herdr-mcp MCP server");
    let server = HerdrMcpServer::with_config((*persistence).clone(), registry, config)
        .with_herdr_client(herdr_client);
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
    herdr_socket: Option<std::path::PathBuf>,
    reconnect_attempts: u32,
    reconnect_backoff_ms: u64,
) -> std::sync::Arc<HerdrClient> {
    // The socket path is already resolved by Config::herdr_socket_path() which
    // includes env override (HERDR_SOCKET_PATH) and HERDR_SESSION fallback.
    // No need to re-check env here.
    let socket_path = herdr_socket.unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        std::path::PathBuf::from(home).join(".config/herdr/herdr.sock")
    });
    let persistence = Persistence::new(data_dir.to_path_buf());
    let client = HerdrClient::new(
        std::sync::Arc::new(persistence),
        socket_path,
        reconnect_attempts,
        reconnect_backoff_ms,
    );
    std::sync::Arc::new(client)
}

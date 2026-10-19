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
/// bridge; schedules, triggers and trim badges once this process is the data
/// dir's daemon, see [`spawn_daemon`]) and spawn its background tasks. Does NOT block —
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
    spawn_daemon(server.clone(), herdr_client.clone(), data_dir.clone());

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

/// Lock file in the data dir. Its holder is the data dir's daemon: the one
/// process that fires schedules and triggers and pushes trim badges, however
/// many herdr-mcp processes (MCP clients, the bridge, the dashboard) share it.
const DAEMON_LOCK: &str = "daemon.lock";
/// How often a process without the lock tries again, so one of them takes
/// over when the daemon exits.
const DAEMON_RETRY: std::time::Duration = std::time::Duration::from_secs(15);

/// Become the data dir's daemon once its lock is free, then run the
/// background work only the daemon does. The lock is held (and released by
/// the OS) for the life of the process.
fn spawn_daemon(
    server: HerdrMcpServer,
    herdr_client: std::sync::Arc<HerdrClient>,
    data_dir: std::path::PathBuf,
) {
    let path = data_dir.join(DAEMON_LOCK);
    tokio::spawn(async move {
        let mut told = false;
        let _lock = loop {
            match try_lock(&path) {
                Ok(Some(file)) => break file,
                Ok(None) if !told => {
                    tracing::info!(
                        "another herdr-mcp holds {}; it runs schedules and triggers",
                        path.display()
                    );
                    told = true;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        "cannot lock {}: {e}; schedules and triggers off",
                        path.display()
                    );
                    return;
                }
            }
            tokio::time::sleep(DAEMON_RETRY).await;
        };
        tracing::info!(
            "daemon for {}: running schedules and triggers",
            data_dir.display()
        );
        // Periodic trim-badge refresh so savings badges survive server restarts.
        spawn_trim_poller(std::sync::Arc::new(server.clone()));
        triggers::spawn(server.clone(), herdr_client.events());
        server.scheduler.run().await;
    });
}

/// The lock file, exclusively locked, or `None` if another process holds it.
fn try_lock(path: &std::path::Path) -> std::io::Result<Option<std::fs::File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_daemon_per_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DAEMON_LOCK);
        let first = try_lock(&path).unwrap();
        assert!(first.is_some());
        assert!(try_lock(&path).unwrap().is_none(), "second holder");
        drop(first);
        assert!(try_lock(&path).unwrap().is_some(), "takeover after exit");
    }
}

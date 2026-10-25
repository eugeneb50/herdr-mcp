//! Startup for `herdr-mcp`: tracing, the full serve stack (`serve_core`)
//! and the MCP stdio server (`run_serve`). Kept in the CLI crate rather than
//! moved into `herdr-mcp-server` so the server crate gains no CLI deps.

use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};

use herdr_mcp_core::Config;
use herdr_mcp_server::{HerdrClient, HerdrMcpServer, Persistence, server::start_http, triggers};

/// Tracing to stderr, or appended to `log_file` while the TUI owns the
/// terminal. Never stdout: stdout is MCP JSON-RPC when serving stdio.
pub fn init_tracing(log_file: Option<&std::path::Path>) -> Result<()> {
    let fmt = tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::from_default_env().add_directive("herdr_mcp=info".parse()?),
    );
    match log_file {
        Some(path) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            fmt.with_writer(std::sync::Mutex::new(file))
                .with_ansi(false)
                .init();
        }
        None => fmt.with_writer(std::io::stderr).init(),
    }
    Ok(())
}

/// Build the full serve stack (persistence, herdr subscriber, registry, HTTP
/// bridge; schedules and triggers once this process is the data
/// dir's daemon, see [`spawn_daemon`]) and spawn its background tasks. Does NOT block —
/// the spawned tasks live on the Tokio runtime until it shuts down. Returns
/// the one `HerdrMcpServer` every surface shares (HTTP, stdio, triggers): a
/// single scheduler and one set of live runs. [`run_serve`] awaits the MCP
/// server afterwards; the TUI mode in `main.rs` runs the TUI on the main task.
pub async fn serve_core(config: &Config) -> Result<HerdrMcpServer> {
    let data_dir = config.data_dir.clone();
    let persistence = Persistence::new(data_dir.clone());
    persistence.init().await?;
    let persistence = std::sync::Arc::new(persistence);

    let (settings, _) = config.settings();
    let herdr_client = build_herdr_client(
        &data_dir,
        config.herdr_socket_path(),
        u32::try_from(settings.number(herdr_mcp_api::settings::RECONNECT_ATTEMPTS))
            .unwrap_or(u32::MAX),
        settings.number(herdr_mcp_api::settings::RECONNECT_BACKOFF_MS),
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
        let web_dir = config.http.web_dir.clone();
        tracing::info!("HTTP playground listening on http://{bind}:{port}");
        if let Some(token) = &server.security.token {
            tracing::info!(
                "the bridge needs its token ({}): open {}",
                config.data_dir.join(herdr_mcp_api::TOKEN_FILE).display(),
                herdr_mcp_server::security::web_url(port, Some(token))
            );
        }
        tokio::spawn(async move {
            if let Err(e) = start_http(server, &bind, port, web_dir.as_deref()).await {
                tracing::error!("HTTP server failed: {e}");
            }
        });
    }

    Ok(server)
}

/// Lock file in the data dir. Its holder is the data dir's daemon: the one
/// process that fires schedules and triggers, however
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
            "daemon for {}: running schedules, triggers and run retention",
            data_dir.display()
        );
        triggers::spawn(server.clone(), herdr_client.events());
        // Runs past their retention since the last start (keep-days ages
        // runs of collections that no longer run).
        herdr_mcp_server::runs::prune_all(&server).await;
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

/// Headless serve: MCP stdio + the HTTP bridge, or the bridge alone with
/// `http.http_only`.
pub async fn run_serve(config: &Config) -> Result<()> {
    let server = serve_core(config).await?;

    if config.http.http_only {
        tracing::info!("HTTP-only mode — waiting for shutdown signal");
        tokio::signal::ctrl_c().await?;
        tracing::info!("Shutting down");
    } else {
        tracing::info!("Starting herdr-mcp MCP server");
        let service = server.serve(stdio()).await?;
        tracing::info!("herdr-mcp server initialized, waiting for requests");
        service.waiting().await?;
        tracing::info!("herdr-mcp server stopped");
    }
    tokio::task::spawn_blocking(herdr_mcp_server::clipboard::release).await?;
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

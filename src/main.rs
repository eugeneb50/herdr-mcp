#![deny(clippy::correctness)]
#![deny(clippy::suspicious)]

use anyhow::Result;
use clap::{Parser, Subcommand};
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

mod server;
mod persistence;
mod variables;
mod scheduler;
mod templates;
mod herdr_client;
mod trim;

#[derive(Parser)]
#[command(name = "herdr-mcp", version)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the herdr MCP server (stdio and/or HTTP).
    Serve {
        /// Start an HTTP server on the given port for the web playground
        #[arg(long)]
        http: Option<u16>,

        /// Run HTTP server only (skip MCP stdio transport)
        #[arg(long)]
        http_only: bool,

        /// Data directory for storing recipes and variables
        #[arg(long, default_value = "./data")]
        data_dir: std::path::PathBuf,

        /// Path to the herdr API socket. Defaults to $HERDR_SOCKET_PATH or
        /// ~/.config/herdr/herdr.sock. Used for the live agent registry.
        #[arg(long)]
        herdr_socket: Option<std::path::PathBuf>,
    },

    /// Run the message-trim pipeline on text or a file and print the result.
    Trim {
        /// Trim stages, in order (e.g. `caveman:full` then `pfc1`).
        #[arg(long = "stage", value_name = "STAGE")]
        stage: Vec<String>,

        /// Decompress mode: reverse a PFC1-compressed input instead of compressing.
        #[arg(long)]
        decompress: bool,

        /// Data directory (holds the persistent PFC1 memory).
        #[arg(long, default_value = "./data")]
        data_dir: std::path::PathBuf,

        /// Read input from a file instead of the positional argument.
        #[arg(long, value_name = "FILE")]
        file: Option<std::path::PathBuf>,

        /// Input text (trailing arguments). Ignored when `--file` is given.
        #[arg(trailing_var_arg = true, value_name = "TEXT")]
        text: Vec<String>,
    },

    /// Open a live ANSI dashboard of trim savings across all workspaces.
    Dashboard {
        /// Data directory holding the per-workspace trim stats.
        #[arg(long, default_value = "./data")]
        data_dir: std::path::PathBuf,
    },
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

    match args.command {
        Command::Trim {
            stage,
            decompress,
            data_dir,
            file,
            text,
        } => run_trim(stage, decompress, &data_dir, file, text).await,
        Command::Serve {
            http,
            http_only,
            data_dir,
            herdr_socket,
        } => run_serve(http, http_only, data_dir, herdr_socket).await,
        Command::Dashboard { data_dir } => run_dashboard(&data_dir).await,
    }
}

/// One-shot message-trim runner (CLI).
async fn run_trim(
    stage: Vec<String>,
    decompress: bool,
    data_dir: &std::path::Path,
    file: Option<std::path::PathBuf>,
    text: Vec<String>,
) -> Result<()> {
    let input = if let Some(path) = file {
        std::fs::read_to_string(&path)?
    } else {
        text.join(" ")
    };
    if input.trim().is_empty() {
        anyhow::bail!("no input: provide TEXT or --file");
    }

    let runner = trim::runner::PipelineRunner::new(data_dir).await;

    if decompress {
        let out = runner.decompress(&input);
        print!("{out}");
        return Ok(());
    }

    let stages = if stage.is_empty() {
        trim::pipeline::parse_stage_specs(&["caveman:full".to_string(), "pfc1".to_string()])
            .unwrap_or_default()
    } else {
        match trim::pipeline::parse_stage_specs(&stage) {
            Ok(s) => s,
            Err(e) => anyhow::bail!(e),
        }
    };
    let result = runner.run(&input, &stages).await;
    let report = serde_json::json!({
        "input_bytes": result.input.len(),
        "output_bytes": result.output.len(),
        "total_savings_bytes": result.total_savings_bytes,
        "total_ratio_pct": result.total_ratio,
        "stages": result.stages.iter().map(|s| serde_json::json!({
            "stage": s.stage,
            "output_len": s.output.len(),
            "skipped": s.skipped,
            "stats": s.stats,
        })).collect::<Vec<_>>(),
        "output": result.output,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Run the live trim-savings dashboard (TUI).
async fn run_dashboard(data_dir: &std::path::Path) -> Result<()> {
    trim::dashboard::run(data_dir).await
}

/// Run the herdr MCP server (stdio and/or HTTP).
async fn run_serve(
    http: Option<u16>,
    http_only: bool,
    data_dir: std::path::PathBuf,
    herdr_socket: Option<std::path::PathBuf>,
) -> Result<()> {
    let persistence = persistence::Persistence::new(data_dir.clone());
    persistence.init().await?;
    let persistence = std::sync::Arc::new(persistence);

    // Build the shared agent registry + herdr event subscriber. The registry is
    // fed live by herdr's `pane.agent_status_changed` stream so recipe steps and
    // MCP tools can address agents by role/pane id across the workspace.
    let herdr_client = build_herdr_client(&data_dir, herdr_socket);
    herdr_client.spawn_subscriber();
    let registry = herdr_client.registry.clone();

    if let Some(port) = http {
        let server = server::HerdrMcpServer::new((*persistence).clone(), registry.clone());
        tokio::spawn(async move {
            if let Err(e) = server::start_http(server, port).await {
                tracing::error!("HTTP server failed: {e}");
            }
        });
        tracing::info!("HTTP playground listening on http://localhost:{port}");
    }

    if http_only {
        tracing::info!("HTTP-only mode — waiting for shutdown signal");
        tokio::signal::ctrl_c().await?;
        tracing::info!("Shutting down");
        return Ok(());
    }

    tracing::info!("Starting herdr-mcp MCP server");

    let server = server::HerdrMcpServer::new((*persistence).clone(), registry.clone());
    let service = server.serve(stdio()).await?;

    tracing::info!("herdr-mcp server initialized, waiting for requests");

    service.waiting().await?;

    tracing::info!("herdr-mcp server stopped");

    Ok(())
}

/// Resolve the herdr socket path and construct the event-subscribing client.
fn build_herdr_client(
    data_dir: &std::path::Path,
    herdr_socket: Option<std::path::PathBuf>,
) -> std::sync::Arc<herdr_client::HerdrClient> {
    let socket_path = match herdr_socket {
        Some(p) => p,
        None => {
            let from_env = std::env::var("HERDR_SOCKET_PATH").ok().map(std::path::PathBuf::from);
            from_env.unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                std::path::PathBuf::from(home).join(".config/herdr/herdr.sock")
            })
        }
    };
    let persistence = persistence::Persistence::new(data_dir.to_path_buf());
    let client = herdr_client::HerdrClient::new(
        std::sync::Arc::new(persistence),
        socket_path,
    );
    std::sync::Arc::new(client)
}

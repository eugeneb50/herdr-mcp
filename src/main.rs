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

    /// Manage per-folder PFC1 phonetic keys (scan, list, decompress).
    FolderKey {
        #[command(subcommand)]
        action: FolderKeyAction,
    },
}

/// Subcommands for `herdr-mcp folder-key`.
#[derive(Debug, Clone, clap::Subcommand)]
enum FolderKeyAction {
    /// Scan a folder and write a self-contained PFC1 key to `<folder>/.pfc1_key.json`.
    Build {
        /// Folder to scan recursively for text/markdown files.
        folder: std::path::PathBuf,
        /// Data directory (holds the master key + central registry).
        #[arg(long, default_value = "./data")]
        data_dir: std::path::PathBuf,
        /// Minimum term frequency (default 3).
        #[arg(long)]
        min_frequency: Option<usize>,
        /// Minimum term length in bytes (default 4).
        #[arg(long)]
        min_length: Option<usize>,
        /// Maximum symbols in the key (default 85).
        #[arg(long)]
        max_terms: Option<usize>,
        /// Also write the key to the central registry.
        #[arg(long)]
        persist_central: bool,
        /// Fold accepted terms into the persistent master key (default on).
        #[arg(long, default_value_t = true)]
        learn_master: bool,
    },
    /// List all folder keys in the central registry.
    List {
        #[arg(long, default_value = "./data")]
        data_dir: std::path::PathBuf,
    },
    /// Show a folder's key (discovered by walking up the tree).
    Show {
        folder: std::path::PathBuf,
    },
    /// Decompress text using a folder's key (header-less).
    Decompress {
        folder: std::path::PathBuf,
        /// PFC1-compressed text to decode.
        text: String,
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
        Command::FolderKey { action } => run_folder_key(action).await,
    }
}

/// Dispatch for `herdr-mcp folder-key`.
async fn run_folder_key(action: FolderKeyAction) -> Result<()> {
    match action {
        FolderKeyAction::Build {
            folder,
            data_dir,
            min_frequency,
            min_length,
            max_terms,
            persist_central,
            learn_master,
        } => {
            let mut opts = crate::trim::folder_key::FolderKeyOptions::default();
            if let Some(v) = min_frequency {
                opts.min_frequency = v;
            }
            if let Some(v) = min_length {
                opts.min_length = v;
            }
            if let Some(v) = max_terms {
                opts.max_terms = v;
            }
            opts.persist_central = persist_central;
            opts.learn_master = learn_master;
            let key = crate::trim::folder_key::build_folder_key(&folder, &opts, &data_dir).await?;
            println!(
                "Built key for {} — {} terms ({} phrases, {} code terms), {} files scanned",
                folder.display(),
                key.stats.terms_accepted,
                key.stats.phrases_found,
                key.stats.code_terms,
                key.stats.files_scanned,
            );
            println!("Key file: {}", folder.join(crate::trim::folder_key::FOLDER_KEY_FILE).display());
        }
        FolderKeyAction::List { data_dir } => {
            let keys = crate::trim::folder_key::list_central_keys(&data_dir).await;
            println!("{} folder key(s) in registry:", keys.len());
            for k in &keys {
                println!("  {} — {} terms", k.folder.display(), k.key.len());
            }
        }
        FolderKeyAction::Show { folder } => {
            match crate::trim::folder_key::load_folder_key(&folder).await? {
                Some(k) => println!("{}", serde_json::to_string_pretty(&k)?),
                None => println!("No folder key found for {}", folder.display()),
            }
        }
        FolderKeyAction::Decompress { folder, text } => {
            let out = crate::trim::folder_key::decompress_with_folder_key(&folder, &text).await;
            println!("{out}");
        }
    }
    Ok(())
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
    let herdr_client = build_herdr_client(&data_dir, herdr_socket, 3, 500);
    herdr_client.spawn_subscriber();
    let registry = herdr_client.registry.clone();

    // Periodic trim-badge refresh so savings badges survive server restarts.
    server::spawn_trim_poller(std::sync::Arc::new(
        server::HerdrMcpServer::new((*persistence).clone(), registry.clone())
            .with_herdr_client(herdr_client.clone()),
    ));

    if let Some(port) = http {
        let server = server::HerdrMcpServer::new((*persistence).clone(), registry.clone())
            .with_herdr_client(herdr_client.clone());
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

    let server = server::HerdrMcpServer::new((*persistence).clone(), registry)
        .with_herdr_client(herdr_client);
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
    reconnect_attempts: u32,
    reconnect_backoff_ms: u64,
) -> std::sync::Arc<herdr_client::HerdrClient> {
    // The socket path is already resolved by the caller (run_serve) using the same
    // logic as Config::herdr_socket_path(). No need to re-check env here.
    let socket_path = herdr_socket.unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        std::path::PathBuf::from(home).join(".config/herdr/herdr.sock")
    });
    let persistence = persistence::Persistence::new(data_dir.to_path_buf());
    let client = herdr_client::HerdrClient::new(
        std::sync::Arc::new(persistence),
        socket_path,
        reconnect_attempts,
        reconnect_backoff_ms,
    );
    std::sync::Arc::new(client)
}

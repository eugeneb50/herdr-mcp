#![deny(clippy::correctness)]
#![deny(clippy::suspicious)]

use anyhow::Result;
use clap::{Parser, Subcommand};
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

use herdr_mcp_core::{CliOverrides, Config};
use herdr_mcp_server::{
    AgentRegistry, HerdrClient, HerdrMcpServer, Persistence,
    server::{spawn_trim_poller, start_http},
};
use herdr_mcp_trim::{dashboard, folder_key as fk, pipeline, runner::PipelineRunner};

#[derive(Parser)]
#[command(name = "herdr-mcp", version)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the herdr MCP server (stdio and/or HTTP).
    Serve {
        #[arg(long)]
        http: Option<u16>,
        #[arg(long)]
        http_only: bool,
        #[arg(long, default_value = "./data")]
        data_dir: std::path::PathBuf,
        #[arg(long)]
        herdr_socket: Option<std::path::PathBuf>,
    },

    /// Run the message-trim pipeline on text or a file and print the result.
    Trim {
        #[arg(long = "stage", value_name = "STAGE")]
        stage: Vec<String>,
        #[arg(long)]
        decompress: bool,
        #[arg(long, default_value = "./data")]
        data_dir: std::path::PathBuf,
        #[arg(long, value_name = "FILE")]
        file: Option<std::path::PathBuf>,
        #[arg(trailing_var_arg = true, value_name = "TEXT")]
        text: Vec<String>,
    },

    /// Open a live ANSI dashboard of trim savings across all workspaces.
    Dashboard {
        #[arg(long, default_value = "./data")]
        data_dir: std::path::PathBuf,
        /// HTTP bridge port the dashboard talks to (playground/recipe builder).
        #[arg(long, default_value_t = 7676)]
        http_port: u16,
        /// Run the legacy trim-only dashboard instead of the kitchen-sink TUI.
        #[arg(long)]
        legacy: bool,
    },

    /// Manage per-folder PFC1 phonetic keys (scan, list, decompress).
    FolderKey {
        #[command(subcommand)]
        action: FolderKeyAction,
    },
}

#[derive(Debug, Clone, clap::Subcommand)]
enum FolderKeyAction {
    Build {
        folder: std::path::PathBuf,
        #[arg(long, default_value = "./data")]
        data_dir: std::path::PathBuf,
        #[arg(long)]
        min_frequency: Option<usize>,
        #[arg(long)]
        min_length: Option<usize>,
        #[arg(long)]
        max_terms: Option<usize>,
        #[arg(long)]
        persist_central: bool,
        #[arg(long, default_value_t = true)]
        learn_master: bool,
    },
    List {
        #[arg(long, default_value = "./data")]
        data_dir: std::path::PathBuf,
    },
    Show {
        folder: std::path::PathBuf,
    },
    Decompress {
        folder: std::path::PathBuf,
        text: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("herdr_mcp=info".parse()?))
        .with_writer(std::io::stderr)
        .init();

    // Load config with CLI overrides
    let cli_overrides = match &args.command {
        Some(Command::Serve {
            http,
            http_only,
            data_dir,
            herdr_socket,
        }) => Some(CliOverrides {
            data_dir: Some(data_dir.clone()),
            http_port: *http,
            http_only: Some(*http_only),
            http_bind: None,
            herdr_socket: herdr_socket.clone(),
            log_level: None,
            trim_stages: Vec::new(),
        }),
        Some(Command::Trim { .. }) => None,
        Some(Command::Dashboard {
            data_dir,
            http_port,
            ..
        }) => Some(CliOverrides {
            data_dir: Some(data_dir.clone()),
            // Dashboard runs on top of the serve stack's HTTP bridge; MCP stdio
            // is disabled so the TUI owns the terminal (stdout stays clean for
            // the bridge even though it never prints to stdout).
            http_port: Some(*http_port),
            http_only: Some(true),
            http_bind: None,
            herdr_socket: None,
            log_level: None,
            trim_stages: Vec::new(),
        }),
        Some(Command::FolderKey { .. }) => None,
        None => Some(CliOverrides {
            data_dir: None,
            http_port: Some(7676),
            http_only: Some(false),
            http_bind: None,
            herdr_socket: None,
            log_level: None,
            trim_stages: Vec::new(),
        }),
    };

    let config = Config::load(cli_overrides)?;

    match args.command {
        None => run_serve(&config).await,
        Some(Command::Trim {
            stage,
            decompress,
            data_dir,
            file,
            text,
        }) => run_trim(stage, decompress, &data_dir, file, text).await,
        Some(Command::Serve {
            http: _,
            http_only: _,
            data_dir: _,
            herdr_socket: _,
        }) => run_serve(&config).await,
        Some(Command::Dashboard {
            data_dir,
            http_port,
            legacy,
        }) => {
            if legacy {
                dashboard::run(&data_dir).await
            } else {
                // A1: run the dashboard on top of the serve stack in-process.
                // The HTTP bridge + herdr event subscriber come up headless
                // (MCP stdio disabled), then the TUI runs on the main task and
                // reads pane state from the live AgentRegistry via the bridge.
                run_serve_headless(&config).await?;
                let opts = herdr_mcp_trim::tui::DashboardOptions {
                    data_dir,
                    http_port,
                };
                herdr_mcp_trim::tui::run(opts).await
            }
        }
        Some(Command::FolderKey { action }) => run_folder_key(action).await,
    }
}

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
            let mut opts = fk::FolderKeyOptions::default();
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
            let key = fk::build_folder_key(&folder, &opts, &data_dir).await?;
            println!(
                "Built key for {} — {} terms ({} phrases, {} code terms), {} files scanned",
                folder.display(),
                key.stats.terms_accepted,
                key.stats.phrases_found,
                key.stats.code_terms,
                key.stats.files_scanned,
            );
            println!("Key file: {}", folder.join(fk::FOLDER_KEY_FILE).display());
        }
        FolderKeyAction::List { data_dir } => {
            let keys = fk::list_central_keys(&data_dir).await;
            println!("{} folder key(s) in registry:", keys.len());
            for k in &keys {
                println!("  {} — {} terms", k.folder.display(), k.key.len());
            }
        }
        FolderKeyAction::Show { folder } => match fk::load_folder_key(&folder).await? {
            Some(k) => println!("{}", serde_json::to_string_pretty(&k)?),
            None => println!("No folder key found for {}", folder.display()),
        },
        FolderKeyAction::Decompress { folder, text } => {
            let out = fk::decompress_with_folder_key(&folder, &text).await;
            println!("{out}");
        }
    }
    Ok(())
}

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
    let runner = PipelineRunner::new(data_dir).await;
    if decompress {
        print!("{}", runner.decompress(&input));
        return Ok(());
    }
    let stages = if stage.is_empty() {
        pipeline::parse_stage_specs(&["caveman:full".to_string(), "pfc1".to_string()])
            .unwrap_or_default()
    } else {
        match pipeline::parse_stage_specs(&stage) {
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

/// Build the full serve stack (persistence, herdr subscriber, registry, HTTP
/// bridge) and spawn its background tasks. Does NOT block — the spawned tasks
/// live on the Tokio runtime until it shuts down. Returns the shared
/// `AgentRegistry` + `Persistence` so callers can build a stdio server against
/// the exact same registry the HTTP bridge uses. Shared by both `serve` and
/// `dashboard`: the former awaits the MCP/HTTP server afterwards, the latter
/// runs the TUI on the main task against the same registry.
async fn serve_core(config: &Config) -> Result<(AgentRegistry, std::sync::Arc<Persistence>)> {
    let data_dir = config.data_dir.clone();
    let persistence = Persistence::new(data_dir.clone());
    persistence.init().await?;
    let persistence = std::sync::Arc::new(persistence);

    let herdr_client = build_herdr_client(&data_dir, config.herdr.socket_path.clone());
    herdr_client.spawn_subscriber();
    let registry = herdr_client.registry.clone();

    // Periodic trim-badge refresh so savings badges survive server restarts.
    spawn_trim_poller(std::sync::Arc::new(HerdrMcpServer::with_config(
        (*persistence).clone(),
        registry.clone(),
        config,
    )));

    if let Some(port) = config.http.port {
        let server = HerdrMcpServer::with_config((*persistence).clone(), registry.clone(), config);
        tokio::spawn(async move {
            if let Err(e) = start_http(server, port).await {
                tracing::error!("HTTP server failed: {e}");
            }
        });
        tracing::info!("HTTP playground listening on http://localhost:{port}");
    }
    Ok((registry, persistence))
}

/// `serve` — full MCP server (stdio + optional HTTP).
async fn run_serve(config: &Config) -> Result<()> {
    let (registry, persistence) = serve_core(config).await?;

    if config.http.http_only {
        tracing::info!("HTTP-only mode — waiting for shutdown signal");
        tokio::signal::ctrl_c().await?;
        tracing::info!("Shutting down");
        return Ok(());
    }

    tracing::info!("Starting herdr-mcp MCP server");
    let server = HerdrMcpServer::with_config((*persistence).clone(), registry, config);
    let service = server.serve(stdio()).await?;
    tracing::info!("herdr-mcp server initialized, waiting for requests");
    service.waiting().await?;
    tracing::info!("herdr-mcp server stopped");
    Ok(())
}

/// `dashboard` (A1) — serve stack headless (MCP stdio disabled), then the TUI
/// runs on top of it. The caller runs the TUI afterwards; this only brings up
/// the HTTP bridge + herdr event subscriber.
async fn run_serve_headless(config: &Config) -> Result<()> {
    serve_core(config).await?;
    Ok(())
}

fn build_herdr_client(
    data_dir: &std::path::Path,
    herdr_socket: Option<std::path::PathBuf>,
) -> std::sync::Arc<HerdrClient> {
    let socket_path = match herdr_socket {
        Some(p) => p,
        None => {
            let from_env = std::env::var("HERDR_SOCKET_PATH")
                .ok()
                .map(std::path::PathBuf::from);
            from_env.unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                std::path::PathBuf::from(home).join(".config/herdr/herdr.sock")
            })
        }
    };
    let persistence = Persistence::new(data_dir.to_path_buf());
    let client = HerdrClient::new(std::sync::Arc::new(persistence), socket_path);
    std::sync::Arc::new(client)
}

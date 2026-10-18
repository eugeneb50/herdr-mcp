//! `herdr-trim` — local tools binary: trim runner, dashboard, folder keys.
//!
//! Offline-capable where possible (`trim` runs the pipeline locally);
//! `dashboard` is a client of the `herdr-mcp serve` HTTP bridge.

#![deny(clippy::correctness)]
#![deny(clippy::suspicious)]

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use herdr_mcp_core::{CliOverrides, Config};
use herdr_mcp_trim::{folder_key as fk, pipeline, runner::PipelineRunner};

#[allow(dead_code)]
#[path = "../common.rs"]
mod common;

#[derive(Parser)]
#[command(name = "herdr-trim", version)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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

    /// Open the dashboard (bridge-backed UI) for trim savings.
    Dashboard {
        /// Data directory (default `./data`, `HERDR_MCP_DATA_DIR`, herdmcp.toml).
        #[arg(long)]
        data_dir: Option<std::path::PathBuf>,
        /// HTTP bridge port the dashboard talks to (default: `http.port`
        /// from config).
        #[arg(long)]
        http_port: Option<u16>,
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

fn main() -> Result<()> {
    let args = Args::parse();
    common::init_tracing()?;
    if let Command::Dashboard {
        data_dir,
        http_port,
    } = &args.command
    {
        // Run the dashboard on top of the serve stack in-process.
        // The HTTP bridge + herdr event subscriber come up headless
        // (MCP stdio disabled), then the TUI runs on the main task and
        // reads pane state from the live AgentRegistry via the bridge.
        let config = Config::load(Some(CliOverrides {
            data_dir: data_dir.clone(),
            http_port: *http_port,
            http_only: Some(true),
            ..Default::default()
        }))?;
        // SAFETY: single-threaded here; the runtime starts below.
        unsafe { config.export_herdr_socket() };
        let http_port = config
            .http
            .port
            .context("no bridge port: pass --http-port or set http.port")?;
        return tokio::runtime::Runtime::new()?.block_on(async move {
            common::run_serve_headless(&config).await?;
            let opts = herdr_mcp_tui::DashboardOptions {
                data_dir: config.data_dir.clone(),
                http_port,
            };
            herdr_mcp_tui::run(opts).await
        });
    }
    tokio::runtime::Runtime::new()?.block_on(run(args.command))
}

async fn run(command: Command) -> Result<()> {
    match command {
        Command::Trim {
            stage,
            decompress,
            data_dir,
            file,
            text,
        } => run_trim(stage, decompress, &data_dir, file, text).await,
        Command::Dashboard { .. } => unreachable!("handled in main"),
        Command::FolderKey { action } => run_folder_key(action).await,
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

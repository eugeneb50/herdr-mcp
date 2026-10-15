//! `herdr-mcp` — daemon binary: MCP stdio server + optional HTTP bridge.
//!
//! Serve-only. Local tools (`trim`, `dashboard`, `folder-key`) live in the
//! sibling `herdr-trim` binary. Running `herdr-mcp trim …` is a hard break:
//! clap rejects it — use `herdr-trim`.

#![deny(clippy::correctness)]
#![deny(clippy::suspicious)]

use anyhow::Result;
use clap::{Parser, Subcommand};

use herdr_mcp_core::{CliOverrides, Config};

#[allow(dead_code)]
#[path = "../common.rs"]
mod common;

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
        /// Auto-start the HTTPS intercepting proxy on this port when set.
        #[arg(long)]
        proxy_port: Option<u16>,
        /// Tool surface: `core`, `extended`, or comma list (e.g. `core,recipes`).
        /// Currently parsed for forward-compat; tier gating lands with the
        /// tool-tier change.
        #[arg(long, default_value = "extended")]
        tools: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    common::init_tracing()?;
    config_tools_note(&args)?;

    // Load config with CLI overrides
    let cli_overrides = match &args.command {
        Some(Command::Serve {
            http,
            http_only,
            data_dir,
            herdr_socket,
            proxy_port,
            ..
        }) => Some(CliOverrides {
            data_dir: Some(data_dir.clone()),
            http_port: *http,
            http_only: Some(*http_only),
            http_bind: None,
            herdr_socket: herdr_socket.clone(),
            log_level: None,
            trim_stages: Vec::new(),
            proxy_port: *proxy_port,
        }),
        None => Some(CliOverrides {
            data_dir: None,
            http_port: Some(7676),
            http_only: Some(false),
            http_bind: None,
            herdr_socket: None,
            log_level: None,
            trim_stages: Vec::new(),
            proxy_port: None,
        }),
    };

    let config = Config::load(cli_overrides)?;

    match args.command {
        None => common::run_serve(&config).await,
        Some(Command::Serve { .. }) => common::run_serve(&config).await,
    }
}

/// Forward-compat stub: `--tools` is accepted and validated, gating follows.
fn config_tools_note(args: &Args) -> Result<()> {
    let selection = match &args.command {
        Some(Command::Serve { tools, .. }) => tools.as_str(),
        None => "extended",
    };
    if selection != "core" && selection != "extended" && !selection.starts_with("core,") {
        anyhow::bail!("--tools must be `core`, `extended`, or `core,...` (got `{selection}`)");
    }
    Ok(())
}

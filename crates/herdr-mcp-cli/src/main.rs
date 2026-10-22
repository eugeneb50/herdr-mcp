//! `herdr-mcp` — the one binary. With no subcommand it serves: the TUI
//! dashboard over this process's HTTP bridge when run in a terminal, else
//! (an MCP client launching it over pipes, or `--no-tui`) MCP stdio + HTTP.
//! `--http-only` serves the bridge alone. `plugin` is herdr-mcp as a herdr
//! plugin (see `plugin.rs`).

#![deny(clippy::correctness)]
#![deny(clippy::suspicious)]

use std::io::IsTerminal;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use herdr_mcp_core::{CliOverrides, Config};

mod common;
mod plugin;

/// Where tracing goes, in the data dir, while the TUI owns the terminal.
const LOG_FILE: &str = "herdr-mcp.log";

#[derive(Parser)]
#[command(name = "herdr-mcp", version)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Serve MCP over stdio + HTTP without the TUI (the default when stdin
    /// is not a terminal, e.g. launched by an MCP client).
    #[arg(long)]
    no_tui: bool,
    /// HTTP bridge port (default: `http.port` from config).
    #[arg(long)]
    http: Option<u16>,
    /// Serve the HTTP bridge only: no TUI, no MCP stdio.
    #[arg(long)]
    http_only: bool,
    /// Data directory (default `./data`; also `HERDR_MCP_DATA_DIR` or
    /// `data-dir` in the config file — this flag wins over both).
    #[arg(long)]
    data_dir: Option<std::path::PathBuf>,
    #[arg(long)]
    herdr_socket: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// herdr plugin: its manifest, and the entry points herdr runs.
    Plugin {
        #[command(subcommand)]
        command: plugin::PluginCommand,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(Command::Plugin { command }) = args.command {
        common::init_tracing(None)?;
        let config = Config::load(None)?;
        return tokio::runtime::Runtime::new()?.block_on(plugin::run(command, &config));
    }

    let config = Config::load(Some(CliOverrides {
        data_dir: args.data_dir,
        http_port: args.http,
        // Only a passed flag overrides config / env.
        http_only: args.http_only.then_some(true),
        herdr_socket: args.herdr_socket,
    }))?;
    // SAFETY: single-threaded here; the runtime starts below.
    unsafe { config.export_herdr_socket() };
    let tui = !config.http.http_only && !args.no_tui && std::io::stdin().is_terminal();
    let log_file = tui.then(|| config.data_dir.join(LOG_FILE));
    common::init_tracing(log_file.as_deref())?;
    let rt = tokio::runtime::Runtime::new()?;
    if !tui {
        return rt.block_on(common::run_serve(&config));
    }
    let http_port = config
        .http
        .port
        .context("the TUI needs the HTTP bridge: pass --http <port> or set http.port")?;
    rt.block_on(async move {
        common::serve_core(&config).await?;
        let opts = herdr_mcp_tui::DashboardOptions {
            data_dir: config.data_dir.clone(),
            http_port,
        };
        let res = herdr_mcp_tui::run(opts).await;
        tokio::task::spawn_blocking(herdr_mcp_server::clipboard::release).await?;
        res
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command the plugin manifest runs through this binary parses.
    #[test]
    fn manifest_commands_parse() {
        let m = plugin::manifest();
        let commands = m.actions.iter().map(|a| &a.command);
        let panes = m.panes.iter().map(|p| &p.command);
        let hooks = m.startup.iter().map(|h| &h.command);
        let events = m.events.iter().map(|e| &e.command);
        let all = commands.chain(panes).chain(hooks).chain(events);
        for argv in all.filter(|c| c[0] == plugin::EXE) {
            assert!(Args::try_parse_from(argv).is_ok(), "{argv:?}");
        }
    }
}

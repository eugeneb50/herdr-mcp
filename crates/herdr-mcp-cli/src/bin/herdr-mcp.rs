//! `herdr-mcp` — daemon binary: MCP stdio server + optional HTTP bridge,
//! and `plugin` (herdr-mcp as a herdr plugin, see `plugin.rs`).
//!
//! Local tools (`trim`, `dashboard`, `folder-key`) live in the
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
#[path = "../plugin.rs"]
mod plugin;

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
        /// Data directory (default `./data`; also `HERDR_MCP_DATA_DIR` or
        /// `data_dir` in herdmcp.toml — this flag wins over both).
        #[arg(long)]
        data_dir: Option<std::path::PathBuf>,
        #[arg(long)]
        herdr_socket: Option<std::path::PathBuf>,
    },
    /// herdr plugin: its manifest, and the entry points herdr runs.
    Plugin {
        #[command(subcommand)]
        command: plugin::PluginCommand,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();
    common::init_tracing()?;

    // Load config with CLI overrides
    if let Some(Command::Plugin { command }) = args.command {
        let config = Config::load(None)?;
        return tokio::runtime::Runtime::new()?.block_on(plugin::run(command, &config));
    }

    let cli_overrides = match &args.command {
        Some(Command::Serve {
            http,
            http_only,
            data_dir,
            herdr_socket,
        }) => Some(CliOverrides {
            data_dir: data_dir.clone(),
            http_port: *http,
            http_only: Some(*http_only),
            herdr_socket: herdr_socket.clone(),
        }),
        None => None,
        Some(Command::Plugin { .. }) => unreachable!("handled above"),
    };

    let config = Config::load(cli_overrides)?;
    // SAFETY: single-threaded here; the runtime starts below.
    unsafe { config.export_herdr_socket() };
    tokio::runtime::Runtime::new()?.block_on(common::run_serve(&config))
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

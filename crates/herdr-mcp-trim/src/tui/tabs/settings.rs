//! Settings tab: herdr-mcp runtime settings + herdr sidecar context profiler.
//!
//! Read-only view of effective config + env override sources. Keyboard: `r`
//! refresh, `q` quit (handled globally).

use std::fmt::Write;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};

use crate::tui::App;
use crate::tui::render::{bold, emerald, move_to, muted, red, truncate};

pub async fn handle_key(_app: &mut App, _code: KeyCode, _mods: KeyModifiers) -> Result<bool> {
    // Settings is currently a read-only profiler; nothing to consume locally.
    Ok(false)
}

pub fn render(f: &mut String, app: &App) {
    let row = 4u16;
    let _ = write!(f, "{}", move_to(2, row));
    let _ = write!(f, "{}   {}\r\n", bold("Settings"), muted("runtime configuration"));

    let _ = write!(f, "{}", move_to(2, row + 2));
    let _ = write!(f, "{}\r\n", bold("Runtime"));

    let _ = write!(f, "{}", move_to(2, row + 3));
    let _ = write!(f, "  HTTP port        {}\r\n", emerald(&app.settings.http_port.to_string()));
    let _ = write!(f, "{}", move_to(2, row + 4));
    let _ = write!(f, "  Data directory   {}\r\n", muted(&truncate(&app.settings.data_dir, 40)));
    let _ = write!(f, "{}", move_to(2, row + 5));
    let _ = write!(f, "  herdr socket     {}\r\n", muted(&truncate(&app.settings.herdr_socket, 40)));
    let _ = write!(f, "{}", move_to(2, row + 6));
    let herdr_bin = std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".to_string());
    let _ = write!(f, "  HERDR_BIN        {}\r\n", muted(&herdr_bin));

    let _ = write!(f, "{}", move_to(2, row + 8));
    let _ = write!(f, "{}\r\n", bold("Environment overrides"));
    let _ = write!(f, "{}", move_to(2, row + 9));
    let _ = write!(f, "  HERDR_SOCKET_PATH   {}\r\n", env_or_unset("HERDR_SOCKET_PATH"));
    let _ = write!(f, "{}", move_to(2, row + 10));
    let _ = write!(f, "  HERDR_MCP_DATA_DIR  {}\r\n", env_or_unset("HERDR_MCP_DATA_DIR"));
    let _ = write!(f, "{}", move_to(2, row + 11));
    let _ = write!(f, "  HERDR_MCP_HTTP_PORT {}\r\n", env_or_unset("HERDR_MCP_HTTP_PORT"));
    let _ = write!(f, "{}", move_to(2, row + 12));
    let _ = write!(f, "  HERDR_MCP_CONFIG    {}\r\n", env_or_unset("HERDR_MCP_CONFIG"));

    let _ = write!(f, "{}", move_to(2, row + 14));
    let _ = write!(f, "{}\r\n", bold("Herdr sidecar profiler"));
    let _ = write!(f, "{}", move_to(2, row + 15));
    match &app.herdr.workspace {
        Some(ws) => {
            let _ = write!(f, "  Current workspace: {}\r\n", emerald(ws));
        }
        None => {
            let _ = write!(f, "  Current workspace: {}\r\n", red("(unknown — not running inside herdr?)"));
        }
    }
    let _ = write!(f, "{}", move_to(2, row + 16));
    match &app.herdr.pane_id {
        Some(p) => {
            let _ = write!(f, "  Current pane:      {}\r\n", emerald(p));
        }
        None => {
            let _ = write!(f, "  Current pane:      {}\r\n", red("(unknown)"));
        }
    }
    let _ = write!(f, "{}", move_to(2, row + 17));
    let _ = write!(f, "  Panes tracked:     {}\r\n", emerald(&app.herdr.pane_count.to_string()));
}

fn env_or_unset(name: &str) -> String {
    match std::env::var(name) {
        Ok(v) => truncate(&v, 40),
        Err(_) => muted("(unset)").to_string(),
    }
}

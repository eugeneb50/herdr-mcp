//! Overview tab: brand intro + quick stats + tool index.

use std::fmt::Write;

use crate::tui::App;
use crate::tui::render::{bold, dim, emerald, fmt_bytes, move_to, muted};

pub fn render(f: &mut String, app: &App) {
    let row = 4u16;
    let _ = write!(f, "{}", move_to(2, row));
    let _ = write!(f, "{} {}\r\n", emerald("herdr-mcp"), dim("dashboard"));

    let _ = write!(f, "{}", move_to(2, row + 1));
    let _ = write!(
        f,
        "{} MCP server + HTTP bridge on port {}\r\n",
        muted("Local control surface for herdr. "),
        emerald(&app.settings.http_port.to_string()),
    );

    let _ = write!(f, "{}", move_to(2, row + 3));
    let _ = write!(f, "{}\r\n", bold("Quick stats"));

    let trim = app.trim_status.as_ref();
    let savings = trim
        .and_then(|t| t.get("workspace_savings_pct"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let net = trim
        .and_then(|t| t.get("net_saved_bytes"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let msgs = trim
        .and_then(|t| t.get("messages_trimmed"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let _ = write!(f, "{}", move_to(2, row + 4));
    let _ = write!(
        f,
        "  savings {}   net {}   {} msgs trimmed\r\n",
        emerald(&format!("{}%", savings.round() as i64)),
        muted(&fmt_bytes(net as usize)),
        muted(&msgs.to_string()),
    );

    let tool_count = app.playground.tools_list.len();
    let _ = write!(f, "{}", move_to(2, row + 6));
    let _ = write!(
        f,
        "  {} tools available   {} panes tracked\r\n",
        emerald(&tool_count.to_string()),
        emerald(&app.herdr.pane_count.to_string()),
    );

    let _ = write!(f, "{}", move_to(2, row + 9));
    let _ = write!(f, "{}\r\n", bold("Tabs"));
    let _ = write!(f, "{}", move_to(2, row + 10));
    let _ = write!(
        f,
        "  1 Overview    2 Playground (tool runner + recipe builder)\r\n",
    );
    let _ = write!(f, "{}", move_to(2, row + 11));
    let _ = write!(f, "  3 Trim        4 Variables\r\n");
    let _ = write!(f, "{}", move_to(2, row + 12));
    let _ = write!(f, "  5 Settings    herdr sidecar context\r\n");

    let _ = write!(f, "{}", move_to(2, row + 15));
    let _ = write!(f, "{}\r\n", bold("Herdr sidecar context"));
    let _ = write!(f, "{}", move_to(2, row + 16));
    match &app.herdr.workspace {
        Some(ws) => {
            let _ = write!(f, "  Workspace: {}\r\n", emerald(ws));
        }
        None => {
            let _ = write!(f, "  Workspace: {}\r\n", muted("(unknown)"));
        }
    }
    let _ = write!(f, "{}", move_to(2, row + 17));
    match &app.herdr.pane_id {
        Some(p) => {
            let _ = write!(f, "  Pane:      {}\r\n", emerald(p));
        }
        None => {
            let _ = write!(f, "  Pane:      {}\r\n", muted("(unknown)"));
        }
    }

    let _ = write!(f, "{}", move_to(2, row + 20));
    let _ = write!(
        f,
        "{} Press 1-5 to switch tabs or click the tab strip above.\r\n",
        muted("Tip:"),
    );
}

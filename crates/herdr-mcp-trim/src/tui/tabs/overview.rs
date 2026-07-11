//! Overview tab: brand intro + quick stats + tool index, in bordered panels.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use crate::tui::App;
use crate::tui::theme::{detail_line, panel_block};

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),  // intro + stats
            Constraint::Length(8),  // tabs guide
            Constraint::Min(0),     // herdr context
        ])
        .split(area);

    render_intro(frame, chunks[0], app);
    render_tabs_guide(frame, chunks[1]);
    render_sidecar(frame, chunks[2], app);
}

fn render_intro(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(" Overview ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

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
    let tool_count = app.playground.tools_list.len();

    let lines = vec![
        Line::from(format!(
            "Local control surface for herdr. MCP server + HTTP bridge on port {}",
            app.settings.http_port
        )),
        Line::from(""),
        detail_line("savings", &format!("{}%", savings.round() as i64), 12),
        detail_line("net saved", &fmt_bytes(net as usize), 12),
        detail_line("msgs trimmed", &msgs.to_string(), 12),
        detail_line("tools", &tool_count.to_string(), 12),
        detail_line("panes tracked", &app.herdr.pane_count.to_string(), 12),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_tabs_guide(frame: &mut ratatui::Frame, area: Rect) {
    let block = panel_block(" Tabs ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from("1 Overview      2 Playground (tool runner + recipe builder)"),
        Line::from("3 Trim          4 Variables"),
        Line::from("5 Settings      herdr sidecar context"),
        Line::from(""),
        Line::from("Press 1-5 to switch tabs or click the tab strip above."),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_sidecar(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(" Herdr sidecar context ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::new();
    match &app.herdr.workspace {
        Some(ws) => lines.push(detail_line("workspace", ws, 12)),
        None => lines.push(detail_line("workspace", "(unknown)", 12)),
    }
    match &app.herdr.pane_id {
        Some(p) => lines.push(detail_line("pane", p, 12)),
        None => lines.push(detail_line("pane", "(unknown)", 12)),
    }
    lines.push(detail_line("panes", &app.herdr.pane_count.to_string(), 12));
    lines.push(Line::from(""));
    lines.push(Line::from(
        "Tip: running this dashboard inside a herdr pane fills in the sidecar context.",
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Pretty-print bytes like the web UI (K/M suffixes).
pub fn fmt_bytes(n: usize) -> String {
    if n >= 1_048_576 {
        format!("{:.1}M", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{:.1}K", n as f64 / 1024.0)
    } else {
        n.to_string()
    }
}

//! Overview tab: brand intro + quick stats + pane table, in bordered panels.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Cell, Paragraph, Row, Table, Wrap};

use crate::tui::App;
use crate::tui::theme::{detail_line, focused_panel_block, panel_block};

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9), // intro + stats
            Constraint::Length(8), // tabs guide
            Constraint::Min(4),    // pane table (at least header + 3 rows)
            Constraint::Length(7), // selected pane detail
        ])
        .split(area);

    render_intro(frame, chunks[0], app);
    render_tabs_guide(frame, chunks[1]);
    render_pane_table(frame, chunks[2], app);
    render_pane_detail(frame, chunks[3], app);
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
        Line::from("5 Settings      6 Proxy"),
        Line::from(""),
        Line::from("Press 1-6 to switch tabs or click the tab strip above."),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_pane_table(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let pane_count = app.herdr.panes.len();
    let title = if pane_count == 0 {
        " Panes "
    } else {
        &format!(" Panes ({}) ", pane_count)
    };
    let block = if app.overview.focused_frame == 0 {
        focused_panel_block(title)
    } else {
        panel_block(title)
    };
    let inner = block.inner(area);
    app.pane_table_inner = inner;
    frame.render_widget(block, area);

    if pane_count == 0 {
        let mut lines = vec![
            Line::from("No herdr panes detected."),
            Line::from(""),
            Line::from("Pane list comes from the live AgentRegistry (HTTP bridge /api/agents)."),
        ];
        if let Some(err) = &app.herdr.last_error {
            lines.push(Line::from(format!("Last refresh error: {err}")));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    }

    let focused_pane_id = app.herdr.pane_id.as_deref();

    let header = Row::new(vec![
        Cell::from("Label"),
        Cell::from("Agent"),
        Cell::from("Status"),
        Cell::from("CWD"),
    ])
    .style(
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );

    let clamped = app.overview.pane_index.min(pane_count - 1);

    let rows = app.herdr.panes.iter().enumerate().map(|(i, pane)| {
        let is_focused = Some(pane.pane_id.as_str()) == focused_pane_id;
        let status_color = match pane.status.as_str() {
            "working" => Color::Green,
            "idle" => Color::DarkGray,
            "done" => Color::Cyan,
            _ => Color::Gray,
        };
        let cwd_short = trim_cwd(&pane.cwd);
        let mut label = pane.label.clone();
        if is_focused {
            label = format!("● {}", label);
        }
        let row_style = if i == clamped {
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Row::new(vec![
            Cell::from(label).style(row_style),
            Cell::from(pane.agent.clone().unwrap_or_default()).style(row_style),
            Cell::from(pane.status.clone()).style(Style::default().fg(status_color).add_modifier(
                if i == clamped {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                },
            )),
            Cell::from(cwd_short).style(row_style),
        ])
    });

    let widths = [
        Constraint::Length(20),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Min(20),
    ];

    let mut table_state = ratatui::widgets::TableState::default();
    table_state.select(Some(clamped));

    let table = Table::new(rows, widths).header(header).row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    frame.render_stateful_widget(table, inner, &mut table_state);
}

fn render_pane_detail(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(" Selected Pane ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let idx = app
        .overview
        .pane_index
        .min(app.herdr.panes.len().saturating_sub(1));
    let Some(pane) = app.herdr.panes.get(idx) else {
        frame.render_widget(
            Paragraph::new("No pane selected.").wrap(Wrap { trim: false }),
            inner,
        );
        return;
    };

    let lines = vec![
        detail_line("pane_id", &pane.pane_id, 10),
        detail_line("label", &pane.label, 10),
        detail_line("agent", pane.agent.as_deref().unwrap_or("—"), 10),
        detail_line("status", &pane.status, 10),
        detail_line("cwd", &pane.cwd, 10),
        detail_line("focused", if pane.focused { "yes" } else { "no" }, 10),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Keyboard handler for the Overview tab.
pub async fn handle_key(app: &mut App, code: KeyCode, _mods: KeyModifiers) -> Result<bool> {
    let pane_count = app.herdr.panes.len();
    // Clamp if panes were removed since last frame.
    if pane_count == 0 {
        app.overview.pane_index = 0;
    } else if app.overview.pane_index >= pane_count {
        app.overview.pane_index = pane_count - 1;
    }
    match code {
        KeyCode::Up => {
            app.overview.pane_index = app.overview.pane_index.saturating_sub(1);
            Ok(true)
        }
        KeyCode::Down => {
            if pane_count > 0 {
                app.overview.pane_index = (app.overview.pane_index + 1).min(pane_count - 1);
            }
            Ok(true)
        }
        KeyCode::Home => {
            app.overview.pane_index = 0;
            Ok(true)
        }
        KeyCode::End => {
            if pane_count > 0 {
                app.overview.pane_index = pane_count - 1;
            }
            Ok(true)
        }
        KeyCode::PageUp => {
            app.overview.pane_index = app.overview.pane_index.saturating_sub(5);
            Ok(true)
        }
        KeyCode::PageDown => {
            if pane_count > 0 {
                app.overview.pane_index = (app.overview.pane_index + 5).min(pane_count - 1);
            }
            Ok(true)
        }
        // Tab/Shift+Tab consumed (no-op for single-frame tab).
        KeyCode::Tab | KeyCode::BackTab => Ok(true),
        _ => Ok(false),
    }
}

/// Trim long absolute paths to show last 2-3 segments.
fn trim_cwd(cwd: &str) -> String {
    let parts: Vec<&str> = cwd.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 3 {
        return cwd.to_string();
    }
    let tail = &parts[parts.len() - 2..];
    format!("…/{}", tail.join("/"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{App, DashboardOptions, HerdrPane};

    fn test_app_with_panes(count: usize) -> App {
        let mut app = App::new(DashboardOptions {
            data_dir: std::path::PathBuf::from("/tmp"),
            http_port: 1,
        });
        app.herdr.panes = (0..count)
            .map(|i| HerdrPane {
                pane_id: format!("ws:p{i}"),
                label: format!("pane-{i}"),
                agent: Some(format!("agent-{i}")),
                status: "idle".into(),
                role: String::new(),
                output: String::new(),
                trim_policy: None,
                updated_at: 0,
                cwd: format!("/tmp/pane-{i}"),
                focused: i == 0,
                tab_id: None,
            })
            .collect();
        app.herdr.pane_count = count;
        app
    }

    #[test]
    fn trim_cwd_short() {
        assert_eq!(trim_cwd("/tmp"), "/tmp");
        assert_eq!(trim_cwd("/home/user"), "/home/user");
    }

    #[test]
    fn trim_cwd_long() {
        assert_eq!(
            trim_cwd("/home/producer32/code/herdr-mcp/crates/herdr-mcp-trim"),
            "…/crates/herdr-mcp-trim"
        );
        assert_eq!(trim_cwd("/a/b/c/d/e"), "…/d/e");
    }

    #[test]
    fn trim_cwd_exactly_3() {
        assert_eq!(trim_cwd("/a/b/c"), "/a/b/c");
    }

    #[test]
    fn overview_state_defaults() {
        let app = test_app_with_panes(0);
        assert_eq!(app.overview.pane_index, 0);
    }

    #[tokio::test]
    async fn overview_key_arrows() {
        let mut app = test_app_with_panes(4);
        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 1);
        handle_key(&mut app, KeyCode::Up, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 0);
    }

    #[tokio::test]
    async fn overview_key_home() {
        let mut app = test_app_with_panes(5);
        app.overview.pane_index = 3;
        handle_key(&mut app, KeyCode::Home, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 0);
    }

    #[tokio::test]
    async fn overview_key_end() {
        let mut app = test_app_with_panes(5);
        app.overview.pane_index = 0;
        handle_key(&mut app, KeyCode::End, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 4);
    }

    #[tokio::test]
    async fn overview_key_pageup() {
        let mut app = test_app_with_panes(10);
        app.overview.pane_index = 7;
        handle_key(&mut app, KeyCode::PageUp, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 2);
    }

    #[tokio::test]
    async fn overview_key_pagedown() {
        let mut app = test_app_with_panes(10);
        app.overview.pane_index = 2;
        handle_key(&mut app, KeyCode::PageDown, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 7);
    }

    #[tokio::test]
    async fn overview_key_tab_noop() {
        let mut app = test_app_with_panes(3);
        app.overview.pane_index = 1;
        handle_key(&mut app, KeyCode::Tab, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 1);
    }

    #[tokio::test]
    async fn overview_unrelated_key_not_consumed() {
        let mut app = test_app_with_panes(3);
        let consumed = handle_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE)
            .await
            .unwrap();
        assert!(!consumed);
    }

    #[tokio::test]
    async fn overview_clamp_on_empty_panes() {
        let mut app = test_app_with_panes(0);
        app.overview.pane_index = 5;
        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 0);
    }

    #[tokio::test]
    async fn overview_end_on_empty_panes() {
        let mut app = test_app_with_panes(0);
        handle_key(&mut app, KeyCode::End, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 0);
    }
}

//! Settings tab: herdr-mcp runtime settings + herdr sidecar context profiler.
//!
//! Rows are selectable (↑/↓); environment overrides render as `[✓]`/`[ ]`
//! checkboxes to distinguish set vs. unset. Read-only runtime config.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::tui::App;
use crate::tui::theme::{accent_style, dim_style, panel_block, selected_style};

pub async fn handle_key(app: &mut App, code: KeyCode, _mods: KeyModifiers) -> Result<bool> {
    const ROW_COUNT: usize = 9; // runtime(4) + env(5) rows are selectable
    match code {
        KeyCode::Up => {
            if app.settings.selected > 0 {
                app.settings.selected -= 1;
            }
            Ok(true)
        }
        KeyCode::Down => {
            if app.settings.selected + 1 < ROW_COUNT {
                app.settings.selected += 1;
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let header = Paragraph::new(Line::from(vec![
        Span::styled("Settings", accent_style().add_modifier(Modifier::BOLD)),
        Span::styled("  runtime configuration   (↑/↓ navigate)", dim_style()),
    ]));
    frame.render_widget(header, Rect::new(area.x, area.y, area.width, 1));

    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(body);

    render_runtime(frame, chunks[0], app);
    render_env_and_sidecar(frame, chunks[1], app);
}

fn row(selected: bool, label: String, value: String) -> Line<'static> {
    let style = if selected {
        selected_style()
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled(format!("{:>20} ", label), dim_style()),
        Span::styled(value, style),
    ])
    .style(style)
}

fn render_runtime(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(" Runtime ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let herdr_bin = std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".to_string());
    let lines = vec![
        row(
            app.settings.selected == 0,
            "HTTP port".into(),
            app.settings.http_port.to_string(),
        ),
        row(
            app.settings.selected == 1,
            "Data directory".into(),
            truncate(&app.settings.data_dir, 40),
        ),
        row(
            app.settings.selected == 2,
            "herdr socket".into(),
            truncate(&app.settings.herdr_socket, 40),
        ),
        row(app.settings.selected == 3, "HERDR_BIN".into(), herdr_bin),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_env_and_sidecar(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(" Environment & sidecar ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let env_rows = [
        ("HERDR_SOCKET_PATH", 4),
        ("HERDR_MCP_DATA_DIR", 5),
        ("HERDR_MCP_HTTP_PORT", 6),
        ("HERDR_MCP_CONFIG", 7),
        ("HERDR_BIN", 8),
    ];
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from("Overrides:"));
    for (name, sel) in env_rows {
        let set = std::env::var(name).is_ok();
        let mark = if set { "[✓]" } else { "[ ]" };
        let value = if set {
            truncate(&std::env::var(name).unwrap_or_default(), 30)
        } else {
            "(unset)".into()
        };
        let style = if app.settings.selected == sel {
            selected_style()
        } else {
            Style::default()
        };
        let mark_style = if set { accent_style() } else { dim_style() };
        lines.push(
            Line::from(vec![
                Span::styled(format!("{mark} "), mark_style),
                Span::styled(format!("{:>20} ", name), dim_style()),
                Span::styled(value, style),
            ])
            .style(style),
        );
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Sidecar profiler:"));
    match &app.herdr.workspace {
        Some(ws) => lines.push(row(
            app.settings.selected == 9,
            "workspace".into(),
            ws.clone(),
        )),
        None => lines.push(row(
            app.settings.selected == 9,
            "workspace".into(),
            "(unknown — not inside herdr?)".into(),
        )),
    }
    lines.push(raw_row(format!(
        "  pane:      {}",
        app.herdr.pane_id.as_deref().unwrap_or("(unknown)")
    )));
    lines.push(raw_row(format!("  panes:     {}", app.herdr.pane_count)));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn raw_row(text: String) -> Line<'static> {
    Line::from(Span::styled(text, dim_style()))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t = String::new();
    for (i, c) in s.chars().enumerate() {
        if i + 1 >= max {
            t.push('…');
            break;
        }
        t.push(c);
    }
    t
}

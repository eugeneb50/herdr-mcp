//! Trim tab: savings dashboard, per-pane bars, active policies, diagnose —
//! laid out in bordered panels.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use crate::tui::App;
use crate::tui::theme::{accent_style, dim_style, panel_block};

/// Returns `true` if the key was consumed.
pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    match code {
        KeyCode::Char('d') if mods.contains(KeyModifiers::CONTROL) => {
            if let Some(http) = app.http.clone() {
                match http.trim_diagnose().await {
                    Ok(v) => {
                        app.trim.diagnose = Some(v);
                        app.status_msg = "diagnose ok".into();
                    }
                    Err(e) => {
                        app.status_msg = format!("diagnose failed: {e}");
                    }
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_status(frame, chunks[0], app);
    render_policies(frame, chunks[1], app);
}

fn render_status(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(&format!(" {} ", "Trim Dashboard"));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    if let Some(t) = app.trim_status.as_ref() {
        let savings = t
            .get("workspace_savings_pct")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let net = t.get("net_saved_bytes").and_then(|v| v.as_u64());
        let gross = t.get("gross_saved_bytes").and_then(|v| v.as_u64());
        let total = t.get("total_input_bytes").and_then(|v| v.as_u64());
        let msgs = t.get("messages_trimmed").and_then(|v| v.as_u64());

        lines.push(Line::from(vec![
            ratatui::text::Span::styled("savings ", dim_style()),
            ratatui::text::Span::styled(format!("{}%", savings.round() as i64), accent_style()),
            ratatui::text::Span::styled(
                format!(
                    "   net {}  gross {}  input {}  {} msgs",
                    net.map(|n| fmt_bytes(n as usize)).unwrap_or_default(),
                    gross.map(|n| fmt_bytes(n as usize)).unwrap_or_default(),
                    total.map(|n| fmt_bytes(n as usize)).unwrap_or_default(),
                    msgs.map(|n| n.to_string()).unwrap_or_default()
                ),
                dim_style(),
            ),
        ]));
        lines.push(Line::from(""));

        if let Some(panes) = t.get("per_pane").and_then(|p| p.as_object()) {
            let mut entries: Vec<_> = panes
                .iter()
                .map(|(k, v)| {
                    let net = v
                        .get("net_saved_bytes")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0) as usize;
                    (k.clone(), net)
                })
                .collect();
            entries.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            let max = entries.first().map(|(_, n)| (*n).max(1)).unwrap_or(1);
            for (pane, net) in entries.iter().take(12) {
                lines.push(pane_bar(pane, *net, max));
            }
        }
    } else if let Some(s) = &app.trim_stats_file {
        lines.push(Line::from(format!(
            "file stats: savings {}%  net {}  gross {}  {} msgs",
            s.savings_pct().round() as i64,
            fmt_bytes(s.net_saved_bytes),
            fmt_bytes(s.gross_saved_bytes),
            s.messages_trimmed
        )));
        let mut panes: Vec<_> = s.per_pane.iter().collect();
        panes.sort_by_key(|(_, p)| std::cmp::Reverse(p.net_saved_bytes));
        let max = panes.first().map(|(_, p)| p.net_saved_bytes.max(1)).unwrap_or(1);
        for (pane, p) in panes.iter().take(12) {
            lines.push(pane_bar(pane, p.net_saved_bytes, max));
        }
    } else {
        lines.push(Line::from(
            "No trim stats yet. Run `herdr-mcp trim` or send a trimmed message.",
        ));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn pane_bar(pane: &str, net: usize, max: usize) -> Line<'static> {
    let w = 24usize;
    let filled = if net == 0 {
        0
    } else {
        (((net as f64 / max as f64) * w as f64).round() as usize).min(w)
    };
    let bar = "█".repeat(filled) + &"░".repeat(w - filled);
    Line::from(vec![
        ratatui::text::Span::styled(format!("{:>14} ", truncate(pane, 14)), dim_style()),
        ratatui::text::Span::styled(bar, accent_style()),
        ratatui::text::Span::styled(format!(" {}", fmt_bytes(net)), dim_style()),
    ])
}

fn render_policies(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(" Active policies ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    if let Some(t) = app.trim_status.as_ref()
        && let Some(policies) = t.get("active_policies").and_then(|p| p.as_object())
    {
        if policies.is_empty() {
                lines.push(Line::from("(none)"));
            } else {
                for (pane, stages) in policies.iter().take(8) {
                    let stage_list = match stages {
                        serde_json::Value::Array(a) => a
                            .iter()
                            .filter_map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(","),
                        _ => String::new(),
                    };
                    lines.push(Line::from(format!(
                        "{} {}",
                        truncate(pane, 14),
                        stage_list
                    )));
                }
            }
        }

    if let Some(d) = &app.trim.diagnose {
        lines.push(Line::from(""));
        lines.push(Line::from("Diagnose:"));
        let pretty = serde_json::to_string_pretty(d).unwrap_or_else(|_| d.to_string());
        for line in pretty.lines().take(8) {
            lines.push(Line::from(line.to_string()));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default()),
        inner,
    );
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

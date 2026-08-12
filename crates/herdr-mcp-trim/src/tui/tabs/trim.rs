//! Trim tab: savings dashboard + trim policy settings with stage picker.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::overview::fmt_bytes;
use crate::policy::TrimDirection;
use crate::tui::App;
use crate::tui::theme::{
    accent_style, dim_style, focused_panel_block, panel_block, selected_style,
};
use crate::tui::truncate;

pub(crate) const AVAILABLE_STAGES: &[(&str, &str)] = &[
    ("caveman:lite", "Lite style compression"),
    ("caveman:full", "Full style compression"),
    ("caveman:ultra", "Ultra style compression"),
    ("pfc1", "Phonetic dictionary (lossless)"),
];

/// Direction options for the radio selector.
const DIRECTIONS: &[TrimDirection] = &[
    TrimDirection::None,
    TrimDirection::Outbound,
    TrimDirection::OutboundWithAck,
];

pub(crate) fn direction_label(d: &TrimDirection) -> &'static str {
    match d {
        TrimDirection::None => "none",
        TrimDirection::Outbound => "outbound",
        TrimDirection::OutboundWithAck => "outbound_with_ack",
    }
}

/// Returns `true` if the key was consumed.
pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    // Stage picker overlay captures all keys when open.
    if app.trim.stage_picker_open {
        return handle_stage_picker_key(app, code).await;
    }

    // Tab/BackTab cycle frames (3 frames: dashboard=0, pane list=1, stage list=2).
    match code {
        KeyCode::Tab => {
            app.trim.focused_frame = (app.trim.focused_frame + 1) % 3;
            return Ok(true);
        }
        KeyCode::BackTab => {
            app.trim.focused_frame = (app.trim.focused_frame + 2) % 3;
            return Ok(true);
        }
        _ => {}
    }

    // Ctrl+modified shortcuts (global across frames).
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
            return Ok(true);
        }
        KeyCode::Char('s') if mods.contains(KeyModifiers::CONTROL) => {
            if let Some(http) = app.http.clone() {
                match http.trim_summary().await {
                    Ok(_) => app.status_msg = "summary notification sent".into(),
                    Err(e) => app.status_msg = format!("summary failed: {e}"),
                }
            }
            return Ok(true);
        }
        KeyCode::Char('o') if mods.contains(KeyModifiers::CONTROL) => {
            if let Some(http) = app.http.clone() {
                match http.trim_dashboard_open().await {
                    Ok(v) => {
                        let pane = v.get("pane_id").and_then(|x| x.as_str()).unwrap_or("?");
                        app.status_msg = format!("opened dashboard pane {pane}");
                    }
                    Err(e) => app.status_msg = format!("open failed: {e}"),
                }
            }
            return Ok(true);
        }
        _ => {}
    }

    // Frame-specific dispatch.
    match app.trim.focused_frame {
        0 => handle_frame_dashboard(app, code).await,
        1 => handle_frame_pane_list(app, code, mods).await,
        2 => handle_frame_stage_list(app, code, mods).await,
        _ => Ok(false),
    }
}

/// Frame 0: Dashboard (read-only display, no navigation needed).
async fn handle_frame_dashboard(_app: &mut App, _code: KeyCode) -> Result<bool> {
    Ok(false)
}

/// Frame 1: Pane list — Up/Down/Home/End/PgUp/PgDn + actions.
async fn handle_frame_pane_list(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    let pane_count = app.herdr.panes.len();
    match code {
        KeyCode::Up => {
            app.trim.pane_index = app.trim.pane_index.saturating_sub(1);
            Ok(true)
        }
        KeyCode::Down => {
            if pane_count > 0 {
                app.trim.pane_index = (app.trim.pane_index + 1).min(pane_count - 1);
            }
            Ok(true)
        }
        KeyCode::Home => {
            app.trim.pane_index = 0;
            Ok(true)
        }
        KeyCode::End => {
            if pane_count > 0 {
                app.trim.pane_index = pane_count - 1;
            }
            Ok(true)
        }
        KeyCode::PageUp => {
            app.trim.pane_index = app.trim.pane_index.saturating_sub(5);
            Ok(true)
        }
        KeyCode::PageDown => {
            if pane_count > 0 {
                app.trim.pane_index = (app.trim.pane_index + 5).min(pane_count - 1);
            }
            Ok(true)
        }
        KeyCode::Char('g') if mods.contains(KeyModifiers::CONTROL) => {
            get_policy(app).await;
            Ok(true)
        }
        KeyCode::Char('d') => {
            apply_policy(app).await;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Frame 2: Stage list — Up/Down/Home/End/PgUp/PgDn + direction + actions.
async fn handle_frame_stage_list(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    match code {
        // Move stage up (Ctrl+Up)
        KeyCode::Up if mods.contains(KeyModifiers::CONTROL) && app.trim.stage_index > 0 => {
            let idx = app.trim.stage_index;
            app.trim.stages.swap(idx, idx - 1);
            app.trim.stage_index = idx - 1;
            Ok(true)
        }
        KeyCode::Up => {
            app.trim.stage_index = app.trim.stage_index.saturating_sub(1);
            Ok(true)
        }
        // Move stage down (Ctrl+Down)
        KeyCode::Down
            if mods.contains(KeyModifiers::CONTROL)
                && app.trim.stage_index + 1 < app.trim.stages.len() =>
        {
            let idx = app.trim.stage_index;
            app.trim.stages.swap(idx, idx + 1);
            app.trim.stage_index = idx + 1;
            Ok(true)
        }
        KeyCode::Down => {
            if !app.trim.stages.is_empty() {
                app.trim.stage_index = (app.trim.stage_index + 1).min(app.trim.stages.len() - 1);
            }
            Ok(true)
        }
        KeyCode::Home => {
            app.trim.stage_index = 0;
            Ok(true)
        }
        KeyCode::End => {
            if !app.trim.stages.is_empty() {
                app.trim.stage_index = app.trim.stages.len() - 1;
            }
            Ok(true)
        }
        KeyCode::PageUp => {
            app.trim.stage_index = app.trim.stage_index.saturating_sub(5);
            Ok(true)
        }
        KeyCode::PageDown => {
            if !app.trim.stages.is_empty() {
                app.trim.stage_index = (app.trim.stage_index + 5).min(app.trim.stages.len() - 1);
            }
            Ok(true)
        }
        // Direction cycling
        KeyCode::Left => {
            let cur = DIRECTIONS
                .iter()
                .position(|d| *d == app.trim.direction)
                .unwrap_or(0);
            app.trim.direction = if cur > 0 {
                DIRECTIONS[cur - 1]
            } else {
                DIRECTIONS[DIRECTIONS.len() - 1]
            };
            Ok(true)
        }
        KeyCode::Right => {
            let cur = DIRECTIONS
                .iter()
                .position(|d| *d == app.trim.direction)
                .unwrap_or(0);
            app.trim.direction = if cur + 1 < DIRECTIONS.len() {
                DIRECTIONS[cur + 1]
            } else {
                DIRECTIONS[0]
            };
            Ok(true)
        }
        // Add stage — open picker
        KeyCode::Char('a') => {
            app.trim.stage_picker_open = true;
            app.trim.stage_picker_index = 0;
            Ok(true)
        }
        // Remove selected stage
        KeyCode::Char('r') => {
            if app.trim.stage_index < app.trim.stages.len() {
                app.trim.stages.remove(app.trim.stage_index);
            }
            if !app.trim.stages.is_empty() && app.trim.stage_index >= app.trim.stages.len() {
                app.trim.stage_index = app.trim.stages.len() - 1;
            }
            Ok(true)
        }
        KeyCode::Char('g') if mods.contains(KeyModifiers::CONTROL) => {
            get_policy(app).await;
            Ok(true)
        }
        KeyCode::Char('d') => {
            apply_policy(app).await;
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn handle_stage_picker_key(app: &mut App, code: KeyCode) -> Result<bool> {
    let count = AVAILABLE_STAGES.len();
    match code {
        KeyCode::Up => {
            app.trim.stage_picker_index = app.trim.stage_picker_index.saturating_sub(1);
        }
        KeyCode::Down => {
            if count > 0 {
                app.trim.stage_picker_index = (app.trim.stage_picker_index + 1).min(count - 1);
            }
        }
        KeyCode::Home => {
            app.trim.stage_picker_index = 0;
        }
        KeyCode::End => {
            if count > 0 {
                app.trim.stage_picker_index = count - 1;
            }
        }
        KeyCode::Enter => {
            if let Some((stage, _)) = AVAILABLE_STAGES.get(app.trim.stage_picker_index) {
                app.trim.stages.push(stage.to_string());
                app.trim.stage_index = app.trim.stages.len().saturating_sub(1);
            }
            app.trim.stage_picker_open = false;
        }
        KeyCode::Esc => {
            app.trim.stage_picker_open = false;
        }
        _ => {}
    }
    Ok(true)
}

async fn get_policy(app: &mut App) {
    let pane_count = app.herdr.panes.len();
    if pane_count > 0 {
        app.trim.pane_index = app.trim.pane_index.min(pane_count.saturating_sub(1));
    }
    let target = match app.herdr.panes.get(app.trim.pane_index) {
        Some(p) => p.pane_id.clone(),
        None => {
            app.trim.policy_msg = "no pane selected".into();
            return;
        }
    };
    let http = match &app.http {
        Some(h) => h.clone(),
        None => {
            app.trim.policy_msg = "bridge not connected".into();
            return;
        }
    };
    app.trim.policy_msg = "getting...".into();
    match http.trim_policy_get(&target).await {
        Ok(v) => {
            // The tool response is a CallToolResult wrapper; the actual data
            // lives in content[0].text as a JSON string.
            let policy = v
                .pointer("/content/0/text")
                .and_then(|t| t.as_str())
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .and_then(|inner| inner.get("policy").cloned());

            match policy {
                Some(policy) if !policy.is_null() => {
                    if let Some(stages) = policy.get("stages").and_then(|s| s.as_array()) {
                        app.trim.stages = stages
                            .iter()
                            .filter_map(|s| s.as_str().map(str::to_string))
                            .collect();
                        app.trim.stage_index = 0;
                    }
                    if let Some(dir) = policy.get("direction").and_then(|d| d.as_str()) {
                        app.trim.direction = match dir {
                            "outbound" => TrimDirection::Outbound,
                            "outbound_with_ack" => TrimDirection::OutboundWithAck,
                            _ => TrimDirection::None,
                        };
                    }
                    app.trim.policy_msg = format!("got policy: {} stages", app.trim.stages.len());
                }
                _ => {
                    app.trim.stages.clear();
                    app.trim.direction = TrimDirection::None;
                    app.trim.policy_msg = "no policy set".into();
                }
            }
        }
        Err(e) => {
            app.trim.policy_msg = format!("get failed: {e}");
        }
    }
}

async fn apply_policy(app: &mut App) {
    let pane_count = app.herdr.panes.len();
    if pane_count > 0 {
        app.trim.pane_index = app.trim.pane_index.min(pane_count.saturating_sub(1));
    }
    let target = match app.herdr.panes.get(app.trim.pane_index) {
        Some(p) => p.pane_id.clone(),
        None => {
            app.trim.policy_msg = "no pane selected".into();
            return;
        }
    };
    let http = match &app.http {
        Some(h) => h.clone(),
        None => {
            app.trim.policy_msg = "bridge not connected".into();
            return;
        }
    };
    let policy = if app.trim.stages.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!({
            "stages": app.trim.stages,
            "direction": direction_label(&app.trim.direction),
        })
    };
    app.trim.policy_msg = "applying...".into();
    match http.trim_policy_set(&target, policy).await {
        Ok(_) => {
            app.trim.policy_msg = format!(
                "applied: {} stages, {}",
                app.trim.stages.len(),
                direction_label(&app.trim.direction)
            );
        }
        Err(e) => {
            app.trim.policy_msg = format!("apply failed: {e}");
        }
    }
}

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_status(frame, chunks[0], app);
    render_trim_settings(frame, chunks[1], app);

    // Stage picker overlay (rendered on top)
    if app.trim.stage_picker_open {
        render_stage_picker(frame, area, app);
    }
}

fn render_status(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = if app.trim.focused_frame == 0 {
        focused_panel_block(" Trim Dashboard ")
    } else {
        panel_block(" Trim Dashboard ")
    };
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
            Span::styled("savings ", dim_style()),
            Span::styled(format!("{}%", savings.round() as i64), accent_style()),
            Span::styled(
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
        let max = panes
            .first()
            .map(|(_, p)| p.net_saved_bytes.max(1))
            .unwrap_or(1);
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

fn render_trim_settings(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = if app.trim.focused_frame == 1 || app.trim.focused_frame == 2 {
        focused_panel_block(" Trim Policy ")
    } else {
        panel_block(" Trim Policy ")
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    // ── Pane list ──────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        "Target pane:",
        accent_style().add_modifier(Modifier::BOLD),
    )));

    if app.herdr.panes.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no panes — start herdr)",
            dim_style(),
        )));
    } else {
        // Show up to 6 panes around the selected index
        let total = app.herdr.panes.len();
        let sel = app.trim.pane_index.min(total.saturating_sub(1));
        let visible = 6.min(total);
        let start = if sel >= visible / 2 {
            (sel - visible / 2).min(total - visible)
        } else {
            0
        };

        for i in start..start + visible {
            let p = &app.herdr.panes[i];
            let selected = i == sel;
            let marker = if selected { "> " } else { "  " };
            let label = if p.label.is_empty() {
                truncate(&p.pane_id, 16)
            } else {
                truncate(&p.label, 16)
            };
            let agent = p.agent.as_deref().unwrap_or("—");
            let status = &p.status;

            // Policy badge: try pane.trim_policy first, fall back to
            // trim_status.active_policies for panes from the CLI fallback path.
            let policy_stages = p
                .trim_policy
                .as_ref()
                .map(|tp| tp.stages.join(","))
                .or_else(|| {
                    app.trim_status
                        .as_ref()
                        .and_then(|t| t.get("active_policies"))
                        .and_then(|p| p.as_object())
                        .and_then(|pol| pol.get(&p.pane_id))
                        .and_then(|s| s.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(",")
                        })
                })
                .unwrap_or_default();

            let style = if selected {
                selected_style()
            } else {
                Style::default()
            };

            lines.push(Line::from(vec![
                Span::styled(
                    marker,
                    if selected {
                        accent_style()
                    } else {
                        dim_style()
                    },
                ),
                Span::styled(format!("{:<14}", label), style),
                Span::styled(format!(" {:<8}", agent), dim_style()),
                Span::styled(format!(" {:<8}", status), status_style(status)),
                Span::styled(
                    if policy_stages.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", truncate(&policy_stages, 20))
                    },
                    dim_style(),
                ),
            ]));
        }
        if total > visible {
            lines.push(Line::from(Span::styled(
                format!("  ... {}/{} panes", sel + 1, total),
                dim_style(),
            )));
        }
    }

    lines.push(Line::from(""));

    // ── Pipeline stages ────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        "Pipeline stages:",
        accent_style().add_modifier(Modifier::BOLD),
    )));

    if app.trim.stages.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (empty — press [a] to add a stage)",
            dim_style(),
        )));
    } else {
        for (i, stage) in app.trim.stages.iter().enumerate() {
            let selected = i == app.trim.stage_index;
            let marker = if selected { "● " } else { "○ " };
            let style = if selected {
                selected_style()
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {}", marker),
                    if selected {
                        accent_style()
                    } else {
                        dim_style()
                    },
                ),
                Span::styled(stage.clone(), style),
            ]));
        }
    }

    lines.push(Line::from(""));

    // ── Direction selector ─────────────────────────────────────
    lines.push(Line::from(Span::styled(
        "Direction:",
        accent_style().add_modifier(Modifier::BOLD),
    )));
    let dir_line: Vec<Span> = DIRECTIONS
        .iter()
        .flat_map(|d| {
            let active = *d == app.trim.direction;
            let marker = if active { "● " } else { "○ " };
            let style = if active {
                selected_style()
            } else {
                dim_style()
            };
            vec![
                Span::styled(format!("{}{} ", marker, direction_label(d)), style),
                Span::raw("  "),
            ]
        })
        .collect();
    lines.push(Line::from(dir_line));

    lines.push(Line::from(""));

    // ── Action buttons ─────────────────────────────────────────
    let btn_line = Line::from(vec![
        Span::styled(" [g] Get  ", accent_style()),
        Span::styled(" [d] Apply  ", accent_style()),
        Span::styled(" [a] Add stage  [r] Remove  ", dim_style()),
        Span::styled(" [^↑/^↓] Reorder", dim_style()),
    ]);
    lines.push(btn_line);

    // ── Status message ─────────────────────────────────────────
    if !app.trim.policy_msg.is_empty() {
        let msg_style = if app.trim.policy_msg.contains("failed") {
            Style::default().fg(ratatui::style::Color::Red)
        } else if app.trim.policy_msg.starts_with("applied")
            || app.trim.policy_msg.starts_with("got")
        {
            accent_style()
        } else {
            dim_style()
        };
        lines.push(Line::from(Span::styled(
            format!("  {}", app.trim.policy_msg),
            msg_style,
        )));
    }

    // ── Diagnose output ────────────────────────────────────────
    if let Some(d) = &app.trim.diagnose {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Diagnose:",
            dim_style().add_modifier(Modifier::BOLD),
        )));
        let pretty = serde_json::to_string_pretty(d).unwrap_or_else(|_| d.to_string());
        for line in pretty.lines().take(6) {
            lines.push(Line::from(Span::styled(line.to_string(), dim_style())));
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_stage_picker(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    // Center the picker overlay
    let picker_width = 40.min(area.width.saturating_sub(4));
    let picker_height = (AVAILABLE_STAGES.len() as u16) + 4; // title + items + border
    let x = area.x + (area.width.saturating_sub(picker_width)) / 2;
    let y = area.y + (area.height.saturating_sub(picker_height)) / 2;
    let picker_area = Rect::new(x, y, picker_width, picker_height);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Add Stage ",
            accent_style().add_modifier(Modifier::BOLD),
        ))
        .border_style(accent_style());
    let inner = block.inner(picker_area);
    frame.render_widget(block, picker_area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, (stage, desc)) in AVAILABLE_STAGES.iter().enumerate() {
        let selected = i == app.trim.stage_picker_index;
        let marker = if selected { "▸ " } else { "  " };
        let style = if selected {
            selected_style()
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(
                marker,
                if selected {
                    accent_style()
                } else {
                    dim_style()
                },
            ),
            Span::styled(format!("{:<18}", stage), style),
            Span::styled(truncate(desc, 18), dim_style()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter to add, Esc to cancel",
        dim_style(),
    )));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn status_style(status: &str) -> Style {
    match status {
        "working" => accent_style().add_modifier(Modifier::BOLD),
        "idle" => dim_style(),
        "done" => Style::default().fg(ratatui::style::Color::Yellow),
        _ => Style::default().fg(ratatui::style::Color::DarkGray),
    }
}

fn pane_bar(pane: &str, net: usize, max: usize) -> Line<'static> {
    let w = 24usize;
    let filled = if net == 0 {
        0
    } else {
        (((net as f64 / max as f64) * w as f64).round() as usize).min(w)
    };
    let bar = "\u{2588}".repeat(filled) + &"\u{2591}".repeat(w - filled);
    Line::from(vec![
        Span::styled(format!("{:>14} ", truncate(pane, 14)), dim_style()),
        Span::styled(bar, accent_style()),
        Span::styled(format!(" {}", fmt_bytes(net)), dim_style()),
    ])
}

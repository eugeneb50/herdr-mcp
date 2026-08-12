//! Proxy tab: HTTPS intercepting proxy status, CA management, per-pane policies.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::trim::AVAILABLE_STAGES;
use crate::tui::App;
use crate::tui::theme::{
    accent_style, dim_style, focused_panel_block, panel_block, selected_style,
};
use crate::tui::truncate;

/// Returns `true` if the key was consumed.
pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    if app.proxy.stage_picker_open {
        return handle_stage_picker_key(app, code).await;
    }

    match code {
        KeyCode::Tab => {
            app.proxy.focused_frame = (app.proxy.focused_frame + 1) % 3;
            return Ok(true);
        }
        KeyCode::BackTab => {
            app.proxy.focused_frame = (app.proxy.focused_frame + 2) % 3;
            return Ok(true);
        }
        _ => {}
    }

    match code {
        KeyCode::Char('d') if mods.contains(KeyModifiers::CONTROL) => {
            if let Some(http) = app.http.clone() {
                match http.proxy_diagnose().await {
                    Ok(v) => {
                        app.proxy.diagnose = Some(v);
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
                match http.proxy_startup(None).await {
                    Ok(v) => {
                        app.proxy.startup_result = Some(v);
                        app.status_msg = "proxy started".into();
                    }
                    Err(e) => {
                        app.status_msg = format!("startup failed: {e}");
                    }
                }
            }
            return Ok(true);
        }
        _ => {}
    }

    match app.proxy.focused_frame {
        0 => handle_frame_diagnose(app, code).await,
        1 => handle_frame_pane_list(app, code, mods).await,
        2 => handle_frame_editor(app, code, mods).await,
        _ => Ok(false),
    }
}

async fn handle_frame_diagnose(_app: &mut App, _code: KeyCode) -> Result<bool> {
    Ok(false)
}

async fn handle_frame_pane_list(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    let pane_count = app.herdr.panes.len();
    match code {
        KeyCode::Up => {
            app.proxy.pane_index = app.proxy.pane_index.saturating_sub(1);
            Ok(true)
        }
        KeyCode::Down => {
            if pane_count > 0 {
                app.proxy.pane_index = (app.proxy.pane_index + 1).min(pane_count - 1);
            }
            Ok(true)
        }
        KeyCode::Home => {
            app.proxy.pane_index = 0;
            Ok(true)
        }
        KeyCode::End => {
            if pane_count > 0 {
                app.proxy.pane_index = pane_count - 1;
            }
            Ok(true)
        }
        KeyCode::PageUp => {
            app.proxy.pane_index = app.proxy.pane_index.saturating_sub(5);
            Ok(true)
        }
        KeyCode::PageDown => {
            if pane_count > 0 {
                app.proxy.pane_index = (app.proxy.pane_index + 5).min(pane_count - 1);
            }
            Ok(true)
        }
        KeyCode::Char('g') if mods.contains(KeyModifiers::CONTROL) => {
            get_policy(app).await;
            Ok(true)
        }
        KeyCode::Char('a') if mods.contains(KeyModifiers::CONTROL) => {
            apply_policy(app).await;
            Ok(true)
        }
        KeyCode::Enter => {
            app.proxy.editing = !app.proxy.editing;
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn handle_frame_editor(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    match code {
        KeyCode::Left => {
            let cur = if app.proxy.trim_outbound { 1 } else { 0 };
            app.proxy.trim_outbound = cur == 1;
            Ok(true)
        }
        KeyCode::Right => {
            let cur = if app.proxy.trim_inbound { 1 } else { 0 };
            app.proxy.trim_inbound = cur == 1;
            Ok(true)
        }
        KeyCode::Char('a') if mods.contains(KeyModifiers::CONTROL) => {
            apply_policy(app).await;
            Ok(true)
        }
        KeyCode::Char('a') => {
            app.proxy.stage_picker_open = true;
            app.proxy.stage_picker_index = 0;
            Ok(true)
        }
        KeyCode::Char('r') => {
            if app.proxy.stage_index < app.proxy.stages.len() {
                app.proxy.stages.remove(app.proxy.stage_index);
            }
            if !app.proxy.stages.is_empty() && app.proxy.stage_index >= app.proxy.stages.len() {
                app.proxy.stage_index = app.proxy.stages.len() - 1;
            }
            Ok(true)
        }
        KeyCode::Char('g') if mods.contains(KeyModifiers::CONTROL) => {
            get_policy(app).await;
            Ok(true)
        }
        KeyCode::Up if app.proxy.stage_index > 0 => {
            app.proxy.stage_index -= 1;
            Ok(true)
        }
        KeyCode::Down => {
            if !app.proxy.stages.is_empty() {
                app.proxy.stage_index = (app.proxy.stage_index + 1).min(app.proxy.stages.len() - 1);
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn handle_stage_picker_key(app: &mut App, code: KeyCode) -> Result<bool> {
    let count = AVAILABLE_STAGES.len();
    match code {
        KeyCode::Up => {
            app.proxy.stage_picker_index = app.proxy.stage_picker_index.saturating_sub(1);
        }
        KeyCode::Down => {
            if count > 0 {
                app.proxy.stage_picker_index = (app.proxy.stage_picker_index + 1).min(count - 1);
            }
        }
        KeyCode::Home => {
            app.proxy.stage_picker_index = 0;
        }
        KeyCode::End => {
            if count > 0 {
                app.proxy.stage_picker_index = count - 1;
            }
        }
        KeyCode::Enter => {
            if let Some((stage, _)) = AVAILABLE_STAGES.get(app.proxy.stage_picker_index) {
                app.proxy.stages.push(stage.to_string());
                app.proxy.stage_index = app.proxy.stages.len().saturating_sub(1);
            }
            app.proxy.stage_picker_open = false;
        }
        KeyCode::Esc => {
            app.proxy.stage_picker_open = false;
        }
        _ => {}
    }
    Ok(true)
}

async fn get_policy(app: &mut App) {
    let pane_count = app.herdr.panes.len();
    if pane_count > 0 {
        app.proxy.pane_index = app.proxy.pane_index.min(pane_count.saturating_sub(1));
    }
    let target = match app.herdr.panes.get(app.proxy.pane_index) {
        Some(p) => p.pane_id.clone(),
        None => {
            app.proxy.policy_msg = "no pane selected".into();
            return;
        }
    };
    let http = match &app.http {
        Some(h) => h.clone(),
        None => {
            app.proxy.policy_msg = "bridge not connected".into();
            return;
        }
    };
    app.proxy.policy_msg = "getting...".into();
    match http.proxy_policy_get(&target).await {
        Ok(v) => {
            let policy = v
                .pointer("/content/0/text")
                .and_then(|t| t.as_str())
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .and_then(|inner| inner.get("policy").cloned());
            match policy {
                Some(policy) if !policy.is_null() => {
                    if let Some(stages) = policy.get("stages").and_then(|s| s.as_array()) {
                        app.proxy.stages = stages
                            .iter()
                            .filter_map(|s| s.as_str().map(str::to_string))
                            .collect();
                        app.proxy.stage_index = 0;
                    }
                    app.proxy.trim_outbound = policy
                        .get("trim_outbound")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    app.proxy.trim_inbound = policy
                        .get("trim_inbound")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    app.proxy.policy_msg = format!("got policy: {} stages", app.proxy.stages.len());
                }
                _ => {
                    app.proxy.stages.clear();
                    app.proxy.trim_outbound = false;
                    app.proxy.trim_inbound = false;
                    app.proxy.policy_msg = "no policy set".into();
                }
            }
        }
        Err(e) => {
            app.proxy.policy_msg = format!("get failed: {e}");
        }
    }
}

async fn apply_policy(app: &mut App) {
    let pane_count = app.herdr.panes.len();
    if pane_count > 0 {
        app.proxy.pane_index = app.proxy.pane_index.min(pane_count.saturating_sub(1));
    }
    let target = match app.herdr.panes.get(app.proxy.pane_index) {
        Some(p) => p.pane_id.clone(),
        None => {
            app.proxy.policy_msg = "no pane selected".into();
            return;
        }
    };
    let http = match &app.http {
        Some(h) => h.clone(),
        None => {
            app.proxy.policy_msg = "bridge not connected".into();
            return;
        }
    };
    let _policy = serde_json::json!({
        "stages": app.proxy.stages,
        "trim_outbound": app.proxy.trim_outbound,
        "trim_inbound": app.proxy.trim_inbound,
    });
    app.proxy.policy_msg = "applying...".into();
    match http
        .proxy_policy_set(
            &target,
            app.proxy.trim_outbound,
            app.proxy.trim_inbound,
            &app.proxy.stages,
        )
        .await
    {
        Ok(_) => {
            app.proxy.policy_msg = format!("applied: {} stages", app.proxy.stages.len());
        }
        Err(e) => {
            app.proxy.policy_msg = format!("apply failed: {e}");
        }
    }
}

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_diagnose(frame, chunks[0], app);
    render_policy_panel(frame, chunks[1], app);

    if app.proxy.stage_picker_open {
        render_stage_picker(frame, area, app);
    }
}

fn render_diagnose(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = if app.proxy.focused_frame == 0 {
        focused_panel_block(" Proxy Diagnostics ")
    } else {
        panel_block(" Proxy Diagnostics ")
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    if let Some(d) = &app.proxy.diagnose {
        let ca = d.get("ca");
        let ca_exists = ca
            .and_then(|c| c.get("exists").and_then(|v| v.as_bool()))
            .unwrap_or(false);
        let ca_fp = ca
            .and_then(|c| c.get("fingerprint").and_then(|f| f.as_str()))
            .unwrap_or("");

        lines.push(Line::from(Span::styled(
            if ca_exists {
                "CA: present"
            } else {
                "CA: missing"
            },
            if ca_exists {
                accent_style()
            } else {
                Style::default().fg(ratatui::style::Color::Red)
            },
        )));
        if !ca_fp.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  fingerprint: {}", truncate(ca_fp, 20)),
                dim_style(),
            )));
        }

        let default_cfg = d.get("default_config");
        if let Some(cfg) = default_cfg {
            let port = cfg.get("port").and_then(|v| v.as_u64()).unwrap_or(0);
            let bind = cfg.get("bind_addr").and_then(|v| v.as_str()).unwrap_or("");
            lines.push(Line::from(Span::styled(
                format!("  bind: {}:{port}", bind, port = port),
                dim_style(),
            )));
            let trim_o = cfg
                .get("trim_outbound")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let trim_i = cfg
                .get("trim_inbound")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            lines.push(Line::from(Span::styled(
                format!("  default trim: outbound={trim_o}, inbound={trim_i}"),
                dim_style(),
            )));
        }

        lines.push(Line::from(""));

        if let Some(targets) = d
            .get("ca")
            .and_then(|c| c.get("target_hosts").and_then(|t| t.as_array()))
        {
            lines.push(Line::from(Span::styled("Target hosts:", accent_style())));
            for t in targets {
                if let Some(s) = t.as_str() {
                    lines.push(Line::from(Span::styled(format!("  - {s}"), dim_style())));
                }
            }
        }

        let policies = d.get("active_pane_policies");
        if let Some(plist) = policies.and_then(|p| p.as_array()) {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Active policies ({})", plist.len()),
                accent_style(),
            )));
            for p in plist.iter().take(8) {
                let pid = p.get("pane_id").and_then(|v| v.as_str()).unwrap_or("?");
                let label = p.get("label").and_then(|v| v.as_str()).unwrap_or("");
                let o = p
                    .get("trim_outbound")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let i = p
                    .get("trim_inbound")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let stages = p
                    .get("stages")
                    .and_then(|s| s.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                lines.push(Line::from(Span::styled(
                    format!("  {label} ({pid}): out={o} in={i} [{stages}]"),
                    dim_style(),
                )));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            "No diagnose data yet. Press Ctrl+D to fetch.",
            dim_style(),
        )));
    }

    if let Some(s) = &app.proxy.startup_result {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Startup:",
            accent_style().add_modifier(Modifier::BOLD),
        )));
        let bind = s.get("bind").and_then(|v| v.as_str()).unwrap_or("");
        let fp = s
            .get("ca_fingerprint")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        lines.push(Line::from(Span::styled(
            format!("  bind: {bind}"),
            dim_style(),
        )));
        lines.push(Line::from(Span::styled(
            format!("  CA fingerprint: {fp}"),
            dim_style(),
        )));
    }

    lines.push(Line::from(""));
    let btn_line = Line::from(vec![
        Span::styled(" [Ctrl+D] Diagnose  ", accent_style()),
        Span::styled(" [Ctrl+S] Start proxy ", accent_style()),
    ]);
    lines.push(btn_line);

    if !app.proxy.policy_msg.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {}", app.proxy.policy_msg),
            dim_style(),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_policy_panel(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let block = if app.proxy.focused_frame == 1 || app.proxy.focused_frame == 2 {
        focused_panel_block(" Proxy Policies ")
    } else {
        panel_block(" Proxy Policies ")
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let pane_count = app.herdr.panes.len();
    let mut lines: Vec<Line> = Vec::new();

    // ── Pane list ────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        "Target pane:",
        accent_style().add_modifier(Modifier::BOLD),
    )));

    if pane_count == 0 {
        lines.push(Line::from(Span::styled(
            "  (no panes — start herdr)",
            dim_style(),
        )));
    } else {
        let sel = app.proxy.pane_index.min(pane_count.saturating_sub(1));
        let visible = 6.min(pane_count);
        let start = if sel >= visible / 2 {
            (sel - visible / 2).min(pane_count - visible)
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
            ]));
        }
        if pane_count > visible {
            lines.push(Line::from(Span::styled(
                format!("  ... {}/{} panes", sel + 1, pane_count),
                dim_style(),
            )));
        }
    }

    lines.push(Line::from(""));

    // ── Policy editor ────────────────────────────────────
    if app.proxy.editing {
        lines.push(Line::from(Span::styled(
            "Policy editor (Ctrl+A to apply):",
            accent_style().add_modifier(Modifier::BOLD),
        )));

        let o_marker = if app.proxy.trim_outbound {
            "●"
        } else {
            "○"
        };
        let i_marker = if app.proxy.trim_inbound { "●" } else { "○" };
        lines.push(Line::from(Span::styled(
            format!("  trim_outbound: {}", o_marker),
            if app.proxy.trim_outbound {
                selected_style()
            } else {
                dim_style()
            },
        )));
        lines.push(Line::from(Span::styled(
            format!("  trim_inbound:  {}", i_marker),
            if app.proxy.trim_inbound {
                selected_style()
            } else {
                dim_style()
            },
        )));

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Stages:", accent_style())));
        if app.proxy.stages.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (empty — press [a] to add)",
                dim_style(),
            )));
        } else {
            for (i, stage) in app.proxy.stages.iter().enumerate() {
                let selected = i == app.proxy.stage_index;
                let marker = if selected { "● " } else { "○ " };
                let style = if selected {
                    selected_style()
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {marker}"), style),
                    Span::styled(stage.clone(), style),
                ]));
            }
        }

        lines.push(Line::from(""));
        let btn_line = Line::from(vec![
            Span::styled(" [g] Get  ", accent_style()),
            Span::styled(" [a] Apply  ", accent_style()),
            Span::styled(" [a] Add stage  [r] Remove  ", dim_style()),
        ]);
        lines.push(btn_line);
    } else {
        lines.push(Line::from(Span::styled(
            "Press Enter to edit policy",
            dim_style(),
        )));
    }

    // ── Status message ───────────────────────────────────
    if !app.proxy.policy_msg.is_empty() {
        let msg_style = if app.proxy.policy_msg.contains("failed") {
            Style::default().fg(ratatui::style::Color::Red)
        } else if app.proxy.policy_msg.starts_with("applied")
            || app.proxy.policy_msg.starts_with("got")
        {
            accent_style()
        } else {
            dim_style()
        };
        lines.push(Line::from(Span::styled(
            format!("  {}", app.proxy.policy_msg),
            msg_style,
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_stage_picker(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let picker_width = 40.min(area.width.saturating_sub(4));
    let picker_height = (AVAILABLE_STAGES.len() as u16) + 4;
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
        let selected = i == app.proxy.stage_picker_index;
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

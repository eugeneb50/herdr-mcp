//! Playground tab: tool runner (per-field form) + recipe builder.
//!
//! Runner sub-tab lays out a tool list (left) and a per-field parameter form
//! (right). Each tool `inputSchema` property becomes one editable field;
//! `↑/↓` move focus, typing edits text/number fields, `Space` toggles
//! booleans, `←/→` cycle enums, `Enter` runs the tool.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use serde_json::{Map, Value};

use crate::tui::{
    App, FieldKind, PlaygroundSub, ToolField,
};
use crate::tui::theme::{accent_style, dim_style, panel_block, selected_style, warn_style};

/// Returns `true` if the key was consumed (don't run global handler).
pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    match app.playground.sub_tab {
        PlaygroundSub::Runner => handle_runner(app, code, mods).await,
        PlaygroundSub::Builder => handle_builder(app, code, mods).await,
    }
}

async fn handle_runner(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    // Plain printable characters must never be swallowed as hotkeys, so every
    // command key here requires Ctrl (or is a navigation/control key).
    let pg = &mut app.playground;
    let plain = !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT);
    match code {
        KeyCode::Char('s') if mods.contains(KeyModifiers::CONTROL) => {
            pg.sub_tab = PlaygroundSub::Runner;
            Ok(true)
        }
        KeyCode::Char('b') if mods.contains(KeyModifiers::CONTROL) => {
            pg.sub_tab = PlaygroundSub::Builder;
            Ok(true)
        }
        KeyCode::Up if !pg.editing_field => {
            if pg.field_focus > 0 {
                pg.field_focus -= 1;
            }
            Ok(true)
        }
        KeyCode::Down if !pg.editing_field => {
            if pg.field_focus + 1 < pg.fields.len() {
                pg.field_focus += 1;
            }
            Ok(true)
        }
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => {
            // Reset all fields to their defaults.
            pg.field_values = pg.fields.iter().map(|f| f.default.clone().unwrap_or_default()).collect();
            pg.editing_field = false;
            pg.error = None;
            Ok(true)
        }
        KeyCode::Esc => {
            pg.editing_field = false;
            Ok(true)
        }
        KeyCode::Enter => {
            if pg.editing_field {
                pg.editing_field = false;
            } else {
                run_tool(app).await;
            }
            Ok(true)
        }
        KeyCode::Char(' ') if !pg.editing_field => {
            let i = pg.field_focus;
            if let Some(f) = pg.fields.get(i)
                && matches!(f.kind, FieldKind::Boolean)
            {
                let cur = pg.field_values[i].trim() == "true";
                pg.field_values[i] = if cur { "false" } else { "true" }.to_string();
            }
            Ok(true)
        }
        KeyCode::Left | KeyCode::Right if !pg.editing_field => {
            let i = pg.field_focus;
            if let Some(f) = pg.fields.get(i)
                && let Some(variants) = &f.enum_variants
                && !variants.is_empty()
            {
                let cur = pg.field_values[i].clone();
                let pos = variants.iter().position(|v| v == &cur).unwrap_or(0);
                let next = if code == KeyCode::Right {
                    (pos + 1) % variants.len()
                } else {
                    (pos + variants.len() - 1) % variants.len()
                };
                pg.field_values[i] = variants[next].clone();
            }
            Ok(true)
        }
        KeyCode::Backspace if pg.editing_field => {
            let i = pg.field_focus;
            if let Some(v) = pg.field_values.get_mut(i) {
                v.pop();
            }
            Ok(true)
        }
        // Only insert a character when no Ctrl/Alt modifier is held, otherwise
        // the key is a (global) hotkey and must not be typed into the field.
        KeyCode::Char(c) if pg.editing_field && plain => {
            let i = pg.field_focus;
            if let Some(v) = pg.field_values.get_mut(i) {
                v.push(c);
            }
            Ok(true)
        }
        KeyCode::Char(c) if !pg.editing_field && plain => {
            // Start editing text/number fields by typing.
            let i = pg.field_focus;
            if let Some(f) = pg.fields.get(i)
                && matches!(f.kind, FieldKind::Text | FieldKind::Number)
            {
                pg.editing_field = true;
                let v = if pg.field_values[i].is_empty() {
                    String::new()
                } else {
                    pg.field_values[i].clone()
                };
                pg.field_values[i] = v;
                pg.field_values[i].push(c);
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn handle_builder(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    match code {
        KeyCode::Char('s') if mods.contains(KeyModifiers::CONTROL) => {
            app.playground.sub_tab = PlaygroundSub::Runner;
            Ok(true)
        }
        KeyCode::Char('b') if mods.contains(KeyModifiers::CONTROL) => {
            app.playground.sub_tab = PlaygroundSub::Builder;
            Ok(true)
        }
        KeyCode::Enter => {
            let recipes = app.recipes.clone();
            if let Some(Value::Array(arr)) = recipes
                && let Some(first_id) = arr.first().and_then(|r| r.get("id")).and_then(|v| v.as_str())
                && let Some(http) = app.http.clone()
            {
                match http.run_recipe_by_id(first_id).await {
                    Ok(v) => {
                        app.playground.result = Some(v);
                        app.status_msg = format!("ran recipe {first_id}");
                    }
                    Err(e) => app.status_msg = format!("run failed: {e}"),
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn run_tool(app: &mut App) {
    let name = match app
        .playground
        .tools_list
        .get(app.playground.tool_index)
        .map(|(n, _)| n.clone())
    {
        Some(n) => n,
        None => {
            app.playground.error = Some("no tool selected".into());
            return;
        }
    };

    let mut map: Map<String, Value> = Map::new();
    for (f, v) in app
        .playground
        .fields
        .iter()
        .zip(app.playground.field_values.iter())
    {
        if v.is_empty() && f.default.is_none() {
            continue;
        }
        let val = match f.kind {
            FieldKind::Boolean => Value::Bool(v.trim() == "true"),
            FieldKind::Number => match v.parse::<f64>() {
                Ok(n) => Value::Number(
                    serde_json::Number::from_f64(n).unwrap_or(serde_json::Number::from(0)),
                ),
                Err(_) => continue,
            },
            _ => Value::String(v.clone()),
        };
        map.insert(f.name.clone(), val);
    }
    let params = Value::Object(map);

    let Some(http) = app.http.clone() else {
        app.playground.error = Some("HTTP bridge not reachable".into());
        return;
    };
    app.status_msg = format!("running {name}…");
    match http.call_tool(&name, params).await {
        Ok(v) => {
            app.playground.result = Some(v);
            app.playground.error = None;
            app.status_msg = format!("{name} ok");
        }
        Err(e) => {
            app.playground.error = Some(e.to_string());
            app.playground.result = None;
            app.status_msg = format!("{name} failed");
        }
    }
}

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    app.sync_tool_fields();

    let conn = if app.bridge_connected {
        Span::styled(
            format!("  connected :{}  •  {} tools", app.opts.http_port, app.playground.tools_list.len()),
            accent_style(),
        )
    } else {
        Span::styled("  not connected", warn_style())
    };
    let header = Paragraph::new(Line::from(vec![
        Span::styled("Playground", accent_style().add_modifier(Modifier::BOLD)),
        Span::styled(
            match app.playground.sub_tab {
                PlaygroundSub::Runner => "  Runner",
                PlaygroundSub::Builder => "  Builder",
            },
            dim_style(),
        ),
        Span::styled("   (Ctrl+s runner  Ctrl+b builder)", dim_style()),
        conn,
    ]));
    frame.render_widget(header, Rect::new(area.x, area.y, area.width, 1));

    let body = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1));
    match app.playground.sub_tab {
        PlaygroundSub::Runner => render_runner(frame, body, app),
        PlaygroundSub::Builder => render_builder(frame, body, app),
    }
}

fn render_runner(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    render_tool_list(frame, chunks[0], app);
    render_form_and_result(frame, chunks[1], app);
}

fn render_tool_list(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let block = panel_block(" Tools ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !app.bridge_connected {
        let msg = vec![
            Line::from("Not connected to herdr-mcp bridge."),
            Line::from(""),
            Line::from("Start `herdr-mcp serve` (or run"),
            Line::from("`herdr-mcp dashboard --http-port <port>`)."),
            Line::from(""),
            Line::from(format!(
                "Auto-discovering on :{} / :8080 / :7676 …",
                app.opts.http_port
            )),
        ];
        frame.render_widget(Paragraph::new(msg).wrap(Wrap { trim: false }), inner);
        return;
    }

    let items: Vec<ListItem> = app
        .playground
        .tools_list
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let marker = if i == app.playground.tool_index { "▸ " } else { "  " };
            let style = if i == app.playground.tool_index {
                selected_style()
            } else {
                Style::default()
            };
            let line = Line::from(vec![
                Span::styled(marker.to_string(), accent_style()),
                Span::styled(name.clone(), style.add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}", desc), dim_style()),
            ]);
            ListItem::new(line)
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.playground.tool_index));
    let list = List::new(items).highlight_symbol("");
    frame.render_stateful_widget(list, inner, &mut state);

    // Capture inner rect for mouse hit-testing.
    app.tool_list_inner.clone_from(&inner);
}

fn render_form_and_result(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    render_form(frame, chunks[0], app);
    render_result(frame, chunks[1], app);
}

fn render_form(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(" Parameters ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let pg = &app.playground;
    if pg.fields.is_empty() {
        let line = Line::from("No parameters (or schema unavailable). Press Enter to run.");
        frame.render_widget(Paragraph::new(vec![line]).wrap(Wrap { trim: false }), inner);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    // Selected tool name + description as a header above the form.
    if let Some((name, desc)) = pg.tools_list.get(pg.tool_index) {
        lines.push(Line::from(vec![
            Span::styled(name.clone(), accent_style().add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", desc), dim_style()),
        ]));
        lines.push(Line::from(""));
    }

    let mut lines: Vec<Line> = Vec::new();
    for (i, f) in pg.fields.iter().enumerate() {
        let focused = i == pg.field_focus;
        let value = render_field_value(f, &pg.field_values[i], focused && pg.editing_field);
        let req = if f.required { " *" } else { "  " };
        let label = format!("{:>14}", f.name.clone());
        let mut spans = vec![
            Span::styled(label, dim_style()),
            Span::styled(": ", dim_style()),
            Span::styled(value, if focused { accent_style() } else { Style::default() }),
            Span::styled(req, dim_style()),
        ];
        if focused {
            spans.push(Span::styled("  ←", accent_style()));
        }
        let style = if focused {
            selected_style()
        } else {
            Style::default()
        };
        lines.push(Line::styled(
            spans
                .into_iter()
                .map(|s| s.content)
                .collect::<String>(),
            style,
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(
        "Enter run  ↑/↓ field  type to edit  Space bool  ←/→ enum  Ctrl+c clear",
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_field_value(f: &ToolField, value: &str, editing: bool) -> String {
    match f.kind {
        FieldKind::Boolean => {
            let on = value.trim() == "true";
            format!("[{}]", if on { "on " } else { "off" })
        }
        FieldKind::Enum => {
            if value.is_empty() {
                "<select>".to_string()
            } else {
                value.to_string()
            }
        }
        _ => {
            let mut s = value.to_string();
            if editing {
                s.push('▌');
            }
            s
        }
    }
}

fn render_result(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(" Result ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = if let Some(e) = &app.playground.error {
        vec![Line::from(format!("error: {e}"))]
    } else if let Some(r) = &app.playground.result {
        let pretty = serde_json::to_string_pretty(r).unwrap_or_else(|_| r.to_string());
        pretty
            .lines()
            .take(inner.height as usize)
            .map(|l| Line::from(l.to_string()))
            .collect()
    } else {
        vec![Line::from("(no result yet — pick a tool and press Enter)")]
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_builder(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(" Recipe Builder ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let count = app
        .recipes
        .as_ref()
        .and_then(|r| r.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    let mut lines: Vec<Line> = Vec::new();
    if count == 0 {
        lines.push(Line::from(
            "No saved recipes yet. POST one via /api/recipes, then press r to run the first.",
        ));
    } else {
        lines.push(Line::from(format!("{count} saved recipe(s). Press r to run the first.")));
        if let Some(arr) = app.recipes.as_ref().and_then(|r| r.as_array()) {
            for r in arr.iter().take(12) {
                let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("(unnamed)");
                let steps = r
                    .get("steps")
                    .and_then(|s| s.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                lines.push(Line::from(format!("  {id}  {name}  —  {steps} steps")));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

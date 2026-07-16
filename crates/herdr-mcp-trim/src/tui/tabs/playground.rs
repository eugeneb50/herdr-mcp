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

use crate::tui::theme::{accent_style, dim_style, panel_block, selected_style, warn_style};
use crate::tui::{App, FieldKind, PlaygroundSub, ToolField};

/// Returns `true` if the key was consumed (don't run global handler).
pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    match app.playground.sub_tab {
        PlaygroundSub::Runner => handle_runner(app, code, mods).await,
        PlaygroundSub::Builder => handle_builder(app, code, mods).await,
    }
}

async fn handle_runner(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    let frame_count = if app.playground.sub_tab == PlaygroundSub::Runner {
        3
    } else {
        1
    };

    // Tab/BackTab cycle frames (when not editing).
    if !app.playground.editing_field {
        match code {
            KeyCode::Tab => {
                app.playground.focused_frame = (app.playground.focused_frame + 1) % frame_count;
                return Ok(true);
            }
            KeyCode::BackTab => {
                app.playground.focused_frame =
                    (app.playground.focused_frame + frame_count - 1) % frame_count;
                return Ok(true);
            }
            _ => {}
        }
    }

    // Sub-tab switching (global across frames).
    match code {
        KeyCode::Char('s') if mods.contains(KeyModifiers::CONTROL) => {
            app.playground.sub_tab = PlaygroundSub::Runner;
            app.playground.focused_frame = 0;
            return Ok(true);
        }
        KeyCode::Char('b') if mods.contains(KeyModifiers::CONTROL) => {
            app.playground.sub_tab = PlaygroundSub::Builder;
            app.playground.focused_frame = 0;
            return Ok(true);
        }
        _ => {}
    }

    // Frame 2: Result pane — arrows scroll, Esc unfocuses.
    if app.playground.focused_frame == 2 && !app.playground.editing_field {
        match code {
            KeyCode::Esc => {
                app.playground.focused_frame = 1;
                return Ok(true);
            }
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown => {
                app.feed_result_textarea(code, mods);
                return Ok(true);
            }
            _ => return Ok(false),
        }
    }

    handle_runner_fields(app, code, mods).await
}

async fn handle_runner_fields(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    // Plain printable characters must never be swallowed as hotkeys, so every
    // command key here requires Ctrl (or is a navigation/control key).
    let pg = &mut app.playground;
    let plain = !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT);

    // While editing a text/number field (frame 1), route keys into the live TextArea.
    // Clipboard hotkeys (Ctrl+C/V/X) fall through to the global handler.
    if pg.editing_field {
        match code {
            KeyCode::Enter | KeyCode::Esc => {
                pg.field_values[pg.field_focus] = pg.edit_area.lines().join("\n");
                pg.editing_field = false;
            }
            KeyCode::Char(c)
                if mods.contains(KeyModifiers::CONTROL) && matches!(c, 'c' | 'v' | 'x') =>
            {
                return Ok(false);
            }
            _ => app.feed_active_textarea(code, mods),
        }
        return Ok(true);
    }

    // Frame 0: Tool list navigation.
    if pg.focused_frame == 0 {
        match code {
            KeyCode::Up => {
                pg.tool_index = pg.tool_index.saturating_sub(1);
                Ok(true)
            }
            KeyCode::Down => {
                if !pg.tools_list.is_empty() {
                    pg.tool_index = (pg.tool_index + 1).min(pg.tools_list.len() - 1);
                }
                Ok(true)
            }
            KeyCode::Home => {
                pg.tool_index = 0;
                Ok(true)
            }
            KeyCode::End => {
                if !pg.tools_list.is_empty() {
                    pg.tool_index = pg.tools_list.len() - 1;
                }
                Ok(true)
            }
            KeyCode::PageUp => {
                pg.tool_index = pg.tool_index.saturating_sub(5);
                Ok(true)
            }
            KeyCode::PageDown => {
                if !pg.tools_list.is_empty() {
                    pg.tool_index = (pg.tool_index + 5).min(pg.tools_list.len() - 1);
                }
                Ok(true)
            }
            KeyCode::Enter => {
                run_tool(app).await;
                Ok(true)
            }
            _ => Ok(false),
        }
    } else {
        // Frame 1: Fields form navigation.
        match code {
            KeyCode::Up => {
                pg.field_focus = pg.field_focus.saturating_sub(1);
                Ok(true)
            }
            KeyCode::Down => {
                if pg.field_focus + 1 < pg.fields.len() {
                    pg.field_focus += 1;
                }
                Ok(true)
            }
            KeyCode::Home => {
                pg.field_focus = 0;
                Ok(true)
            }
            KeyCode::End => {
                if !pg.fields.is_empty() {
                    pg.field_focus = pg.fields.len() - 1;
                }
                Ok(true)
            }
            KeyCode::Char('k') if mods.contains(KeyModifiers::CONTROL) => {
                // Reset all fields to their defaults.
                pg.field_values = pg
                    .fields
                    .iter()
                    .map(|f| f.default.clone().unwrap_or_default())
                    .collect();
                pg.edit_area = crate::tui::TextArea::default();
                pg.editing_field = false;
                pg.error = None;
                Ok(true)
            }
            KeyCode::Esc => {
                pg.editing_field = false;
                Ok(true)
            }
            KeyCode::Enter => {
                run_tool(app).await;
                Ok(true)
            }
            KeyCode::Char(' ') => {
                let i = pg.field_focus;
                if let Some(f) = pg.fields.get(i)
                    && matches!(f.kind, FieldKind::Boolean)
                {
                    let cur = pg.field_values[i].trim() == "true";
                    pg.field_values[i] = if cur { "false" } else { "true" }.to_string();
                }
                Ok(true)
            }
            KeyCode::Left | KeyCode::Right => {
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
            // Start editing text/number fields by typing a printable character.
            KeyCode::Char(_c) if plain => {
                let i = pg.field_focus;
                if let Some(f) = pg.fields.get(i)
                    && matches!(f.kind, FieldKind::Text | FieldKind::Number)
                {
                    pg.editing_field = true;
                    let cur = pg.field_values.get(i).cloned().unwrap_or_default();
                    pg.edit_area = crate::tui::TextArea::new(vec![cur]);
                    pg.edit_area
                        .input(crate::tui::to_textarea_input(code, mods).unwrap());
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

async fn handle_builder(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    // Sub-tab switching.
    match code {
        KeyCode::Char('s') if mods.contains(KeyModifiers::CONTROL) => {
            app.playground.sub_tab = PlaygroundSub::Runner;
            app.playground.focused_frame = 0;
            return Ok(true);
        }
        KeyCode::Char('b') if mods.contains(KeyModifiers::CONTROL) => {
            app.playground.sub_tab = PlaygroundSub::Builder;
            app.playground.focused_frame = 0;
            return Ok(true);
        }
        // Tab/BackTab consumed (no-op for single-frame builder).
        KeyCode::Tab | KeyCode::BackTab => return Ok(true),
        _ => {}
    }
    match code {
        KeyCode::Enter => {
            let recipes = app.recipes.clone();
            if let Some(Value::Array(arr)) = recipes
                && let Some(first_id) = arr
                    .first()
                    .and_then(|r| r.get("id"))
                    .and_then(|v| v.as_str())
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
            app.playground.result = Some(v.clone());
            app.playground.error = None;
            let pretty = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
            app.playground.result_area =
                crate::tui::TextArea::new(pretty.lines().map(str::to_string).collect());
            app.status_msg = format!("{name} ok");
        }
        Err(e) => {
            app.playground.error = Some(e.to_string());
            app.playground.result = None;
            app.playground.result_area = crate::tui::TextArea::new(vec![format!("error: {e}")]);
            app.status_msg = format!("{name} failed");
        }
    }
}

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    app.sync_tool_fields();

    let conn = if app.bridge_connected {
        Span::styled(
            format!(
                "  connected :{}  •  {} tools",
                app.opts.http_port,
                app.playground.tools_list.len()
            ),
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

    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
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
            let marker = if i == app.playground.tool_index {
                "▸ "
            } else {
                "  "
            };
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

fn render_form_and_result(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
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

    if pg.editing_field {
        let name = pg
            .fields
            .get(pg.field_focus)
            .map(|f| f.name.clone())
            .unwrap_or_default();
        let header = Line::from(vec![
            Span::styled(
                format!("editing {name}"),
                accent_style().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   (Enter/Esc stop  Shift+←/→ select  right-click menu)",
                dim_style(),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(header),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        let edit_area = Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(1),
        );
        let ta = app.playground.edit_area.clone();
        frame.render_widget(&ta, edit_area);
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

    for (i, f) in pg.fields.iter().enumerate() {
        let focused = i == pg.field_focus;
        let value = render_field_value(f, &pg.field_values[i], false);
        let req = if f.required { " *" } else { "  " };
        let label = format!("{:>14}", f.name.clone());
        let mut spans = vec![
            Span::styled(label, dim_style()),
            Span::styled(": ", dim_style()),
            Span::styled(
                value,
                if focused {
                    accent_style()
                } else {
                    Style::default()
                },
            ),
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
            spans.into_iter().map(|s| s.content).collect::<String>(),
            style,
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(
        "Enter run  ↑/↓ field  type to edit  Space bool  ←/→ enum  Ctrl+k clear",
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

fn render_result(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let block = panel_block(" Result ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Capture inner rect for mouse hit-testing / focus.
    app.result_inner.clone_from(&inner);

    if app.playground.focused_frame == 2 {
        // Render the selectable Result view (selection visible while focused).
        let ta = app.playground.result_area.clone();
        frame.render_widget(&ta, inner);
        return;
    }

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
        lines.push(Line::from(format!(
            "{count} saved recipe(s). Press r to run the first."
        )));
        if let Some(arr) = app.recipes.as_ref().and_then(|r| r.as_array()) {
            for r in arr.iter().take(12) {
                let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let name = r
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unnamed)");
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

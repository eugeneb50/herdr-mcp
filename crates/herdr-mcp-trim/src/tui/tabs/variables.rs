//! Variables tab: session variable store management. Bordered list + edit form.
//!
//! ↑/↓ navigate, `e`/`n` edit (key/value fields), `x` delete, Enter save.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};

use crate::tui::{App, EditField};
use crate::tui::theme::{accent_style, dim_style, panel_block, selected_style};

pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    if app.variables_state.editing {
        return handle_edit(app, code, mods).await;
    }
    match code {
        KeyCode::Up => {
            if app.variables_state.selected > 0 {
                app.variables_state.selected -= 1;
            }
            Ok(true)
        }
        KeyCode::Down => {
            if app.variables_state.selected + 1 < app.variables_state.entries.len() {
                app.variables_state.selected += 1;
            }
            Ok(true)
        }
        KeyCode::Char('e') if mods.contains(KeyModifiers::CONTROL) => {
            let (k, v) = app
                .variables_state
                .entries
                .get(app.variables_state.selected)
                .cloned()
                .unwrap_or_default();
            app.variables_state.editing = true;
            app.variables_state.edit_key = k;
            app.variables_state.edit_value = v;
            app.variables_state.edit_field = EditField::Key;
            Ok(true)
        }
        KeyCode::Char('n') if mods.contains(KeyModifiers::CONTROL) => {
            app.variables_state.editing = true;
            app.variables_state.edit_key = String::new();
            app.variables_state.edit_value = String::new();
            app.variables_state.edit_field = EditField::Key;
            Ok(true)
        }
        KeyCode::Char('x') if mods.contains(KeyModifiers::CONTROL) => {
            if let Some((k, _)) = app
                .variables_state
                .entries
                .get(app.variables_state.selected)
                .cloned()
                && let Some(http) = app.http.clone()
            {
                if let Err(e) = http.delete_variable(&k).await {
                    app.status_msg = format!("delete failed: {e}");
                } else {
                    app.status_msg = format!("deleted {k}");
                    app.refresh().await;
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn handle_edit(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    // Only insert a character when no Ctrl/Alt modifier is held.
    let plain = !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT);
    match code {
        KeyCode::Tab => {
            app.variables_state.edit_field = match app.variables_state.edit_field {
                EditField::Key => EditField::Value,
                EditField::Value => EditField::Key,
            };
            Ok(true)
        }
        KeyCode::Esc => {
            app.variables_state.editing = false;
            Ok(true)
        }
        KeyCode::Enter => {
            save(app).await;
            Ok(true)
        }
        KeyCode::Backspace => {
            let buf = match app.variables_state.edit_field {
                EditField::Key => &mut app.variables_state.edit_key,
                EditField::Value => &mut app.variables_state.edit_value,
            };
            buf.pop();
            Ok(true)
        }
        KeyCode::Left | KeyCode::Right => Ok(true),
        KeyCode::Char(c) if plain => {
            let buf = match app.variables_state.edit_field {
                EditField::Key => &mut app.variables_state.edit_key,
                EditField::Value => &mut app.variables_state.edit_value,
            };
            buf.push(c);
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn save(app: &mut App) {
    let key = app.variables_state.edit_key.trim().to_string();
    let value = app.variables_state.edit_value.clone();
    if key.is_empty() {
        app.status_msg = "key cannot be empty".into();
        return;
    }
    let body = serde_json::json!({ "key": key, "value": value });
    if let Some(http) = app.http.clone() {
        match http.save_variable(body).await {
            Ok(_) => {
                app.status_msg = format!("saved {key}");
                app.variables_state.editing = false;
                app.refresh().await;
            }
            Err(e) => app.status_msg = format!("save failed: {e}"),
        }
    }
}

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let header = Paragraph::new(Line::from(vec![
        Span::styled("Variables", accent_style().add_modifier(Modifier::BOLD)),
        Span::styled(
            "  session variables store   (↑/↓ nav  Ctrl+e edit  Ctrl+n new  Ctrl+x delete)",
            dim_style(),
        ),
    ]));
    frame.render_widget(header, Rect::new(area.x, area.y, area.width, 1));

    let body = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1));

    if app.variables_state.editing {
        render_edit(frame, body, app);
        return;
    }

    render_list(frame, body, app);
}

fn render_list(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let block = panel_block(" Variables ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.variables_state.entries.is_empty() {
        let line = Line::from("(no variables — press n to add one)");
        frame.render_widget(Paragraph::new(vec![line]).wrap(Wrap { trim: false }), inner);
        return;
    }

    let items: Vec<ListItem> = app
        .variables_state
        .entries
        .iter()
        .take(inner.height as usize)
        .enumerate()
        .map(|(i, (k, v))| {
            let style = if i == app.variables_state.selected {
                selected_style()
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled("▸ ", accent_style()),
                Span::styled(format!("{:>20} = ", truncate(k, 20)), dim_style()),
                Span::styled(truncate(v, 50), Style::default()),
            ]))
            .style(style)
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.variables_state.selected));
    frame.render_stateful_widget(List::new(items), inner, &mut state);

    app.var_list_inner.clone_from(&inner);
}

fn render_edit(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(" Edit variable ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state = &app.variables_state;
    let k_focus = state.edit_field == EditField::Key;
    let v_focus = state.edit_field == EditField::Value;

    let k_cursor = if k_focus { "▌" } else { " " };
    let v_cursor = if v_focus { "▌" } else { " " };

    let lines = vec![
        Line::from(vec![
            Span::styled("  key:   ", dim_style()),
            Span::styled(state.edit_key.clone(), if k_focus { accent_style() } else { Style::default() }),
            Span::styled(k_cursor, accent_style()),
        ]),
        Line::from(vec![
            Span::styled("  value: ", dim_style()),
            Span::styled(truncate(&state.edit_value, 60), if v_focus { accent_style() } else { Style::default() }),
            Span::styled(v_cursor, accent_style()),
        ]),
        Line::from(""),
        Line::from("  Tab=switch field   Enter=save   Esc=cancel"),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
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

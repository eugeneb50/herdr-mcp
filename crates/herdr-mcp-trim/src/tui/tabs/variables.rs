//! Variables tab: session variable store management. Bordered list + edit form.
//!
//! ↑/↓ navigate, `e`/`n` edit (key/value fields), `x` delete, Enter save.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};

use crate::tui::theme::{
    accent_style, dim_style, focused_panel_block, panel_block, selected_style,
};
use crate::tui::{App, EditField, truncate};

pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    if app.variables_state.editing {
        return handle_edit(app, code, mods).await;
    }
    // Tab/BackTab consumed (no-op for single-frame).
    match code {
        KeyCode::Tab | KeyCode::BackTab => return Ok(true),
        _ => {}
    }
    match code {
        KeyCode::Up => {
            app.variables_state.selected = app.variables_state.selected.saturating_sub(1);
            Ok(true)
        }
        KeyCode::Down => {
            let count = app.variables_state.entries.len();
            if count > 0 {
                app.variables_state.selected = (app.variables_state.selected + 1).min(count - 1);
            }
            Ok(true)
        }
        KeyCode::Home => {
            app.variables_state.selected = 0;
            Ok(true)
        }
        KeyCode::End => {
            let count = app.variables_state.entries.len();
            if count > 0 {
                app.variables_state.selected = count - 1;
            }
            Ok(true)
        }
        KeyCode::PageUp => {
            app.variables_state.selected = app.variables_state.selected.saturating_sub(5);
            Ok(true)
        }
        KeyCode::PageDown => {
            let count = app.variables_state.entries.len();
            if count > 0 {
                app.variables_state.selected = (app.variables_state.selected + 5).min(count - 1);
            }
            Ok(true)
        }
        KeyCode::Char('e') if mods.contains(KeyModifiers::CONTROL) => {
            // Don't allow editing auto-stored variables (they're regenerated on each run)
            let (k, _) = app
                .variables_state
                .entries
                .get(app.variables_state.selected)
                .cloned()
                .unwrap_or_default();
            if is_auto_stored_key(&k) {
                app.status_msg = "auto-stored variables cannot be edited".into();
                return Ok(true);
            }
            let (k, v) = app
                .variables_state
                .entries
                .get(app.variables_state.selected)
                .cloned()
                .unwrap_or_default();
            app.variables_state.editing = true;
            app.variables_state.edit_key_area = crate::tui::TextArea::new(vec![k]);
            app.variables_state.edit_value_area = crate::tui::TextArea::new(vec![v]);
            app.variables_state.edit_field = EditField::Key;
            Ok(true)
        }
        KeyCode::Char('n') if mods.contains(KeyModifiers::CONTROL) => {
            app.variables_state.editing = true;
            app.variables_state.edit_key_area = crate::tui::TextArea::default();
            app.variables_state.edit_value_area = crate::tui::TextArea::default();
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
                // Can only delete user-created variables, not auto-stored ones
                if is_auto_stored_key(&k) {
                    app.status_msg = "auto-stored variables are regenerated on each run".into();
                    return Ok(true);
                }
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

/// Check if a variable key indicates it was auto-stored from a tool result.
fn is_auto_stored_key(key: &str) -> bool {
    key.starts_with("_") || 
    key.starts_with("pane_") || 
    key.contains("[")
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
        // Let global clipboard hotkeys (Ctrl+C/V/X) reach the global handler.
        KeyCode::Char(c)
            if mods.contains(KeyModifiers::CONTROL) && matches!(c, 'c' | 'v' | 'x') =>
        {
            Ok(false)
        }
        KeyCode::Backspace | KeyCode::Left | KeyCode::Right | KeyCode::Char(_) if plain => {
            app.feed_active_textarea(code, mods);
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn save(app: &mut App) {
    let key = app
        .variables_state
        .edit_key_area
        .lines()
        .join("\n")
        .trim()
        .to_string();
    let value = app.variables_state.edit_value_area.lines().join("\n");
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

    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );

    if app.variables_state.editing {
        render_edit(frame, body, app);
        return;
    }

    // Legend for variable types
    let legend = Paragraph::new(Line::from(vec![
        Span::styled("⟐ ", muted_style()),
        Span::styled("auto-stored from tool results   ", dim_style()),
        Span::styled("▸ ", muted_style()),
        Span::styled("user-created", dim_style()),
    ]));
    frame.render_widget(legend, Rect::new(area.x, area.y + 1, area.width, 1));

    render_list(frame, Rect::new(
        area.x,
        area.y + 2,
        area.width,
        area.height.saturating_sub(2),
    ), app);
}

fn render_list(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let block = if app.variables_state.focused_frame == 0 {
        focused_panel_block(" Variables ")
    } else {
        panel_block(" Variables ")
    };
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
            let is_auto = is_auto_stored_key(k);
            let style = if i == app.variables_state.selected {
                selected_style()
            } else {
                Style::default()
            };
            let marker = if is_auto {
                "⟐ " // Diamond marker for auto-stored variables
            } else {
                "▸ "
            };
            let key_display = if is_auto {
                format!("{:>18} ", truncate(k, 18))
            } else {
                format!("{:>20} = ", truncate(k, 20))
            };
            let value_display = if is_auto {
                truncate(v, 40)
            } else {
                truncate(v, 50)
            };
            ListItem::new(Line::from(vec![
                Span::styled(marker, accent_style()),
                Span::styled(key_display, if is_auto { dim_style() } else { dim_style() }),
                Span::styled(value_display, Style::default()),
                // Auto-stored indicator
                if is_auto {
                    Span::styled(" [auto]", muted_style())
                } else {
                    Span::styled("", Style::default())
                },
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
    let block = if app.variables_state.focused_frame == 0 {
        focused_panel_block(" Edit variable ")
    } else {
        panel_block(" Edit variable ")
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state = &app.variables_state;
    let k_focus = state.edit_field == EditField::Key;
    let v_focus = state.edit_field == EditField::Value;

    // key label + editor
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "key:",
            if k_focus { accent_style() } else { dim_style() },
        ))),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let ta_k = app.variables_state.edit_key_area.clone();
    frame.render_widget(&ta_k, Rect::new(inner.x, inner.y + 1, inner.width, 1));

    // value label + editor
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "value:",
            if v_focus { accent_style() } else { dim_style() },
        ))),
        Rect::new(inner.x, inner.y + 2, inner.width, 1),
    );
    let ta_v = app.variables_state.edit_value_area.clone();
    frame.render_widget(&ta_v, Rect::new(inner.x, inner.y + 3, inner.width, 1));

    let help_y = (inner.y + 4).min(inner.y + inner.height.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(Line::from(
            "Tab=switch field  Ctrl+v paste  Enter=save  Esc=cancel",
        )),
        Rect::new(inner.x, help_y, inner.width, 1),
    );
}

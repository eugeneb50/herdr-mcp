//! Variables tab: session variable store management.
//!
//! ↑/↓ navigate, `e` edit (key/value fields), Enter to save via HTTP.

use std::fmt::Write;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};

use crate::tui::{App, EditField};
use crate::tui::render::{bold, emerald, move_to, muted, red, truncate};

pub async fn handle_key(app: &mut App, code: KeyCode, _mods: KeyModifiers) -> Result<bool> {
    if app.variables_state.editing {
        return handle_edit(app, code).await;
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
        KeyCode::Char('e') => {
            if let Some((k, v)) = app
                .variables_state
                .entries
                .get(app.variables_state.selected)
                .cloned()
            {
                app.variables_state.editing = true;
                app.variables_state.edit_key = k;
                app.variables_state.edit_value = v;
                app.variables_state.edit_field = EditField::Key;
            } else {
                app.variables_state.editing = true;
                app.variables_state.edit_key = String::new();
                app.variables_state.edit_value = String::new();
                app.variables_state.edit_field = EditField::Key;
            }
            Ok(true)
        }
        KeyCode::Char('n') => {
            app.variables_state.editing = true;
            app.variables_state.edit_key = String::new();
            app.variables_state.edit_value = String::new();
            app.variables_state.edit_field = EditField::Key;
            Ok(true)
        }
        KeyCode::Char('x') => {
            if let Some((k, _)) = app.variables_state.entries.get(app.variables_state.selected).cloned() {
                if let Some(http) = app.http.clone() {
                    if let Err(e) = http.delete_variable(&k).await {
                        app.status_msg = format!("delete failed: {e}");
                    } else {
                        app.status_msg = format!("deleted {k}");
                        app.refresh().await;
                    }
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn handle_edit(app: &mut App, code: KeyCode) -> Result<bool> {
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
        KeyCode::Char(c) => {
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

pub fn render(f: &mut String, app: &App) {
    let row = 4u16;
    let _ = write!(f, "{}", move_to(2, row));
    let _ = write!(f, "{}   {} (↑/↓ nav  e/n edit  x delete)\r\n", bold("Variables"), muted("session variables store"));

    let state = &app.variables_state;
    if state.editing {
        render_edit(f, app, row + 2);
        return;
    }

    if state.entries.is_empty() {
        let _ = write!(f, "{}", move_to(2, row + 2));
        let _ = write!(f, "{}\r\n", muted("(no variables — press n to add one)"));
        return;
    }
    for (i, (k, v)) in state.entries.iter().take(18).enumerate() {
        let _ = write!(f, "{}", move_to(2, row + 2 + i as u16));
        let marker = if i == state.selected {
            emerald("▸")
        } else {
            muted(" ")
        };
        let _ = write!(f, "{marker} {} = {}\r\n", bold(&truncate(k, 24)), muted(&truncate(v, 60)));
    }
}

fn render_edit(f: &mut String, app: &App, row: u16) {
    let state = &app.variables_state;
    let _ = write!(f, "{}", move_to(2, row));
    let _ = write!(f, "{} (Tab=switch field  Enter=save  Esc=cancel)\r\n", bold("Edit"));
    let _ = write!(f, "{}", move_to(2, row + 1));
    let k_cursor = if state.edit_field == EditField::Key {
        red("_")
    } else {
        muted(" ")
    };
    let _ = write!(f, "  key:   {}{}\r\n", &state.edit_key, k_cursor);
    let _ = write!(f, "{}", move_to(2, row + 2));
    let v_cursor = if state.edit_field == EditField::Value {
        red("_")
    } else {
        muted(" ")
    };
    let _ = write!(f, "  value: {}{}\r\n", &truncate(&state.edit_value, 60), v_cursor);
}

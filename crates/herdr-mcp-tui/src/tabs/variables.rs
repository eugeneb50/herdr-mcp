//! Variables tab: every variable a request can read, in one list. Globals,
//! then each collection's own variables (both editable), then the auto
//! variables herdr's panes resolve to (read-only).
//!
//! ↑/↓ navigate, Enter or Ctrl+E edit, Ctrl+N add to the selected group,
//! Ctrl+X delete; in the form Tab switches field, Enter saves, Esc cancels.

use std::collections::HashMap;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use herdr_mcp_api::playground::{
    collection_vars, delete_collection_var, display_value, parse_value, save_collection_var,
    save_global,
};
use herdr_mcp_api::{Recipe, Uuid, VariableStore};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use serde_json::Value;

use crate::theme::{accent_style, dim_style, focused_panel_block, muted_style, panel_block};
use crate::{App, EditField, truncate};

/// Where a variable lives.
#[derive(Clone, Debug, PartialEq)]
pub enum Owner {
    Global,
    Collection(Uuid),
    /// Resolved live from herdr; not stored, so not editable.
    Auto,
}

/// A line of the list: a group heading (its owner and title) or a variable.
#[derive(Clone, Debug, PartialEq)]
pub enum VarRow {
    Heading(Owner, String),
    Var(Owner, String, String),
}

impl VarRow {
    fn owner(&self) -> &Owner {
        match self {
            Self::Heading(o, _) | Self::Var(o, _, _) => o,
        }
    }
}

/// The list: globals, each collection by name, then the auto variables.
/// Every collection gets a heading, so one without variables can get one.
pub fn var_rows(
    globals: &[VariableStore],
    recipes: &[Recipe],
    auto: &HashMap<String, Value>,
) -> Vec<VarRow> {
    let sorted = |vars: Vec<(String, String)>, owner: &Owner| {
        let mut vars = vars;
        vars.sort();
        vars.into_iter()
            .map(|(k, v)| VarRow::Var(owner.clone(), k, v))
            .collect::<Vec<_>>()
    };
    let mut rows = vec![VarRow::Heading(Owner::Global, "Globals".into())];
    let global: Vec<_> = globals
        .iter()
        .map(|v| (v.key.clone(), display_value(&v.value)))
        .collect();
    rows.extend(sorted(global, &Owner::Global));
    for (id, name, vars) in collection_vars(recipes) {
        let owner = Owner::Collection(id);
        rows.push(VarRow::Heading(
            owner.clone(),
            format!("Collection · {name}"),
        ));
        rows.extend(sorted(vars, &owner));
    }
    rows.push(VarRow::Heading(
        Owner::Auto,
        "Auto · live from herdr's panes, read-only".into(),
    ));
    let auto: Vec<_> = auto
        .iter()
        .map(|(k, v)| (k.clone(), display_value(v)))
        .collect();
    rows.extend(sorted(auto, &Owner::Auto));
    rows
}

impl App {
    /// Re-read globals, collections and auto variables into the list,
    /// keeping the selection on the same row where it still exists.
    pub(crate) async fn refresh_variables(&mut self) {
        let Some(http) = self.http.clone() else {
            return;
        };
        let (Ok(globals), Ok(recipes)) = (http.list_variables().await, http.list_recipes().await)
        else {
            return;
        };
        let auto = http
            .auto_variables(None)
            .await
            .map(|a| a.variables)
            .unwrap_or_default();
        let before = self.recipes.take().unwrap_or_default();
        self.playground.follow_tree(&before, &recipes);
        let rows = var_rows(&globals, &recipes, &auto);
        self.recipes = Some(recipes);
        let state = &mut self.variables_state;
        let picked = state.rows.get(state.selected).cloned();
        let same = |r: &VarRow| match (r, &picked) {
            (VarRow::Var(o, k, _), Some(VarRow::Var(po, pk, _))) => o == po && k == pk,
            (r, Some(p)) => r == p,
            _ => false,
        };
        state.selected = rows
            .iter()
            .position(same)
            .unwrap_or_else(|| state.selected.min(rows.len().saturating_sub(1)));
        state.rows = rows;
    }

    /// The name of the group `owner` heads, for the edit form's title.
    fn owner_title(&self, owner: &Owner) -> String {
        match owner {
            Owner::Global => "global".into(),
            Owner::Collection(id) => self
                .recipes
                .as_deref()
                .unwrap_or_default()
                .iter()
                .find(|r| r.id == *id)
                .map_or_else(|| "collection".into(), |r| format!("collection {}", r.name)),
            Owner::Auto => "auto".into(),
        }
    }
}

pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    if app.variables_state.editing {
        return handle_edit(app, code, mods).await;
    }
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let state = &mut app.variables_state;
    let count = state.rows.len();
    let last = count.saturating_sub(1);
    match code {
        KeyCode::Tab | KeyCode::BackTab => return Ok(true),
        KeyCode::Up => state.selected = state.selected.saturating_sub(1),
        KeyCode::Down => state.selected = (state.selected + 1).min(last),
        KeyCode::Home => state.selected = 0,
        KeyCode::End => state.selected = last,
        KeyCode::PageUp => state.selected = state.selected.saturating_sub(10),
        KeyCode::PageDown => state.selected = (state.selected + 10).min(last),
        KeyCode::Enter => begin_edit(app),
        KeyCode::Char('e') if ctrl => begin_edit(app),
        KeyCode::Char('n') if ctrl => begin_new(app),
        KeyCode::Char('x') if ctrl => delete(app).await,
        _ => return Ok(false),
    }
    Ok(true)
}

/// Edit the selected variable.
fn begin_edit(app: &mut App) {
    let state = &app.variables_state;
    let Some(row) = state.rows.get(state.selected).cloned() else {
        return;
    };
    match row {
        VarRow::Var(Owner::Auto, ..) => {
            app.status_msg =
                "auto variables are read-only; save one as a global to change it".into();
        }
        VarRow::Var(owner, k, v) => open_form(app, owner, Some(k), v),
        VarRow::Heading(..) => begin_new(app),
    }
}

/// Add a variable to the selected row's group.
fn begin_new(app: &mut App) {
    let state = &app.variables_state;
    let owner = state
        .rows
        .get(state.selected)
        .map_or(Owner::Global, |r| r.owner().clone());
    if owner == Owner::Auto {
        app.status_msg = "auto variables are read-only; add a global instead".into();
        return;
    }
    open_form(app, owner, None, String::new());
}

fn open_form(app: &mut App, owner: Owner, old: Option<String>, value: String) {
    let title = app.owner_title(&owner);
    let state = &mut app.variables_state;
    state.editing = true;
    // Typing goes on at the end of the text.
    let area = |lines: Vec<String>| {
        let mut area = crate::TextArea::new(lines);
        area.move_cursor(tui_textarea::CursorMove::Bottom);
        area.move_cursor(tui_textarea::CursorMove::End);
        area
    };
    state.edit_key_area = area(vec![old.clone().unwrap_or_default()]);
    state.edit_value_area = area(value.split('\n').map(String::from).collect());
    state.edit_field = if old.is_some() {
        EditField::Value
    } else {
        EditField::Key
    };
    state.edit_owner = owner;
    state.edit_old = old;
    state.edit_title = title;
}

async fn delete(app: &mut App) {
    let state = &app.variables_state;
    let Some(VarRow::Var(owner, key, _)) = state.rows.get(state.selected).cloned() else {
        return;
    };
    let Some(http) = app.http.clone() else {
        return;
    };
    let result = match &owner {
        Owner::Global => http.delete_variable(&key).await.map_err(|e| e.to_string()),
        Owner::Collection(id) => delete_collection_var(&http, *id, &key).await,
        Owner::Auto => {
            app.status_msg = "auto variables are read-only".into();
            return;
        }
    };
    app.status_msg = match result {
        Ok(()) => format!("deleted {key}"),
        Err(e) => format!("delete failed: {e}"),
    };
    app.refresh_variables().await;
}

async fn handle_edit(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    // Only insert a character when no Ctrl/Alt modifier is held.
    let plain = !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT);
    match code {
        KeyCode::Tab | KeyCode::BackTab => {
            let state = &mut app.variables_state;
            state.edit_field = match state.edit_field {
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
        KeyCode::Backspace
        | KeyCode::Delete
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::Char(_)
            if plain =>
        {
            app.feed_active_textarea(code, mods);
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn save(app: &mut App) {
    let state = &app.variables_state;
    let key = state.edit_key_area.lines().join("").trim().to_string();
    let value = parse_value(&state.edit_value_area.lines().join("\n"));
    if key.is_empty() {
        app.status_msg = "the name cannot be empty".into();
        return;
    }
    let (owner, old) = (state.edit_owner.clone(), state.edit_old.clone());
    let Some(http) = app.http.clone() else {
        app.status_msg = "bridge not connected".into();
        return;
    };
    let result = match &owner {
        Owner::Global => save_global(&http, old.as_deref(), &key, value).await,
        Owner::Collection(id) => save_collection_var(&http, *id, old.as_deref(), &key, value).await,
        Owner::Auto => return,
    };
    match result {
        Ok(()) => {
            app.status_msg = format!("saved {key}");
            app.variables_state.editing = false;
            // Select what was saved.
            app.variables_state
                .rows
                .push(VarRow::Var(owner, key, String::new()));
            app.variables_state.selected = app.variables_state.rows.len() - 1;
            app.refresh_variables().await;
        }
        Err(e) => app.status_msg = format!("save failed: {e}"),
    }
}

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let header = Paragraph::new(Line::from(vec![
        Span::styled("Variables", accent_style().add_modifier(Modifier::BOLD)),
        Span::styled(
            "  globals, each collection's own, auto   (↑/↓ nav  Enter edit  Ctrl+n new in group  Ctrl+x delete)",
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
    } else {
        render_list(frame, body, app);
    }
}

fn render_list(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let block = if app.variables_state.focused_frame == 0 {
        focused_panel_block(" Variables ")
    } else {
        panel_block(" Variables ")
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let state = &mut app.variables_state;
    if state.rows.is_empty() {
        frame.render_widget(
            Paragraph::new("(loading — Ctrl+R refreshes if the bridge is down)"),
            inner,
        );
        return;
    }
    let width = inner.width as usize;
    let items: Vec<ListItem> = state
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let marker = if i == state.selected { "▸ " } else { "  " };
            let line = match row {
                VarRow::Heading(_, title) => Line::from(vec![
                    Span::styled(marker, accent_style()),
                    Span::styled(title.clone(), muted_style().add_modifier(Modifier::BOLD)),
                ]),
                VarRow::Var(owner, k, v) => {
                    let key = truncate(k, 28);
                    let room = width.saturating_sub(4 + key.chars().count() + 3);
                    let value = truncate(&v.replace('\n', " ⏎ "), room);
                    Line::from(vec![
                        Span::styled(marker, accent_style()),
                        Span::styled(
                            format!("  {key}"),
                            if *owner == Owner::Auto {
                                dim_style()
                            } else {
                                accent_style()
                            },
                        ),
                        Span::styled(" = ", dim_style()),
                        Span::styled(
                            value,
                            if *owner == Owner::Auto {
                                dim_style()
                            } else {
                                ratatui::style::Style::default()
                            },
                        ),
                    ])
                }
            };
            ListItem::new(line)
        })
        .collect();
    state.list.select(Some(state.selected));
    frame.render_stateful_widget(List::new(items), inner, &mut state.list);
    app.var_list_inner = inner;
}

fn render_edit(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let state = &app.variables_state;
    let verb = if state.edit_old.is_some() {
        "Edit"
    } else {
        "New"
    };
    let title = format!(" {verb} {} variable ", state.edit_title);
    let block = if state.focused_frame == 0 {
        focused_panel_block(&title)
    } else {
        panel_block(&title)
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let k_focus = state.edit_field == EditField::Key;
    let label = |text: &str, on: bool| {
        Paragraph::new(Line::from(Span::styled(
            text.to_string(),
            if on { accent_style() } else { dim_style() },
        )))
    };
    frame.render_widget(
        label("name:", k_focus),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    frame.render_widget(
        &state.edit_key_area,
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );
    frame.render_widget(
        label("value (JSON is stored typed):", !k_focus),
        Rect::new(inner.x, inner.y + 2, inner.width, 1),
    );
    frame.render_widget(
        &state.edit_value_area,
        Rect::new(inner.x, inner.y + 3, inner.width, 1),
    );
    let help_y = (inner.y + 5).min(inner.y + inner.height.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(Line::styled(
            "Tab switch field  Ctrl+v paste  Enter save  Esc cancel",
            dim_style(),
        )),
        Rect::new(inner.x, help_y, inner.width, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_group_globals_collections_then_auto() {
        let global: VariableStore = serde_json::from_value(serde_json::json!({
            "id": Uuid::nil(), "key": "token", "value": "t",
            "created_at": "2026-10-25T00:00:00Z", "updated_at": "2026-10-25T00:00:00Z"
        }))
        .unwrap();
        let recipe = |name: &str, vars: Value| -> Recipe {
            serde_json::from_value(serde_json::json!({
                "id": Uuid::new_v4(), "name": name, "description": null, "steps": [],
                "variables": vars, "created_at": "2026-10-25T00:00:00Z",
                "updated_at": "2026-10-25T00:00:00Z", "is_template": false
            }))
            .unwrap()
        };
        let (b, a) = (
            recipe("b", serde_json::json!({ "n": 2 })),
            recipe("a", serde_json::json!({})),
        );
        let auto = [("w1:p1".to_string(), serde_json::json!({ "label": "x" }))].into();
        let rows = var_rows(&[global], &[b.clone(), a.clone()], &auto);
        assert_eq!(
            rows,
            [
                VarRow::Heading(Owner::Global, "Globals".into()),
                VarRow::Var(Owner::Global, "token".into(), "t".into()),
                VarRow::Heading(Owner::Collection(a.id), "Collection · a".into()),
                VarRow::Heading(Owner::Collection(b.id), "Collection · b".into()),
                VarRow::Var(Owner::Collection(b.id), "n".into(), "2".into()),
                VarRow::Heading(
                    Owner::Auto,
                    "Auto · live from herdr's panes, read-only".into()
                ),
                VarRow::Var(Owner::Auto, "w1:p1".into(), r#"{"label":"x"}"#.into()),
            ]
        );
    }
}

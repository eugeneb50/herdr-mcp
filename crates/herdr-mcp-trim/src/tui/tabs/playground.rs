//! Playground tab: tool runner + recipe builder.
//!
//! Two sub-tabs reached with `s`/`b`:
//! - Runner  : pick a tool (↑/↓), edit params JSON, Enter runs, result shown.
//! - Builder : basic recipe skeleton (list saved recipes, run by id).

use std::fmt::Write;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use serde_json::Value;

use crate::tui::{App, PlaygroundSub};
use crate::tui::render::{bold, emerald, move_to, muted, red, truncate};

/// Returns `true` if the key was consumed (don't run global handler).
pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    match app.playground.sub_tab {
        PlaygroundSub::Runner => handle_runner(app, code, mods).await,
        PlaygroundSub::Builder => handle_builder(app, code, mods).await,
    }
}

async fn handle_runner(app: &mut App, code: KeyCode, _mods: KeyModifiers) -> Result<bool> {
    match code {
        KeyCode::Char('s') => {
            app.playground.sub_tab = PlaygroundSub::Runner;
            Ok(true)
        }
        KeyCode::Char('b') => {
            app.playground.sub_tab = PlaygroundSub::Builder;
            Ok(true)
        }
        KeyCode::Up => {
            if app.playground.tool_index > 0 {
                app.playground.tool_index -= 1;
            }
            Ok(true)
        }
        KeyCode::Down => {
            if app.playground.tool_index + 1 < app.playground.tools_list.len() {
                app.playground.tool_index += 1;
            }
            Ok(true)
        }
        KeyCode::Enter => {
            run_tool(app).await;
            Ok(true)
        }
        KeyCode::Char('c') if _mods.contains(KeyModifiers::CONTROL) => {
            app.playground.param_text = "{}".to_string();
            app.playground.param_cursor = 1;
            Ok(true)
        }
        KeyCode::Backspace => {
            if app.playground.param_cursor > 0 {
                app.playground.param_cursor -= 1;
                app.playground.param_text.remove(app.playground.param_cursor);
            }
            Ok(true)
        }
        KeyCode::Left => {
            if app.playground.param_cursor > 0 {
                app.playground.param_cursor -= 1;
            }
            Ok(true)
        }
        KeyCode::Right => {
            if app.playground.param_cursor < app.playground.param_text.len() {
                app.playground.param_cursor += 1;
            }
            Ok(true)
        }
        KeyCode::Char(c) => {
            app.playground
                .param_text
                .insert(app.playground.param_cursor, c);
            app.playground.param_cursor += c.len_utf8();
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn handle_builder(app: &mut App, code: KeyCode, _mods: KeyModifiers) -> Result<bool> {
    match code {
        KeyCode::Char('s') => {
            app.playground.sub_tab = PlaygroundSub::Runner;
            Ok(true)
        }
        KeyCode::Char('b') => {
            app.playground.sub_tab = PlaygroundSub::Builder;
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn run_tool(app: &mut App) {
    let Some((name, _)) = app
        .playground
        .tools_list
        .get(app.playground.tool_index)
        .cloned()
    else {
        app.playground.error = Some("no tool selected".into());
        return;
    };
    let params: Value = match serde_json::from_str(&app.playground.param_text) {
        Ok(v) => v,
        Err(e) => {
            app.playground.error = Some(format!("invalid JSON: {e}"));
            app.playground.result = None;
            return;
        }
    };
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

pub fn render(f: &mut String, app: &App) {
    let row = 4u16;
    let _ = write!(f, "{}", move_to(2, row));
    let sub = match app.playground.sub_tab {
        PlaygroundSub::Runner => "Runner",
        PlaygroundSub::Builder => "Builder",
    };
    let _ = write!(f, "{}   {}  (s=runner  b=builder)\r\n", bold("Playground"), muted(sub));

    match app.playground.sub_tab {
        PlaygroundSub::Runner => render_runner(f, app, row + 2),
        PlaygroundSub::Builder => render_builder(f, app, row + 2),
    }
}

fn render_runner(f: &mut String, app: &App, row: u16) {
    let _ = write!(f, "{}", move_to(2, row));
    let _ = write!(f, "{}  (↑/↓ choose  Enter run  Ctrl-C clear params)\r\n", bold("Tools"));

    let list_start = row + 1;
    let visible = 8usize;
    let idx = app.playground.tool_index;
    let start = idx.saturating_sub(visible / 2);
    for (i, (name, desc)) in app
        .playground
        .tools_list
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
    {
        let _ = write!(f, "{}", move_to(2, list_start + (i - start) as u16));
        let marker = if i == idx { emerald("▸") } else { muted(" ") };
        let _ = write!(f, "{marker} {} {}", bold(name), muted(&truncate(desc, 40)));
    }

    let p_row = list_start + visible as u16 + 1;
    let _ = write!(f, "{}", move_to(2, p_row));
    let _ = write!(f, "{}  (edit JSON, Enter to run)\r\n", bold("Params"));
    let _ = write!(f, "{}", move_to(2, p_row + 1));
    let _ = write!(f, "  {}\r\n", app.playground.param_text);

    let r_row = p_row + 3;
    let _ = write!(f, "{}", move_to(2, r_row));
    let _ = write!(f, "{}\r\n", bold("Result"));
    if let Some(e) = &app.playground.error {
        let _ = write!(f, "{}", move_to(2, r_row + 1));
        let _ = write!(f, "{}\r\n", red(&truncate(e, 80)));
    } else if let Some(r) = &app.playground.result {
        let pretty = serde_json::to_string_pretty(r).unwrap_or_else(|_| r.to_string());
        for (i, line) in pretty.lines().take(14).enumerate() {
            let _ = write!(f, "{}", move_to(2, r_row + 1 + i as u16));
            let _ = write!(f, "{}\r\n", muted(&truncate(line, 100)));
        }
    } else {
        let _ = write!(f, "{}", move_to(2, r_row + 1));
        let _ = write!(f, "{}\r\n", muted("(no result yet — pick a tool and press Enter)"));
    }
}

fn render_builder(f: &mut String, app: &App, row: u16) {
    let _ = write!(f, "{}", move_to(2, row));
    let _ = write!(f, "{}  (saved recipes from /api/recipes)\r\n", bold("Recipe Builder"));

    let count = app
        .recipes
        .as_ref()
        .and_then(|r| r.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    let _ = write!(f, "{}", move_to(2, row + 2));
    if count == 0 {
        let _ = write!(
            f,
            "{} POST /api/recipes to save one, then run it with /api/recipes/:id/run.\r\n",
            muted("No saved recipes yet."),
        );
    } else {
        let _ = write!(f, "{} saved recipe(s):\r\n", emerald(&count.to_string()));
        if let Some(arr) = app.recipes.as_ref().and_then(|r| r.as_array()) {
            for (i, r) in arr.iter().take(12).enumerate() {
                let _ = write!(f, "{}", move_to(2, row + 3 + i as u16));
                let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("(unnamed)");
                let steps = r
                    .get("steps")
                    .and_then(|s| s.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let _ = write!(
                    f,
                    "  {} {} {} {} {} steps\r\n",
                    emerald(&truncate(id, 12)),
                    bold(name),
                    muted("—"),
                    muted(""),
                    muted(&steps.to_string()),
                );
            }
        }
    }
    let _ = write!(f, "{}", move_to(2, row + 16));
    let _ = write!(
        f,
        "{} Save via tool/save, then {} to run. Try {} in Runner sub-tab first.\r\n",
        muted("Tip: build recipes through the HTTP API."),
        emerald("run_recipe_by_id"),
        emerald("list_templates"),
    );
}

#[allow(dead_code)]
fn _unused(_app: &App) {
    // Placeholder to keep imports resolved if builder grows.
}

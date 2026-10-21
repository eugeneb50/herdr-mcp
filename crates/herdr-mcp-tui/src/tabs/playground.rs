//! Playground tab: tool runner (per-field form) + recipe builder.
//!
//! Runner sub-tab lays out a tool list (left) and a per-field parameter form
//! (right). Each tool `inputSchema` property becomes one editable field;
//! `↑/↓` move focus, typing edits text/number fields, `Space` toggles
//! booleans, `←/→` cycle enums, `Enter` runs the tool.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use herdr_mcp_api::{
    CreateRecipeRequest, Recipe, RecipeTemplate, RunRecord, RunStatus, StartRunRequest,
    UpdateRecipeRequest,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use serde_json::{Map, Value};

use super::variables::is_auto_stored_key;
use crate::theme::{
    accent_style, dim_style, focused_panel_block, muted_style, panel_block, selected_style,
    warn_style,
};
use crate::{App, FieldKind, PlaygroundSub, RecipeStep, ToolCategory, ToolField};

/// Group the flat tool list from `app.tools` by each tool's
/// `_meta.category` (set on the tool's server definition), categories and
/// tools sorted by name. Tools without one go under "Other".
pub fn parse_tool_categories(tool_arr: &[Value]) -> Vec<ToolCategory> {
    let mut groups: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();
    for tool in tool_arr {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let desc = tool
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let category = tool
            .pointer("/_meta/category")
            .and_then(Value::as_str)
            .unwrap_or("Other");
        groups
            .entry(category.to_string())
            .or_default()
            .push((name.to_string(), desc.to_string()));
    }
    groups
        .into_iter()
        .map(|(label, mut tools)| {
            tools.sort();
            ToolCategory { label, tools }
        })
        .collect()
}

/// Auto-generate a step ID from the tool name and step index.
fn auto_step_id(tool: &str, index: usize) -> String {
    format!("{tool}_{index}")
}

/// Validate a recipe step for common issues.
/// Returns a list of validation errors (empty = valid).
fn validate_recipe_step(step: &RecipeStep, all_steps: &[RecipeStep]) -> Vec<String> {
    let mut errors = Vec::new();

    // Check for empty/missing ID
    if step.id.is_empty() {
        errors.push("step ID is empty".to_string());
    }

    // Check for empty tool name
    if step.tool.is_empty() {
        errors.push("tool name is empty".to_string());
    }

    // Check for duplicate IDs
    if all_steps.iter().filter(|s| s.id == step.id).count() > 1 {
        errors.push(format!("duplicate step ID '{}'", step.id));
    }

    // Check params for {{ }} variable references that might be invalid.
    // `params` is the schema object itself (`Map`), so iterate it directly.
    {
        let params = &step.params;
        for (key, value) in params {
            if let Some(str_val) = value.as_str() {
                // Check for malformed variable references
                if str_val.contains("{{") && !str_val.contains("}}") {
                    errors.push(format!("unclosed variable reference in param '{}'", key));
                }
                if !str_val.contains("{{") && str_val.contains("}}") {
                    errors.push(format!("unopened variable reference in param '{}'", key));
                }
            }
        }
    }

    errors
}

/// Validate the entire recipe for issues.
/// Returns validation warnings/errors.
fn validate_recipe(steps: &[RecipeStep]) -> Vec<String> {
    let mut warnings = Vec::new();

    if steps.is_empty() {
        warnings.push("recipe has no steps".to_string());
        return warnings;
    }

    // Validate each step
    static STEP_REF_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\{\{\s*(\w+)\.").unwrap());
    for (i, step) in steps.iter().enumerate() {
        let step_errors = validate_recipe_step(step, steps);
        for err in step_errors {
            warnings.push(format!("step {i} ({}): {}", step.tool, err));
        }

        // Check for variable references to steps that don't exist.
        // `params` is the schema object itself (`Map`), so iterate it directly.
        {
            let params = &step.params;
            for (key, value) in params {
                if let Some(str_val) = value.as_str() {
                    // Extract step references from {{ stepId.result.path }} patterns
                    for cap in STEP_REF_RE.captures_iter(str_val) {
                        let ref_step_id = &cap[1];
                        // Check if this step ID exists
                        let exists = steps.iter().any(|s| s.id == *ref_step_id);
                        if !exists {
                            warnings.push(format!(
                                "step {i} references non-existent step '{}' in param '{}'",
                                ref_step_id, key
                            ));
                        }
                    }
                }
            }
        }
    }

    warnings
}

/// Compute the index of the focused tool within the current picker category,
/// accounting for search filtering.
fn picker_filtered_tools<'a>(
    categories: &'a [ToolCategory],
    cat_idx: usize,
    search: &str,
) -> Vec<&'a (String, String)> {
    let search_lower = search.to_lowercase();
    categories
        .get(cat_idx)
        .map(|c| {
            c.tools
                .iter()
                .filter(|(name, desc)| {
                    search.is_empty()
                        || name.to_lowercase().contains(&search_lower)
                        || desc.to_lowercase().contains(&search_lower)
                })
                .collect()
        })
        .unwrap_or_default()
}

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
                pg.edit_area = crate::TextArea::default();
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
                    pg.edit_area = crate::TextArea::new(vec![cur]);
                    pg.edit_area
                        .input(crate::to_textarea_input(code, mods).unwrap());
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

async fn handle_builder(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    // --- Sub-tab switching (global) ---
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

    // --- Tool picker modal (owns all input while open) ---
    if app.playground.builder_picker_open {
        return handle_picker(app, code, mods).await;
    }

    // --- Library panel (owns input when open and focused) ---
    if app.playground.builder_show_library {
        // Library only captures input when focus is on it (step 0 = library).
        // For now library always captures when open.
        return handle_library(app, code, mods).await;
    }

    // --- Param editing mode (TextArea active) ---
    if app.playground.builder_editing_param {
        return handle_builder_param_edit(app, code, mods).await;
    }

    // --- Step detail mode (step params visible) ---
    if app.playground.builder_open_step.is_some() {
        return handle_builder_step_detail(app, code, mods).await;
    }

    // --- Step list navigation + global hotkeys ---
    handle_builder_global(app, code, mods).await
}

// ---------------------------------------------------------------------------
// Builder sub-handlers
// ---------------------------------------------------------------------------

/// Handle keys when the tool picker overlay is open.
async fn handle_picker(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    let pg = &mut app.playground;
    let cat_count = pg.builder_tool_categories.len();
    if cat_count == 0 {
        pg.builder_picker_open = false;
        return Ok(true);
    }

    let cat_idx = pg.builder_picker_category.min(cat_count - 1);
    let filtered = picker_filtered_tools(
        &pg.builder_tool_categories,
        cat_idx,
        &pg.builder_picker_search,
    );
    let tool_count = filtered.len();

    match code {
        KeyCode::Esc => {
            pg.builder_picker_open = false;
            return Ok(true);
        }
        KeyCode::Enter => {
            if let Some((name, desc)) = filtered.get(pg.builder_picker_tool) {
                let tool_name = name.clone();
                let tool_desc = desc.clone();
                let step_id = auto_step_id(&tool_name, pg.builder_steps.len());
                let step = RecipeStep {
                    id: step_id,
                    tool: tool_name,
                    params: Map::new(),
                    description: Some(tool_desc),
                };
                pg.builder_steps.push(step);
                pg.builder_step_focus = pg.builder_steps.len().saturating_sub(1);
                pg.builder_picker_open = false;
                pg.builder_picker_search.clear();
                pg.builder_picker_tool = 0;
            }
            return Ok(true);
        }
        KeyCode::Up => {
            pg.builder_picker_tool = pg.builder_picker_tool.saturating_sub(1);
            return Ok(true);
        }
        KeyCode::Down => {
            if tool_count > 0 {
                pg.builder_picker_tool = (pg.builder_picker_tool + 1).min(tool_count - 1);
            }
            return Ok(true);
        }
        KeyCode::Left | KeyCode::Tab => {
            pg.builder_picker_category = pg.builder_picker_category.saturating_sub(1);
            pg.builder_picker_tool = 0;
            return Ok(true);
        }
        KeyCode::Right | KeyCode::BackTab => {
            if cat_count > 0 {
                pg.builder_picker_category = (pg.builder_picker_category + 1).min(cat_count - 1);
            }
            pg.builder_picker_tool = 0;
            return Ok(true);
        }
        KeyCode::Char(c)
            if !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) =>
        {
            pg.builder_picker_search.push(c);
            pg.builder_picker_tool = 0;
            return Ok(true);
        }
        KeyCode::Backspace => {
            pg.builder_picker_search.pop();
            pg.builder_picker_tool = 0;
            return Ok(true);
        }
        _ => {}
    }
    Ok(true)
}

/// Handle keys when the library panel is open.
async fn handle_library(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    // Escape closes library.
    if matches!(code, KeyCode::Esc) {
        app.playground.builder_show_library = false;
        return Ok(true);
    }

    // Shift+Tab / Tab cycles between Templates and Saved tabs.
    if matches!(code, KeyCode::BackTab | KeyCode::Tab) {
        app.playground.builder_library_tab = match app.playground.builder_library_tab {
            crate::BuilderLibraryTab::Templates => crate::BuilderLibraryTab::Saved,
            crate::BuilderLibraryTab::Saved => crate::BuilderLibraryTab::Templates,
        };
        app.playground.builder_library_focus = 0;
        return Ok(true);
    }

    // Up/Down navigation.
    let item_count = match app.playground.builder_library_tab {
        crate::BuilderLibraryTab::Saved => app.recipes.as_ref().map_or(0, Vec::len),
        crate::BuilderLibraryTab::Templates => app.templates.as_ref().map_or(0, Vec::len),
    };
    match code {
        KeyCode::Up => {
            app.playground.builder_library_focus =
                app.playground.builder_library_focus.saturating_sub(1);
            return Ok(true);
        }
        KeyCode::Down => {
            if item_count > 0 {
                app.playground.builder_library_focus =
                    (app.playground.builder_library_focus + 1).min(item_count - 1);
            }
            return Ok(true);
        }
        KeyCode::Enter | KeyCode::Char('o') if mods.contains(KeyModifiers::CONTROL) => {
            // Load selected item into builder.
            // Extract data we need before calling into `app` mutably.
            let lib_tab = app.playground.builder_library_tab;
            let lib_focus = app.playground.builder_library_focus;
            match lib_tab {
                crate::BuilderLibraryTab::Saved => {
                    let id = app
                        .recipes
                        .as_ref()
                        .and_then(|r| r.get(lib_focus))
                        .map(|r| r.id);
                    if let (Some(http), Some(id)) = (app.http.clone(), id) {
                        match http.get_recipe(id).await {
                            Ok(recipe) => {
                                load_recipe_into_builder(app, &recipe);
                                app.status_msg = format!("loaded recipe {id}");
                            }
                            Err(e) => {
                                app.playground.builder_error = Some(e.to_string());
                            }
                        }
                    }
                }
                crate::BuilderLibraryTab::Templates => {
                    let tmpl = app
                        .templates
                        .as_ref()
                        .and_then(|t| t.get(lib_focus))
                        .cloned();
                    if let Some(tmpl) = tmpl {
                        load_template_into_builder(app, &tmpl);
                        app.status_msg = format!("loaded template {}", tmpl.name);
                    }
                }
            }
            app.playground.builder_show_library = false;
            return Ok(true);
        }
        KeyCode::Char('x')
            if app.playground.builder_library_tab == crate::BuilderLibraryTab::Saved =>
        {
            let id = app
                .recipes
                .as_ref()
                .and_then(|r| r.get(app.playground.builder_library_focus))
                .map(|r| r.id);
            if let (Some(http), Some(id)) = (app.http.clone(), id) {
                match http.delete_recipe(id).await {
                    Ok(()) => {
                        app.status_msg = format!("deleted recipe {id}");
                        app.recipes = None;
                        if app.playground.builder_library_focus > 0 {
                            app.playground.builder_library_focus -= 1;
                        }
                    }
                    Err(e) => {
                        app.playground.builder_error = Some(e.to_string());
                    }
                }
            }
            return Ok(true);
        }
        _ => {}
    }
    Ok(true)
}

/// Load a server-side recipe into the builder state.
fn load_recipe_into_builder(app: &mut App, recipe: &Recipe) {
    load_steps_into_builder(app, &recipe.name, &recipe.steps);
    app.playground.builder_editing_recipe_id = Some(recipe.id);
}

/// Load a bundled template into the builder (as a new, unsaved recipe).
fn load_template_into_builder(app: &mut App, tmpl: &RecipeTemplate) {
    load_steps_into_builder(app, &tmpl.name, &tmpl.steps);
    app.playground.builder_editing_recipe_id = None;
}

fn load_steps_into_builder(app: &mut App, name: &str, steps: &[herdr_mcp_api::RecipeStep]) {
    let pg = &mut app.playground;
    pg.builder_recipe_name = name.to_string();
    pg.builder_steps = steps.iter().map(RecipeStep::from).collect();
    pg.builder_step_focus = 0;
    pg.builder_param_focus = 0;
    pg.builder_open_step = None;
    pg.builder_editing_param = false;
    pg.builder_error = None;
    pg.builder_result = None;
}

/// Handle keys when editing a param value (TextArea active).
async fn handle_builder_param_edit(
    app: &mut App,
    code: KeyCode,
    mods: KeyModifiers,
) -> Result<bool> {
    let pg = &mut app.playground;

    // Ctrl+C / Ctrl+V / Ctrl+X fall through to global clipboard handler.
    if mods.contains(KeyModifiers::CONTROL)
        && let KeyCode::Char('c' | 'v' | 'x') = code
    {
        return Ok(false);
    }

    match code {
        KeyCode::Enter | KeyCode::Esc => {
            let new_val = pg.builder_param_edit_area.lines().join("\n");

            // param_focus == usize::MAX means we're editing the recipe name.
            if pg.builder_param_focus == usize::MAX {
                pg.builder_recipe_name = new_val;
            } else if let Some(open_idx) = pg.builder_open_step
                && let Some(step) = pg.builder_steps.get_mut(open_idx)
            {
                let mut keys: Vec<String> = step.params.keys().cloned().collect();
                keys.sort();
                if let Some(key) = keys.get(pg.builder_param_focus) {
                    step.params.insert(key.clone(), Value::String(new_val));
                }
            }
            pg.builder_editing_param = false;
            return Ok(true);
        }
        _ => {
            if let Some(input) = crate::to_textarea_input(code, mods) {
                pg.builder_param_edit_area.input(input);
            }
        }
    }
    Ok(true)
}

/// Handle keys when viewing step detail (params panel, not editing a param).
async fn handle_builder_step_detail(
    app: &mut App,
    code: KeyCode,
    mods: KeyModifiers,
) -> Result<bool> {
    let pg = &mut app.playground;
    let open_idx = pg.builder_open_step.unwrap_or(0);

    if matches!(code, KeyCode::Esc) {
        pg.builder_open_step = None;
        pg.builder_editing_param = false;
        return Ok(true);
    }

    let param_count = pg
        .builder_steps
        .get(open_idx)
        .map(|s| s.params.len())
        .unwrap_or(0);

    match code {
        KeyCode::Up => {
            pg.builder_param_focus = pg.builder_param_focus.saturating_sub(1);
            return Ok(true);
        }
        KeyCode::Down => {
            if param_count > 0 {
                pg.builder_param_focus = (pg.builder_param_focus + 1).min(param_count - 1);
            }
            return Ok(true);
        }
        KeyCode::Enter => {
            if let Some(step) = pg.builder_steps.get(open_idx) {
                let mut keys: Vec<String> = step.params.keys().cloned().collect();
                keys.sort();
                if let Some(key) = keys.get(pg.builder_param_focus) {
                    let val = step
                        .params
                        .get(key)
                        .map(|v| match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .unwrap_or_default();
                    pg.builder_param_edit_area = crate::TextArea::new(vec![val]);
                    pg.builder_editing_param = true;
                }
            }
            return Ok(true);
        }
        KeyCode::Char('d') if mods.contains(KeyModifiers::CONTROL) => {
            if let Some(step) = pg.builder_steps.get_mut(open_idx) {
                let mut keys: Vec<String> = step.params.keys().cloned().collect();
                keys.sort();
                if let Some(key) = keys.get(pg.builder_param_focus) {
                    step.params.remove(key);
                    if pg.builder_param_focus > 0 {
                        pg.builder_param_focus -= 1;
                    }
                }
            }
            return Ok(true);
        }
        _ => {}
    }
    Ok(true)
}

/// Handle keys in the step list / global builder view (no modal, no editing).
async fn handle_builder_global(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    let step_count = app.playground.builder_steps.len();

    match code {
        // --- Global hotkeys ---
        KeyCode::Char('t') if mods.contains(KeyModifiers::CONTROL) => {
            if app.playground.builder_tool_categories.is_empty()
                && let Some(tools) = &app.tools
            {
                app.playground.builder_tool_categories = parse_tool_categories(tools);
            }
            app.playground.builder_picker_open = true;
            app.playground.builder_picker_category = 0;
            app.playground.builder_picker_tool = 0;
            app.playground.builder_picker_search.clear();
            return Ok(true);
        }
        KeyCode::Char('l') if mods.contains(KeyModifiers::CONTROL) => {
            app.playground.builder_show_library = !app.playground.builder_show_library;
            app.playground.builder_library_focus = 0;
            return Ok(true);
        }
        KeyCode::Char('n') if mods.contains(KeyModifiers::CONTROL) => {
            let cur = app.playground.builder_recipe_name.clone();
            app.playground.builder_param_edit_area = crate::TextArea::new(vec![cur]);
            app.playground.builder_open_step = None;
            app.playground.builder_editing_param = true;
            app.playground.builder_param_focus = usize::MAX;
            return Ok(true);
        }
        KeyCode::Char('w') if mods.contains(KeyModifiers::CONTROL) => {
            builder_save(app).await;
            return Ok(true);
        }
        KeyCode::Char('r') if mods.contains(KeyModifiers::CONTROL) => {
            builder_run_inline(app).await;
            return Ok(true);
        }
        KeyCode::Char('v') if mods.contains(KeyModifiers::CONTROL) => {
            // Show validation status
            let warnings = validate_recipe(&app.playground.builder_steps);
            if warnings.is_empty() {
                app.status_msg = "✓ recipe validates cleanly".into();
                app.playground.builder_error = None;
            } else {
                let warning_text = warnings.join("\n");
                app.playground.builder_error = Some(format!("Validation issues:\n{warning_text}"));
                app.status_msg = format!("✗ {} validation issue(s)", warnings.len());
            }
            return Ok(true);
        }
        KeyCode::Char('r')
            if mods.contains(KeyModifiers::CONTROL) && mods.contains(KeyModifiers::SHIFT) =>
        {
            if let Some(id) = app.playground.builder_editing_recipe_id {
                if let Some(http) = app.http.clone() {
                    app.playground.builder_loading = true;
                    app.status_msg = "running recipe…".into();
                    let req = StartRunRequest {
                        recipe_id: Some(id),
                        ..Default::default()
                    };
                    show_run(app, http.run_and_wait(&req).await);
                }
            } else {
                app.playground.builder_error =
                    Some("Save the recipe first (Ctrl+W) before running by id".into());
            }
            return Ok(true);
        }

        // --- Step list navigation ---
        KeyCode::Up => {
            app.playground.builder_step_focus = app.playground.builder_step_focus.saturating_sub(1);
            return Ok(true);
        }
        KeyCode::Down => {
            if step_count > 0 {
                app.playground.builder_step_focus =
                    (app.playground.builder_step_focus + 1).min(step_count - 1);
            }
            return Ok(true);
        }
        KeyCode::Enter => {
            if step_count > 0 {
                let idx = app.playground.builder_step_focus.min(step_count - 1);
                app.playground.builder_open_step = Some(idx);
                app.playground.builder_param_focus = 0;
                app.playground.builder_editing_param = false;
            }
            return Ok(true);
        }
        KeyCode::Char('d') if mods.contains(KeyModifiers::CONTROL) => {
            if step_count > 0 {
                let idx = app.playground.builder_step_focus.min(step_count - 1);
                app.playground.builder_steps.remove(idx);
                let new_len = app.playground.builder_steps.len();
                if new_len > 0 {
                    app.playground.builder_step_focus =
                        app.playground.builder_step_focus.min(new_len - 1);
                } else {
                    app.playground.builder_step_focus = 0;
                }
            }
            return Ok(true);
        }
        KeyCode::Char('j') if mods.contains(KeyModifiers::CONTROL) => {
            let i = app.playground.builder_step_focus;
            if i + 1 < step_count {
                app.playground.builder_steps.swap(i, i + 1);
                app.playground.builder_step_focus = i + 1;
            }
            return Ok(true);
        }
        KeyCode::Char('k') if mods.contains(KeyModifiers::CONTROL) => {
            let i = app.playground.builder_step_focus;
            if i > 0 {
                app.playground.builder_steps.swap(i, i - 1);
                app.playground.builder_step_focus = i - 1;
            }
            return Ok(true);
        }
        KeyCode::Char('c')
            if mods.contains(KeyModifiers::CONTROL) && mods.contains(KeyModifiers::SHIFT) =>
        {
            app.playground.builder_steps.clear();
            app.playground.builder_step_focus = 0;
            app.playground.builder_open_step = None;
            return Ok(true);
        }
        KeyCode::Tab | KeyCode::BackTab => return Ok(true),
        _ => {}
    }
    Ok(false)
}

/// Save the current builder state as a recipe (POST new or PUT existing).
async fn builder_save(app: &mut App) {
    let Some(http) = app.http.clone() else {
        app.playground.builder_error = Some("bridge not connected".into());
        return;
    };

    // Validate the recipe first
    let warnings = validate_recipe(&app.playground.builder_steps);
    if !warnings.is_empty() {
        let warning_text = warnings.join("\n");
        app.playground.builder_error = Some(format!("Validation warnings:\n{warning_text}"));
        app.status_msg = "recipe has validation issues - save anyway?".into();
        // Still allow saving, but surface the warnings
    }

    let steps: Vec<herdr_mcp_api::RecipeStep> = app
        .playground
        .builder_steps
        .iter()
        .map(RecipeStep::to_api)
        .collect();
    let name = if app.playground.builder_recipe_name.is_empty() {
        "untitled recipe".to_string()
    } else {
        app.playground.builder_recipe_name.clone()
    };

    let result = if let Some(id) = app.playground.builder_editing_recipe_id {
        let req = UpdateRecipeRequest {
            name: Some(name),
            steps: Some(steps),
            ..Default::default()
        };
        http.update_recipe(id, &req).await
    } else {
        let req = CreateRecipeRequest {
            name,
            description: None,
            steps,
            variables: Default::default(),
            category: None,
            triggers: Vec::new(),
        };
        http.create_recipe(&req).await
    };

    match result {
        Ok(recipe) => {
            app.playground.builder_editing_recipe_id = Some(recipe.id);
            app.playground.builder_error = None;
            app.status_msg = "recipe saved".into();
        }
        Err(e) => {
            app.playground.builder_error = Some(e.to_string());
            app.status_msg = format!("save failed: {e}");
        }
    }
}

/// Run the current builder steps inline (POST /api/recipe).
async fn builder_run_inline(app: &mut App) {
    let Some(http) = app.http.clone() else {
        app.playground.builder_error = Some("bridge not connected".into());
        return;
    };

    // Validate the recipe before running
    let warnings = validate_recipe(&app.playground.builder_steps);
    if !warnings.is_empty() {
        let warning_text = warnings.join("\n");
        app.playground.builder_error =
            Some(format!("Cannot run: validation errors:\n{warning_text}"));
        app.status_msg = "recipe validation failed".into();
        return;
    }

    let req = StartRunRequest {
        name: Some(app.playground.builder_recipe_name.clone()),
        steps: Some(
            app.playground
                .builder_steps
                .iter()
                .map(RecipeStep::to_api)
                .collect(),
        ),
        ..Default::default()
    };
    app.playground.builder_loading = true;
    app.status_msg = "running recipe…".into();
    show_run(app, http.run_and_wait(&req).await);
}

/// Put a finished run (or the request error) in the builder's result pane.
fn show_run(app: &mut App, run: Result<RunRecord, herdr_mcp_api::ApiError>) {
    match run {
        Ok(run) => {
            app.playground.builder_result = serde_json::to_value(&run).ok();
            if run.status == RunStatus::Completed {
                app.playground.builder_error = None;
                app.status_msg = "recipe run ok".into();
            } else {
                let err = run
                    .error
                    .unwrap_or_else(|| format!("status {:?}", run.status));
                app.playground.builder_error = Some(err.clone());
                app.status_msg = format!("recipe {:?}: {err}", run.status);
            }
        }
        Err(e) => {
            app.playground.builder_error = Some(e.to_string());
            app.playground.builder_result = None;
            app.status_msg = format!("run failed: {e}");
        }
    }
    app.playground.builder_loading = false;
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
    match http.call_tool(&name, &params).await {
        Ok(v) => {
            app.playground.result = Some(v.clone());
            app.playground.error = None;
            let pretty = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
            app.playground.result_area =
                crate::TextArea::new(pretty.lines().map(str::to_string).collect());
            app.status_msg = format!("{name} ok");

            // The bridge saved any auto-variables for this tool; show them.
            if let Ok(vars) = http.list_variables().await {
                app.variables_state.entries = crate::parse_variables(&vars);
            }
        }
        Err(e) => {
            app.playground.error = Some(e.to_string());
            app.playground.result = None;
            app.playground.result_area = crate::TextArea::new(vec![format!("error: {e}")]);
            app.status_msg = format!("{name} failed");
        }
    }
    // Persist last-used values so tab navigation + restarts restore context.
    app.persist_playground().await;
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
    let block = if app.playground.focused_frame == 0 {
        focused_panel_block(" Tools ")
    } else {
        panel_block(" Tools ")
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !app.bridge_connected {
        let msg = vec![
            Line::from("Not connected to herdr-mcp bridge."),
            Line::from(""),
            Line::from("It runs in this process: check herdr-mcp.log"),
            Line::from("in the data dir for why it did not start."),
            Line::from(""),
            Line::from(format!("Waiting for a bridge on :{} …", app.opts.http_port)),
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
    let block = if app.playground.focused_frame == 1 {
        focused_panel_block(" Parameters ")
    } else {
        panel_block(" Parameters ")
    };
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
    let block = if app.playground.focused_frame == 2 {
        focused_panel_block(" Result ")
    } else {
        panel_block(" Result ")
    };
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
    let pg = &app.playground;
    let _step_count = pg.builder_steps.len();
    let editing_name = pg.builder_editing_param && pg.builder_param_focus == usize::MAX;

    // --- Header line ---
    let recipe_label = if pg.builder_recipe_name.is_empty() {
        "(unnamed)".to_string()
    } else {
        pg.builder_recipe_name.clone()
    };
    let edit_marker = if pg.builder_editing_recipe_id.is_some() {
        " [saved]"
    } else {
        ""
    };
    let loading = if pg.builder_loading {
        "  ⟳ running…"
    } else {
        ""
    };
    let mut header_spans = vec![
        Span::styled(
            "Recipe Builder",
            accent_style().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("   {recipe_label}{edit_marker}"), accent_style()),
        Span::styled(
            "   (Ctrl+T pick  Ctrl+L lib  Ctrl+N name  Ctrl+W save  Ctrl+R run  Ctrl+V validate)",
            dim_style(),
        ),
        Span::styled(loading, warn_style()),
    ];
    if let Some(e) = &pg.builder_error {
        header_spans.push(Span::styled(format!("  ⚠ {e}"), warn_style()));
    }
    let header = Paragraph::new(Line::from(header_spans));
    frame.render_widget(header, Rect::new(area.x, area.y, area.width, 1));

    // --- Body ---
    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );

    // If library is open, split left 30% library | 70% main.
    let main_area = if pg.builder_show_library {
        let lc = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(body);
        render_library(frame, lc[0], app);
        lc[1]
    } else {
        body
    };

    // Main area: steps list (40%) | detail+result (60%).
    let mc = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(main_area);

    render_step_list(frame, mc[0], app);

    // Right side: detail (60%) | result (40%).
    let rc = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(mc[1]);

    render_step_detail(frame, rc[0], app);
    render_builder_result(frame, rc[1], app);

    // --- Tool picker overlay (centered modal) ---
    if pg.builder_picker_open {
        render_picker_overlay(frame, body, app);
    }

    // --- Name editing overlay (when editing recipe name) ---
    if editing_name {
        render_name_edit_overlay(frame, body, app);
    }
}

/// Render the steps list panel.
fn render_step_list(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let pg = &app.playground;
    let block = panel_block(" Steps ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if pg.builder_steps.is_empty() {
        let lines = vec![
            Line::from("No steps yet."),
            Line::from(""),
            Line::from("Ctrl+T  open tool picker"),
            Line::from("Ctrl+L  load from library"),
        ];
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    }

    let items: Vec<ListItem> = pg
        .builder_steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let focused = i == pg.builder_step_focus;
            let open = pg.builder_open_step == Some(i);
            let marker = if focused { "▸ " } else { "  " };
            let open_dot = if open { "●" } else { "○" };
            let style = if focused {
                selected_style()
            } else {
                Style::default()
            };
            let desc = step
                .description
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(30)
                .collect::<String>();
            let line = Line::from(vec![
                Span::styled(marker.to_string(), accent_style()),
                Span::styled(
                    format!("{open_dot} {} ", step.tool),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(desc, dim_style()),
            ]);
            ListItem::new(line)
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(pg.builder_step_focus));
    let list = List::new(items).highlight_symbol("");
    frame.render_stateful_widget(list, inner, &mut state);
}

/// Render the step detail / params panel.
fn render_step_detail(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let pg = &app.playground;
    let block = panel_block(" Detail ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(open_idx) = pg.builder_open_step else {
        let lines = vec![
            Line::from("Select a step and press Enter to view details."),
            Line::from(""),
            Line::from("↑/↓ steps   Enter open   Ctrl+T pick"),
        ];
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    };

    let Some(step) = pg.builder_steps.get(open_idx) else {
        frame.render_widget(Paragraph::new("step index out of range"), inner);
        return;
    };

    // If editing a param, show the TextArea.
    if pg.builder_editing_param && pg.builder_param_focus != usize::MAX {
        let header = Line::from(vec![
            Span::styled(
                "editing param".to_string(),
                accent_style().add_modifier(Modifier::BOLD),
            ),
            Span::styled("   (Enter/Esc confirm  Ctrl+D remove param)", dim_style()),
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
        let ta = pg.builder_param_edit_area.clone();
        frame.render_widget(&ta, edit_area);
        return;
    }

    // Normal detail view.
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("tool: ", dim_style()),
        Span::styled(
            step.tool.clone(),
            accent_style().add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("id: ", dim_style()),
        Span::styled(step.id.clone(), Style::default()),
    ]));
    if let Some(desc) = &step.description {
        lines.push(Line::from(vec![
            Span::styled("desc: ", dim_style()),
            Span::styled(desc.clone(), Style::default()),
        ]));
    }
    lines.push(Line::from(""));

    // Params.
    let mut keys: Vec<String> = step.params.keys().cloned().collect();
    keys.sort();
    if keys.is_empty() {
        lines.push(Line::from("No params (tool runs with defaults)."));
    } else {
        lines.push(Line::styled("Params:", accent_style()));
        for (i, key) in keys.iter().enumerate() {
            let val = step
                .params
                .get(key)
                .map(render_json_value)
                .unwrap_or_default();
            let focused = i == pg.builder_param_focus && !pg.builder_editing_param;
            let marker = if focused { "▸ " } else { "  " };
            let style = if focused {
                selected_style()
            } else {
                Style::default()
            };
            lines.push(Line::styled(format!("{marker}{key} = {val}"), style));
        }
    }

    // Available step references.
    if open_idx > 0 {
        lines.push(Line::from(""));
        let refs: Vec<String> = pg.builder_steps[..open_idx]
            .iter()
            .map(|s| s.id.clone())
            .collect();
        lines.push(Line::from(vec![
            Span::styled("step refs: ", dim_style()),
            Span::styled(refs.join(", "), muted_style()),
        ]));
    }

    // Available stored variables (for {{ variable_name }} interpolation)
    if !app.variables_state.entries.is_empty() {
        lines.push(Line::from(""));
        let var_keys: Vec<String> = app
            .variables_state
            .entries
            .iter()
            .filter(|(k, _)| !is_auto_stored_key(k) || k.starts_with("pane_")) // Show useful auto-stored vars
            .take(10)
            .map(|(k, _)| k.clone())
            .collect();
        if !var_keys.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("vars: ", dim_style()),
                Span::styled(var_keys.join(", "), muted_style()),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(
        "Enter edit param  ↑/↓ nav  Ctrl+D remove  Esc back  {{ var }} interpolate",
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Render a JSON value for display.
fn render_json_value(v: &Value) -> String {
    match v {
        Value::String(s) => {
            if s.len() > 40 {
                format!("\"{}…\"", &s[..37])
            } else {
                format!("\"{s}\"")
            }
        }
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(a) => format!("[…{} items]", a.len()),
        Value::Object(o) => format!("{{…{} keys}}", o.len()),
    }
}

/// Render the result / error panel.
fn render_builder_result(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let pg = &app.playground;
    let block = panel_block(" Result ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if pg.builder_loading {
        frame.render_widget(Paragraph::new("Running…"), inner);
        return;
    }

    let lines = if let Some(e) = &pg.builder_error {
        vec![Line::from(Span::styled(
            format!("error: {e}"),
            warn_style(),
        ))]
    } else if let Some(r) = &pg.builder_result {
        let pretty = serde_json::to_string_pretty(r).unwrap_or_else(|_| r.to_string());
        pretty
            .lines()
            .take(inner.height as usize)
            .map(|l| Line::from(l.to_string()))
            .collect()
    } else {
        vec![Line::from("(no result — Ctrl+R to run, or Ctrl+W to save)")]
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Render the library panel (templates + saved).
fn render_library(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let pg = &app.playground;
    let tab_label = match pg.builder_library_tab {
        crate::BuilderLibraryTab::Templates => "Templates",
        crate::BuilderLibraryTab::Saved => "Saved",
    };
    let block = panel_block(&format!(" Library [{tab_label}] "));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let hint = Line::from(vec![
        Span::styled("Tab ", dim_style()),
        Span::styled("switch", muted_style()),
        Span::styled("  Enter ", dim_style()),
        Span::styled("load", muted_style()),
        Span::styled("  x ", dim_style()),
        Span::styled("delete", muted_style()),
        Span::styled("  Esc ", dim_style()),
        Span::styled("close", muted_style()),
    ]);
    frame.render_widget(
        Paragraph::new(hint),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let list_area = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    );

    match pg.builder_library_tab {
        crate::BuilderLibraryTab::Templates => {
            // Same list (and order) the loader indexes into: `/api/templates`.
            let templates = app.templates.as_deref().unwrap_or_default();
            if templates.is_empty() {
                frame.render_widget(Paragraph::new("No templates (bridge offline?)."), list_area);
                return;
            }
            let items: Vec<ListItem> = templates
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let (name, cat) = (&t.name, &t.category);
                    let focused = i == pg.builder_library_focus;
                    let marker = if focused { "▸ " } else { "  " };
                    let style = if focused {
                        selected_style()
                    } else {
                        Style::default()
                    };
                    let line = Line::from(vec![
                        Span::styled(marker.to_string(), accent_style()),
                        Span::styled(
                            format!("{name}  [{cat}]"),
                            style.add_modifier(Modifier::BOLD),
                        ),
                    ]);
                    ListItem::new(line)
                })
                .collect();
            let mut state = ListState::default();
            state.select(Some(pg.builder_library_focus));
            let list = List::new(items).highlight_symbol("");
            frame.render_stateful_widget(list, list_area, &mut state);
        }
        crate::BuilderLibraryTab::Saved => {
            let recipes = app.recipes.as_ref();
            if let Some(arr) = recipes {
                if arr.is_empty() {
                    frame.render_widget(Paragraph::new("No saved recipes yet."), list_area);
                } else {
                    let items: Vec<ListItem> = arr
                        .iter()
                        .enumerate()
                        .map(|(i, r)| {
                            let focused = i == pg.builder_library_focus;
                            let marker = if focused { "▸ " } else { "  " };
                            let name = if r.name.is_empty() {
                                "(unnamed)"
                            } else {
                                &r.name
                            };
                            let step_count = r.steps.len();
                            let style = if focused {
                                selected_style()
                            } else {
                                Style::default()
                            };
                            let line = Line::from(vec![
                                Span::styled(marker.to_string(), accent_style()),
                                Span::styled(
                                    format!("{name}  ({step_count} steps)"),
                                    style.add_modifier(Modifier::BOLD),
                                ),
                            ]);
                            ListItem::new(line)
                        })
                        .collect();
                    let mut state = ListState::default();
                    state.select(Some(pg.builder_library_focus));
                    let list = List::new(items).highlight_symbol("");
                    frame.render_stateful_widget(list, list_area, &mut state);
                }
            } else {
                frame.render_widget(Paragraph::new("No saved recipes yet."), list_area);
            }
        }
    }
}

/// Render the centered tool picker overlay.
fn render_picker_overlay(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let pg = &app.playground;
    let w = (area.width * 70 / 100).max(40);
    let h = (area.height * 70 / 100).max(12);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Tool Picker ")
        .border_style(accent_style());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    // Three columns: categories | tools | search at bottom.
    let search_h = 3u16;
    let top_h = inner.height.saturating_sub(search_h);
    let top = Rect::new(inner.x, inner.y, inner.width, top_h);
    let bottom = Rect::new(inner.x, inner.y + top_h, inner.width, search_h);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(top);

    // Categories.
    let cat_items: Vec<ListItem> = pg
        .builder_tool_categories
        .iter()
        .enumerate()
        .map(|(i, cat)| {
            let focused = i == pg.builder_picker_category;
            let marker = if focused { "▸ " } else { "  " };
            let style = if focused {
                selected_style()
            } else {
                Style::default()
            };
            let line = Line::from(vec![
                Span::styled(marker.to_string(), accent_style()),
                Span::styled(format!("{} ({})", cat.label, cat.tools.len()), style),
            ]);
            ListItem::new(line)
        })
        .collect();
    let mut cat_state = ListState::default();
    cat_state.select(Some(pg.builder_picker_category));
    let cat_list = List::new(cat_items).highlight_symbol("");
    frame.render_stateful_widget(cat_list, cols[0], &mut cat_state);

    // Tools in current category.
    let cat_idx = pg
        .builder_picker_category
        .min(pg.builder_tool_categories.len().saturating_sub(1));
    let filtered = picker_filtered_tools(
        &pg.builder_tool_categories,
        cat_idx,
        &pg.builder_picker_search,
    );
    let tool_items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let focused = i == pg.builder_picker_tool;
            let marker = if focused { "▸ " } else { "  " };
            let style = if focused {
                selected_style()
            } else {
                Style::default()
            };
            let short_desc: String = desc.chars().take(50).collect();
            let line = Line::from(vec![
                Span::styled(marker.to_string(), accent_style()),
                Span::styled(name.clone(), style.add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {short_desc}"), dim_style()),
            ]);
            ListItem::new(line)
        })
        .collect();
    let mut tool_state = ListState::default();
    tool_state.select(Some(pg.builder_picker_tool));
    let tool_list = List::new(tool_items).highlight_symbol("");
    frame.render_stateful_widget(tool_list, cols[1], &mut tool_state);

    // Search bar.
    let search_block = Block::default()
        .borders(Borders::ALL)
        .title(" Search ")
        .border_style(dim_style());
    let search_inner = search_block.inner(bottom);
    frame.render_widget(search_block, bottom);
    let search_text = if pg.builder_picker_search.is_empty() {
        Line::from(Span::styled("type to filter…", dim_style()))
    } else {
        Line::from(vec![
            Span::styled(&pg.builder_picker_search, Style::default()),
            Span::styled("▌", accent_style()),
        ])
    };
    frame.render_widget(
        Paragraph::new(search_text),
        Rect::new(search_inner.x, search_inner.y, search_inner.width, 1),
    );
}

/// Render the recipe name edit overlay.
fn render_name_edit_overlay(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let w = (area.width * 50 / 100).max(30);
    let h = 5u16;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Recipe Name ")
        .border_style(accent_style());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let header = Line::from(vec![Span::styled("Enter confirm  Esc cancel", dim_style())]);
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
    let ta = app.playground.builder_param_edit_area.clone();
    frame.render_widget(&ta, edit_area);
}

//! Recipe library + builder. Port of `RecipeLibrary`/`RecipeBuilder`/
//! `RecipeStep` with one deliberate simplification: up/down buttons replace
//! `@dnd-kit` drag-sorting (spec §8).

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use super::forms::{DynamicForm, ResponseViewer, build_params};
use super::runner::{ToolInfo, schema_parts, tool_list};
use crate::client::use_api;
use crate::vars::substitute;
use herdr_mcp_api::{
    CreateRecipeRequest, InstantiateTemplateRequest, Recipe, RecipeStep, RecipeTemplate,
    StartRunRequest, UpdateRecipeRequest, Uuid,
};

#[derive(Clone, PartialEq)]
pub struct Step {
    pub id: String,
    pub tool: String,
    pub params: HashMap<String, String>,
    pub description: String,
}

fn step_from_api(i: usize, s: &RecipeStep) -> Step {
    let params = s
        .params
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, val)| {
                    let v = match val {
                        Value::String(v) => v.clone(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };
                    (k.clone(), v)
                })
                .collect()
        })
        .unwrap_or_default();
    Step {
        id: if s.id.is_empty() {
            format!("step{i}")
        } else {
            s.id.clone()
        },
        tool: s.tool.clone(),
        params,
        description: s.description.clone().unwrap_or_default(),
    }
}

/// A step as the server runs it: params typed per the tool's schema
/// (`{{…}}` placeholders stay strings until the run resolves them).
fn step_to_api(s: &Step, tools: &[ToolInfo]) -> RecipeStep {
    let schema = tools
        .iter()
        .find(|t| t.name == s.tool)
        .map(|t| t.schema.clone())
        .unwrap_or(Value::Null);
    let (props, _) = schema_parts(&schema);
    RecipeStep {
        id: s.id.clone(),
        tool: s.tool.clone(),
        params: build_params(&props, &s.params, str::to_string),
        description: Some(s.description.clone()),
    }
}

fn steps_from_api(steps: &[RecipeStep]) -> Vec<Step> {
    steps
        .iter()
        .enumerate()
        .map(|(i, s)| step_from_api(i, s))
        .collect()
}

fn group_label(name: &str) -> &str {
    match name.split('_').next().unwrap_or("") {
        "list" | "get" | "status" => "Reads",
        "create" | "split" | "close" => "Workspace/Tab",
        "start" | "send" | "run" => "Commands",
        "wait" => "Waits",
        "agent" | "var" => "Agents",
        "trim" | "compress" | "decompress" => "Trim",
        "proxy" | "clipboard" | "folder" => "Ops",
        _ => "Other",
    }
}

#[component]
pub fn RecipeBuilder(
    initial_name: String,
    initial_steps: Vec<Step>,
    on_saved: EventHandler<()>,
) -> Element {
    let mut tools: Signal<Vec<ToolInfo>> = use_signal(Vec::new);
    let mut name: Signal<String> = use_signal(|| initial_name.clone());
    let mut steps: Signal<Vec<Step>> = use_signal(|| initial_steps.clone());
    let mut results: Signal<Option<Value>> = use_signal(|| None);
    let mut loading: Signal<bool> = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let mut saved_id: Signal<Option<Uuid>> = use_signal(|| None);
    let client = use_api();
    let mut picker_open: Signal<bool> = use_signal(|| false);

    let fresh_id = |steps: &[Step]| {
        let max = steps
            .iter()
            .filter_map(|s| {
                s.id.strip_prefix("step")
                    .and_then(|n| n.parse::<usize>().ok())
            })
            .max()
            .unwrap_or(0);
        format!("step{}", max + 1)
    };
    let user_vars: Signal<HashMap<String, String>> = use_signal(HashMap::new);
    let auto_vars: Signal<HashMap<String, String>> = use_signal(HashMap::new);

    use_effect(move || {
        let client = client();
        spawn(async move {
            if let Ok(v) = client.list_tools().await {
                tools.set(tool_list(&v));
            }
        });
    });

    let run = move |_| {
        let client = client();
        spawn(async move {
            loading.set(true);
            error.set(None);
            results.set(None);
            let req = match *saved_id.read() {
                Some(id) => StartRunRequest {
                    recipe_id: Some(id),
                    ..Default::default()
                },
                None => StartRunRequest {
                    name: Some(name.read().clone()),
                    steps: Some(
                        steps
                            .read()
                            .iter()
                            .map(|s| step_to_api(s, &tools.read()))
                            .collect(),
                    ),
                    ..Default::default()
                },
            };
            let res = client
                .run_and_wait(&req)
                .await
                .map(|r| serde_json::to_value(r).unwrap_or_default());
            match res {
                Err(e) => error.set(Some(format!("Request failed: {e}"))),
                Ok(data) => results.set(Some(data)),
            }
            loading.set(false);
        });
    };

    let save = move |_| {
        let client = client();
        spawn(async move {
            let n = name.read().trim().to_string();
            if n.is_empty() {
                save_error.set(Some("Recipe name is required".to_string()));
                return;
            }
            if steps.read().is_empty() {
                save_error.set(Some("Add at least one step before saving".to_string()));
                return;
            }
            save_error.set(None);
            let payload: Vec<RecipeStep> = steps
                .read()
                .iter()
                .map(|s| step_to_api(s, &tools.read()))
                .collect();
            let saved = *saved_id.read();
            let res = match saved {
                Some(id) => {
                    let req = UpdateRecipeRequest {
                        name: Some(n),
                        steps: Some(payload),
                        ..Default::default()
                    };
                    client.update_recipe(id, &req).await
                }
                None => {
                    let req = CreateRecipeRequest {
                        name: n,
                        description: Some(format!("{} step(s)", payload.len())),
                        steps: payload,
                        variables: HashMap::new(),
                        category: None,
                    };
                    client.create_recipe(&req).await
                }
            };
            match res {
                Err(e) => save_error.set(Some(format!("Failed to save recipe: {e}"))),
                Ok(recipe) => {
                    saved_id.set(Some(recipe.id));
                    on_saved.call(());
                }
            }
        });
    };

    // Group tools for the add-step picker.
    let tools_list = tools.read().clone();
    let mut groups: HashMap<&str, Vec<ToolInfo>> = HashMap::new();
    for t in tools_list.iter() {
        groups
            .entry(group_label(&t.name))
            .or_default()
            .push(t.clone());
    }
    let mut groups: Vec<(&str, Vec<ToolInfo>)> = groups.into_iter().collect();
    groups.sort_by(|a, b| a.0.cmp(b.0));

    rsx! {
        div { class: "builder-head",
            input {
                class: "input",
                r#type: "text",
                placeholder: "Recipe name...",
                value: "{name}",
                oninput: move |e| name.set(e.value()),
            }
            if saved_id.read().is_some() {
                span { class: "pill", "Saved" }
            }
        }
        if steps.read().is_empty() {
            p { class: "muted", "No steps yet. Click \"+ Add Step\" to begin." }
        }
        for (i, step) in steps.read().iter().cloned().enumerate() {
            StepCard {
                key: "{step.id}",
                index: i,
                total: steps.read().len(),
                step: step.clone(),
                tools: tools.read().clone(),
                user_vars: user_vars.read().clone(),
                auto_vars: auto_vars.read().clone(),
                on_change: move |(idx, next): (usize, Step)| {
                    steps.write()[idx] = next;
                },
                on_remove: move |idx: usize| {
                    steps.write().remove(idx);
                },
                on_move: move |(idx, dir): (usize, i8)| {
                    let mut w = steps.write();
                    let j = idx as i64 + dir as i64;
                    if j >= 0 && (j as usize) < w.len() {
                        w.swap(idx, j as usize);
                    }
                },
            }
        }
        div { class: "btn-row",
            button {
                class: "btn",
                onclick: move |_| {
                    let open = *picker_open.read();
                    picker_open.set(!open);
                },
                "+ Add Step"
            }
            button {
                class: "btn btn-primary",
                disabled: *loading.read() || steps.read().is_empty(),
                onclick: run,
                if *loading.read() { "Running..." } else { "Run Recipe" }
            }
            button {
                class: "btn",
                disabled: steps.read().is_empty() || name.read().trim().is_empty(),
                onclick: save,
                if saved_id.read().is_some() { "Update" } else { "Save" }
            }
        }
        if *picker_open.read() {
            div { class: "card picker",
                for (label, group) in groups {
                    div { class: "var-group-title", "{label}" }
                    for t in group {
                        button {
                            class: "picker-item mono",
                            onclick: {
                                let tool = t.name.clone();
                                move |_| {
                                    let id = fresh_id(&steps.read());
                                    steps.write().push(Step {
                                        id,
                                        tool: tool.clone(),
                                        params: HashMap::new(),
                                        description: String::new(),
                                    });
                                    picker_open.set(false);
                                }
                            },
                            "{t.name}"
                        }
                    }
                }
            }
        }
        if let Some(e) = save_error.read().as_deref() {
            div { class: "alert alert-error", "{e}" }
        }
        if let Some(e) = error.read().as_deref() {
            div { class: "alert alert-error", "{e}" }
        }
        if let Some(data) = results.read().as_ref() {
            RunResults {
                data: data.clone(),
                order: steps.read().iter().map(|s| s.id.clone()).collect(),
            }
        }
    }
}

/// A finished run (`RunRecord`): its status, then each step's result in
/// step order.
#[component]
fn RunResults(data: Value, order: Vec<String>) -> Element {
    let status = data
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let error = data
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string);
    let results = data.get("results").cloned().unwrap_or(Value::Null);
    let ran: Vec<(usize, String, Value)> = order
        .iter()
        .enumerate()
        .filter_map(|(i, id)| Some((i + 1, id.clone(), results.get(id)?.clone())))
        .collect();
    let ok = status == "completed";
    rsx! {
        div { class: "card",
            div { class: "response-head",
                span { class: "response-title", "Run" }
                span { class: if ok { "pill" } else { "pill pill-error" }, "{status}" }
            }
            if let Some(e) = error {
                div { class: "alert alert-error", "{e}" }
            }
            for (n, id, result) in ran {
                div { key: "{id}",
                    div { class: "form-label", "#{n} {id}" }
                    match result.get("error").and_then(Value::as_str) {
                        Some(e) => rsx! { div { class: "alert alert-error", "{e}" } },
                        None => rsx! { ResponseViewer { data: result.clone() } },
                    }
                }
            }
        }
    }
}

#[component]
#[allow(clippy::too_many_arguments)]
fn StepCard(
    index: usize,
    total: usize,
    step: Step,
    tools: Vec<ToolInfo>,
    user_vars: HashMap<String, String>,
    auto_vars: HashMap<String, String>,
    on_change: EventHandler<(usize, Step)>,
    on_remove: EventHandler<usize>,
    on_move: EventHandler<(usize, i8)>,
) -> Element {
    let schema = tools
        .iter()
        .find(|t| t.name == step.tool)
        .map(|t| t.schema.clone())
        .unwrap_or(Value::Null);
    let (properties, required) = {
        let props: HashMap<String, Value> = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        let req: HashSet<String> = schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        (props, req)
    };
    let mut preview = HashMap::new();
    for (k, v) in step.params.iter() {
        preview.insert(k.clone(), substitute(v, &user_vars, &auto_vars));
    }

    rsx! {
        div { class: "card step-card",
            div { class: "step-head",
                span { class: "mono muted", "#{index + 1}" }
                select {
                    class: "input",
                    value: "{step.tool}",
                    onchange: {
                        let mut s = step.clone();
                        move |e| {
                            s.tool = e.value();
                            // Preserve params whose keys exist in the new schema.
                            if let Some(info) = tools.iter().find(|t| t.name == s.tool) {
                                let new_props: HashSet<String> = info
                                    .schema
                                    .get("properties")
                                    .and_then(|p| p.as_object())
                                    .map(|m| m.keys().cloned().collect())
                                    .unwrap_or_default();
                                s.params.retain(|k, _| new_props.contains(k));
                            } else {
                                s.params.clear();
                            }
                            on_change.call((index, s.clone()));
                        }
                    },
                    option { value: "", "— pick tool —" }
                    for t in tools.iter() {
                        option { key: "{t.name}", value: "{t.name}", "{t.name}" }
                    }
                }
                button { class: "link-btn", disabled: index == 0, onclick: move |_| on_move.call((index, -1)), "↑" }
                button { class: "link-btn", disabled: index + 1 >= total, onclick: move |_| on_move.call((index, 1)), "↓" }
                button { class: "link-btn link-danger", onclick: move |_| on_remove.call(index), "✕" }
            }
            input {
                class: "input",
                r#type: "text",
                placeholder: "Step description (optional)",
                value: "{step.description}",
                oninput: {
                    let mut s = step.clone();
                    move |e| {
                        s.description = e.value();
                        on_change.call((index, s.clone()));
                    }
                },
            }
            if !properties.is_empty() {
                DynamicForm {
                    properties: properties.clone(),
                    required: required.clone(),
                    values: step.params.clone(),
                    preview: preview.clone(),
                    on_change: {
                        let mut s = step.clone();
                        move |next| {
                            s.params = next;
                            on_change.call((index, s.clone()));
                        }
                    },
                }
            }
        }
    }
}

#[component]
pub fn RecipeLibrary(on_select: EventHandler<(String, Vec<Step>)>, refresh_key: u32) -> Element {
    let mut templates: Signal<Vec<RecipeTemplate>> = use_signal(Vec::new);
    let mut saved: Signal<Vec<Recipe>> = use_signal(Vec::new);
    // Template whose variables are being filled in.
    let mut filling: Signal<Option<RecipeTemplate>> = use_signal(|| None);
    let client = use_api();

    use_effect(move || {
        let client = client();
        spawn(async move {
            let _ = refresh_key;
            if let Ok(v) = client.list_templates().await {
                templates.set(v);
            }
            if let Ok(v) = client.list_recipes().await {
                saved.set(v);
            }
        });
    });

    let pick = move |(name, steps): (String, Vec<RecipeStep>)| {
        on_select.call((name, steps_from_api(&steps)));
    };

    let delete = move |id: Uuid| {
        let client = client();
        spawn(async move {
            if client.delete_recipe(id).await.is_ok() {
                saved.write().retain(|r| r.id != id);
            }
        });
    };

    rsx! {
        div {
            h3 { class: "card-title", "Recipe Library" }
            if !templates.read().is_empty() {
                div { class: "var-group-title", "Templates" }
                if let Some(tpl) = filling.read().clone() {
                    TemplateForm {
                        key: "{tpl.id}",
                        template: tpl,
                        on_load: move |(name, steps): (String, Vec<RecipeStep>)| {
                            filling.set(None);
                            pick((name, steps));
                        },
                        on_cancel: move |_| filling.set(None),
                    }
                }
                for tpl in templates.read().iter().cloned() {
                    {
                        let label = tpl.name.clone();
                        let desc = tpl.description.clone();
                        rsx! {
                            button {
                                class: "lib-item",
                                onclick: move |_| filling.set(Some(tpl.clone())),
                                div { class: "lib-name", "{label}" }
                                div { class: "lib-desc", "{desc}" }
                            }
                        }
                    }
                }
            }
            div { class: "var-group-title", "My Recipes ({saved.read().len()})" }
            if saved.read().is_empty() {
                p { class: "muted", "No saved recipes yet. Build one and click Save." }
            }
            for recipe in saved.read().iter().cloned() {
                {
                    let id = recipe.id;
                    let label = recipe.name.clone();
                    let desc = recipe.description.clone().unwrap_or_default();
                    let picked = (recipe.name.clone(), recipe.steps.clone());
                    rsx! {
                        div { class: "lib-row",
                            button {
                                class: "lib-item lib-item-flex",
                                onclick: move |_| pick(picked.clone()),
                                div { class: "lib-name", "{label}" }
                                div { class: "lib-desc", "{desc}" }
                            }
                            button {
                                class: "link-btn link-danger",
                                onclick: move |_| delete(id),
                                "Delete"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Variables of a template, filled in before it loads into the builder.
/// The server does the substitution (`/api/templates/{id}/instantiate`);
/// pane fields suggest the live pane ids.
#[component]
fn TemplateForm(
    template: RecipeTemplate,
    on_load: EventHandler<(String, Vec<RecipeStep>)>,
    on_cancel: EventHandler<()>,
) -> Element {
    let client = use_api();
    let mut values: Signal<HashMap<String, String>> = use_signal(|| {
        template
            .variables
            .iter()
            .map(|v| {
                let default = v.default_value.as_ref().map(crate::vars::display_value);
                (v.name.clone(), default.unwrap_or_default())
            })
            .collect()
    });
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let panes = use_resource(move || {
        let client = client();
        async move { live_panes(&client).await }
    });
    let missing = template.variables.iter().any(|v| {
        v.required
            && values
                .read()
                .get(&v.name)
                .is_none_or(|s| s.trim().is_empty())
    });
    let id = template.id.clone();
    let name = template.name.clone();
    let load = move |_| {
        let client = client();
        let (id, name) = (id.clone(), name.clone());
        let variables = values
            .read()
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        spawn(async move {
            let req = InstantiateTemplateRequest {
                variables,
                name: Some(name),
            };
            match client.instantiate_template(&id, &req).await {
                Ok(recipe) => on_load.call((recipe.name, recipe.steps)),
                Err(e) => error.set(Some(format!("Could not fill in template: {e}"))),
            }
        });
    };
    rsx! {
        div { class: "card",
            h3 { class: "card-title", "{template.name}" }
            p { class: "field-desc", "{template.description}" }
            datalist { id: "live-panes",
                for (pane, label) in panes.read().clone().unwrap_or_default() {
                    option { value: "{pane}", "{label}" }
                }
            }
            for var in template.variables.iter().cloned() {
                div { key: "{var.name}", class: "field",
                    label { class: "field-head",
                        code { class: "mono accent", "{var.name}" }
                        if var.required {
                            span { class: "required", "required" }
                        }
                    }
                    p { class: "field-desc", "{var.description}" }
                    input {
                        class: "input",
                        r#type: "text",
                        list: if var.name.contains("pane") { "live-panes" } else { "" },
                        value: "{values.read().get(&var.name).cloned().unwrap_or_default()}",
                        oninput: move |e| {
                            values.write().insert(var.name.clone(), e.value());
                        },
                    }
                }
            }
            if let Some(e) = error.read().as_deref() {
                div { class: "alert alert-error", "{e}" }
            }
            div { class: "btn-row",
                button {
                    class: "btn btn-primary",
                    disabled: missing,
                    onclick: load,
                    "Load into builder"
                }
                button { class: "btn", onclick: move |_| on_cancel.call(()), "Cancel" }
            }
        }
    }
}

/// `(pane_id, "label · cwd")` for every pane herdr reports (empty if the
/// call fails: suggestions are optional).
async fn live_panes(client: &crate::client::ApiClient) -> Vec<(String, String)> {
    let Ok(result) = client.call_tool("list_panes", &serde_json::json!({})).await else {
        return Vec::new();
    };
    let text = herdr_mcp_api::tool_text(&result);
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    json.pointer("/result/panes")
        .and_then(Value::as_array)
        .map(|panes| {
            panes
                .iter()
                .filter_map(|p| {
                    let id = p.get("pane_id")?.as_str()?.to_string();
                    let label = [p.get("label"), p.get("agent"), p.get("cwd")]
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" · ");
                    Some((id, label))
                })
                .collect()
        })
        .unwrap_or_default()
}

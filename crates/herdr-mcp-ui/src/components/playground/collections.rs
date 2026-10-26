//! Collections, Postman style. A collection is a saved recipe and its
//! requests are the recipe's steps: send one request (a run of just that
//! step, `step_ids`) or run the whole collection. Extract rules save part of
//! a result as a global or collection variable for the next request. Every
//! `{{ }}` resolves on the server, in the one scope runs use: the preview
//! asks `/api/variables/resolve`, sends and runs go through `/api/runs`.

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::HashMap;

use super::builders::{HttpBuilder, LlmWizard};
use super::flow::{GateEditor, TrailEditor};
use super::forms::{DynamicForm, ResponseViewer};
use super::recipes::{RunResults, TemplateForm, TriggerEditor};
use crate::client::use_api;
use crate::components::variables::{GlobalsTable, user_entries};
use herdr_mcp_api::playground::{
    HERDR_API, HTTP_REQUEST, LLM_CALL, ToolInfo, add_request, collections, delete_step,
    display_value, folders, fresh_collection_name, method_groups, methods, parse_value, raw_values,
    request_method, request_params, request_schema, request_values, result_paths,
    save_collection_var, save_step, send_summary, set_steps, sorted_vars, tool_groups, tool_label,
    tool_list, var_name,
};
use herdr_mcp_api::{
    CreateRecipeRequest, Extract, Recipe, RecipeStep, RecipeTemplate, ResolveRequest, RunRecord,
    StartRunRequest, UpdateRecipeRequest, Uuid, VarScope,
};

/// What the main panel shows.
#[derive(Clone, PartialEq, Debug)]
pub enum Pick {
    Nothing,
    Globals,
    Collection(Uuid),
    Request(Uuid, String),
}

#[component]
pub fn Collections() -> Element {
    let client = use_api();
    let pick: Signal<Pick> = use_signal(|| Pick::Nothing);
    let refresh: Signal<u32> = use_signal(|| 0);
    let recipes = use_resource(move || {
        let client = client();
        async move {
            let _ = refresh.read();
            client.list_recipes().await
        }
    });
    let tools = use_resource(move || {
        let client = client();
        async move { client.list_tools().await.map(|v| tool_list(&v)) }
    });

    let list = match &*recipes.read() {
        Some(Ok(v)) => v.clone(),
        Some(Err(e)) => {
            return rsx! { div { class: "alert alert-error", "Cannot load collections: {e}" } };
        }
        None => return rsx! { p { class: "muted", "Loading collections…" } },
    };
    let tools = match &*tools.read() {
        Some(Ok(t)) => t.clone(),
        _ => Vec::new(),
    };
    let current = pick.read().clone();
    let find = |id: Uuid| list.iter().find(|r| r.id == id).cloned();

    rsx! {
        div { class: "pg-grid",
            Sidebar { recipes: list.clone(), pick, refresh }
            div { class: "pg-main",
                match current {
                    Pick::Nothing => rsx! {
                        p { class: "muted",
                            "Pick a collection or a request on the left, or start a new collection."
                        }
                    },
                    Pick::Globals => rsx! { GlobalsView {} },
                    Pick::Collection(id) => match find(id) {
                        Some(recipe) => rsx! {
                            CollectionView {
                                key: "{id}",
                                recipe,
                                collections: collections(&list).into_iter().filter(|(c, _)| *c != id).collect::<Vec<_>>(),
                                pick,
                                refresh,
                            }
                        },
                        None => rsx! { p { class: "muted", "That collection is gone." } },
                    },
                    Pick::Request(id, step) => match find(id) {
                        Some(recipe) if recipe.steps.iter().any(|s| s.id == step) => rsx! {
                            RequestEditor {
                                key: "{id}/{step}",
                                recipe,
                                step_id: step.clone(),
                                tools,
                                pick,
                                refresh,
                            }
                        },
                        _ => rsx! { p { class: "muted", "That request is gone." } },
                    },
                }
            }
        }
    }
}

#[component]
fn Sidebar(recipes: Vec<Recipe>, pick: Signal<Pick>, refresh: Signal<u32>) -> Element {
    let client = use_api();
    let templates = use_resource(move || {
        let client = client();
        async move { client.list_templates().await.unwrap_or_default() }
    });
    let mut filling: Signal<Option<RecipeTemplate>> = use_signal(|| None);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let create = move |(name, steps): (String, Vec<RecipeStep>)| {
        let client = client();
        spawn(async move {
            let req = CreateRecipeRequest {
                name,
                description: None,
                steps,
                variables: HashMap::new(),
                category: None,
                triggers: Vec::new(),
                gate: None,
                trail: Default::default(),
            };
            match client.create_recipe(&req).await {
                Ok(r) => {
                    error.set(None);
                    pick.set(Pick::Collection(r.id));
                    refresh += 1;
                }
                Err(e) => error.set(Some(format!("Could not create the collection: {e}"))),
            }
        });
    };
    let sel = |p: Pick| if *pick.read() == p { " tree-sel" } else { "" };
    let new_name = fresh_collection_name(&recipes);

    rsx! {
        div { class: "pg-side",
            button {
                class: "tree-item{sel(Pick::Globals)}",
                onclick: move |_| pick.set(Pick::Globals),
                "Globals"
            }
            button {
                class: "btn btn-block",
                onclick: move |_| create((new_name.clone(), Vec::new())),
                "+ New collection"
            }
            if let Some(e) = error.read().as_deref() {
                div { class: "alert alert-error", "{e}" }
            }
            for (folder, items) in folders(&recipes) {
                if !folder.is_empty() {
                    div { class: "tree-folder", "{folder}" }
                }
                for r in items {
                    {
                        let id = r.id;
                        let triggers = r.triggers.len();
                        rsx! {
                            button {
                                key: "{id}",
                                class: "tree-item{sel(Pick::Collection(id))}",
                                onclick: move |_| pick.set(Pick::Collection(id)),
                                span { class: "tree-name", "{r.name}" }
                                if triggers > 0 {
                                    span { class: "pill", title: "triggers", "⚡ {triggers}" }
                                }
                            }
                            for s in r.steps.iter().cloned() {
                                {
                                    let target = Pick::Request(id, s.id.clone());
                                    rsx! {
                                        button {
                                            key: "{id}/{s.id}",
                                            class: "tree-item tree-req{sel(target.clone())}",
                                            onclick: move |_| pick.set(target.clone()),
                                            span { class: "mono", "{s.id}" }
                                            span { class: "muted mono", "{tool_label(&s)}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            details { class: "details",
                summary { "New from a template" }
                if let Some(tpl) = filling.read().clone() {
                    TemplateForm {
                        key: "{tpl.id}",
                        template: tpl,
                        on_load: move |made: (String, Vec<RecipeStep>)| {
                            filling.set(None);
                            create(made);
                        },
                        on_cancel: move |_| filling.set(None),
                    }
                }
                for tpl in templates.read().clone().unwrap_or_default() {
                    {
                        let (label, desc) = (tpl.name.clone(), tpl.description.clone());
                        rsx! {
                            button {
                                key: "{tpl.id}",
                                class: "lib-item",
                                onclick: move |_| filling.set(Some(tpl.clone())),
                                div { class: "lib-name", "{label}" }
                                div { class: "lib-desc", "{desc}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Global variables: every request and run sees them.
#[component]
fn GlobalsView() -> Element {
    let client = use_api();
    let mut refresh: Signal<u32> = use_signal(|| 0);
    let mut search: Signal<String> = use_signal(String::new);
    let vars = use_resource(move || {
        let client = client();
        async move {
            let _ = refresh.read();
            client.list_variables().await
        }
    });
    let entries = match &*vars.read() {
        Some(Ok(v)) => user_entries(v),
        _ => Vec::new(),
    };
    rsx! {
        h2 { class: "card-title", "Globals" }
        p { class: "field-desc",
            "Every request and run sees these. Extract rules with scope "
            code { "global" }
            " write them; a collection's own variables win over them."
        }
        div { class: "form-row",
            input {
                class: "input",
                r#type: "text",
                placeholder: "Filter variables...",
                value: "{search}",
                oninput: move |e| search.set(e.value()),
            }
        }
        GlobalsTable {
            entries,
            search: search.read().clone(),
            on_changed: move |_| refresh += 1,
        }
    }
}

#[component]
fn CollectionView(
    recipe: Recipe,
    collections: Vec<(Uuid, String)>,
    pick: Signal<Pick>,
    refresh: Signal<u32>,
) -> Element {
    let client = use_api();
    let id = recipe.id;
    let mut name = use_signal(|| recipe.name.clone());
    let mut folder = use_signal(|| recipe.category.clone().unwrap_or_default());
    let mut vars: Signal<Vec<(String, String)>> = use_signal(|| sorted_vars(&recipe.variables));
    let mut triggers = use_signal(|| recipe.triggers.clone());
    let gate = use_signal(|| {
        recipe.gate.clone().unwrap_or(herdr_mcp_api::Gate {
            rules: Vec::new(),
            max_steps: herdr_mcp_api::gate::DEFAULT_MAX_STEPS,
        })
    });
    let trail = use_signal(|| recipe.trail);
    let mut run: Signal<Option<Value>> = use_signal(|| None);
    let mut busy = use_signal(|| false);
    let mut msg: Signal<Option<Result<String, String>>> = use_signal(|| None);
    let steps = recipe.steps.clone();

    let save = move |_| {
        let client = client();
        spawn(async move {
            let variables = vars
                .read()
                .iter()
                .filter(|(k, _)| !k.trim().is_empty())
                .map(|(k, v)| (k.trim().to_string(), parse_value(v)))
                .collect();
            let req = UpdateRecipeRequest {
                name: Some(name.read().trim().to_string()).filter(|n| !n.is_empty()),
                category: Some(folder.read().trim().to_string()),
                variables: Some(variables),
                triggers: Some(triggers.read().clone()),
                // No rules clears the gate.
                gate: Some(gate.read().clone()),
                trail: Some(*trail.read()),
                ..Default::default()
            };
            match client.update_recipe(id, &req).await {
                Ok(_) => {
                    msg.set(Some(Ok("saved".into())));
                    refresh += 1;
                }
                Err(e) => msg.set(Some(Err(format!("save failed: {e}")))),
            }
        });
    };
    let reorder = move |next: Vec<RecipeStep>| {
        let client = client();
        spawn(async move {
            match set_steps(&client, id, next).await {
                Ok(()) => refresh += 1,
                Err(e) => msg.set(Some(Err(e))),
            }
        });
    };
    let new_request = move |tool: &'static str, params: Value| {
        let client = client();
        spawn(async move {
            match add_request(&client, id, tool, params).await {
                Ok(step) => {
                    pick.set(Pick::Request(id, step));
                    refresh += 1;
                }
                Err(e) => msg.set(Some(Err(e))),
            }
        });
    };
    let run_all = move |_| {
        let client = client();
        spawn(async move {
            busy.set(true);
            msg.set(None);
            let req = StartRunRequest {
                recipe_id: Some(id),
                ..Default::default()
            };
            match client.run_and_wait(&req).await {
                Ok(r) => run.set(Some(serde_json::to_value(r).unwrap_or_default())),
                Err(e) => msg.set(Some(Err(format!("run failed: {e}")))),
            }
            // Extract rules may have written collection variables.
            if let Ok(r) = client.get_recipe(id).await {
                vars.set(sorted_vars(&r.variables));
            }
            busy.set(false);
        });
    };
    let delete = move |_| {
        let sure = web_sys::window()
            .and_then(|w| w.confirm_with_message("Delete this collection?").ok())
            .unwrap_or(false);
        if !sure {
            return;
        }
        let client = client();
        spawn(async move {
            match client.delete_recipe(id).await {
                Ok(()) => {
                    pick.set(Pick::Nothing);
                    refresh += 1;
                }
                Err(e) => msg.set(Some(Err(format!("delete failed: {e}")))),
            }
        });
    };

    rsx! {
        div { class: "card",
            div { class: "form-row",
                div { class: "form-col",
                    label { class: "form-label", "Collection" }
                    input {
                        class: "input",
                        r#type: "text",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div { class: "form-col",
                    label { class: "form-label", "Folder" }
                    input {
                        class: "input",
                        r#type: "text",
                        placeholder: "none",
                        value: "{folder}",
                        oninput: move |e| folder.set(e.value()),
                    }
                }
            }
            div { class: "var-group-title", "Requests · {steps.len()}" }
            if steps.is_empty() {
                p { class: "muted", "No requests yet." }
            }
            for (i, s) in steps.iter().cloned().enumerate() {
                {
                    let target = Pick::Request(id, s.id.clone());
                    let (up, down, gone) = (steps.clone(), steps.clone(), steps.clone());
                    let total = steps.len();
                    rsx! {
                        div { key: "{s.id}", class: "req-row",
                            button {
                                class: "tree-item",
                                onclick: move |_| pick.set(target.clone()),
                                span { class: "mono accent", "{s.id}" }
                                span { class: "muted mono", "{tool_label(&s)}" }
                                if !s.extract.is_empty() {
                                    span { class: "pill", "→ {s.extract.len()} var(s)" }
                                }
                            }
                            button {
                                class: "link-btn",
                                disabled: i == 0,
                                onclick: move |_| {
                                    let mut next = up.clone();
                                    next.swap(i, i - 1);
                                    reorder(next);
                                },
                                "↑"
                            }
                            button {
                                class: "link-btn",
                                disabled: i + 1 >= total,
                                onclick: move |_| {
                                    let mut next = down.clone();
                                    next.swap(i, i + 1);
                                    reorder(next);
                                },
                                "↓"
                            }
                            button {
                                class: "link-btn link-danger",
                                onclick: move |_| {
                                    let mut next = gone.clone();
                                    next.remove(i);
                                    reorder(next);
                                },
                                "✕"
                            }
                        }
                    }
                }
            }
            div { class: "btn-row",
                button {
                    class: "btn",
                    onclick: move |_| new_request("list_panes", serde_json::json!({})),
                    "+ Request"
                }
                button {
                    class: "btn",
                    title: "A custom HTTP request: method, URL, query, headers, body",
                    onclick: move |_| new_request(HTTP_REQUEST, serde_json::json!({ "method": "GET", "url": "https://" })),
                    "+ HTTP"
                }
                button {
                    class: "btn",
                    title: "An OpenRouter / OpenAI-compatible model call",
                    onclick: move |_| new_request(LLM_CALL, serde_json::json!({ "model": "" })),
                    "+ LLM"
                }
                button {
                    class: "btn btn-primary",
                    disabled: busy() || steps.is_empty(),
                    onclick: run_all,
                    if busy() { "Running…" } else { "Run collection" }
                }
            }
        }
        div { class: "card",
            div { class: "var-group-title", "Collection variables" }
            p { class: "field-desc",
                "Requests in this collection see these over globals. Extract rules with scope "
                code { "collection" }
                " write them. Values that parse as JSON are stored typed."
            }
            for (i, (k, v)) in vars.read().iter().cloned().enumerate() {
                div { key: "{i}", class: "kv-row",
                    input {
                        class: "input mono",
                        r#type: "text",
                        placeholder: "name",
                        value: "{k}",
                        oninput: move |e| vars.write()[i].0 = e.value(),
                    }
                    input {
                        class: "input mono",
                        r#type: "text",
                        placeholder: "value",
                        value: "{v}",
                        oninput: move |e| vars.write()[i].1 = e.value(),
                    }
                    button {
                        class: "link-btn link-danger",
                        onclick: move |_| {
                            vars.write().remove(i);
                        },
                        "✕"
                    }
                }
            }
            button {
                class: "btn",
                onclick: move |_| vars.write().push((String::new(), String::new())),
                "+ Variable"
            }
        }
        GateEditor {
            gate,
            collections,
            paths: steps
                .iter()
                .map(|s| s.id.clone())
                .chain(["run.pass".to_string()])
                .collect::<Vec<_>>(),
        }
        TriggerEditor {
            triggers: triggers.read().clone(),
            on_change: move |next| triggers.set(next),
        }
        TrailEditor { trail }
        div { class: "btn-row",
            button { class: "btn btn-primary", onclick: save, "Save collection" }
            button { class: "btn link-danger", onclick: delete, "Delete collection" }
            match &*msg.read() {
                Some(Ok(m)) => rsx! { span { class: "send-status", "{m}" } },
                Some(Err(m)) => rsx! { span { class: "send-status send-error", "{m}" } },
                None => rsx! {},
            }
        }
        if let Some(data) = run.read().clone() {
            RunResults { data, order: steps.iter().map(|s| s.id.clone()).collect() }
        }
    }
}

#[component]
fn RequestEditor(
    recipe: Recipe,
    step_id: String,
    tools: Vec<ToolInfo>,
    pick: Signal<Pick>,
    refresh: Signal<u32>,
) -> Element {
    let client = use_api();
    let rid = recipe.id;
    let step = recipe
        .steps
        .iter()
        .find(|s| s.id == step_id)
        .cloned()
        .unwrap_or_default();
    let mut id = use_signal(|| step.id.clone());
    let mut description = use_signal(|| step.description.clone().unwrap_or_default());
    let mut tool = use_signal(|| step.tool.clone());
    let mut method = use_signal(|| request_method(&step));
    let mut values = use_signal(|| request_values(&step));
    let mut extract = use_signal(|| step.extract.clone());
    let mut filter = use_signal(String::new);
    // HTTP and LLM requests open in their builder; this shows the plain fields.
    let mut raw_form = use_signal(|| false);
    let mut response: Signal<Option<RunRecord>> = use_signal(|| None);
    let mut busy = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut notice: Signal<Option<String>> = use_signal(|| None);

    // herdr's methods, fetched when the request calls `herdr_api`.
    let schema = use_resource(move || {
        let client = client();
        let wanted = *tool.read() == HERDR_API;
        async move {
            if wanted {
                client.herdr_schema().await.map(|s| methods(&s))
            } else {
                Ok(Vec::new())
            }
        }
    });
    let methods_list = match &*schema.read() {
        Some(Ok(m)) => m.clone(),
        _ => Vec::new(),
    };
    let (props, required) = request_schema(&tools, &methods_list, &tool.read(), &method.read());

    let memo_tools = tools.clone();
    let built = use_memo(move || {
        let methods_list = match &*schema.read() {
            Some(Ok(m)) => m.clone(),
            _ => Vec::new(),
        };
        let (props, _) = request_schema(&memo_tools, &methods_list, &tool.read(), &method.read());
        request_params(&props, &tool.read(), &method.read(), &values.read())
    });
    let preview = use_resource(move || {
        let client = client();
        let value = built();
        async move {
            let req = ResolveRequest {
                recipe_id: Some(rid),
                value,
                ..Default::default()
            };
            client.resolve(&req).await
        }
    });
    let (resolved, unresolved) = match &*preview.read() {
        Some(Ok(r)) => (r.value.clone(), r.unresolved.clone()),
        _ => (Value::Null, Vec::new()),
    };
    let preview_fields = raw_values(if *tool.read() == HERDR_API {
        &resolved["params"]
    } else {
        &resolved
    });

    let current_step = move || RecipeStep {
        id: id.read().trim().to_string(),
        tool: tool.read().clone(),
        params: built(),
        description: Some(description.read().trim().to_string()).filter(|d| !d.is_empty()),
        extract: extract
            .read()
            .iter()
            .filter(|x| !x.var.trim().is_empty())
            .cloned()
            .collect(),
    };
    let old_id = step_id.clone();
    // Save; true when the request kept its name (a rename re-opens it).
    let save = move || {
        let client = client();
        let old_id = old_id.clone();
        let next = current_step();
        async move {
            let new_id = next.id.clone();
            match save_step(&client, rid, &old_id, next).await {
                Err(e) => {
                    error.set(Some(e));
                    false
                }
                Ok(()) => {
                    error.set(None);
                    refresh += 1;
                    if new_id != old_id {
                        pick.set(Pick::Request(rid, new_id));
                        return false;
                    }
                    true
                }
            }
        }
    };
    let on_save = {
        let save = save.clone();
        move |_| {
            let save = save.clone();
            spawn(async move {
                save().await;
            });
        }
    };
    let send = {
        let save = save.clone();
        move |_| {
            let save = save.clone();
            let client = client();
            spawn(async move {
                busy.set(true);
                if save().await {
                    let req = StartRunRequest {
                        recipe_id: Some(rid),
                        step_ids: vec![id.read().trim().to_string()],
                        ..Default::default()
                    };
                    match client.run_and_wait(&req).await {
                        Ok(run) => response.set(Some(run)),
                        Err(e) => error.set(Some(format!("send failed: {e}"))),
                    }
                    // Extract rules may have written collection variables;
                    // the save's refresh fetched the list before they did.
                    refresh += 1;
                }
                busy.set(false);
            });
        }
    };
    let delete = {
        let old_id = step_id.clone();
        move |_| {
            let client = client();
            let old_id = old_id.clone();
            spawn(async move {
                let result = delete_step(&client, rid, &old_id).await;
                match result {
                    Ok(()) => {
                        pick.set(Pick::Collection(rid));
                        refresh += 1;
                    }
                    Err(e) => error.set(Some(e)),
                }
            });
        }
    };

    let herdr = *tool.read() == HERDR_API;
    let description_text = tools
        .iter()
        .find(|t| t.name == *tool.read())
        .map(|t| t.description.clone())
        .unwrap_or_default();

    rsx! {
        div { class: "crumb",
            button {
                class: "link-btn",
                onclick: move |_| pick.set(Pick::Collection(rid)),
                "{recipe.name}"
            }
            " › {step_id}"
        }
        div { class: "runner-grid",
            div { class: "runner-left",
                div { class: "form-row",
                    div { class: "form-col",
                        label { class: "form-label", "Request" }
                        input {
                            class: "input mono",
                            r#type: "text",
                            value: "{id}",
                            oninput: move |e| id.set(e.value()),
                        }
                    }
                    div { class: "form-col",
                        label { class: "form-label", "Tool" }
                        select {
                            class: "input",
                            value: "{tool}",
                            onchange: move |e| {
                                tool.set(e.value());
                                values.set(HashMap::new());
                            },
                            for (label, group) in tool_groups(&tools) {
                                optgroup { key: "{label}", label: "{label}",
                                    for t in group {
                                        option {
                                            key: "{t.name}",
                                            value: "{t.name}",
                                            selected: *tool.read() == t.name,
                                            "{t.name}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                input {
                    class: "input",
                    r#type: "text",
                    placeholder: "Description (optional)",
                    value: "{description}",
                    oninput: move |e| description.set(e.value()),
                }
                if !description_text.is_empty() {
                    p { class: "field-desc", "{description_text}" }
                }
                if herdr {
                    div { class: "form-block",
                        label { class: "form-label", "herdr method" }
                        input {
                            class: "input",
                            r#type: "text",
                            placeholder: "filter, e.g. pane.read",
                            value: "{filter}",
                            oninput: move |e| filter.set(e.value()),
                        }
                        select {
                            class: "input",
                            size: "8",
                            onchange: move |e| {
                                method.set(e.value());
                                values.set(HashMap::new());
                            },
                            for (group, names) in method_groups(&methods_list, &filter.read()) {
                                optgroup { key: "{group}", label: "{group}",
                                    for n in names {
                                        option {
                                            key: "{n}",
                                            value: "{n}",
                                            selected: *method.read() == n,
                                            "{n}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if *tool.read() == HTTP_REQUEST || *tool.read() == LLM_CALL {
                    label { class: "check-row",
                        input {
                            r#type: "checkbox",
                            checked: raw_form(),
                            onchange: move |_| raw_form.toggle(),
                        }
                        span { class: "muted", "Plain fields" }
                    }
                }
                if *tool.read() == HTTP_REQUEST && !raw_form() {
                    HttpBuilder {
                        key: "{rid}/{step_id}/http",
                        values: values.read().clone(),
                        on_change: move |next| values.set(next),
                    }
                } else if *tool.read() == LLM_CALL && !raw_form() {
                    LlmWizard {
                        key: "{rid}/{step_id}/llm",
                        values: values.read().clone(),
                        on_change: move |next| values.set(next),
                    }
                } else if props.is_empty() {
                    p { class: "muted", "No parameters" }
                } else {
                    DynamicForm {
                        key: "{tool}/{method}",
                        properties: props.clone(),
                        required: required.clone(),
                        values: values.read().clone(),
                        preview: preview_fields,
                        on_change: move |next| values.set(next),
                    }
                }
                if !unresolved.is_empty() {
                    p { class: "field-desc",
                        "Not resolved before sending: "
                        for u in unresolved.iter() {
                            code { key: "{u}", "{{{{{u}}}}} " }
                        }
                        "(another request's result only exists while a run is going; extract it into a variable to chain separate sends)."
                    }
                }
                details { class: "details",
                    summary { "Resolved params" }
                    pre { class: "mono pre-wrap code-block",
                        "{serde_json::to_string_pretty(&resolved).unwrap_or_default()}"
                    }
                }
                ExtractEditor {
                    rules: extract.read().clone(),
                    on_change: move |next| extract.set(next),
                }
                div { class: "btn-row",
                    button {
                        class: "btn btn-primary",
                        disabled: busy(),
                        onclick: send,
                        title: "Saves the request, then runs it",
                        if busy() { "Sending…" } else { "Send" }
                    }
                    button { class: "btn", onclick: on_save, "Save" }
                    button { class: "btn link-danger", onclick: delete, "Delete" }
                }
                if let Some(e) = error.read().as_deref() {
                    div { class: "alert alert-error mono pre-wrap", "{e}" }
                }
                if let Some(n) = notice.read().as_deref() {
                    div { class: "alert", "{n}" }
                }
            }
            div { class: "runner-right",
                match response.read().clone() {
                    None => rsx! { p { class: "muted", "Send the request to see its response." } },
                    Some(run) => rsx! {
                        SendResult {
                            run,
                            step_id: id.read().trim().to_string(),
                            rules: extract.read().clone(),
                            // Keep the value as a collection variable now, and
                            // the rule (saved with the request) for later sends.
                            on_extract: {
                                let save = save.clone();
                                move |(path, value): (String, Value)| {
                                    let var = var_name(&path);
                                    extract.write().push(Extract {
                                        path: path.clone(),
                                        var: var.clone(),
                                        scope: VarScope::Collection,
                                    });
                                    let save = save.clone();
                                    let client = client();
                                    spawn(async move {
                                        if !save().await {
                                            return;
                                        }
                                        match save_collection_var(&client, rid, None, &var, value).await {
                                            Ok(()) => {
                                                notice.set(Some(format!("{path} saved as collection variable {var}")));
                                                refresh += 1;
                                            }
                                            Err(e) => error.set(Some(format!("could not save {var}: {e}"))),
                                        }
                                    });
                                }
                            },
                        }
                    },
                }
            }
        }
    }
}

/// The request's extract rules: path → variable, global or collection.
#[component]
fn ExtractEditor(rules: Vec<Extract>, on_change: EventHandler<Vec<Extract>>) -> Element {
    rsx! {
        div { class: "card",
            div { class: "var-group-title", "Extract → variables" }
            p { class: "field-desc",
                "After a successful send or run, save part of the result as a variable for the next request. The path is followed into the result (e.g. "
                code { "panes[0].pane_id" }
                "); empty saves the text the tool returned. Click a path in the response to save its value as a collection variable and add its rule."
            }
            for (i, x) in rules.iter().cloned().enumerate() {
                div { key: "{i}", class: "extract-row",
                    input {
                        class: "input mono",
                        r#type: "text",
                        placeholder: "path",
                        value: "{x.path}",
                        oninput: {
                            let rules = rules.clone();
                            move |e: FormEvent| {
                                let mut next = rules.clone();
                                next[i].path = e.value();
                                on_change.call(next);
                            }
                        },
                    }
                    input {
                        class: "input mono",
                        r#type: "text",
                        placeholder: "variable",
                        value: "{x.var}",
                        oninput: {
                            let rules = rules.clone();
                            move |e: FormEvent| {
                                let mut next = rules.clone();
                                next[i].var = e.value();
                                on_change.call(next);
                            }
                        },
                    }
                    select {
                        class: "input",
                        onchange: {
                            let rules = rules.clone();
                            move |e: FormEvent| {
                                let mut next = rules.clone();
                                next[i].scope = if e.value() == "collection" {
                                    VarScope::Collection
                                } else {
                                    VarScope::Global
                                };
                                on_change.call(next);
                            }
                        },
                        option { value: "global", selected: x.scope == VarScope::Global, "global" }
                        option {
                            value: "collection",
                            selected: x.scope == VarScope::Collection,
                            "collection"
                        }
                    }
                    button {
                        class: "link-btn link-danger",
                        onclick: {
                            let rules = rules.clone();
                            move |_| {
                                let mut next = rules.clone();
                                next.remove(i);
                                on_change.call(next);
                            }
                        },
                        "✕"
                    }
                }
            }
            button {
                class: "btn",
                onclick: {
                    let rules = rules.clone();
                    move |_| {
                        let mut next = rules.clone();
                        next.push(Extract {
                            path: String::new(),
                            var: String::new(),
                            scope: VarScope::Collection,
                        });
                        on_change.call(next);
                    }
                },
                "+ Extract"
            }
        }
    }
}

/// One send's outcome: status, the tool's response, the variables its
/// extract rules set, and the result's paths (click one to extract it).
#[component]
fn SendResult(
    run: RunRecord,
    step_id: String,
    rules: Vec<Extract>,
    on_extract: EventHandler<(String, Value)>,
) -> Element {
    let status = send_summary(&run, &step_id);
    let ok = run.status == herdr_mcp_api::RunStatus::Completed;
    let result = run.results.get(&step_id).cloned();
    let tool_result = result.clone().filter(|r| r.get("error").is_none());
    let payload = tool_result
        .as_ref()
        .and_then(herdr_mcp_api::tool_payload)
        .unwrap_or(Value::Null);
    let set: Vec<(String, String, String)> = if ok {
        rules
            .iter()
            .filter_map(|x| {
                let v = run.variables.get(&x.var)?;
                let scope = match x.scope {
                    VarScope::Global => "global",
                    VarScope::Collection => "collection",
                };
                Some((x.var.clone(), display_value(v), scope.to_string()))
            })
            .collect()
    } else {
        Vec::new()
    };
    let paths = result_paths(&payload, 200);

    rsx! {
        div { class: "card",
            div { class: "response-head",
                span { class: "response-title", "Sent" }
                span { class: if ok { "pill" } else { "pill pill-error" }, "{status}" }
            }
            if let Some(e) = run.error.as_deref() {
                div { class: "alert alert-error mono pre-wrap", "{e}" }
            }
            if !set.is_empty() {
                div { class: "var-group-title", "Variables set" }
                for (var, value, scope) in set {
                    div { key: "{var}", class: "mono",
                        span { class: "accent", "{var}" }
                        " = {value} "
                        span { class: "muted", "({scope})" }
                    }
                }
            }
        }
        if let Some(data) = tool_result {
            ResponseViewer { data }
        }
        if !paths.is_empty() {
            details { class: "details", open: true,
                summary { "Result paths · click to extract" }
                table { class: "table",
                    tbody {
                        for (path, value) in paths {
                            {
                                let p = (path.clone(), value.clone());
                                let shown = display_value(&value);
                                rsx! {
                                    tr {
                                        key: "{path}",
                                        class: "row-pick",
                                        onclick: move |_| on_extract.call(p.clone()),
                                        td { code { class: "mono accent", "{path}" } }
                                        td { class: "muted cell-path", title: "{shown}", "{shown}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

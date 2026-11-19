//! Collections. A collection is a saved recipe whose flow is drawn on a
//! canvas: add nodes, wire their ports, pick one to edit it. A tool node is
//! a request: send it alone (a run of just that node, `node_ids`) or run
//! the whole flow and watch its nodes light up. Extract rules save part of
//! a result as a global or collection variable. Every `{{ }}` resolves on
//! the server, in the one scope runs use: the preview asks
//! `/api/variables/resolve`, sends and runs go through `/api/runs`.

use dioxus::prelude::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::rc::Rc;

use super::builders::{HttpBuilder, LlmWizard};
use super::canvas::{Canvas, CanvasEdit};
use super::flow::{NodeEditor, TrailEditor};
use super::forms::{DynamicForm, ResponseViewer};
use super::recipes::{RunResults, TemplateForm, TriggerEditor};
use super::runs::{changed, use_run_events};
use crate::client::{ApiClient, use_api};
use crate::components::variables::{GlobalsTable, user_entries};
use herdr_mcp_api::playground::{
    HERDR_API, HTTP_REQUEST, LLM_CALL, NODE_W, ToolInfo, add_node, collections, connect,
    display_value, edit_flow, folders, fresh_collection_name, fresh_node_id, layout, method_groups,
    methods, parse_value, raw_values, remove_node, replace_node, request_method, request_params,
    request_schema, request_values, result_paths, save_collection_var, send_summary, sorted_vars,
    tool_groups, tool_list, var_name,
};
use herdr_mcp_api::{
    CreateRecipeRequest, Extract, Flow, Node, NodeKind, NodeState, Point, Recipe, RecipeStep,
    RecipeTemplate, ResolveRequest, Retry, Rule, RunAction, RunRecord, StartRunRequest,
    UpdateRecipeRequest, Uuid, VarScope, node_states,
};

/// What the main panel shows.
#[derive(Clone, PartialEq, Debug)]
pub enum Pick {
    Nothing,
    Globals,
    Collection(Uuid),
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
                            "Pick a collection on the left, or start a new collection."
                        }
                    },
                    Pick::Globals => rsx! { GlobalsView {} },
                    Pick::Collection(id) => match find(id) {
                        Some(recipe) => rsx! {
                            CollectionView {
                                key: "{id}",
                                recipe,
                                collections: collections(&list).into_iter().filter(|(c, _)| *c != id).collect::<Vec<_>>(),
                                tools: tools.clone(),
                                pick,
                                refresh,
                            }
                        },
                        None => rsx! { p { class: "muted", "That collection is gone." } },
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

    let create = move |(name, flow): (String, Flow)| {
        let client = client();
        spawn(async move {
            let req = CreateRecipeRequest {
                name,
                description: None,
                flow,
                variables: HashMap::new(),
                category: None,
                triggers: Vec::new(),
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
                onclick: move |_| create((new_name.clone(), Flow::default())),
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
                        on_load: move |made: (String, Flow)| {
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

type FlowEdit = Rc<dyn Fn(&mut Flow) -> Result<(), String>>;

/// Apply `edit` to the canvas's copy of the flow now, then to the saved
/// flow; a failed save reloads the saved one.
async fn change(
    client: ApiClient,
    id: Uuid,
    mut flow: Signal<Flow>,
    edit: FlowEdit,
) -> Result<(), String> {
    edit(&mut flow.write())?;
    let saved = edit_flow(&client, id, |f| edit(f)).await;
    if saved.is_err()
        && let Ok(r) = client.get_recipe(id).await
    {
        flow.set(r.flow);
    }
    saved
}

fn tool_kind(tool: &str, params: Value) -> NodeKind {
    NodeKind::Tool {
        tool: tool.into(),
        params,
        extract: Vec::new(),
        retry: Retry::default(),
        timeout_ms: None,
    }
}

#[component]
fn CollectionView(
    recipe: Recipe,
    collections: Vec<(Uuid, String)>,
    tools: Vec<ToolInfo>,
    pick: Signal<Pick>,
    refresh: Signal<u32>,
) -> Element {
    let client = use_api();
    let id = recipe.id;
    let mut name = use_signal(|| recipe.name.clone());
    let mut folder = use_signal(|| recipe.category.clone().unwrap_or_default());
    let mut vars: Signal<Vec<(String, String)>> = use_signal(|| sorted_vars(&recipe.variables));
    let mut triggers = use_signal(|| recipe.triggers.clone());
    let trail = use_signal(|| recipe.trail);
    let flow = use_signal(|| recipe.flow.clone());
    let mut selected: Signal<Option<String>> = use_signal(|| None);
    // The run "Run collection" started, while it goes; its node states.
    let mut live: Signal<Option<Uuid>> = use_signal(|| None);
    let mut states: Signal<HashMap<String, NodeState>> = use_signal(HashMap::new);
    let mut run: Signal<Option<Value>> = use_signal(|| None);
    let mut msg: Signal<Option<Result<String, String>>> = use_signal(|| None);
    let seen = use_run_events();

    use_future(move || async move {
        let client = client();
        let mut read = None;
        loop {
            changed(seen, &mut read).await;
            let Some(run_id) = *live.peek() else {
                continue;
            };
            let Ok(events) = client.run_events(run_id, 0).await else {
                continue;
            };
            states.set(node_states(&events));
            if events.iter().any(|e| e.kind.is_terminal()) {
                live.set(None);
                if let Ok(r) = client.get_run(run_id).await {
                    run.set(Some(serde_json::to_value(r).unwrap_or_default()));
                }
                // Extract rules may have written collection variables.
                if let Ok(r) = client.get_recipe(id).await {
                    vars.set(sorted_vars(&r.variables));
                }
            }
        }
    });

    let apply = move |edit: FlowEdit| {
        let client = client();
        spawn(async move {
            match change(client, id, flow, edit).await {
                Ok(()) => msg.set(None),
                Err(e) => msg.set(Some(Err(e))),
            }
        });
    };
    let on_canvas = move |edit: CanvasEdit| {
        apply(match edit {
            CanvasEdit::Move(node, at) => Rc::new(move |f: &mut Flow| {
                if let Some(n) = f.nodes.iter_mut().find(|n| n.id == node) {
                    n.at = Some(at);
                }
                Ok(())
            }),
            CanvasEdit::Connect(from, port, to) => Rc::new(move |f: &mut Flow| {
                connect(f, &from, port, &to);
                Ok(())
            }),
            CanvasEdit::Disconnect(edge) => Rc::new(move |f: &mut Flow| {
                f.edges.retain(|e| *e != edge);
                Ok(())
            }),
            CanvasEdit::Remove(node) => Rc::new(move |f: &mut Flow| {
                remove_node(f, &node);
                Ok(())
            }),
        });
    };
    // A new node goes right of the picked one, wired from its first port.
    let mut add = move |prefix: &'static str, kind: NodeKind| {
        let f = flow.read().clone();
        let after = selected().and_then(|s| {
            let port = *f.node(&s)?.kind.ports().first()?;
            Some((s, port))
        });
        let at = layout(&f);
        let place = match &after {
            Some((s, _)) => Point {
                x: at[s].x + NODE_W + 80.0,
                y: at[s].y,
            },
            None => Point {
                x: at.values().map(|p| p.x).fold(0.0, f32::max) + NODE_W + 80.0,
                y: 0.0,
            },
        };
        let node_id = fresh_node_id(&f, prefix);
        selected.set(Some(node_id));
        apply(Rc::new(move |f: &mut Flow| {
            let after = after.as_ref().map(|(s, p)| (s.as_str(), *p));
            add_node(f, prefix, kind.clone(), place, after);
            Ok(())
        }));
    };
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
    let run_all = move |_| {
        let client = client();
        spawn(async move {
            msg.set(None);
            run.set(None);
            states.set(HashMap::new());
            let req = StartRunRequest {
                recipe_id: Some(id),
                ..Default::default()
            };
            match client.start_run(&req).await {
                Ok(r) => live.set(Some(r.id)),
                Err(e) => msg.set(Some(Err(format!("run failed to start: {e}")))),
            }
        });
    };
    let control = move |action: RunAction| {
        let Some(run_id) = live() else { return };
        let client = client();
        spawn(async move {
            if let Err(e) = client.control_run(run_id, action).await {
                msg.set(Some(Err(format!("{} failed: {e}", action.as_str()))));
            }
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

    let f = flow.read().clone();
    let problems = f.problems();
    let max_steps = f.max_steps;
    let paths: Vec<String> = f
        .nodes
        .iter()
        .filter(|n| !matches!(n.kind, NodeKind::Start))
        .map(|n| n.id.clone())
        .chain(["run.pass".to_string()])
        .collect();
    let order: Vec<String> = f.tools().map(|n| n.id.clone()).collect();
    let picked = selected().and_then(|s| f.node(&s).cloned());
    let first_other = collections.first().map(|(c, _)| *c);

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
            div { class: "btn-row",
                button { class: "btn", onclick: move |_| add("request", tool_kind("list_panes", json!({}))), "+ Request" }
                button {
                    class: "btn",
                    title: "A custom HTTP request: method, URL, query, headers, body",
                    onclick: move |_| add("http", tool_kind(HTTP_REQUEST, json!({ "method": "GET", "url": "https://" }))),
                    "+ HTTP"
                }
                button {
                    class: "btn",
                    title: "An OpenRouter / OpenAI-compatible model call",
                    onclick: move |_| add("llm", tool_kind(LLM_CALL, json!({ "model": "" }))),
                    "+ LLM"
                }
                button {
                    class: "btn",
                    title: "Check rules and take the port of the first that holds",
                    onclick: move |_| add("branch", NodeKind::Branch { rules: vec![Rule { checks: Vec::new(), any: false }] }),
                    "+ Branch"
                }
                button {
                    class: "btn",
                    title: "Run another collection as a child run and wait for it",
                    disabled: first_other.is_none(),
                    onclick: move |_| {
                        if let Some(collection) = first_other {
                            add("run", NodeKind::Run { collection, inputs: Default::default() });
                        }
                    },
                    "+ Run"
                }
                button { class: "btn", onclick: move |_| add("wait", NodeKind::Wait { ms: 1000 }), "+ Wait" }
                button { class: "btn", onclick: move |_| add("fail", NodeKind::Fail { message: String::new() }), "+ Fail" }
                span { class: "spacer" }
                if live().is_some() {
                    button { class: "btn", onclick: move |_| control(RunAction::Pause), "Pause" }
                    button { class: "btn", onclick: move |_| control(RunAction::Resume), "Resume" }
                    button { class: "btn link-danger", onclick: move |_| control(RunAction::Cancel), "Cancel" }
                }
                button {
                    class: "btn btn-primary",
                    disabled: live().is_some() || !problems.is_empty() || f.nodes.len() < 2,
                    onclick: run_all,
                    if live().is_some() { "Running…" } else { "Run collection" }
                }
            }
            Canvas {
                flow: f.clone(),
                collections: collections.clone(),
                states: states(),
                selected,
                on_edit: on_canvas,
            }
            for p in problems.iter() {
                p { key: "{p}", class: "send-status send-error", "{p}" }
            }
            match &*msg.read() {
                Some(Ok(m)) => rsx! { span { class: "send-status", "{m}" } },
                Some(Err(m)) => rsx! { p { class: "send-status send-error", "{m}" } },
                None => rsx! {},
            }
        }
        match picked {
            Some(node) if node.step().is_some() => rsx! {
                RequestEditor {
                    key: "{node.id}",
                    rid: id,
                    node,
                    flow,
                    tools,
                    selected,
                }
            },
            Some(node) => {
                let (old, gone) = (node.id.clone(), node.id.clone());
                rsx! {
                NodeEditor {
                    key: "{node.id}",
                    node,
                    collections: collections.clone(),
                    paths,
                    on_save: move |next: Node| {
                        let old = old.clone();
                        selected.set(Some(next.id.clone()));
                        apply(Rc::new(move |f: &mut Flow| replace_node(f, &old, next.clone())));
                    },
                    on_delete: move |_| {
                        selected.set(None);
                        on_canvas(CanvasEdit::Remove(gone.clone()));
                    },
                }
                }
            }
            None => rsx! {},
        }
        if let Some(data) = run.read().clone() {
            RunResults { data, order }
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
            div { class: "btn-row",
                label { class: "form-label", "Max node runs per run" }
                input {
                    class: "input input-narrow",
                    r#type: "number",
                    min: "1",
                    title: "Every loop pass and retry counts; a run that would go over fails.",
                    value: "{max_steps}",
                    onchange: move |e| {
                        if let Ok(n) = e.value().trim().parse::<u32>() {
                            apply(Rc::new(move |f: &mut Flow| {
                                f.max_steps = n.max(1);
                                Ok(())
                            }));
                        }
                    },
                }
            }
        }
        TriggerEditor {
            triggers: triggers.read().clone(),
            on_change: move |next| triggers.set(next),
        }
        TrailEditor { trail }
        div { class: "btn-row",
            button {
                class: "btn btn-primary",
                title: "Name, folder, variables, triggers and run history; the flow saves as you edit it",
                onclick: save,
                "Save collection"
            }
            button { class: "btn link-danger", onclick: delete, "Delete collection" }
        }
    }
}

/// A tool node: its request, retry and timeout; Send saves it, then runs
/// it alone (`node_ids`).
#[component]
fn RequestEditor(
    rid: Uuid,
    node: Node,
    flow: Signal<Flow>,
    tools: Vec<ToolInfo>,
    selected: Signal<Option<String>>,
) -> Element {
    let client = use_api();
    let step = node.step().unwrap_or_default();
    let (retry0, timeout0) = match &node.kind {
        NodeKind::Tool {
            retry, timeout_ms, ..
        } => (*retry, *timeout_ms),
        _ => (Retry::default(), None),
    };
    let node_id = node.id.clone();
    let at = node.at;
    let mut id = use_signal(|| step.id.clone());
    let mut description = use_signal(|| step.description.clone().unwrap_or_default());
    let mut tool = use_signal(|| step.tool.clone());
    let mut method = use_signal(|| request_method(&step));
    let mut values = use_signal(|| request_values(&step));
    let mut extract = use_signal(|| step.extract.clone());
    let mut retry = use_signal(|| retry0);
    let mut timeout = use_signal(|| timeout0);
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

    let current = move || {
        let mut n = Node::tool(RecipeStep {
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
        });
        n.at = at;
        if let NodeKind::Tool {
            retry: r,
            timeout_ms,
            ..
        } = &mut n.kind
        {
            *r = *retry.read();
            *timeout_ms = *timeout.read();
        }
        n
    };
    // Save; true when the node kept its id (a rename re-opens it).
    let save = move || {
        let client = client();
        let old_id = node_id.clone();
        let next = current();
        async move {
            let new_id = next.id.clone();
            let edit: FlowEdit = Rc::new(move |f| replace_node(f, &old_id, next.clone()));
            match change(client, rid, flow, edit).await {
                Err(e) => {
                    error.set(Some(e));
                    false
                }
                Ok(()) => {
                    error.set(None);
                    if *selected.peek() != Some(new_id.clone()) {
                        selected.set(Some(new_id));
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
                        node_ids: vec![id.read().trim().to_string()],
                        ..Default::default()
                    };
                    match client.run_and_wait(&req).await {
                        Ok(run) => response.set(Some(run)),
                        Err(e) => error.set(Some(format!("send failed: {e}"))),
                    }
                }
                busy.set(false);
            });
        }
    };

    let herdr = *tool.read() == HERDR_API;
    let description_text = tools
        .iter()
        .find(|t| t.name == *tool.read())
        .map(|t| t.description.clone())
        .unwrap_or_default();
    let number = |v: u64| if v == 0 { String::new() } else { v.to_string() };

    rsx! {
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
                        key: "{rid}/{node.id}/http",
                        values: values.read().clone(),
                        on_change: move |next| values.set(next),
                    }
                } else if *tool.read() == LLM_CALL && !raw_form() {
                    LlmWizard {
                        key: "{rid}/{node.id}/llm",
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
                        "(another node's result only exists while a run is going; extract it into a variable to chain separate sends)."
                    }
                }
                details { class: "details",
                    summary { "Resolved params" }
                    pre { class: "mono pre-wrap code-block",
                        "{serde_json::to_string_pretty(&resolved).unwrap_or_default()}"
                    }
                }
                div { class: "btn-row",
                    label { class: "form-label", "Retries" }
                    input {
                        class: "input input-narrow",
                        r#type: "number",
                        min: "0",
                        title: "Attempts after the first when the call fails",
                        value: "{number(retry.read().times.into())}",
                        onchange: move |e| retry.write().times = e.value().trim().parse().unwrap_or(0),
                    }
                    label { class: "form-label", "every" }
                    input {
                        class: "input input-narrow",
                        r#type: "number",
                        min: "0",
                        value: "{number(retry.read().delay_ms)}",
                        onchange: move |e| retry.write().delay_ms = e.value().trim().parse().unwrap_or(0),
                    }
                    span { class: "muted", "ms" }
                    label { class: "form-label", "Timeout" }
                    input {
                        class: "input input-narrow",
                        r#type: "number",
                        min: "0",
                        placeholder: "none",
                        title: "Each attempt fails after this long",
                        value: "{number(timeout().unwrap_or(0))}",
                        onchange: move |e| timeout.set(e.value().trim().parse().ok().filter(|ms| *ms > 0)),
                    }
                    span { class: "muted", "ms" }
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
                        title: "Saves the node, then runs it alone",
                        if busy() { "Sending…" } else { "Send" }
                    }
                    button { class: "btn", onclick: on_save, "Save" }
                    button {
                        class: "btn link-danger",
                        onclick: {
                            let gone = node.id.clone();
                            move |_| {
                                let gone = gone.clone();
                                selected.set(None);
                                let edit: FlowEdit = Rc::new(move |f| {
                                    remove_node(f, &gone);
                                    Ok(())
                                });
                                let client = client();
                                spawn(async move {
                                    if let Err(e) = change(client, rid, flow, edit).await {
                                        error.set(Some(e));
                                    }
                                });
                            }
                        },
                        "Delete"
                    }
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
                            // the rule (saved with the node) for later sends.
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
                                            Ok(()) => notice.set(Some(format!("{path} saved as collection variable {var}"))),
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

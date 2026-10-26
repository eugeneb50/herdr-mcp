//! Variables page: every variable a request can read. Globals and each
//! collection's own variables (add, edit, rename, delete, import), and the
//! auto variables resolved live from herdr's panes (read-only).

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::HashMap;

use crate::client::use_api;
use crate::vars::group_auto;
use herdr_mcp_api::playground::{
    collection_vars, delete_collection_var, display_value, parse_value, save_collection_var,
    save_global,
};
use herdr_mcp_api::{AutoVariablesResponse, Recipe, Uuid, VariableStore};

pub(crate) fn user_entries(vars: &[VariableStore]) -> Vec<(String, String)> {
    let mut out: Vec<_> = vars
        .iter()
        .map(|v| (v.key.clone(), display_value(&v.value)))
        .collect();
    out.sort();
    out
}

fn auto_entries(auto: &AutoVariablesResponse) -> HashMap<String, String> {
    auto.variables
        .iter()
        .map(|(k, val)| (k.clone(), display_value(val)))
        .collect()
}

/// `entries` whose name or value contains `search` (any case).
fn matching(entries: Vec<(String, String)>, search: &str) -> Vec<(String, String)> {
    let q = search.to_lowercase();
    entries
        .into_iter()
        .filter(|(k, v)| {
            q.is_empty() || k.to_lowercase().contains(&q) || v.to_lowercase().contains(&q)
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Scope {
    Globals,
    Collections,
    Auto,
}

#[component]
pub fn Variables() -> Element {
    let client = use_api();
    let mut tab = use_signal(|| Scope::Globals);
    let mut search: Signal<String> = use_signal(String::new);
    let mut refresh: Signal<u32> = use_signal(|| 0);
    let bump = move |_| refresh += 1;

    let globals = use_resource(move || {
        let client = client();
        async move {
            let _ = refresh.read();
            client.list_variables().await
        }
    });
    let recipes = use_resource(move || {
        let client = client();
        async move {
            let _ = refresh.read();
            client.list_recipes().await
        }
    });
    let auto = use_resource(move || {
        let client = client();
        async move {
            let _ = refresh.read();
            client.auto_variables(None).await
        }
    });

    let global_list = match &*globals.read_unchecked() {
        Some(Ok(v)) => user_entries(v),
        _ => vec![],
    };
    let recipe_list = match &*recipes.read_unchecked() {
        Some(Ok(v)) => v.clone(),
        _ => vec![],
    };
    let collection_count: usize = recipe_list.iter().map(|r| r.variables.len()).sum();
    let auto_map = match &*auto.read_unchecked() {
        Some(Ok(v)) => auto_entries(v),
        _ => HashMap::new(),
    };
    let tab_class = |s: Scope| {
        if *tab.read() == s {
            "tab tab-active"
        } else {
            "tab"
        }
    };

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Variables" }
            h1 { class: "page-title", "Variables" }
            p { class: "lede",
                "What a request's "
                code { "{{{{ }}}}" }
                " placeholders read. A collection's own variables win over globals, and both win over auto variables. Click a result path after a send to save it as a variable of that collection."
            }
        }
        div { class: "tabs",
            button {
                class: tab_class(Scope::Globals),
                onclick: move |_| tab.set(Scope::Globals),
                "Globals ({global_list.len()})"
            }
            button {
                class: tab_class(Scope::Collections),
                onclick: move |_| tab.set(Scope::Collections),
                "Collections ({collection_count})"
            }
            button {
                class: tab_class(Scope::Auto),
                onclick: move |_| tab.set(Scope::Auto),
                "Auto ({auto_map.len()})"
            }
            button { class: "btn tab-refresh", onclick: move |_| refresh += 1, "↻" }
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
        match *tab.read() {
            Scope::Globals => rsx! {
                GlobalsTable { entries: global_list.clone(), search: search.read().clone(), on_changed: bump }
            },
            Scope::Collections => rsx! {
                CollectionTables { recipes: recipe_list.clone(), search: search.read().clone(), on_changed: bump }
            },
            Scope::Auto => rsx! {
                AutoTable { map: auto_map.clone(), search: search.read().clone() }
            },
        }
    }
}

/// Global variables, editable.
#[component]
pub(crate) fn GlobalsTable(
    entries: Vec<(String, String)>,
    search: String,
    on_changed: EventHandler<()>,
) -> Element {
    let client = use_api();
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let save = move |(old, key, value): (Option<String>, String, String)| {
        let client = client();
        spawn(async move {
            match save_global(&client, old.as_deref(), &key, parse_value(&value)).await {
                Ok(()) => {
                    error.set(None);
                    on_changed.call(());
                }
                Err(e) => error.set(Some(format!("save failed: {e}"))),
            }
        });
    };
    let remove = move |key: String| {
        let client = client();
        spawn(async move {
            match client.delete_variable(&key).await {
                Ok(()) => {
                    error.set(None);
                    on_changed.call(());
                }
                Err(e) => error.set(Some(format!("delete failed: {e}"))),
            }
        });
    };
    rsx! {
        if let Some(e) = error.read().as_deref() {
            div { class: "alert alert-error", "{e}" }
        }
        VarTable {
            entries: matching(entries, &search),
            empty: "No globals yet. Add one here, or set one with an extract rule of scope global.",
            on_save: save,
            on_remove: remove,
        }
    }
}

/// Each collection's own variables, editable.
#[component]
fn CollectionTables(recipes: Vec<Recipe>, search: String, on_changed: EventHandler<()>) -> Element {
    let client = use_api();
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let save = move |id: Uuid, (old, key, value): (Option<String>, String, String)| {
        let client = client();
        spawn(async move {
            match save_collection_var(&client, id, old.as_deref(), &key, parse_value(&value)).await
            {
                Ok(()) => {
                    error.set(None);
                    on_changed.call(());
                }
                Err(e) => error.set(Some(format!("save failed: {e}"))),
            }
        });
    };
    let remove = move |id: Uuid, key: String| {
        let client = client();
        spawn(async move {
            match delete_collection_var(&client, id, &key).await {
                Ok(()) => {
                    error.set(None);
                    on_changed.call(());
                }
                Err(e) => error.set(Some(format!("delete failed: {e}"))),
            }
        });
    };
    let q = search.to_lowercase();
    let groups: Vec<_> = collection_vars(&recipes)
        .into_iter()
        .map(|(id, name, vars)| {
            // A collection whose name matches shows all its variables.
            let vars = if name.to_lowercase().contains(&q) {
                vars
            } else {
                matching(vars, &search)
            };
            (id, name, vars)
        })
        .filter(|(_, name, vars)| {
            q.is_empty() || !vars.is_empty() || name.to_lowercase().contains(&q)
        })
        .collect();

    rsx! {
        if let Some(e) = error.read().as_deref() {
            div { class: "alert alert-error", "{e}" }
        }
        if groups.is_empty() {
            p { class: "muted", "No collections match." }
        }
        for (id, name, vars) in groups {
            div { key: "{id}", class: "var-group",
                div { class: "var-group-title", "{name}" }
                VarTable {
                    entries: vars,
                    empty: "No variables in this collection.",
                    on_save: move |args| save(id, args),
                    on_remove: move |key| remove(id, key),
                }
            }
        }
    }
}

/// Name/value rows with add, edit (name and value), delete and a JSON
/// import. `on_save` gets (the old name when editing, name, value).
#[component]
fn VarTable(
    entries: Vec<(String, String)>,
    empty: String,
    on_save: EventHandler<(Option<String>, String, String)>,
    on_remove: EventHandler<String>,
) -> Element {
    let mut new_name: Signal<String> = use_signal(String::new);
    let mut new_value: Signal<String> = use_signal(String::new);
    let mut adding: Signal<bool> = use_signal(|| false);
    let mut editing: Signal<Option<String>> = use_signal(|| None);
    let mut edit_name: Signal<String> = use_signal(String::new);
    let mut edit_value: Signal<String> = use_signal(String::new);
    let mut importing: Signal<bool> = use_signal(|| false);
    let mut import_text: Signal<String> = use_signal(String::new);
    let mut import_error: Signal<Option<String>> = use_signal(|| None);

    rsx! {
        div { class: "btn-row",
            button { class: "btn", onclick: move |_| adding.set(true), "+ Add" }
            button {
                class: "btn",
                onclick: move |_| {
                    let open = *importing.read();
                    importing.set(!open);
                },
                "Import"
            }
        }
        if *importing.read() {
            div { class: "card",
                textarea {
                    class: "input textarea",
                    rows: "3",
                    placeholder: "JSON object of name to value",
                    value: "{import_text}",
                    oninput: move |e| import_text.set(e.value()),
                }
                if let Some(e) = import_error.read().as_deref() {
                    div { class: "alert alert-error", "{e}" }
                }
                div { class: "btn-row",
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            let text = import_text.read().clone();
                            match serde_json::from_str::<HashMap<String, Value>>(&text) {
                                Ok(map) => {
                                    for (k, v) in map {
                                        on_save.call((None, k, display_value(&v)));
                                    }
                                    import_text.set(String::new());
                                    import_error.set(None);
                                    importing.set(false);
                                }
                                Err(_) => import_error.set(Some("Invalid JSON object of name to value".to_string())),
                            }
                        },
                        "Import"
                    }
                    button { class: "btn", onclick: move |_| importing.set(false), "Cancel" }
                }
            }
        }
        if entries.is_empty() {
            p { class: "muted", "{empty}" }
        }
        div { class: "var-list",
            for (name, val) in entries {
                div { key: "{name}", class: "var-row",
                    if *editing.read() == Some(name.clone()) {
                        input {
                            class: "input mono",
                            r#type: "text",
                            value: "{edit_name}",
                            oninput: move |e| edit_name.set(e.value()),
                        }
                        input {
                            class: "input",
                            r#type: "text",
                            value: "{edit_value}",
                            oninput: move |e| edit_value.set(e.value()),
                        }
                        button {
                            class: "btn btn-primary",
                            onclick: {
                                let old = name.clone();
                                move |_| {
                                    let key = edit_name.read().trim().to_string();
                                    if !key.is_empty() {
                                        on_save.call((Some(old.clone()), key, edit_value.read().clone()));
                                        editing.set(None);
                                    }
                                }
                            },
                            "Save"
                        }
                        button { class: "btn", onclick: move |_| editing.set(None), "Cancel" }
                    } else {
                        code { class: "mono var-name", "{name}" }
                        span { class: "mono var-value", "{val}" }
                        button {
                            class: "link-btn",
                            onclick: {
                                let n = name.clone();
                                let vv = val.clone();
                                move |_| {
                                    editing.set(Some(n.clone()));
                                    edit_name.set(n.clone());
                                    edit_value.set(vv.clone());
                                }
                            },
                            "Edit"
                        }
                        button { class: "link-btn link-danger", onclick: move |_| on_remove.call(name.clone()), "Delete" }
                    }
                }
            }
        }
        if *adding.read() {
            div { class: "card",
                input {
                    class: "input",
                    r#type: "text",
                    placeholder: "variable_name",
                    value: "{new_name}",
                    oninput: move |e| new_name.set(e.value()),
                }
                input {
                    class: "input",
                    r#type: "text",
                    placeholder: "value (JSON is stored typed)",
                    value: "{new_value}",
                    oninput: move |e| new_value.set(e.value()),
                }
                div { class: "btn-row",
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            let key = new_name.read().trim().to_string();
                            if !key.is_empty() {
                                on_save.call((None, key, new_value.read().clone()));
                                new_name.set(String::new());
                                new_value.set(String::new());
                                adding.set(false);
                            }
                        },
                        "Add"
                    }
                    button { class: "btn", onclick: move |_| adding.set(false), "Cancel" }
                }
            }
        }
    }
}

#[component]
fn AutoTable(map: HashMap<String, String>, search: String) -> Element {
    let entries = matching(map.into_iter().collect(), &search);
    let (grouped, ungrouped) = group_auto(&entries);

    rsx! {
        p { class: "field-desc",
            "Resolved live from herdr's panes and the agent registry; read-only."
        }
        if entries.is_empty() {
            p { class: "muted", "No auto variables: herdr reports no panes." }
        }
        for (prefix, children) in grouped {
            div { class: "var-group",
                div { class: "var-group-title", "{prefix}" }
                for (key, val) in children {
                    div { class: "var-row",
                        code { class: "mono var-name", "{key}" }
                        span { class: "mono var-value", "{val}" }
                    }
                }
            }
        }
        if !ungrouped.is_empty() {
            div { class: "var-group",
                div { class: "var-group-title var-group-dim", "Aliases" }
                for (key, val) in ungrouped {
                    div { class: "var-row",
                        code { class: "mono var-name", "{key}" }
                        span { class: "mono var-value", "{val}" }
                    }
                }
            }
        }
    }
}

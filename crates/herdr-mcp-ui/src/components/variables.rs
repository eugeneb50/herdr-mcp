//! Variables page: user/auto tabs, search, add/edit/delete, import/export,
//! auto-var grouping. Port of `VariablesPage` (+`VariableStore` server calls).

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::HashMap;

use crate::client::use_api;
use crate::vars::{display_value, group_auto};
use herdr_mcp_api::{AutoVariablesResponse, SaveVariableRequest, VariableStore};

fn user_entries(vars: &[VariableStore]) -> Vec<(String, String)> {
    vars.iter()
        .map(|v| (v.key.clone(), display_value(&v.value)))
        .collect()
}

fn auto_entries(auto: &AutoVariablesResponse) -> HashMap<String, String> {
    auto.variables
        .iter()
        .map(|(k, val)| (k.clone(), display_value(val)))
        .collect()
}

#[component]
pub fn Variables() -> Element {
    let client = use_api();
    let mut tab: Signal<&str> = use_signal(|| "user");
    let mut search: Signal<String> = use_signal(String::new);
    let mut refresh: Signal<u32> = use_signal(|| 0);
    let bump = move |_| {
        *refresh.write() += 1;
    };

    let user = use_resource(move || {
        let client = client();
        async move {
            let _ = refresh.read();
            client.list_variables().await
        }
    });
    let auto = use_resource(move || {
        let client = client();
        async move {
            let _ = refresh.read();
            client.auto_variables(None).await
        }
    });

    let user_list = match &*user.read_unchecked() {
        Some(Ok(v)) => user_entries(v),
        _ => vec![],
    };
    let auto_map = match &*auto.read_unchecked() {
        Some(Ok(v)) => auto_entries(v),
        _ => HashMap::new(),
    };

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Variables" }
            h1 { class: "page-title", "Variable store" }
            p { class: "lede",
                "User variables persist until edited or deleted. Auto variables resolve from the live registry. User wins on name clash."
            }
        }
        div { class: "tabs",
            button {
                class: if *tab.read() == "user" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("user"),
                "User Variables ({user_list.len()})"
            }
            button {
                class: if *tab.read() == "auto" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("auto"),
                "Auto Variables ({auto_map.len()})"
            }
            button {
                class: "btn tab-refresh",
                onclick: move |_| {
                    *refresh.write() += 1;
                },
                "↻"
            }
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
        if *tab.read() == "user" {
            UserTable { entries: user_list.clone(), search: search.read().clone(), on_changed: bump }
        } else {
            AutoTable { map: auto_map.clone(), search: search.read().clone() }
        }
    }
}

#[component]
fn UserTable(
    entries: Vec<(String, String)>,
    search: String,
    on_changed: EventHandler<()>,
) -> Element {
    let mut new_name: Signal<String> = use_signal(String::new);
    let mut new_value: Signal<String> = use_signal(String::new);
    let mut adding: Signal<bool> = use_signal(|| false);
    let mut editing: Signal<Option<String>> = use_signal(|| None);
    let mut edit_value: Signal<String> = use_signal(String::new);
    let mut importing: Signal<bool> = use_signal(|| false);
    let mut import_text: Signal<String> = use_signal(String::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let client = use_api();

    let q = search.to_lowercase();
    let filtered: Vec<(String, String)> = entries
        .into_iter()
        .filter(|(k, v)| {
            q.is_empty() || k.to_lowercase().contains(&q) || v.to_lowercase().contains(&q)
        })
        .collect();

    let save = move |(name, value): (String, String)| {
        let client = client();
        spawn(async move {
            let req = SaveVariableRequest {
                key: name,
                value: Value::String(value),
                ..Default::default()
            };
            match client.save_variable(&req).await {
                Ok(_) => on_changed.call(()),
                Err(e) => error.set(Some(format!("save failed: {e}"))),
            }
        });
    };

    let remove = move |name: String| {
        let client = client();
        spawn(async move {
            match client.delete_variable(&name).await {
                Ok(_) => on_changed.call(()),
                Err(e) => error.set(Some(format!("delete failed: {e}"))),
            }
        });
    };

    rsx! {
        if let Some(e) = error.read().as_deref() {
            div { class: "alert alert-error", "{e}" }
        }
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
                div { class: "btn-row",
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            let text = import_text.read().clone();
                            match serde_json::from_str::<HashMap<String, Value>>(&text) {
                                Ok(map) => {
                                    for (k, v) in map {
                                        save((k, display_value(&v)));
                                    }
                                    import_text.set(String::new());
                                    importing.set(false);
                                }
                                Err(_) => error.set(Some("Invalid JSON object of name to value".to_string())),
                            }
                        },
                        "Import"
                    }
                    button { class: "btn", onclick: move |_| importing.set(false), "Cancel" }
                }
            }
        }
        if filtered.is_empty() {
            p { class: "muted", "No user variables yet. Add one or run a tool to auto-populate." }
        }
        div { class: "var-list",
            for (name, val) in filtered {
                div { class: "var-row",
                    if *editing.read() == Some(name.clone()) {
                        input {
                            class: "input",
                            r#type: "text",
                            value: "{edit_value}",
                            oninput: move |e| edit_value.set(e.value()),
                        }
                        button {
                            class: "btn btn-primary",
                            onclick: {
                                let n = name.clone();
                                move |_| {
                                    save((n.clone(), edit_value.read().clone()));
                                    editing.set(None);
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
                                    edit_value.set(vv.clone());
                                }
                            },
                            "Edit"
                        }
                        button { class: "link-btn link-danger", onclick: move |_| remove(name.clone()), "Delete" }
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
                    placeholder: "value",
                    value: "{new_value}",
                    oninput: move |e| new_value.set(e.value()),
                }
                div { class: "btn-row",
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            if !new_name.read().trim().is_empty() {
                                save((new_name.read().trim().to_string(), new_value.read().clone()));
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
    let q = search.to_lowercase();
    let entries: Vec<(String, String)> = map
        .into_iter()
        .filter(|(k, v)| {
            q.is_empty() || k.to_lowercase().contains(&q) || v.to_lowercase().contains(&q)
        })
        .collect();
    let (grouped, ungrouped) = group_auto(&entries);

    rsx! {
        if entries.is_empty() {
            p { class: "muted", "No auto variables yet. Run a tool to populate them." }
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

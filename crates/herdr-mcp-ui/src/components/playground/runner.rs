//! Interactive single-tool console. Port of `ToolRunner`.
//!
//! Last-used tool + field values restore from the server `PlaygroundSnapshot`
//! (`GET /api/playground`); params persist back on every successful run.
//! `{{var}}` substitution previews client-side, resolves server-side.

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use super::forms::{DynamicForm, ResponseViewer, convert_param};
use crate::client::use_api;
use crate::vars::{display_value, substitute};
use herdr_mcp_api::{AutoVariablesResponse, PlaygroundSnapshot, VariableStore};

#[derive(Clone, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

pub fn tool_list(tools: &[Value]) -> Vec<ToolInfo> {
    tools
        .iter()
        .filter_map(|t| {
            Some(ToolInfo {
                name: t.get("name")?.as_str()?.to_string(),
                description: t
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                schema: t.get("inputSchema").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn schema_parts(schema: &Value) -> (HashMap<String, Value>, HashSet<String>) {
    let props = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let required = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    (props, required)
}

fn user_map(vars: &[VariableStore]) -> HashMap<String, String> {
    vars.iter()
        .map(|v| (v.key.clone(), display_value(&v.value)))
        .collect()
}

fn auto_map(auto: &AutoVariablesResponse) -> HashMap<String, String> {
    auto.variables
        .iter()
        .map(|(k, val)| (k.clone(), display_value(val)))
        .collect()
}

#[component]
pub fn ToolRunner() -> Element {
    let mut tools: Signal<Vec<ToolInfo>> = use_signal(Vec::new);
    let mut selected: Signal<String> = use_signal(String::new);
    let mut params: Signal<HashMap<String, String>> = use_signal(HashMap::new);
    let mut response: Signal<Option<Value>> = use_signal(|| None);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut loading: Signal<bool> = use_signal(|| false);
    let mut user_vars: Signal<HashMap<String, String>> = use_signal(HashMap::new);
    let mut auto_vars: Signal<HashMap<String, String>> = use_signal(HashMap::new);
    let client = use_api();

    // Initial load: tools + vars + last-used snapshot (server canonical).
    use_effect(move || {
        let client = client();
        spawn(async move {
            match client.list_tools().await {
                Err(e) => error.set(Some(format!("Cannot connect to server: {e}"))),
                Ok(v) => {
                    let list = tool_list(&v);
                    let snap = client.get_playground("default").await.ok();
                    let snap_tool = snap
                        .as_ref()
                        .and_then(|s| s.tool_name.clone())
                        .unwrap_or_default();
                    let initial =
                        if !snap_tool.is_empty() && list.iter().any(|t| t.name == snap_tool) {
                            if let Some(s) = snap {
                                params.set(s.field_values);
                            }
                            snap_tool
                        } else {
                            list.first().map(|t| t.name.clone()).unwrap_or_default()
                        };
                    selected.set(initial);
                    tools.set(list);
                }
            }
            if let Ok(v) = client.list_variables().await {
                user_vars.set(user_map(&v));
            }
            if let Ok(v) = client.auto_variables(None).await {
                auto_vars.set(auto_map(&v));
            }
        });
    });

    let current = tools
        .read()
        .iter()
        .find(|t| t.name == *selected.read())
        .cloned();
    let (properties, required) = current
        .as_ref()
        .map(|t| schema_parts(&t.schema))
        .unwrap_or_default();

    // Resolved preview (client-side; server re-resolves on run).
    let mut preview: HashMap<String, String> = HashMap::new();
    for (k, v) in params.read().iter() {
        preview.insert(
            k.clone(),
            substitute(v, &user_vars.read(), &auto_vars.read()),
        );
    }

    let run = move |_| {
        let name = selected.read().clone();
        if name.is_empty() {
            return;
        }
        let client = client();
        spawn(async move {
            loading.set(true);
            error.set(None);
            response.set(None);
            // Resolve {{vars}} then convert per schema type.
            let schema = client
                .list_tools()
                .await
                .ok()
                .map(|v| {
                    tool_list(&v)
                        .into_iter()
                        .find(|t| t.name == name)
                        .map(|t| t.schema)
                        .unwrap_or(Value::Null)
                })
                .unwrap_or(Value::Null);
            let (props, _) = schema_parts(&schema);
            let mut resolved = serde_json::Map::new();
            for (k, raw) in params.read().iter() {
                let stype = props
                    .get(k)
                    .and_then(|p| p.get("type")?.as_str())
                    .unwrap_or("string");
                let sub = substitute(raw, &user_vars.read(), &auto_vars.read());
                resolved.insert(k.clone(), convert_param(stype, &sub));
            }
            match client.call_tool(&name, &Value::Object(resolved)).await {
                Err(e) => error.set(Some(format!("Request failed: {e}"))),
                Ok(data) => {
                    let is_err = data
                        .get("is_error")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false);
                    if is_err {
                        error.set(Some(
                            data.pointer("/content/0/text")
                                .and_then(|x| x.as_str())
                                .unwrap_or("tool reported is_error")
                                .to_string(),
                        ));
                    } else {
                        response.set(Some(data));
                    }
                    if name == "list_panes"
                        && let Ok(v) = client.auto_variables(None).await
                    {
                        auto_vars.set(auto_map(&v));
                    }
                    // Persist last-used values server-side (UX-4); the server
                    // stamps `updated_at`.
                    let snapshot = PlaygroundSnapshot {
                        workspace_id: "default".to_string(),
                        tool_name: Some(name.clone()),
                        field_values: params.read().clone(),
                        ..Default::default()
                    };
                    if let Err(e) = client.save_playground(&snapshot).await {
                        error.set(Some(format!("Could not save last-used values: {e}")));
                    }
                }
            }
            loading.set(false);
        });
    };

    rsx! {
        div { class: "runner-grid",
            div { class: "runner-left",
                div { class: "form-row",
                    div { class: "form-col",
                        label { class: "form-label", "Tool" }
                        select {
                            class: "input",
                            value: "{selected}",
                            onchange: move |e| {
                                selected.set(e.value());
                                params.set(HashMap::new());
                                response.set(None);
                                error.set(None);
                            },
                            for t in tools.read().iter() {
                                option { key: "{t.name}", value: "{t.name}", "{t.name}" }
                            }
                        }
                        if let Some(c) = current.as_ref() {
                            if !c.description.is_empty() {
                                p { class: "field-desc", "{c.description}" }
                            }
                        }
                    }
                }
                if !properties.is_empty() {
                    DynamicForm {
                        properties: properties.clone(),
                        required: required.clone(),
                        values: params.read().clone(),
                        preview: preview.clone(),
                        on_change: move |next| params.set(next),
                    }
                    details { class: "details",
                        summary { "Resolved params" }
                        pre { class: "mono pre-wrap code-block",
                            "{serde_json::to_string_pretty(&resolved_preview(&params.read(), &properties, &user_vars.read(), &auto_vars.read())).unwrap_or_default()}"
                        }
                    }
                } else {
                    p { class: "muted", "No parameters required" }
                }
                button {
                    class: "btn btn-primary btn-block",
                    disabled: *loading.read() || selected.read().is_empty(),
                    onclick: run,
                    if *loading.read() { "Running..." } else { "Execute" }
                }
                if let Some(e) = error.read().as_deref() {
                    div { class: "alert alert-error", "{e}" }
                }
            }
            div { class: "runner-right",
                match response.read().as_ref() {
                    Some(data) => rsx! { ResponseViewer { data: data.clone() } },
                    None if !*loading.read() => rsx! {
                        p { class: "muted", "Run a tool to see the response" }
                    },
                    None => rsx! { p { class: "muted", "Running…" } },
                }
            }
        }
    }
}

fn resolved_preview(
    params: &HashMap<String, String>,
    properties: &HashMap<String, Value>,
    user: &HashMap<String, String>,
    auto: &HashMap<String, String>,
) -> Value {
    let mut map = serde_json::Map::new();
    for (k, raw) in params {
        let stype = properties
            .get(k)
            .and_then(|p| p.get("type")?.as_str())
            .unwrap_or("string");
        map.insert(
            k.clone(),
            convert_param(stype, &substitute(raw, user, auto)),
        );
    }
    Value::Object(map)
}

//! Schema-driven parameter form + tool response viewer.
//! Port of `DynamicForm` (sans cursor-tracked autocomplete; resolved-value
//! preview kept) and `ResponseViewer`.

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::components::pane_options::pane_list;
use herdr_mcp_api::schema_type;

/// Schema-driven form. `values` holds raw strings keyed by property name;
/// the caller converts with `herdr_mcp_api::playground::build_params` on run.
#[component]
pub fn DynamicForm(
    properties: HashMap<String, Value>,
    required: HashSet<String>,
    values: HashMap<String, String>,
    on_change: EventHandler<HashMap<String, String>>,
    preview: HashMap<String, String>,
) -> Element {
    let mut keys: Vec<String> = properties.keys().cloned().collect();
    keys.sort();

    rsx! {
        div { class: "form-block",
            div { class: "form-label", "Parameters" }
            for key in keys {
                {
                    let values = values.clone();
                    let prop = properties.get(&key).cloned().unwrap_or(Value::Null);
                    let ptype = schema_type(&prop);
                    let choices: Vec<String> = prop
                        .get("enum")
                        .and_then(Value::as_array)
                        .map(|e| e.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    let desc = prop.get("description").and_then(|d| d.as_str()).unwrap_or("");
                    let is_required = required.contains(&key);
                    let val = values.get(&key).cloned().unwrap_or_default();
                    // A boolean left alone shows (and toggles from) its default.
                    let on = match val.as_str() {
                        "" => prop.get("default") == Some(&Value::Bool(true)),
                        v => v == "true",
                    };
                    let resolved = preview.get(&key).cloned().unwrap_or_default();
                    let show_resolved = !resolved.is_empty() && resolved != val;
                    rsx! {
                        div { key: "{key}", class: "field",
                            label { class: "field-head",
                                code { class: "mono accent", "{key}" }
                                span { class: "mono muted", "{ptype}" }
                                if is_required {
                                    span { class: "required", "required" }
                                }
                            }
                            if !desc.is_empty() {
                                p { class: "field-desc", "{desc}" }
                            }
                            if !choices.is_empty() {
                                select {
                                    class: "input",
                                    onchange: {
                                        let key = key.clone();
                                        move |e: Event<FormData>| {
                                            let mut next = values.clone();
                                            next.insert(key.clone(), e.value());
                                            on_change.call(next);
                                        }
                                    },
                                    option { value: "", selected: val.is_empty(),
                                        if is_required { "— choose —" } else { "(default)" }
                                    }
                                    for c in choices {
                                        option { value: "{c}", selected: val == c, "{c}" }
                                    }
                                }
                            } else if ptype == "boolean" {
                                label { class: "check-row",
                                    input {
                                        r#type: "checkbox",
                                        checked: on,
                                        onchange: {
                                            let key = key.clone();
                                            move |_| {
                                                let mut next = values.clone();
                                                next.insert(key.clone(), (!on).to_string());
                                                on_change.call(next);
                                            }
                                        },
                                    }
                                    span { class: "muted", "enabled" }
                                }
                            } else if ptype == "integer" || ptype == "number" {
                                input {
                                    class: "input",
                                    r#type: "number",
                                    placeholder: if is_required { "required" } else { "optional" },
                                    value: "{val}",
                                    oninput: {
                                        let key = key.clone();
                                        move |e| {
                                            let mut next = values.clone();
                                            next.insert(key.clone(), e.value());
                                            on_change.call(next);
                                        }
                                    },
                                }
                            } else if desc.len() > 60 {
                                textarea {
                                    class: "input textarea",
                                    rows: "3",
                                    placeholder: if is_required { "required" } else { "optional" },
                                    value: "{val}",
                                    oninput: {
                                        let key = key.clone();
                                        move |e| {
                                            let mut next = values.clone();
                                            next.insert(key.clone(), e.value());
                                            on_change.call(next);
                                        }
                                    },
                                }
                            } else {
                                input {
                                    class: "input",
                                    r#type: "text",
                                    list: pane_list(&key),
                                    placeholder: if is_required { "required" } else { "optional" },
                                    value: "{val}",
                                    oninput: {
                                        let key = key.clone();
                                        move |e| {
                                            let mut next = values.clone();
                                            next.insert(key.clone(), e.value());
                                            on_change.call(next);
                                        }
                                    },
                                }
                            }
                            if show_resolved {
                                p { class: "field-resolved mono", "→ {resolved}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Tool response viewer with formatted/raw tabs. Port of `ResponseViewer`.
#[component]
pub fn ResponseViewer(data: Value) -> Element {
    let mut tab: Signal<&str> = use_signal(|| "formatted");
    let content: Vec<Value> = data
        .get("content")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    let is_error = herdr_mcp_api::tool_is_error(&data);

    rsx! {
        div { class: "response",
            div { class: "response-head",
                span { class: "response-title", "Response" }
                if is_error {
                    span { class: "pill pill-error", "error" }
                }
                div { class: "response-tabs",
                    button {
                        class: if *tab.read() == "formatted" { "mini-tab mini-tab-active" } else { "mini-tab" },
                        onclick: move |_| tab.set("formatted"),
                        "Formatted"
                    }
                    button {
                        class: if *tab.read() == "raw" { "mini-tab mini-tab-active" } else { "mini-tab" },
                        onclick: move |_| tab.set("raw"),
                        "Raw JSON"
                    }
                }
            }
            div { class: "response-body",
                if *tab.read() == "raw" {
                    pre { class: "mono pre-wrap",
                        "{serde_json::to_string_pretty(&data).unwrap_or_default()}"
                    }
                } else {
                    if content.is_empty() && !is_error {
                        p { class: "muted", "Empty response" }
                    }
                    for item in content {
                        {
                            let text = item.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
                            let mime = item.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
                            rsx! {
                                if mime == "application/json" {
                                    pre { class: "mono pre-wrap code-block",
                                        "{pretty_or_raw(&text)}"
                                    }
                                } else {
                                    div { class: "mono pre-wrap", "{text}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn pretty_or_raw(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| text.to_string()))
        .unwrap_or_else(|_| text.to_string())
}

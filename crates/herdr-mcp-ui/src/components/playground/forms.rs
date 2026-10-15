//! Schema-driven parameter form + tool response viewer.
//! Port of `DynamicForm` (sans cursor-tracked autocomplete; resolved-value
//! preview kept) and `ResponseViewer`.

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Convert a raw string field to a JSON value per schema type.
pub fn convert_param(schema_type: &str, raw: &str) -> Value {
    match schema_type {
        "boolean" => Value::Bool(matches!(raw, "true" | "1" | "yes")),
        "integer" | "number" => raw
            .trim()
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| {
                raw.trim()
                    .parse::<f64>()
                    .map(|f| serde_json::json!(f))
                    .unwrap_or(Value::String(raw.to_string()))
            }),
        _ => Value::String(raw.to_string()),
    }
}

/// Schema-driven form. `values` holds raw strings keyed by property name;
/// the caller converts with [`convert_param`] on run.
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
                    let ptype = prop.get("type").and_then(|t| t.as_str()).unwrap_or("string");
                    let desc = prop.get("description").and_then(|d| d.as_str()).unwrap_or("");
                    let is_required = required.contains(&key);
                    let val = values.get(&key).cloned().unwrap_or_default();
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
                            if ptype == "boolean" {
                                label { class: "check-row",
                                    input {
                                        r#type: "checkbox",
                                        checked: val == "true",
                                        onchange: {
                                            let key = key.clone();
                                            let cur = val.clone();
                                            move |_| {
                                                let mut next = values.clone();
                                                next.insert(
                                                    key.clone(),
                                                    if cur == "true" { "false".to_string() } else { "true".to_string() },
                                                );
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
    let is_error = data
        .get("is_error")
        .and_then(|e| e.as_bool())
        .unwrap_or(false);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_param_types() {
        assert_eq!(convert_param("boolean", "true"), Value::Bool(true));
        assert_eq!(convert_param("integer", "42"), serde_json::json!(42));
        assert_eq!(
            convert_param("string", "{{pane.x}}"),
            Value::String("{{pane.x}}".into())
        );
    }
}

//! Schema-driven parameter form + tool response viewer.
//! Port of `DynamicForm` (sans cursor-tracked autocomplete; resolved-value
//! preview kept) and `ResponseViewer`.

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use herdr_mcp_api::schema_type;

/// Tool arguments from raw form values: each `resolve`d (variable
/// substitution) and converted per its schema type. Blank fields are left
/// out, so optional params fall back to their defaults.
pub fn build_params(
    properties: &HashMap<String, Value>,
    values: &HashMap<String, String>,
    resolve: impl Fn(&str) -> String,
) -> Value {
    let mut map = serde_json::Map::new();
    for (k, raw) in values {
        if raw.trim().is_empty() {
            continue;
        }
        let stype = properties.get(k).map(schema_type).unwrap_or("string");
        map.insert(k.clone(), convert_param(stype, &resolve(raw)));
    }
    Value::Object(map)
}

/// Convert a raw string field to a JSON value per schema type. Arrays take
/// JSON (`["Enter"]`) or a comma list (`Enter, ctrl+c`).
pub fn convert_param(schema_type: &str, raw: &str) -> Value {
    match schema_type {
        "array" => serde_json::from_str::<Value>(raw)
            .ok()
            .filter(Value::is_array)
            .unwrap_or_else(|| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| Value::String(s.to_string()))
                    .collect()
            }),
        "object" => serde_json::from_str(raw).unwrap_or(Value::String(raw.to_string())),
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
        assert_eq!(
            convert_param("array", "Enter, ctrl+c"),
            serde_json::json!(["Enter", "ctrl+c"])
        );
        assert_eq!(
            convert_param("array", r#"["a,b"]"#),
            serde_json::json!(["a,b"])
        );
    }

    /// Optional params (`Option<u32>`, `Option<bool>`) arrive as
    /// `["integer","null"]`; they must still be sent typed, and blanks dropped.
    #[test]
    fn build_params_types_optional_fields_and_skips_blanks() {
        let schema = serde_json::json!({
            "lines": { "type": ["integer", "null"] },
            "check": { "type": ["boolean", "null"] },
            "keys": { "type": "array", "items": { "type": "string" } },
            "pane_id": { "type": ["string", "null"] },
            "command": { "type": "string" }
        });
        let props: HashMap<String, Value> = serde_json::from_value(schema).unwrap();
        let values: HashMap<String, String> = [
            ("lines", "5"),
            ("check", "true"),
            ("keys", "Enter"),
            ("pane_id", "  "),
            ("command", "echo {{x}}"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let params = build_params(&props, &values, |raw| raw.replace("{{x}}", "hi"));
        assert_eq!(
            params,
            serde_json::json!({
                "lines": 5, "check": true, "keys": ["Enter"], "command": "echo hi"
            })
        );
    }
}

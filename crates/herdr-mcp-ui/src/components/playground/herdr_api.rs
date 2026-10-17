//! Every herdr socket API method, straight from the installed herdr's schema
//! (`GET /api/herdr/schema`): pick a method, fill its params, run it through
//! the `herdr_api` tool, see herdr's response. Nothing here is hand-listed,
//! so it always matches the herdr version that is running.

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use super::forms::{DynamicForm, ResponseViewer, build_params};
use crate::client::use_api;

#[derive(Clone, PartialEq)]
struct Method {
    name: String,
    props: HashMap<String, Value>,
    required: HashSet<String>,
}

/// The methods in herdr's request schema, with flattened params.
fn methods(schema: &Value) -> Vec<Method> {
    let request = &schema["schemas"]["request"];
    let defs = &request["$defs"];
    let Some(variants) = request["oneOf"].as_array() else {
        return Vec::new();
    };
    variants
        .iter()
        .filter_map(|v| {
            let name = v.pointer("/properties/method/const")?.as_str()?.to_string();
            let params = v
                .pointer("/properties/params")
                .map(|p| resolve(p, defs))
                .unwrap_or(Value::Null);
            let props = params["properties"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, p)| (k.clone(), flatten(p, defs)))
                        .collect()
                })
                .unwrap_or_default();
            let required = params["required"]
                .as_array()
                .map(|r| {
                    r.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Some(Method {
                name,
                props,
                required,
            })
        })
        .collect()
}

/// Follow `$ref`s and `anyOf [X, null]` to the schema that says what X is.
fn resolve(schema: &Value, defs: &Value) -> Value {
    if let Some(name) = schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.rsplit('/').next())
    {
        return resolve(&defs[name], defs);
    }
    if let Some(any) = schema.get("anyOf").and_then(Value::as_array)
        && let Some(inner) = any
            .iter()
            .find(|s| s.get("type") != Some(&Value::from("null")))
    {
        return resolve(inner, defs);
    }
    schema.clone()
}

/// One param as the playground form understands it: a JSON type, an enum,
/// or `object` (entered as JSON), with its description and default.
fn flatten(prop: &Value, defs: &Value) -> Value {
    let target = resolve(prop, defs);
    let consts: Vec<Value> = target["oneOf"]
        .as_array()
        .map(|o| o.iter().filter_map(|v| v.get("const").cloned()).collect())
        .unwrap_or_default();
    let mut out = serde_json::Map::new();
    if let Some(e) = target.get("enum") {
        out.insert("type".into(), "string".into());
        out.insert("enum".into(), e.clone());
    } else if !consts.is_empty() {
        out.insert("type".into(), "string".into());
        out.insert("enum".into(), Value::Array(consts));
    } else if target.get("properties").is_some() || target.get("oneOf").is_some() {
        out.insert("type".into(), "object".into());
    } else {
        out.insert(
            "type".into(),
            target.get("type").cloned().unwrap_or("string".into()),
        );
    }
    let mut desc = [prop.get("description"), target.get("description")]
        .into_iter()
        .flatten()
        .find_map(Value::as_str)
        .unwrap_or("")
        .to_string();
    let kind = herdr_mcp_api::schema_type(&Value::Object(out.clone())).to_string();
    if kind == "object" || kind == "array" {
        desc = format!("{desc} (JSON)").trim().to_string();
    }
    if let Some(d) = prop.get("default").or(target.get("default")) {
        out.insert("default".into(), d.clone());
        desc = format!("{desc} — default {d}")
            .trim_start_matches(" — ")
            .to_string();
    }
    out.insert("description".into(), desc.into());
    Value::Object(out)
}

#[component]
pub fn HerdrApi() -> Element {
    let client = use_api();
    let schema = use_resource(move || {
        let client = client();
        async move { client.herdr_schema().await.map(|s| methods(&s)) }
    });
    let mut filter = use_signal(String::new);
    let mut selected = use_signal(|| "workspace.list".to_string());
    let mut values: Signal<HashMap<String, String>> = use_signal(HashMap::new);
    let mut response: Signal<Option<Value>> = use_signal(|| None);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut running = use_signal(|| false);

    let all = match &*schema.read() {
        None => return rsx! { p { class: "muted", "Loading herdr's API schema…" } },
        Some(Err(e)) => {
            return rsx! { div { class: "alert alert-error", "Cannot read herdr's API schema: {e}" } };
        }
        Some(Ok(m)) => m.clone(),
    };
    let needle = filter.read().to_lowercase();
    let shown: Vec<&Method> = all
        .iter()
        .filter(|m| needle.is_empty() || m.name.contains(&needle))
        .collect();
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for m in &shown {
        let group = m.name.split('.').next().unwrap_or("").to_string();
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, names)) => names.push(m.name.clone()),
            None => groups.push((group, vec![m.name.clone()])),
        }
    }
    let current = all.iter().find(|m| m.name == *selected.read()).cloned();

    let run = {
        let current = current.clone();
        move |_| {
            let Some(method) = current.clone() else {
                return;
            };
            let client = client();
            let params = build_params(&method.props, &values.read(), str::to_string);
            spawn(async move {
                running.set(true);
                error.set(None);
                response.set(None);
                let body = serde_json::json!({ "method": method.name, "params": params });
                match client.call_tool("herdr_api", &body).await {
                    Ok(v) => response.set(Some(v)),
                    Err(e) => error.set(Some(e.to_string())),
                }
                running.set(false);
            });
        }
    };

    rsx! {
        p { class: "muted",
            "All {all.len()} methods of the running herdr, read from its own API schema."
        }
        div { class: "runner-grid",
            div { class: "runner-left",
                div { class: "form-row",
                    div { class: "form-col",
                        label { class: "form-label", "Method" }
                        input {
                            class: "input",
                            r#type: "text",
                            placeholder: "filter, e.g. pane.read",
                            value: "{filter}",
                            oninput: move |e| filter.set(e.value()),
                        }
                        select {
                            class: "input",
                            size: "10",
                            onchange: move |e| {
                                selected.set(e.value());
                                values.set(HashMap::new());
                                response.set(None);
                                error.set(None);
                            },
                            for (group, names) in groups {
                                optgroup { label: "{group}",
                                    for name in names {
                                        option {
                                            value: "{name}",
                                            selected: *selected.read() == name,
                                            "{name}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(m) = current.clone() {
                    h3 { class: "card-title mono", "{m.name}" }
                    if m.props.is_empty() {
                        p { class: "muted", "No parameters" }
                    } else {
                        DynamicForm {
                            key: "{m.name}",
                            properties: m.props.clone(),
                            required: m.required.clone(),
                            values: values.read().clone(),
                            on_change: move |next| values.set(next),
                            preview: HashMap::new(),
                        }
                    }
                    button {
                        class: "btn btn-primary btn-block",
                        disabled: *running.read(),
                        onclick: run,
                        if *running.read() { "Running…" } else { "Run" }
                    }
                }
                if let Some(e) = error.read().as_deref() {
                    div { class: "alert alert-error mono pre-wrap", "{e}" }
                }
            }
            div { class: "runner-right",
                match response.read().clone() {
                    Some(v) => rsx! { ResponseViewer { data: v } },
                    None => rsx! { p { class: "muted", "Run a method to see herdr's response" } },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_refs_nullables_enums_and_objects() {
        let schema = serde_json::json!({ "schemas": { "request": {
            "oneOf": [{ "properties": {
                "method": { "const": "pane.read" },
                "params": { "$ref": "#/schemas/request/$defs/PaneReadParams" }
            }}],
            "$defs": {
                "PaneReadParams": {
                    "properties": {
                        "pane_id": { "type": "string" },
                        "source": { "$ref": "#/schemas/request/$defs/ReadSource" },
                        "lines": { "type": ["integer", "null"] },
                        "wait": { "anyOf": [{ "$ref": "#/schemas/request/$defs/Wait" }, { "type": "null" }] },
                        "strip_ansi": { "type": "boolean", "default": true }
                    },
                    "required": ["pane_id", "source"]
                },
                "ReadSource": { "enum": ["visible", "recent"], "type": "string" },
                "Wait": { "properties": { "timeout_ms": { "type": "integer" } }, "type": "object" }
            }
        }}});
        let m = &methods(&schema)[0];
        assert_eq!(m.name, "pane.read");
        assert!(m.required.contains("source"));
        assert_eq!(
            m.props["source"]["enum"],
            serde_json::json!(["visible", "recent"])
        );
        assert_eq!(herdr_mcp_api::schema_type(&m.props["lines"]), "integer");
        assert_eq!(m.props["wait"]["type"], "object");
        assert!(
            m.props["strip_ansi"]["description"]
                .as_str()
                .unwrap()
                .contains("default true")
        );
    }
}

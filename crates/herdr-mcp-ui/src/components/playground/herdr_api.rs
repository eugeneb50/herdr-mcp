//! Every herdr socket API method, straight from the installed herdr's schema
//! (`GET /api/herdr/schema`), for the `herdr_api` request form. Nothing here
//! is hand-listed, so it always matches the herdr version that is running.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Clone, PartialEq)]
pub struct Method {
    pub name: String,
    pub props: HashMap<String, Value>,
    pub required: HashSet<String>,
}

/// The methods in herdr's request schema, with flattened params.
pub fn methods(schema: &Value) -> Vec<Method> {
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

/// Method names containing `filter`, grouped by their prefix (`pane`,
/// `tab`, …) in schema order.
pub fn method_groups(methods: &[Method], filter: &str) -> Vec<(String, Vec<String>)> {
    let needle = filter.to_lowercase();
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for m in methods.iter().filter(|m| m.name.contains(&needle)) {
        let group = m.name.split('.').next().unwrap_or("").to_string();
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, names)) => names.push(m.name.clone()),
            None => groups.push((group, vec![m.name.clone()])),
        }
    }
    groups
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

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

pub use herdr_mcp_api::{ExecutionResult, ExecutionStatus, Recipe, RecipeStep};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledRecipe {
    pub id: Uuid,
    pub recipe_id: Uuid,
    pub cron_schedule: String,
    pub next_run: Option<DateTime<Utc>>,
    pub last_run: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

pub fn extract_variables(result: &serde_json::Value) -> HashMap<String, serde_json::Value> {
    let mut vars = HashMap::new();
    extract_recursive(result, &mut vars, "result");
    vars
}

fn extract_recursive(
    value: &serde_json::Value,
    vars: &mut HashMap<String, serde_json::Value>,
    prefix: &str,
) {
    match value {
        serde_json::Value::Object(obj) => {
            for (key, val) in obj {
                let full_key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                extract_recursive(val, vars, &full_key);
            }
        }
        serde_json::Value::String(s) => {
            vars.insert(prefix.to_string(), serde_json::json!(s));
        }
        serde_json::Value::Number(n) => {
            vars.insert(prefix.to_string(), serde_json::json!(n));
        }
        serde_json::Value::Bool(b) => {
            vars.insert(prefix.to_string(), serde_json::json!(b));
        }
        serde_json::Value::Null => {
            vars.insert(prefix.to_string(), serde_json::Value::Null);
        }
        _ => {}
    }
}

/// Tools whose playground results (`POST /api/tools/{name}`) are saved as
/// global variables so later recipes can reference pane ids, agent info, etc.
const PLAYGROUND_AUTO_STORE_TOOLS: &[&str] = &[
    "list_workspaces",
    "list_tabs",
    "list_panes",
    "list_agents",
    "get_pane",
    "get_agent",
    "status",
    "agent_spawn",
    "agent_read",
];

/// Variables to save after a playground run of `tool` that returned `result`
/// (a serialized `CallToolResult`). Empty for other tools and error results.
///
/// Keys: `_{tool}_result` (the payload: JSON parsed from the first text
/// content, else that text), `_{tool}_result_[i]` per array item, and for
/// `list_panes` / `get_pane` arrays `pane_{i}_{id,label,status}`.
pub fn playground_auto_variables(
    tool: &str,
    result: &serde_json::Value,
) -> Vec<(String, serde_json::Value)> {
    if !PLAYGROUND_AUTO_STORE_TOOLS.contains(&tool)
        || result.get("isError").and_then(|v| v.as_bool()) == Some(true)
    {
        return Vec::new();
    }
    let content = result_payload(result);
    let key = format!("_{tool}_result");
    let mut vars = vec![(key.clone(), content.clone())];
    if let Some(arr) = content.as_array() {
        for (i, item) in arr.iter().enumerate() {
            vars.push((format!("{key}_[{i}]"), item.clone()));
        }
        if tool == "list_panes" || tool == "get_pane" {
            for (i, pane) in arr.iter().enumerate() {
                for (field, suffix) in [
                    ("pane_id", "id"),
                    ("label", "label"),
                    ("agent_status", "status"),
                ] {
                    if let Some(v) = pane.get(field).and_then(|v| v.as_str()) {
                        vars.push((format!("pane_{i}_{suffix}"), serde_json::json!(v)));
                    }
                }
            }
        }
    }
    vars
}

/// The useful payload of a serialized `CallToolResult`: the first text
/// content parsed as JSON (or the raw text), else the first `json` content.
fn result_payload(result: &serde_json::Value) -> serde_json::Value {
    let items = result.get("content").and_then(|c| c.as_array());
    for item in items.into_iter().flatten() {
        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
            return serde_json::from_str(text).unwrap_or_else(|_| serde_json::json!(text));
        }
        if item.get("type").and_then(|t| t.as_str()) == Some("json")
            && let Some(json) = item.get("data")
        {
            return json.clone();
        }
    }
    result.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playground_auto_variables_flattens_pane_lists() {
        let panes = r#"[{"pane_id":"p1","label":"a","agent_status":"idle"},{"pane_id":"p2"}]"#;
        let result = serde_json::json!({"content": [{"type": "text", "text": panes}]});
        let vars: HashMap<_, _> = playground_auto_variables("list_panes", &result)
            .into_iter()
            .collect();
        assert_eq!(vars["_list_panes_result"].as_array().map(Vec::len), Some(2));
        assert_eq!(vars["_list_panes_result_[1]"]["pane_id"], "p2");
        assert_eq!(vars["pane_0_id"], "p1");
        assert_eq!(vars["pane_0_status"], "idle");
        assert_eq!(vars["pane_1_id"], "p2");
        assert!(!vars.contains_key("pane_1_label"));

        let text = serde_json::json!({"content": [{"type": "text", "text": "ok"}]});
        assert_eq!(
            playground_auto_variables("status", &text),
            vec![("_status_result".to_string(), serde_json::json!("ok"))]
        );
        // Other tools and error results store nothing.
        assert!(playground_auto_variables("send_text", &result).is_empty());
        let err = serde_json::json!({"isError": true, "content": [{"type": "text", "text": "x"}]});
        assert!(playground_auto_variables("status", &err).is_empty());
    }
    use pretty_assertions::assert_eq;

    #[test]
    fn test_extract_string() {
        let v = serde_json::json!({ "result": "hello" });
        let vars = extract_variables(&v);
        // Keys are always prefixed with "result." by extract_variables.
        assert_eq!(
            vars.get("result.result").unwrap(),
            &serde_json::json!("hello")
        );
    }

    #[test]
    fn test_extract_nested_object() {
        let v = serde_json::json!({ "result": { "pane_id": "p1", "output": "done" } });
        let vars = extract_variables(&v);
        assert_eq!(
            vars.get("result.result.pane_id").unwrap(),
            &serde_json::json!("p1")
        );
        assert_eq!(
            vars.get("result.result.output").unwrap(),
            &serde_json::json!("done")
        );
    }

    #[test]
    fn test_extract_number() {
        let v = serde_json::json!({ "result": 42 });
        let vars = extract_variables(&v);
        assert_eq!(vars.get("result.result").unwrap(), &serde_json::json!(42));
    }

    #[test]
    fn test_extract_bool() {
        let v = serde_json::json!({ "result": true });
        let vars = extract_variables(&v);
        assert_eq!(vars.get("result.result").unwrap(), &serde_json::json!(true));
    }

    #[test]
    fn test_extract_null() {
        let v = serde_json::json!({ "result": null });
        let vars = extract_variables(&v);
        assert_eq!(vars.get("result.result").unwrap(), &serde_json::Value::Null);
    }

    #[test]
    fn test_extract_multiple_top_level_keys() {
        let v = serde_json::json!({
            "result": {
                "status": "ok",
                "pane_id": "p1",
                "count": 3
            }
        });
        let vars = extract_variables(&v);
        assert_eq!(
            vars.get("result.result.status").unwrap(),
            &serde_json::json!("ok")
        );
        assert_eq!(
            vars.get("result.result.pane_id").unwrap(),
            &serde_json::json!("p1")
        );
        assert_eq!(
            vars.get("result.result.count").unwrap(),
            &serde_json::json!(3)
        );
        assert_eq!(vars.len(), 3);
    }

    #[test]
    fn test_extract_ignores_arrays() {
        // The extractor only walks objects/primitives; arrays are not flattened.
        let v = serde_json::json!({ "result": { "items": ["a", "b"] } });
        let vars = extract_variables(&v);
        assert!(
            vars.is_empty(),
            "arrays should not be extracted: {:?}",
            vars
        );
    }
}

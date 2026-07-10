use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<RecipeStep>,
    pub variables: HashMap<String, serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub is_template: bool,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeStep {
    pub id: String,
    pub tool: String,
    pub params: serde_json::Value,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExecutionStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub id: Uuid,
    pub recipe_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: ExecutionStatus,
    pub results: HashMap<String, serde_json::Value>,
    pub variables: HashMap<String, serde_json::Value>,
    pub error: Option<String>,
}

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

#[cfg(test)]
mod tests {
    use super::*;
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

use std::collections::HashMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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

fn extract_recursive(value: &serde_json::Value, vars: &mut HashMap<String, serde_json::Value>, prefix: &str) {
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
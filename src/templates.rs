use std::collections::HashMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::variables::{Recipe, RecipeStep};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub variables: Vec<TemplateVariable>,
    pub steps: Vec<RecipeStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateVariable {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub default_value: Option<serde_json::Value>,
    #[serde(default)]
    pub required: bool,
}

/// Get the bundled template library.
pub fn bundled_templates() -> Vec<RecipeTemplate> {
    vec![
        RecipeTemplate {
            id: "dev-watch".to_string(),
            name: "Dev Watch".to_string(),
            description: "Watch files and send text to an agent when changed".to_string(),
            category: "development".to_string(),
            variables: vec![
                TemplateVariable {
                    name: "directory".to_string(),
                    description: "Directory to watch".to_string(),
                    default_value: Some(serde_json::json!(".")),
                    required: true,
                },
                TemplateVariable {
                    name: "agent_target".to_string(),
                    description: "Target agent to notify".to_string(),
                    default_value: None,
                    required: true,
                },
            ],
            steps: vec![
                RecipeStep {
                    id: "list".to_string(),
                    tool: "list_panes".to_string(),
                    params: serde_json::json!({}),
                    description: Some("List available panes".to_string()),
                },
                RecipeStep {
                    id: "send".to_string(),
                    tool: "send_agent".to_string(),
                    params: serde_json::json!({
                        "target": "{{ template.agent_target }}",
                        "text": "Files in {{ template.directory }} changed"
                    }),
                    description: Some("Notify the agent".to_string()),
                },
            ],
        },
        RecipeTemplate {
            id: "git-status".to_string(),
            name: "Git Status".to_string(),
            description: "Run git status in a pane and read output".to_string(),
            category: "git".to_string(),
            variables: vec![
                TemplateVariable {
                    name: "pane_id".to_string(),
                    description: "Pane ID where git is running".to_string(),
                    default_value: None,
                    required: true,
                },
            ],
            steps: vec![
                RecipeStep {
                    id: "run".to_string(),
                    tool: "run_command".to_string(),
                    params: serde_json::json!({
                        "pane_id": "{{ template.pane_id }}",
                        "command": "git status"
                    }),
                    description: Some("Run git status".to_string()),
                },
                RecipeStep {
                    id: "read".to_string(),
                    tool: "read_pane".to_string(),
                    params: serde_json::json!({
                        "pane_id": "{{ template.pane_id }}",
                        "source": "recent"
                    }),
                    description: Some("Read git output".to_string()),
                },
            ],
        },
        RecipeTemplate {
            id: "restart-agent".to_string(),
            name: "Restart Agent".to_string(),
            description: "Stop and restart an agent".to_string(),
            category: "agents".to_string(),
            variables: vec![
                TemplateVariable {
                    name: "agent".to_string(),
                    description: "Agent name (e.g., claude, gpt-4)".to_string(),
                    default_value: None,
                    required: true,
                },
            ],
            steps: vec![
                RecipeStep {
                    id: "close".to_string(),
                    tool: "close_pane".to_string(),
                    params: serde_json::json!({
                        "label": "{{ template.agent }}"
                    }),
                    description: Some("Close existing pane".to_string()),
                },
                RecipeStep {
                    id: "start".to_string(),
                    tool: "start_agent".to_string(),
                    params: serde_json::json!({
                        "name": "{{ template.agent }}",
                        "args": []
                    }),
                    description: Some("Start fresh agent".to_string()),
                },
            ],
        },
        RecipeTemplate {
            id: "build-and-test".to_string(),
            name: "Build & Test".to_string(),
            description: "Run a build then test, waiting for output".to_string(),
            category: "development".to_string(),
            variables: vec![
                TemplateVariable {
                    name: "pane_id".to_string(),
                    description: "Target pane".to_string(),
                    default_value: None,
                    required: true,
                },
            ],
            steps: vec![
                RecipeStep {
                    id: "build".to_string(),
                    tool: "run_command".to_string(),
                    params: serde_json::json!({
                        "pane_id": "{{ template.pane_id }}",
                        "command": "make build"
                    }),
                    description: Some("Run build".to_string()),
                },
                RecipeStep {
                    id: "wait_build".to_string(),
                    tool: "wait_output".to_string(),
                    params: serde_json::json!({
                        "pane_id": "{{ template.pane_id }}",
                        "match_text": "build complete",
                        "timeout_ms": 300000
                    }),
                    description: Some("Wait for build".to_string()),
                },
                RecipeStep {
                    id: "test".to_string(),
                    tool: "run_command".to_string(),
                    params: serde_json::json!({
                        "pane_id": "{{ template.pane_id }}",
                        "command": "make test"
                    }),
                    description: Some("Run tests".to_string()),
                },
            ],
        },
        RecipeTemplate {
            id: "health-check".to_string(),
            name: "Health Check".to_string(),
            description: "Check server status, workspaces, panes".to_string(),
            category: "monitoring".to_string(),
            variables: vec![],
            steps: vec![
                RecipeStep {
                    id: "status".to_string(),
                    tool: "status".to_string(),
                    params: serde_json::json!({}),
                    description: Some("Server status".to_string()),
                },
                RecipeStep {
                    id: "workspaces".to_string(),
                    tool: "list_workspaces".to_string(),
                    params: serde_json::json!({}),
                    description: Some("List workspaces".to_string()),
                },
                RecipeStep {
                    id: "panes".to_string(),
                    tool: "list_panes".to_string(),
                    params: serde_json::json!({}),
                    description: Some("List panes".to_string()),
                },
                RecipeStep {
                    id: "agents".to_string(),
                    tool: "list_agents".to_string(),
                    params: serde_json::json!({}),
                    description: Some("List agents".to_string()),
                },
            ],
        },
    ]
}

pub fn list_templates() -> Vec<RecipeTemplate> {
    bundled_templates()
}

pub fn find_template(id: &str) -> Option<RecipeTemplate> {
    bundled_templates().into_iter().find(|t| t.id == id)
}

/// Convert a template + variables into a Recipe ready to run/save.
pub fn instantiate(
    template_id: &str,
    var_values: HashMap<String, serde_json::Value>,
    name: Option<String>,
) -> Option<Recipe> {
    let template = find_template(template_id)?;
    let mut vars = HashMap::new();
    vars.insert("template".to_string(), serde_json::json!(template.id));
    for tv in &template.variables {
        let key = format!("template.{}", tv.name);
        let value = var_values.get(&tv.name).cloned()
            .or_else(|| tv.default_value.clone())
            .unwrap_or(serde_json::Value::Null);
        vars.insert(key, value);
    }

    let mut steps = template.steps.clone();
    for step in &mut steps {
        substitute_template_vars(&mut step.params, &vars);
    }

    let now = Utc::now();
    Some(Recipe {
        id: Uuid::new_v4(),
        name: name.unwrap_or_else(|| template.name.clone()),
        description: Some(template.description.clone()),
        steps,
        variables: vars,
        created_at: now,
        updated_at: now,
        is_template: false,
        category: Some(template.category.clone()),
    })
}

fn substitute_template_vars(value: &mut serde_json::Value, vars: &HashMap<String, serde_json::Value>) {
    use regex::Regex;
    lazy_static::lazy_static! {
        static ref RE: Regex = Regex::new(r"\{\{\s*([\w.]+)\s*\}\}").unwrap();
    }
    match value {
        serde_json::Value::String(s) => {
            *s = RE.replace_all(s, |caps: &regex::Captures| {
                let key = &caps[1];
                match vars.get(key) {
                    Some(v) if v.is_string() => v.as_str().unwrap_or("").to_string(),
                    Some(v) => v.to_string(),
                    None => caps[0].to_string(),
                }
            }).to_string();
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                substitute_template_vars(v, vars);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                substitute_template_vars(v, vars);
            }
        }
        _ => {}
    }
}
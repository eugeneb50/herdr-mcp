use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

use crate::variables::{Recipe, RecipeStep};

pub use herdr_mcp_api::{RecipeTemplate, TemplateVariable};

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
            description: "Run git status in a shell pane and capture what it printed".to_string(),
            category: "git".to_string(),
            variables: vec![var("pane_id", "Shell pane in the repository", None)],
            steps: vec![step(
                "status",
                "pane_exec",
                serde_json::json!({
                    "pane_id": "{{ template.pane_id }}",
                    "command": "git status --short --branch"
                }),
                "Run git status; {{status.output}} holds the result",
            )],
        },
        RecipeTemplate {
            id: "restart-agent".to_string(),
            name: "Restart Agent".to_string(),
            description: "Stop and restart an agent".to_string(),
            category: "agents".to_string(),
            variables: vec![TemplateVariable {
                name: "agent".to_string(),
                description: "Agent profile or herdr agent kind (e.g. claude, codex, hermes); also its label".to_string(),
                default_value: None,
                required: true,
            }],
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
                    tool: "agent_spawn".to_string(),
                    params: serde_json::json!({
                        "role": "{{ template.agent }}",
                        "agent": "{{ template.agent }}"
                    }),
                    description: Some("Start a fresh agent (profile or herdr agent kind)".to_string()),
                },
            ],
        },
        RecipeTemplate {
            id: "build-and-test".to_string(),
            name: "Build & Test".to_string(),
            description: "Build, then test, in a shell pane; stops at the first failure".to_string(),
            category: "development".to_string(),
            variables: vec![
                var("pane_id", "Shell pane in the project", None),
                var("build", "Build command", Some("make build")),
                var("test", "Test command", Some("make test")),
            ],
            steps: vec![
                step(
                    "build",
                    "pane_exec",
                    serde_json::json!({
                        "pane_id": "{{ template.pane_id }}",
                        "command": "{{ template.build }}",
                        "timeout_ms": 600000,
                        "check": true
                    }),
                    "Build (fails the run on a non-zero exit)",
                ),
                step(
                    "test",
                    "pane_exec",
                    serde_json::json!({
                        "pane_id": "{{ template.pane_id }}",
                        "command": "{{ template.test }}",
                        "timeout_ms": 600000,
                        "check": true
                    }),
                    "Test (fails the run on a non-zero exit)",
                ),
            ],
        },
        RecipeTemplate {
            id: "pane-to-agent".to_string(),
            name: "Ask Agent About a Pane".to_string(),
            description: "Send the recent output of any pane (server log, test run, REPL) to an agent pane with a question. The target must be an agent: multi-line text typed into a bare shell runs each line.".to_string(),
            category: "panes".to_string(),
            variables: vec![
                var("source_pane", "Pane to read from", None),
                var("agent_pane", "Agent pane to ask", None),
                var("question", "What to ask about it", Some("What is going on here?")),
            ],
            steps: vec![
                step(
                    "read",
                    "read_pane",
                    serde_json::json!({
                        "pane_id": "{{ template.source_pane }}",
                        "source": "recent-unwrapped",
                        "lines": 80
                    }),
                    "Read the source pane's last 80 lines",
                ),
                step(
                    "ask",
                    "send_text",
                    serde_json::json!({
                        "pane_id": "{{ template.agent_pane }}",
                        "text": "Output from pane {{ template.source_pane }}:\n{{read}}\n\n{{ template.question }}"
                    }),
                    "Paste it with the question into the agent",
                ),
                step(
                    "submit",
                    "send_keys",
                    serde_json::json!({
                        "pane_id": "{{ template.agent_pane }}",
                        "keys": ["Enter"]
                    }),
                    "Submit",
                ),
            ],
        },
        RecipeTemplate {
            id: "run-and-ask".to_string(),
            name: "Run & Ask Agent".to_string(),
            description: "Run a command in a shell pane and hand its output to an agent pane with a question. The target must be an agent: multi-line text typed into a bare shell runs each line.".to_string(),
            category: "panes".to_string(),
            variables: vec![
                var("shell_pane", "Shell pane to run the command in", None),
                var("agent_pane", "Agent pane to ask", None),
                var("command", "Command to run", Some("cargo test")),
                var(
                    "question",
                    "What to ask about the output",
                    Some("Explain any failures above and propose a fix."),
                ),
            ],
            steps: vec![
                step(
                    "run",
                    "pane_exec",
                    serde_json::json!({
                        "pane_id": "{{ template.shell_pane }}",
                        "command": "{{ template.command }}",
                        "timeout_ms": 600000
                    }),
                    "Run the command and capture its output and exit code",
                ),
                step(
                    "ask",
                    "send_text",
                    serde_json::json!({
                        "pane_id": "{{ template.agent_pane }}",
                        "text": "$ {{ template.command }}  (exit {{run.exit_code}})\n{{run.output}}\n\n{{ template.question }}"
                    }),
                    "Paste output + question into the agent",
                ),
                step(
                    "submit",
                    "send_keys",
                    serde_json::json!({
                        "pane_id": "{{ template.agent_pane }}",
                        "keys": ["Enter"]
                    }),
                    "Submit",
                ),
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
        RecipeTemplate {
            id: "a2a-pane-inventory".to_string(),
            name: "A2A Pane Inventory".to_string(),
            description: "List panes + registry agents and store labels/ids as session variables (panes.snapshot, agents.snapshot)".to_string(),
            category: "a2a".to_string(),
            variables: vec![
                TemplateVariable {
                    name: "workspace_id".to_string(),
                    description: "Workspace filter for agent_list (empty = all)".to_string(),
                    default_value: Some(serde_json::json!("")),
                    required: false,
                },
                TemplateVariable {
                    name: "session_id".to_string(),
                    description: "Session scope for stored snapshot variables".to_string(),
                    default_value: Some(serde_json::json!("w1")),
                    required: true,
                },
            ],
            steps: vec![
                RecipeStep {
                    id: "panes".to_string(),
                    tool: "list_panes".to_string(),
                    params: serde_json::json!({}),
                    description: Some("List panes (ids + labels)".to_string()),
                },
                RecipeStep {
                    id: "agents".to_string(),
                    tool: "agent_list".to_string(),
                    params: serde_json::json!({
                        "workspace_id": "{{ template.workspace_id }}"
                    }),
                    description: Some("List registry agents".to_string()),
                },
                RecipeStep {
                    id: "store_panes".to_string(),
                    tool: "var_set".to_string(),
                    params: serde_json::json!({
                        "session_id": "{{ template.session_id }}",
                        "key": "panes.snapshot",
                        "value": "{{ panes.result }}"
                    }),
                    description: Some("Store pane labels/ids".to_string()),
                },
                RecipeStep {
                    id: "store_agents".to_string(),
                    tool: "var_set".to_string(),
                    params: serde_json::json!({
                        "session_id": "{{ template.session_id }}",
                        "key": "agents.snapshot",
                        "value": "{{ agents.result }}"
                    }),
                    description: Some("Store agent labels/ids".to_string()),
                },
            ],
        },
        RecipeTemplate {
            id: "a2a-ping".to_string(),
            name: "A2A Ping".to_string(),
            description: "Send a message to one agent and read its output back".to_string(),
            category: "a2a".to_string(),
            variables: vec![
                TemplateVariable {
                    name: "target".to_string(),
                    description: "Target agent role or pane id".to_string(),
                    default_value: None,
                    required: true,
                },
                TemplateVariable {
                    name: "message".to_string(),
                    description: "Text to send".to_string(),
                    default_value: Some(serde_json::json!("ping from recipe")),
                    required: false,
                },
            ],
            steps: vec![
                RecipeStep {
                    id: "send".to_string(),
                    tool: "agent_message".to_string(),
                    params: serde_json::json!({
                        "target": "{{ template.target }}",
                        "text": "{{ template.message }}"
                    }),
                    description: Some("Message the agent".to_string()),
                },
                RecipeStep {
                    id: "read".to_string(),
                    tool: "agent_read".to_string(),
                    params: serde_json::json!({
                        "target": "{{ template.target }}"
                    }),
                    description: Some("Read its output".to_string()),
                },
            ],
        },
        RecipeTemplate {
            id: "a2a-fanout-collect".to_string(),
            name: "A2A Fanout Collect".to_string(),
            description: "Message two agents, read both outputs, store the pair as a session variable".to_string(),
            category: "a2a".to_string(),
            variables: vec![
                TemplateVariable {
                    name: "session_id".to_string(),
                    description: "Session scope for the collected variable".to_string(),
                    default_value: Some(serde_json::json!("w1")),
                    required: true,
                },
                TemplateVariable {
                    name: "target_a".to_string(),
                    description: "First agent role or pane id".to_string(),
                    default_value: None,
                    required: true,
                },
                TemplateVariable {
                    name: "target_b".to_string(),
                    description: "Second agent role or pane id".to_string(),
                    default_value: None,
                    required: true,
                },
                TemplateVariable {
                    name: "message".to_string(),
                    description: "Text to send to both".to_string(),
                    default_value: Some(serde_json::json!("task from recipe")),
                    required: false,
                },
            ],
            steps: vec![
                RecipeStep {
                    id: "send_a".to_string(),
                    tool: "agent_message".to_string(),
                    params: serde_json::json!({
                        "target": "{{ template.target_a }}",
                        "text": "{{ template.message }}"
                    }),
                    description: Some("Message agent A".to_string()),
                },
                RecipeStep {
                    id: "send_b".to_string(),
                    tool: "agent_message".to_string(),
                    params: serde_json::json!({
                        "target": "{{ template.target_b }}",
                        "text": "{{ template.message }}"
                    }),
                    description: Some("Message agent B".to_string()),
                },
                RecipeStep {
                    id: "read_a".to_string(),
                    tool: "agent_read".to_string(),
                    params: serde_json::json!({
                        "target": "{{ template.target_a }}"
                    }),
                    description: Some("Read agent A output".to_string()),
                },
                RecipeStep {
                    id: "read_b".to_string(),
                    tool: "agent_read".to_string(),
                    params: serde_json::json!({
                        "target": "{{ template.target_b }}"
                    }),
                    description: Some("Read agent B output".to_string()),
                },
                RecipeStep {
                    id: "collect".to_string(),
                    tool: "var_set".to_string(),
                    params: serde_json::json!({
                        "session_id": "{{ template.session_id }}",
                        "key": "a2a.collected",
                        "value": "{{ read_a.result }}"
                    }),
                    description: Some("Store collected output".to_string()),
                },
            ],
        },
    ]
}

fn var(name: &str, description: &str, default: Option<&str>) -> TemplateVariable {
    TemplateVariable {
        name: name.to_string(),
        description: description.to_string(),
        default_value: default.map(|d| serde_json::json!(d)),
        required: default.is_none(),
    }
}

fn step(id: &str, tool: &str, params: serde_json::Value, description: &str) -> RecipeStep {
    RecipeStep {
        id: id.to_string(),
        tool: tool.to_string(),
        params,
        description: Some(description.to_string()),
    }
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
        let value = var_values
            .get(&tv.name)
            .cloned()
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
        triggers: Vec::new(),
    })
}

fn substitute_template_vars(
    value: &mut serde_json::Value,
    vars: &HashMap<String, serde_json::Value>,
) {
    use regex::Regex;
    static TEMPLATE_VAR_RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"\{\{\s*([\w.]+)\s*\}\}").unwrap());
    match value {
        serde_json::Value::String(s) => {
            *s = TEMPLATE_VAR_RE
                .replace_all(s, |caps: &regex::Captures| {
                    let key = &caps[1];
                    match vars.get(key) {
                        Some(v) if v.is_string() => v.as_str().unwrap_or("").to_string(),
                        Some(v) => v.to_string(),
                        None => caps[0].to_string(),
                    }
                })
                .to_string();
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_list_templates_nonempty() {
        let t = list_templates();
        assert!(!t.is_empty());
    }

    #[test]
    fn test_find_template_known() {
        assert!(find_template("dev-watch").is_some());
        assert!(find_template("git-status").is_some());
        assert!(find_template("build-and-test").is_some());
        assert!(find_template("restart-agent").is_some());
        assert!(find_template("health-check").is_some());
        assert!(find_template("a2a-pane-inventory").is_some());
        assert!(find_template("a2a-ping").is_some());
        assert!(find_template("a2a-fanout-collect").is_some());
        assert!(find_template("pane-to-agent").is_some());
        assert!(find_template("run-and-ask").is_some());
    }

    #[test]
    fn test_find_template_unknown_none() {
        assert!(find_template("does-not-exist").is_none());
    }

    #[test]
    fn test_instantiate_dev_watch_with_vars() {
        let mut vars = HashMap::new();
        vars.insert("directory".to_string(), serde_json::json!("/src"));
        vars.insert("agent_target".to_string(), serde_json::json!("agentA"));
        let recipe = instantiate("dev-watch", vars, None).expect("should instantiate");
        assert_eq!(recipe.name, "Dev Watch");
        assert!(recipe.steps.iter().any(|s| s.tool == "list_panes"));
        assert!(recipe.steps.iter().any(|s| s.tool == "send_agent"));
        let send = recipe
            .steps
            .iter()
            .find(|s| s.tool == "send_agent")
            .unwrap();
        assert!(send.params.to_string().contains("agentA"));
        assert!(send.params.to_string().contains("/src"));
    }

    #[test]
    fn test_instantiate_uses_default_value() {
        // dev-watch directory has default "."; agent_target is required (no default)
        let mut vars = HashMap::new();
        vars.insert("agent_target".to_string(), serde_json::json!("agentA"));
        let recipe = instantiate("dev-watch", vars, None).expect("should instantiate");
        let send = recipe
            .steps
            .iter()
            .find(|s| s.tool == "send_agent")
            .unwrap();
        assert!(
            send.params.to_string().contains("Files in . changed"),
            "default directory should be substituted: {}",
            send.params
        );
    }

    #[test]
    fn test_instantiate_git_status() {
        let mut vars = HashMap::new();
        vars.insert("pane_id".to_string(), serde_json::json!("p1"));
        let recipe = instantiate("git-status", vars, None).expect("should instantiate");
        let run = recipe.steps.iter().find(|s| s.tool == "pane_exec").unwrap();
        assert!(run.params.to_string().contains("git status"));
        assert!(run.params.to_string().contains("p1"));
    }

    #[test]
    fn test_instantiate_build_and_test_steps() {
        let mut vars = HashMap::new();
        vars.insert("pane_id".to_string(), serde_json::json!("p9"));
        let recipe = instantiate("build-and-test", vars, None).expect("should instantiate");
        assert_eq!(recipe.steps.len(), 2);
        assert!(recipe.steps.iter().all(|s| s.tool == "pane_exec"
            && s.params["check"] == true
            && s.params["pane_id"] == "p9"));
        assert_eq!(recipe.steps[0].params["command"], "make build", "default");
    }

    #[test]
    fn test_instantiate_health_check_no_vars() {
        let recipe = instantiate("health-check", HashMap::new(), None).expect("should instantiate");
        assert!(recipe.steps.iter().any(|s| s.tool == "status"));
        assert!(recipe.steps.iter().any(|s| s.tool == "list_agents"));
    }

    #[test]
    fn test_instantiate_substitutes_template_vars() {
        let mut vars = HashMap::new();
        vars.insert("agent".to_string(), serde_json::json!("claude"));
        let recipe = instantiate("restart-agent", vars, None).expect("should instantiate");
        let start = recipe
            .steps
            .iter()
            .find(|s| s.tool == "agent_spawn")
            .unwrap();
        assert!(start.params.to_string().contains("claude"));
    }

    #[test]
    fn test_instantiate_restart_agent() {
        let mut vars = HashMap::new();
        vars.insert("agent".to_string(), serde_json::json!("gpt"));
        let recipe = instantiate("restart-agent", vars, None).expect("should instantiate");
        assert!(recipe.steps.iter().any(|s| s.tool == "close_pane"));
        assert!(recipe.steps.iter().any(|s| s.tool == "agent_spawn"));
    }

    #[test]
    fn test_instantiate_unknown_returns_none() {
        assert!(instantiate("nope", HashMap::new(), None).is_none());
    }

    #[test]
    fn test_instantiate_a2a_pane_inventory() {
        let mut vars = HashMap::new();
        vars.insert("session_id".to_string(), serde_json::json!("w1"));
        let recipe = instantiate("a2a-pane-inventory", vars, None).expect("should instantiate");
        assert!(recipe.steps.iter().any(|s| s.tool == "list_panes"));
        assert!(recipe.steps.iter().any(|s| s.tool == "agent_list"));
        assert!(recipe.steps.iter().any(|s| s.tool == "var_set"));
        // Template vars are substituted; runtime step refs ({{ panes.result }}) remain.
        assert!(
            !recipe
                .steps
                .iter()
                .any(|s| s.params.to_string().contains("{{ template."))
        );
        assert!(
            recipe
                .steps
                .iter()
                .any(|s| s.params.to_string().contains("{{ panes.result }}"))
        );
    }

    #[test]
    fn test_instantiate_a2a_ping() {
        let mut vars = HashMap::new();
        vars.insert("target".to_string(), serde_json::json!("agentA"));
        let recipe = instantiate("a2a-ping", vars, None).expect("should instantiate");
        let send = recipe
            .steps
            .iter()
            .find(|s| s.tool == "agent_message")
            .unwrap();
        assert!(send.params.to_string().contains("agentA"));
        assert!(send.params.to_string().contains("ping from recipe"));
    }

    #[test]
    fn test_instantiate_a2a_fanout_collect() {
        let mut vars = HashMap::new();
        vars.insert("target_a".to_string(), serde_json::json!("agentA"));
        vars.insert("target_b".to_string(), serde_json::json!("agentB"));
        vars.insert("session_id".to_string(), serde_json::json!("w1"));
        let recipe = instantiate("a2a-fanout-collect", vars, None).expect("should instantiate");
        assert_eq!(recipe.steps.len(), 5);
        assert!(recipe.steps.iter().any(|s| s.tool == "agent_read"));
        assert!(recipe.steps.iter().any(|s| s.tool == "var_set"));
    }
}

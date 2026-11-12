//! Tool parameter structs: what each MCP tool takes.

use rmcp::schemars;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};

// ── Parameter structs ────────────────────────────────────────────────

/// Parameters for listing tabs.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct ListTabsParams {
    /// Filter by workspace ID (e.g., "1").
    pub workspace_id: Option<String>,
    /// herdr session name (see `list_sessions`); default: this server's.
    pub session: Option<String>,
}

/// Parameters for tools that only pick a herdr session.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct SessionParams {
    /// herdr session name (see `list_sessions`); default: this server's.
    pub session: Option<String>,
}

/// Parameters for getting a specific pane by ID or label.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct GetPaneParams {
    /// Pane ID (alternative to label).
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
    /// herdr session name (see `list_sessions`); default: this server's.
    pub session: Option<String>,
}

/// Parameters for creating a new workspace.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceParams {
    /// Working directory for the workspace.
    pub cwd: Option<String>,
    /// Label for the workspace.
    pub label: Option<String>,
    /// Whether to skip focusing the new workspace.
    pub no_focus: Option<bool>,
}

/// Parameters for creating a new tab in a workspace.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct CreateTabParams {
    /// Workspace ID to create the tab in.
    pub workspace_id: Option<String>,
    /// Label for the new tab.
    pub label: Option<String>,
    /// Working directory for the tab.
    pub cwd: Option<String>,
}

/// Parameters for splitting a pane.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct SplitPaneParams {
    /// Pane ID to split (or label). Neither: the caller's own pane, when
    /// the MCP client runs inside herdr.
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
    /// "right" or "down"; default right for a wide pane, down for a tall one.
    pub direction: Option<String>,
    /// Working directory for the new pane.
    pub cwd: Option<String>,
    /// Leave focus where it is (default true).
    pub no_focus: Option<bool>,
}

/// Parameters for closing a pane.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct ClosePaneParams {
    /// Pane ID to close (alternative to label).
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
}

/// Parameters for reading text output from a pane.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct ReadPaneParams {
    /// Pane ID to read from (alternative to label).
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
    /// Source: "visible", "recent", or "recent-unwrapped".
    pub source: Option<String>,
    /// Number of lines to read.
    pub lines: Option<u32>,
    /// herdr session name (see `list_sessions`); default: this server's.
    pub session: Option<String>,
}

/// Parameters for sending text to a pane.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct SendTextParams {
    /// Pane ID to send text to (alternative to label).
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
    /// Text to send (without pressing Enter).
    pub text: String,
}

/// Parameters for sending key presses to a pane.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct SendKeysParams {
    /// Pane ID to send keys to (alternative to label).
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
    /// Keys to send (e.g., "Enter", "Escape", "Ctrl+c").
    pub keys: Vec<String>,
}

/// Parameters for running a command in a pane.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct RunCommandParams {
    /// Pane ID to run the command in (alternative to label).
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
    /// Command to run (text + Enter sent atomically).
    pub command: String,
}

/// Parameters for a raw herdr socket API call.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct HerdrApiParams {
    /// herdr API method, e.g. `pane.read`, `workspace.list`, `agent.prompt`
    /// (all methods + param schemas: `GET /api/herdr/schema`).
    pub method: String,
    /// The method's params object (default `{}`).
    pub params: Option<serde_json::Value>,
    /// How long to wait for herdr's answer (default 60000).
    pub timeout_ms: Option<u32>,
}

/// Parameters for running a command in a pane and capturing its result.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct PaneExecParams {
    /// Pane ID to run the command in (alternative to label). The pane must
    /// be at a POSIX shell prompt (sh, bash, zsh).
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
    /// Command line to run.
    pub command: String,
    /// How long to wait for it to finish (default 120000).
    pub timeout_ms: Option<u32>,
    /// Scrollback lines searched for the output (default 500).
    pub lines: Option<u32>,
    /// Fail (stopping a recipe) when the command exits non-zero.
    pub check: Option<bool>,
}

/// Parameters for waiting for specific text in a pane.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct WaitOutputParams {
    /// Pane ID to watch (alternative to label).
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
    /// Text or pattern to match.
    pub match_text: String,
    /// Timeout in milliseconds.
    pub timeout_ms: Option<u32>,
    /// Snapshot source: "visible", "recent" (default) or "recent-unwrapped".
    pub source: Option<String>,
    /// Whether to treat match_text as a regex pattern.
    pub use_regex: Option<bool>,
}

/// Parameters for `pane_watch`.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct PaneWatchParams {
    /// Pane ID to watch (alternative to label).
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
    /// Text (or regex, with use_regex) to watch for.
    pub match_text: String,
    /// Whether to treat match_text as a regex pattern.
    pub use_regex: Option<bool>,
    /// Snapshot source: "visible", "recent" (default) or "recent-unwrapped".
    pub source: Option<String>,
    /// Give up after this long; without it, watch until matched or the run
    /// is cancelled.
    pub timeout_ms: Option<u32>,
}

/// Parameters for the `agent_spawn` a2a step. Starts an agent, registers its
/// role→pane mapping, optionally waits for dependencies (`needs`) to go idle,
/// and captures its work product on idle.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct AgentSpawnParams {
    /// Human role used for `{{role.output}}` interpolation (e.g. "agentA").
    #[serde(default)]
    pub role: String,
    /// Name of the agent to start.
    pub agent: String,
    /// Arguments to pass to the agent (after '--').
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// Split the first pane of this workspace (or of `tab_id`). Neither: an
    /// MCP client inside herdr splits its own pane, anything else the
    /// session's first pane.
    pub workspace_id: Option<String>,
    pub tab_id: Option<String>,
    /// `right` or `down`; default right for a wide pane, down for a tall one.
    pub split: Option<String>,
    /// Targets (role or pane id) that must be idle before this agent starts.
    #[serde(default)]
    pub needs: Vec<String>,
    /// Wait for this agent to reach idle before returning (default true).
    #[serde(default = "default_true_bool")]
    pub wait_idle: bool,
    /// Optional friendly label for this agent (addressable via `agent_message`
    /// `target`). Renames the pane and stores the label in the registry.
    #[serde(default)]
    pub label: Option<String>,
    /// Start the agent in this existing pane (e.g. a new workspace's
    /// `root_pane`) instead of splitting a new one. It keeps its directory,
    /// so `cwd`/`split`/`tab_id` don't apply.
    #[serde(default)]
    pub in_pane: Option<String>,
}

/// Parameters for the `agent_message` a2a step (agent-to-agent message).
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct AgentMessageParams {
    /// Target agent: role or pane id.
    pub target: String,
    /// Text to send to the agent's stream.
    pub text: String,
}

/// Parameters for the `agent_prompt` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct AgentPromptParams {
    /// Target agent: role, label or pane id.
    pub target: String,
    /// Prompt text.
    pub text: String,
    /// Give up waiting for the turn to finish after this long (default 300000).
    pub timeout_ms: Option<u32>,
}

/// Parameters for the `agent_interrupt` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct AgentInterruptParams {
    /// Target agent: role, label or pane id.
    pub target: String,
    /// Quit the agent (profile `stop` keys) instead of stopping its turn.
    #[serde(default)]
    pub hard: bool,
}

/// Parameters for the `agent_read` a2a step.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct AgentReadParams {
    /// Target agent: role or pane id.
    pub target: String,
    pub source: Option<String>,
    pub lines: Option<u32>,
}

/// Parameters for the `agent_wait` a2a step.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct AgentWaitParams {
    /// Role, label or pane id.
    pub target: String,
    /// Target status: "idle", "working", "blocked", "done" (default "idle").
    #[serde(default = "default_idle")]
    pub status: String,
    pub timeout_ms: Option<u32>,
}

/// Parameters for the `agent_list` a2a step.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct AgentListParams {
    /// Workspace id to filter by (e.g. "w1"). Lists all workspaces if omitted.
    #[serde(default)]
    pub workspace_id: String,
}

/// Parameters for the `var_get` / `var_set` session-variable steps.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct VarGetParams {
    pub session_id: String,
    pub key: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct VarSetParams {
    pub session_id: String,
    pub key: String,
    pub value: serde_json::Value,
}

/// Parameters for the `list_templates` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct ListTemplatesParams {}

/// Parameters for `http_request`.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct HttpRequestParams {
    /// GET (default), POST, PUT, PATCH, DELETE, HEAD, …
    pub method: Option<String>,
    /// Absolute URL, e.g. https://api.example.com/v1/items
    pub url: String,
    /// Request headers, e.g. {"Authorization": "Bearer {{token}}"}.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Query parameters, added to the URL encoded.
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    /// Text body, sent as is (set a Content-Type header to match).
    pub body: Option<String>,
    /// JSON body (object or array), sent with Content-Type application/json.
    #[schemars(with = "Option<serde_json::Map<String, serde_json::Value>>")]
    pub json: Option<serde_json::Value>,
    /// A non-2xx status fails the call (and stops a recipe). Default false.
    #[serde(default)]
    pub check: bool,
    pub timeout_ms: Option<u32>,
}

/// Parameters for `llm_call`.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct LlmCallParams {
    /// Model id, e.g. "openai/gpt-4o-mini" on OpenRouter, "gpt-4o-mini" on OpenAI.
    pub model: String,
    /// The user message.
    pub prompt: Option<String>,
    /// System message.
    pub system: Option<String>,
    /// Earlier turns, [{"role": "user"|"assistant", "content": "…"}], sent before prompt.
    #[serde(default)]
    pub messages: Vec<serde_json::Value>,
    /// OpenAI-compatible API base (default https://openrouter.ai/api/v1;
    /// OpenAI: https://api.openai.com/v1; Ollama: http://localhost:11434/v1).
    pub base_url: Option<String>,
    /// The key itself; prefer api_key_env or a {{variable}}.
    pub api_key: Option<String>,
    /// Environment variable holding the key (default OPENROUTER_API_KEY /
    /// OPENAI_API_KEY by base_url; none for other servers).
    pub api_key_env: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    /// Extra request-body fields, e.g. {"response_format": {"type": "json_object"}}.
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
    pub timeout_ms: Option<u32>,
}

/// Parameters for `llm_models`.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct LlmModelsParams {
    /// OpenAI-compatible API base (default https://openrouter.ai/api/v1).
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    /// Only models whose id contains this (case-insensitive).
    pub filter: Option<String>,
}

/// Parameters for `recipe_list`.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct RecipeListParams {}

/// Parameters for `recipe_get`.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct RecipeGetParams {
    pub recipe_id: String,
}

/// Parameters for `recipe_save`: no `recipe_id` creates a recipe (`name`
/// and `steps` required); with one, the fields given replace the saved ones.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct RecipeSaveParams {
    pub recipe_id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Run in order; a failing step stops the run. Each step's `params` may
    /// use `{{ stepId.path }}` (an earlier step's result) and `{{ var }}`.
    pub steps: Option<Vec<herdr_mcp_api::RecipeStep>>,
    /// The recipe's own variables (`{{ name }}` in its steps).
    pub variables: Option<HashMap<String, serde_json::Value>>,
    /// herdr events that start a run of this recipe.
    pub triggers: Option<Vec<herdr_mcp_api::RecipeTrigger>>,
    /// Playground folder; "" clears it.
    pub category: Option<String>,
    /// What a full run does after the last step: rules checked in order,
    /// the first match decides (end, restart from step 1, fail, or run
    /// another recipe as a child run and end as it ends). A gate with no
    /// rules removes it.
    pub gate: Option<herdr_mcp_api::Gate>,
    /// How long this recipe's runs are kept (`keep_runs` newest, or
    /// `keep_days`) and the pane lines a failed run saves
    /// (`snapshot_lines`); unset fields use the global `runs.*` settings,
    /// 0 means no limit / no snapshot.
    pub trail: Option<herdr_mcp_api::RunTrail>,
}

/// Parameters for `recipe_run`.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct RecipeRunParams {
    /// A saved recipe to run (or pass ad-hoc `steps`).
    pub recipe_id: Option<String>,
    /// Ad-hoc steps, run without saving a recipe.
    pub steps: Option<Vec<herdr_mcp_api::RecipeStep>>,
    /// Name for an ad-hoc run.
    pub name: Option<String>,
    /// herdr workspace id the run is scoped to (default: the recipe's hint, else w1).
    pub session_id: Option<String>,
    /// Added to the run's scope over the recipe's own variables.
    #[serde(default)]
    pub variables: HashMap<String, serde_json::Value>,
    /// Run only these steps, in recipe order.
    #[serde(default)]
    pub step_ids: Vec<String>,
    /// Wait for the run to finish and return its final record (default
    /// false: return at once; poll `run_get`).
    #[serde(default)]
    pub wait: bool,
}

/// Parameters for `run_get`.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct RunGetParams {
    pub run_id: String,
    /// Return the run's trail as text instead of its record: every event
    /// with its time, what each step sent and got, why the gate chose its
    /// rule, errors and pane snapshots. Use it to find out why a run failed.
    #[serde(default)]
    pub trail: bool,
}

/// Parameters for `run_control`.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct RunControlParams {
    pub run_id: String,
    /// "pause", "resume" or "cancel".
    pub action: String,
}

/// Parameters for the `get_template` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct GetTemplateParams {
    /// Template id, e.g. `dev-watch`.
    pub template_id: String,
}

/// Parameters for the `instantiate_template` tool. The resulting recipe is
/// auto-saved so it shows up in the recipe library immediately.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct InstantiateTemplateParams {
    /// Template id to instantiate, e.g. `dev-watch`.
    pub template_id: String,
    /// Values for the template's variables. Missing keys fall back to the
    /// template's `default_value` (or null).
    #[serde(default)]
    pub variables: std::collections::HashMap<String, serde_json::Value>,
    /// Optional override for the generated recipe name.
    #[serde(default)]
    pub name: Option<String>,
}

/// Parameters for the `schedule_recipe` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct ScheduleRecipeParams {
    /// Id of the persisted recipe to run on the schedule.
    pub recipe_id: String,
    /// Cron expression, e.g. `0 * * * *` (every hour).
    pub cron_schedule: String,
    /// Whether the schedule is active on creation (default true).
    #[serde(default = "default_true_bool")]
    pub enabled: bool,
}

/// Parameters for the `list_schedules` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct ListSchedulesParams {}

/// Parameters for the `delete_schedule` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct DeleteScheduleParams {
    /// Schedule id returned by `schedule_recipe` / `list_schedules`.
    pub id: String,
}

/// Parameters for the `enable_schedule` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct EnableScheduleParams {
    /// Schedule id to update.
    pub id: String,
    /// New enabled state.
    pub enabled: bool,
}

/// Parameters for the `clipboard_set` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct ClipboardSetParams {
    /// Text to write to the system clipboard.
    pub text: String,
}

/// Parameters for the `clipboard_get` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct ClipboardGetParams {}

/// A `?workspace_id=` query: one workspace, or all when absent or empty.
#[derive(Debug, Default, Deserialize)]
pub struct WorkspaceQuery {
    #[serde(default)]
    pub workspace_id: Option<String>,
}

fn default_true_bool() -> bool {
    true
}
fn default_idle() -> String {
    "idle".to_string()
}

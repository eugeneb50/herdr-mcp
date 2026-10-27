use herdr_mcp_api::settings;
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::herdr_client::{AgentRegistry, HerdrClient, SubscriberPhase};
use crate::persistence::Persistence;
use crate::scheduler::Scheduler;
use crate::security::Origin;
use crate::templates as tmpl;
use crate::variables::{Recipe, ScheduledRecipe};

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

// ── Server ───────────────────────────────────────────────────────────

/// The herdr MCP server.
///
/// Stateless — every tool call shells out to the local `herdr` CLI binary.
#[derive(Clone)]
pub struct HerdrMcpServer {
    pub persistence: std::sync::Arc<Persistence>,
    pub scheduler: Scheduler,
    pub registry: AgentRegistry,
    /// Live clipboard backend config: seeded from `Config`, replaced by
    /// `PUT /api/config/clipboard` (which also writes the config file).
    pub clipboard: std::sync::Arc<std::sync::RwLock<herdr_mcp_core::ClipboardConfig>>,
    /// Live tunables (timeouts, limits, TUI keys): seeded from the config
    /// file's `[settings.*]`, changed by `PUT/DELETE /api/settings`.
    pub settings: std::sync::Arc<std::sync::RwLock<herdr_mcp_api::Settings>>,
    /// The config file's invalid `[settings.*]` entries (on their defaults).
    pub settings_warnings: std::sync::Arc<Vec<String>>,
    /// The config file Settings saves write to (resolved at startup).
    pub config_path: std::path::PathBuf,
    /// Held across each config-file edit and the live update after it, so
    /// concurrent saves neither share a temp file nor apply a stale
    /// snapshot over each other.
    config_edit: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// The herdr event subscriber client (for status reporting), once
    /// attached. Shared by every clone, including the scheduler's.
    pub herdr_client: std::sync::Arc<std::sync::OnceLock<std::sync::Arc<HerdrClient>>>,
    /// Control channels of in-flight recipe runs (state is in their records).
    pub runs: crate::runs::Runs,
    /// In-process MCP client session to this server, opened on first use by
    /// [`dispatch_tool`]: the bridge and run engine call tools through it.
    pub tool_session: ToolSession,
    /// `[security]`: the tool policy, outbound guard, audit log and token.
    pub security: std::sync::Arc<crate::security::Security>,
    /// `[mcp] tools`: what MCP clients list.
    listed: std::sync::Arc<Vec<String>>,
    /// `[mcp] recipes`: whether agents run recipes over the bridge.
    recipes: herdr_mcp_core::RecipeAccess,
    /// Whether this is the copy serving [`ToolSession`], whose calls say
    /// where they came from ([`Origin::META_KEY`]); any other caller is an
    /// MCP client.
    in_process: bool,
}

/// See [`HerdrMcpServer::tool_session`].
pub type ToolSession =
    std::sync::Arc<tokio::sync::OnceCell<rmcp::service::RunningService<rmcp::RoleClient, ()>>>;

/// The tools `[mcp] tools = ["core"]` lists: finding and laying out panes,
/// driving agents and commands, waiting on them, and running saved recipes
/// ([`CORE_RECIPE_TOOLS`]). Authoring recipes, schedules, templates,
/// variables, HTTP, LLM and clipboard tools are for the UIs and recipes,
/// or listed on request.
const CORE_TOOLS: &[&str] = &[
    "list_workspaces",
    "list_tabs",
    "list_panes",
    "create_workspace",
    "create_tab",
    "split_pane",
    "close_pane",
    "read_pane",
    "send_text",
    "send_keys",
    "run_command",
    "pane_exec",
    "herdr_api",
    "wait_output",
    "pane_watch",
    "agent_spawn",
    "agent_prompt",
    "agent_message",
    "agent_wait",
    "agent_read",
    "agent_interrupt",
    "agent_list",
    "recipe_list",
    "recipe_run",
    "run_get",
    "run_control",
];

/// The recipe tools in [`CORE_TOOLS`], which `[mcp] recipes = "bridge"`
/// leaves out of `core`: agents call them through the bridge instead
/// (`skills/herdr-mcp-recipes`).
const CORE_RECIPE_TOOLS: &[&str] = &["recipe_list", "recipe_run", "run_get", "run_control"];

/// Whether `[mcp] tools` entry `entry` lists `tool`; with `recipes` on the
/// bridge, `core` leaves out [`CORE_RECIPE_TOOLS`].
fn lists(entry: &str, tool: &rmcp::model::Tool, recipes: herdr_mcp_core::RecipeAccess) -> bool {
    let category = tool
        .meta
        .as_ref()
        .and_then(|m| m.0.get("category"))
        .and_then(serde_json::Value::as_str);
    match entry {
        "all" => true,
        "core" => {
            let name = tool.name.as_ref();
            CORE_TOOLS.contains(&name)
                && !(recipes == herdr_mcp_core::RecipeAccess::Bridge
                    && CORE_RECIPE_TOOLS.contains(&name))
        }
        _ => entry == tool.name || category.is_some_and(|c| c.eq_ignore_ascii_case(entry)),
    }
}

/// Shrink an input schema for an MCP client's context: an optional field's
/// `"type": ["string", "null"]` becomes `"string"` (leaving it out already
/// means null). The UIs read the full schemas from `/api/tools`.
fn slim_schema(schema: &mut serde_json::Map<String, serde_json::Value>) {
    use serde_json::Value;
    if let Some(Value::Array(types)) = schema.get("type")
        && let [a, b] = types.as_slice()
        && (a == "null" || b == "null")
    {
        let kept = if a == "null" { b.clone() } else { a.clone() };
        schema.insert("type".into(), kept);
    }
    for value in schema.values_mut() {
        match value {
            Value::Object(map) => slim_schema(map),
            Value::Array(items) => items.iter_mut().for_each(|v| {
                if let Value::Object(map) = v {
                    slim_schema(map);
                }
            }),
            _ => {}
        }
    }
}

/// A tool's `_meta.category`: the group the web and TUI playgrounds list it
/// under (read back from `GET /api/tools`).
fn category(name: &str) -> rmcp::model::Meta {
    let mut meta = rmcp::model::Meta::new();
    meta.0.insert("category".into(), name.into());
    meta
}

#[tool_router]
impl HerdrMcpServer {
    pub fn new(persistence: Persistence, registry: AgentRegistry) -> Self {
        Self::with_config(persistence, registry, &herdr_mcp_core::Config::default())
    }

    /// Save `req` as a new recipe.
    pub async fn create_recipe(&self, req: CreateRecipeRequest) -> anyhow::Result<Recipe> {
        let now = chrono::Utc::now();
        let recipe = Recipe {
            id: uuid::Uuid::new_v4(),
            name: req.name,
            description: req.description,
            steps: req.steps,
            variables: req.variables,
            created_at: now,
            updated_at: now,
            is_template: false,
            category: req.category,
            triggers: req.triggers,
            gate: req.gate.filter(|g| !g.rules.is_empty()),
            trail: req.trail,
        };
        self.persistence.save_recipe(&recipe).await?;
        Ok(recipe)
    }

    /// Replace the fields `req` sets on recipe `id` (`None`: no such recipe).
    pub async fn update_recipe(
        &self,
        id: uuid::Uuid,
        req: UpdateRecipeRequest,
    ) -> anyhow::Result<Option<Recipe>> {
        let Some(mut recipe) = self.persistence.load_recipe(&id).await? else {
            return Ok(None);
        };
        if let Some(name) = req.name {
            recipe.name = name;
        }
        if let Some(desc) = req.description {
            recipe.description = Some(desc);
        }
        if let Some(steps) = req.steps {
            recipe.steps = steps;
        }
        if let Some(vars) = req.variables {
            recipe.variables = vars;
        }
        if let Some(triggers) = req.triggers {
            recipe.triggers = triggers;
        }
        if let Some(category) = req.category {
            recipe.category = Some(category).filter(|c| !c.trim().is_empty());
        }
        if let Some(gate) = req.gate {
            recipe.gate = Some(gate).filter(|g| !g.rules.is_empty());
        }
        if let Some(trail) = req.trail {
            recipe.trail = trail;
        }
        recipe.updated_at = chrono::Utc::now();
        self.persistence.save_recipe(&recipe).await?;
        Ok(Some(recipe))
    }

    /// Snapshot of the clipboard commands in effect right now.
    pub fn clipboard_config(&self) -> herdr_mcp_core::ClipboardConfig {
        crate::read(&self.clipboard).clone()
    }

    /// Save clipboard commands to the config file (`None`: the
    /// built-in OS clipboard) and apply them to the running server. The file
    /// is written first, so a failed write leaves the live config unchanged.
    pub async fn save_clipboard_config(
        &self,
        commands: Option<(String, String)>,
    ) -> anyhow::Result<()> {
        let _edit = self.config_edit.lock().await;
        let path = self.config_path.clone();
        let saved = commands.clone();
        tokio::task::spawn_blocking(move || {
            let commands = saved.as_ref().map(|(c, p)| (c.as_str(), p.as_str()));
            herdr_mcp_core::set_clipboard_in_file(&path, commands)
        })
        .await??;
        let (copy_command, paste_command) = commands.unzip();
        *crate::write(&self.clipboard) = herdr_mcp_core::ClipboardConfig {
            copy_command,
            paste_command,
        };
        Ok(())
    }

    /// The tunables in effect right now.
    pub fn settings(&self) -> herdr_mcp_api::Settings {
        crate::read(&self.settings).clone()
    }

    /// A millisecond setting as a herdr `--timeout` / `timeout_ms` value.
    fn timeout_ms(&self, key: &str) -> u32 {
        u32::try_from(self.settings().number(key)).unwrap_or(u32::MAX)
    }

    /// Validate `value` for setting `key` (`None`: back to its default),
    /// write it to the config file, then apply it. `Ok(Err)` is a
    /// value the caller must fix; nothing is written then.
    pub async fn save_setting(
        &self,
        key: String,
        value: Option<herdr_mcp_api::settings::Value>,
    ) -> anyhow::Result<Result<(), String>> {
        let _edit = self.config_edit.lock().await;
        let path = self.config_path.clone();
        let mut next = self.settings();
        let stored = match value {
            Some(v) => match next.set(&key, v) {
                Ok(()) => next.value(&key),
                Err(e) => return Ok(Err(e)),
            },
            None if next.reset(&key) => None,
            None => return Ok(Err(format!("unknown setting {key:?}"))),
        };
        tokio::task::spawn_blocking(move || {
            herdr_mcp_core::set_setting_in_file(&path, &key, stored.as_ref())
        })
        .await??;
        *crate::write(&self.settings) = next;
        Ok(Ok(()))
    }

    /// Every setting back to its default, in the config file and live.
    pub async fn reset_settings(&self) -> anyhow::Result<()> {
        let _edit = self.config_edit.lock().await;
        let path = self.config_path.clone();
        tokio::task::spawn_blocking(move || herdr_mcp_core::reset_settings_in_file(&path))
            .await??;
        *crate::write(&self.settings) = herdr_mcp_api::Settings::default();
        Ok(())
    }

    /// Build the server, resolving the clipboard backend and the tunables
    /// from `config`.
    pub fn with_config(
        persistence: Persistence,
        registry: AgentRegistry,
        config: &herdr_mcp_core::Config,
    ) -> Self {
        let persistence = std::sync::Arc::new(persistence);
        let (settings, warnings) = config.settings();
        for w in &warnings {
            tracing::warn!("{w}");
        }
        // Persisted schedules start with `serve` (see `serve_core`).
        let scheduler = Scheduler::new(persistence.clone());
        let server = Self {
            persistence,
            scheduler,
            registry,
            clipboard: std::sync::Arc::new(std::sync::RwLock::new(config.clipboard.clone())),
            settings: std::sync::Arc::new(std::sync::RwLock::new(settings)),
            settings_warnings: std::sync::Arc::new(warnings),
            config_path: herdr_mcp_core::config_file_or_default(),
            config_edit: std::sync::Arc::default(),
            herdr_client: std::sync::Arc::default(),
            runs: crate::runs::Runs::default(),
            tool_session: ToolSession::default(),
            security: std::sync::Arc::new(crate::security::Security::new(config)),
            listed: std::sync::Arc::new(config.mcp.tools.clone()),
            recipes: config.mcp.recipes,
            in_process: false,
        };
        for tool in &config.security.disabled_tools {
            if !Self::tool_router().has_route(tool) {
                tracing::warn!("[security] disabled-tools: no tool named {tool:?}");
            }
        }
        let tools = Self::tool_router().list_all();
        for entry in &config.mcp.tools {
            if !tools.iter().any(|t| lists(entry, t, config.mcp.recipes)) {
                tracing::warn!("[mcp] tools: {entry:?} is not core, all, a category or a tool");
            }
        }
        // Scheduled recipes run through the run engine like any other run.
        // The closure holds a server clone (and so the scheduler): a
        // reference cycle that lives as long as the process, like the server.
        let runner = server.clone();
        server
            .scheduler
            .set_executor(std::sync::Arc::new(move |recipe_id| {
                let server = runner.clone();
                Box::pin(async move {
                    let req = herdr_mcp_api::StartRunRequest {
                        recipe_id: Some(recipe_id),
                        ..Default::default()
                    };
                    let origin = herdr_mcp_api::RunOrigin::Schedule;
                    match crate::runs::start_run(&server, req, origin).await {
                        Ok((_, done)) => {
                            let _ = done.await;
                        }
                        Err(e) => tracing::warn!("scheduled run of recipe {recipe_id}: {e:?}"),
                    }
                })
            }));
        server
    }

    /// Attach a HerdrClient for socket/subscriber status reporting.
    pub fn with_herdr_client(self, client: std::sync::Arc<HerdrClient>) -> Self {
        let _ = self.herdr_client.set(client);
        self
    }

    /// Run `herdr status --json` and parse the JSON output.
    async fn run_herdr_status_json(&self) -> Result<serde_json::Value, McpError> {
        let output = herdr_cli(&["status", "--json"]).await?;
        serde_json::from_str(&output).map_err(|e| {
            McpError::internal_error(format!("Failed to parse herdr status JSON: {e}"), None)
        })
    }

    // ── Discovery ──────────────────────────────────────────────────────

    #[tool(meta = category("Discovery"), description = "Get overall herdr server status, server status, and client status")]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        // Get herdr's native status
        let herdr_status = self.run_herdr_status_json().await.unwrap_or_else(
            |_| serde_json::json!({"error": "herdr CLI unavailable or status failed"}),
        );

        // Get herdr-mcp's socket/subscriber state
        let (socket_active, subscriber_phase) = match self.herdr_client.get() {
            Some(client) => {
                let active = client.is_socket_active().await;
                let phase = client.subscriber_state().await;
                (active, phase)
            }
            None => (false, SubscriberPhase::Stopped),
        };

        let inside_herdr = herdr_mcp_core::session_context::inside_herdr();
        let pane_id = herdr_mcp_core::session_context::pane_id();

        let merged = serde_json::json!({
            "herdr": herdr_status,
            "herdr_mcp": {
                "inside_herdr": inside_herdr,
                "pane_id": pane_id,
                "socket_active": socket_active,
                "subscriber": format!("{:?}", subscriber_phase),
            }
        });

        let content = Content::json(merged).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize merged status: {e}"), None)
        })?;

        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(meta = category("Discovery"), description = "List every named herdr session: name, running, default, socket. Pass a name as `session` to the Discovery and Read tools to look inside it.")]
    async fn list_sessions(&self) -> Result<CallToolResult, McpError> {
        let sessions = crate::panes::sessions().await?;
        json_result(&herdr_mcp_api::SessionsResponse { sessions })
    }

    #[tool(meta = category("Discovery"), description = "List all workspaces in a herdr session (default: this server's)")]
    async fn list_workspaces(
        &self,
        Parameters(SessionParams { session }): Parameters<SessionParams>,
    ) -> Result<CallToolResult, McpError> {
        herdr_json_in(session.as_deref(), &["workspace", "list"]).await
    }

    #[tool(meta = category("Discovery"), description = "List tabs, optionally filtered by workspace_id (e.g. '1')")]
    async fn list_tabs(
        &self,
        Parameters(ListTabsParams {
            workspace_id,
            session,
        }): Parameters<ListTabsParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut args = vec!["tab", "list"];
        if let Some(ref wid) = workspace_id {
            args.extend(["--workspace", wid]);
        }
        herdr_json_in(session.as_deref(), &args).await
    }

    #[tool(meta = category("Discovery"), description = "List panes as {panes: [...]}: id, workspace, tab, focus, agent + status, label, cwd. Every filter narrows: session, workspace_id, tab_id, agent_status, focused, label (case-insensitive substring of label or agent).")]
    async fn list_panes(
        &self,
        Parameters(filter): Parameters<herdr_mcp_api::PaneFilter>,
    ) -> Result<CallToolResult, McpError> {
        let panes = crate::panes::panes(&filter).await?;
        json_result(&herdr_mcp_api::PanesResponse { panes })
    }

    #[tool(meta = category("Discovery"), description = "List all detected agents in a herdr session (default: this server's)")]
    async fn list_agents(
        &self,
        Parameters(SessionParams { session }): Parameters<SessionParams>,
    ) -> Result<CallToolResult, McpError> {
        herdr_json_in(session.as_deref(), &["agent", "list"]).await
    }

    #[tool(meta = category("Discovery"), description = "Get details about a specific pane by pane_id or label")]
    async fn get_pane(
        &self,
        Parameters(GetPaneParams {
            pane_id,
            label,
            session,
        }): Parameters<GetPaneParams>,
    ) -> Result<CallToolResult, McpError> {
        let session = session.as_deref();
        let pid = resolve_pane_id_in(session, pane_id, label).await?;
        herdr_json_in(session, &["pane", "get", &pid]).await
    }

    // ── Lifecycle ──────────────────────────────────────────────────────

    #[tool(meta = category("Lifecycle"), description = "Create a new workspace, optionally in a directory with a label")]
    async fn create_workspace(
        &self,
        Parameters(CreateWorkspaceParams {
            cwd,
            label,
            no_focus,
        }): Parameters<CreateWorkspaceParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut args = vec!["workspace", "create"];
        if let Some(ref path) = cwd {
            args.extend(["--cwd", path]);
        }
        if let Some(ref lbl) = label {
            args.extend(["--label", lbl]);
        }
        if no_focus.unwrap_or(false) {
            args.push("--no-focus");
        }
        run_herdr_json(&args).await
    }

    #[tool(
        meta = category("Lifecycle"),
        description = "Create a new tab in a workspace, optionally with a label and working directory"
    )]
    async fn create_tab(
        &self,
        Parameters(CreateTabParams {
            workspace_id,
            label,
            cwd,
        }): Parameters<CreateTabParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut args = vec!["tab", "create"];
        if let Some(ref wid) = workspace_id {
            args.extend(["--workspace", wid]);
        }
        if let Some(ref lbl) = label {
            args.extend(["--label", lbl]);
        }
        if let Some(ref path) = cwd {
            args.extend(["--cwd", path]);
        }
        run_herdr_json(&args).await
    }

    #[tool(meta = category("Lifecycle"), description = "Split a pane right or down. Returns the new pane's info.")]
    async fn split_pane(
        &self,
        Parameters(SplitPaneParams {
            pane_id,
            label,
            direction,
            cwd,
            no_focus,
        }): Parameters<SplitPaneParams>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let pid = match (pane_id, label, self.caller_pane(&context.meta)) {
            (None, None, Some(caller)) => caller,
            (pane_id, label, _) => resolve_pane_id(pane_id, label).await?,
        };
        let direction = match direction {
            Some(d) => d,
            None => crate::agents::split_direction(&pid).await.to_string(),
        };
        let mut args = vec!["pane", "split", &pid, "--direction", &direction];
        if let Some(ref path) = cwd {
            args.extend(["--cwd", path]);
        }
        if no_focus.unwrap_or(true) {
            args.push("--no-focus");
        }
        run_herdr_json(&args).await
    }

    #[tool(meta = category("Lifecycle"), description = "Close a pane by pane_id or label")]
    async fn close_pane(
        &self,
        Parameters(ClosePaneParams { pane_id, label }): Parameters<ClosePaneParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        run_herdr_json(&["pane", "close", &pid]).await
    }

    // ── Read ───────────────────────────────────────────────────────────

    #[tool(
        meta = category("Read"),
        description = "Read text output from a pane. Source: visible (current screen), recent (scrollback with wrapping), or recent-unwrapped (scrollback without soft wrapping, best for logs)"
    )]
    async fn read_pane(
        &self,
        Parameters(ReadPaneParams {
            pane_id,
            label,
            source,
            lines,
            session,
        }): Parameters<ReadPaneParams>,
    ) -> Result<CallToolResult, McpError> {
        let session = session.as_deref();
        let pid = resolve_pane_id_in(session, pane_id, label).await?;
        let lines_str = lines.map(|n| n.to_string());
        let mut args = vec!["pane", "read", &pid];
        if let Some(ref src) = source {
            args.extend(["--source", src]);
        }
        if let Some(ref n) = lines_str {
            args.extend(["--lines", n]);
        }
        herdr_text_in(session, &args).await
    }

    // ── Write ──────────────────────────────────────────────────────────

    #[tool(
        meta = category("Write"),
        description = "Send text to a pane (without pressing Enter). Use run_command to send text+Enter atomically."
    )]
    async fn send_text(
        &self,
        Parameters(SendTextParams {
            pane_id,
            label,
            text,
        }): Parameters<SendTextParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        run_herdr_json(&["pane", "send-text", &pid, &text]).await
    }

    #[tool(
        meta = category("Write"),
        description = "Send key presses to a pane. Keys: Enter, Esc, Tab, Backspace, Space, Up, Down, Left, Right, F1-F12, Ctrl+<key> (e.g. Ctrl+c). herdr has no Home, End, PageUp, PageDown or Delete."
    )]
    async fn send_keys(
        &self,
        Parameters(SendKeysParams {
            pane_id,
            label,
            keys,
        }): Parameters<SendKeysParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        let mut args = vec!["pane", "send-keys", &pid];
        for key in &keys {
            args.push(key);
        }
        run_herdr_json(&args).await
    }

    #[tool(
        meta = category("Write"),
        description = "Run a command in a pane (sends text + Enter atomically). Prefer this over send_text + send_keys Enter for commands."
    )]
    async fn run_command(
        &self,
        Parameters(RunCommandParams {
            pane_id,
            label,
            command,
        }): Parameters<RunCommandParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        run_herdr_json(&["pane", "run", &pid, &command]).await
    }

    #[tool(
        meta = category("Write"),
        description = "Run a command in a pane's shell, wait for it to finish, and return {pane_id, command, exit_code, output}. The output is what the command printed (from scrollback). The pane must be at a POSIX shell prompt. With `check: true` a non-zero exit is an error, which stops a recipe. In recipes use {{step.output}} / {{step.exit_code}}."
    )]
    async fn pane_exec(
        &self,
        Parameters(PaneExecParams {
            pane_id,
            label,
            command,
            timeout_ms,
            lines,
            check,
        }): Parameters<PaneExecParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        let token = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        herdr_cli(&["pane", "run", &pid, &exec_line(&command, &token)]).await?;
        let wait = wait_output_args(
            &pid,
            &exec_done_regex(&token),
            true,
            Some("recent-unwrapped"),
            Some(timeout_ms.unwrap_or(self.timeout_ms(settings::PANE_EXEC_MS))),
        );
        herdr_cli(&as_strs(&wait)).await?;
        let lines = lines
            .map_or_else(
                || self.settings().number(settings::PANE_EXEC_LINES),
                u64::from,
            )
            .to_string();
        let screen = herdr_cli(&[
            "pane",
            "read",
            &pid,
            "--source",
            "recent-unwrapped",
            "--lines",
            &lines,
        ])
        .await?;
        let (output, exit_code) = exec_result(&screen, &token).ok_or_else(|| {
            user_err(format!(
                "pane_exec: finished, but its end marker was not in the last {lines} lines"
            ))
        })?;
        if check == Some(true) && exit_code != 0 {
            return Err(user_err(format!(
                "`{command}` exited {exit_code}:\n{}",
                tail(&output, 20)
            )));
        }
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "pane_id": pid,
                "command": command,
                "exit_code": exit_code,
                "output": output,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("Write"),
        description = "Call any herdr socket API method directly and return herdr's response ({id, result} or {id, error}). Covers everything the installed herdr can do: workspace.*, tab.*, pane.*, agent.*, layout.*, worktree.*, plugin.*, server.*, events.wait, ... `GET /api/herdr/schema` (or `herdr api schema --json`) lists every method with its params schema."
    )]
    async fn herdr_api(
        &self,
        Parameters(HerdrApiParams {
            method,
            params,
            timeout_ms,
        }): Parameters<HerdrApiParams>,
    ) -> Result<CallToolResult, McpError> {
        let client = self
            .herdr_client
            .get()
            .ok_or_else(|| user_err("not connected to herdr (no socket client attached)"))?;
        let timeout = timeout_ms.map_or_else(
            || self.settings().duration(settings::HERDR_API_MS),
            |ms| std::time::Duration::from_millis(u64::from(ms)),
        );
        let params = params.unwrap_or_else(|| serde_json::json!({}));
        let response = client
            .call(&method, params, timeout)
            .await
            .map_err(to_mcp_err)?;
        let text = serde_json::to_string_pretty(&response).map_err(to_mcp_err)?;
        Ok(if response.get("error").is_some() {
            CallToolResult::error(vec![Content::text(text)])
        } else {
            CallToolResult::success(vec![Content::text(text)])
        })
    }

    // ── Synchronize ────────────────────────────────────────────────────

    #[tool(
        meta = category("Synchronize"),
        description = "Wait for specific text to appear in a pane. Blocks until matched or timeout. Supports --regex for pattern matching. Returns the matching output on success."
    )]
    async fn wait_output(
        &self,
        Parameters(WaitOutputParams {
            pane_id,
            label,
            match_text,
            timeout_ms,
            source,
            use_regex,
        }): Parameters<WaitOutputParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        let args = wait_output_args(
            &pid,
            &match_text,
            use_regex.unwrap_or(false),
            source.as_deref(),
            timeout_ms,
        );
        run_herdr_json(&as_strs(&args)).await
    }

    #[tool(
        meta = category("Synchronize"),
        description = "Watch a pane until NEW text matching match_text appears, then return {pane_id, matched_line, matches}. Unlike wait_output, matches already in the pane when the watch starts (e.g. the marker inside a prompt you just sent) don't count: it fires when the number of matches goes up. In a recipe, the next step runs once it fires. Without timeout_ms it watches until matched or the run is cancelled."
    )]
    async fn pane_watch(
        &self,
        Parameters(PaneWatchParams {
            pane_id,
            label,
            match_text,
            use_regex,
            source,
            timeout_ms,
        }): Parameters<PaneWatchParams>,
    ) -> Result<CallToolResult, McpError> {
        if match_text.is_empty() {
            return Err(user_err("match_text is required"));
        }
        let pid = resolve_pane_id(pane_id, label).await?;
        let regex = use_regex
            .unwrap_or(false)
            .then(|| Regex::new(&match_text))
            .transpose()
            .map_err(|e| user_err(format!("match_text: {e}")))?;
        let source = source.as_deref().unwrap_or("recent");
        let read = || async {
            herdr_cli(&[
                "pane",
                "read",
                &pid,
                "--source",
                source,
                "--lines",
                WATCH_LINES,
            ])
            .await
        };
        let deadline = timeout_ms
            .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms.into()));
        let mut seen = watch_matches(&read().await?, &match_text, regex.as_ref()).0;
        loop {
            tokio::time::sleep(WATCH_POLL).await;
            let text = read().await?;
            let (n, line) = watch_matches(&text, &match_text, regex.as_ref());
            if n > seen {
                return Ok(CallToolResult::success(vec![
                    Content::json(serde_json::json!({
                        "pane_id": pid,
                        "matched_line": line,
                        "matches": n,
                    }))
                    .map_err(to_mcp_err)?,
                ]));
            }
            // Matches that scrolled out or were redrawn away lower the bar,
            // so the text showing up again still counts as new.
            seen = n;
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                return Err(user_err(format!(
                    "pane_watch: no new {match_text:?} in {pid} within {} ms",
                    timeout_ms.unwrap_or_default()
                )));
            }
        }
    }

    // ── A2A primitives ─────────────────────────────────────────────────
    //
    // These amplify herdr's model: a pane *is* an agent, and the live registry
    // (fed by herdr's agent_status_changed events) lets one agent's captured
    // output become another agent's input. Usable both as recipe steps
    // (conductor) and as standalone MCP tools (peer-to-peer).

    #[tool(
        meta = category("A2A"),
        description = "Spawn an agent in a new pane from an agent profile and register it under `role`. `agent` is a profile id (see /api/profiles: claude, codex, opencode, gemini, hermes, zeroclaw, aider, shell, or your own) or any herdr-native agent kind. Optionally waits for `needs` targets (role or pane id) to be idle first (failing, before spawning, if one is unknown or never idles), then (wait_idle) for the new agent to be idle, capturing its output. Returns {pane_id, role, agent, status, output, wait_error}; a failed wait_idle leaves status `working` and sets wait_error."
    )]
    async fn agent_spawn(
        &self,
        Parameters(AgentSpawnParams {
            role,
            agent,
            args,
            cwd,
            workspace_id,
            tab_id,
            split,
            needs,
            wait_idle,
            label,
            in_pane,
        }): Parameters<AgentSpawnParams>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if in_pane.is_some() && (cwd.is_some() || split.is_some() || tab_id.is_some()) {
            return Err(user_err(
                "in_pane starts the agent in that pane as it is: drop cwd, split and tab_id",
            ));
        }
        // Wait for dependencies first (driven by herdr's agent state).
        let ws_for_resolve = workspace_id.clone().unwrap_or_default();
        for dep in &needs {
            let mut pane = self.registry.resolve(&ws_for_resolve, dep).await;
            if pane.is_none() {
                pane = self.registry.resolve("", dep).await;
            }
            let pane = pane.ok_or_else(|| user_err(format!("needs: no agent '{dep}'")))?;
            let wait = agent_wait_args(
                &pane,
                "idle",
                Some(self.timeout_ms(settings::AGENT_WAIT_MS)),
            );
            herdr_cli(&as_strs(&wait))
                .await
                .map_err(|e| user_err(format!("needs: '{dep}' never went idle: {}", e.message)))?;
        }

        let profile = crate::profiles::get(self.persistence.data_dir(), &agent)
            .await
            .unwrap_or_else(|| herdr_mcp_api::AgentProfile::native(&agent));
        let name = label
            .clone()
            .or_else(|| (!role.is_empty()).then(|| role.clone()))
            .unwrap_or_else(|| {
                format!(
                    "{agent}-{}",
                    &uuid::Uuid::new_v4().simple().to_string()[..6]
                )
            });
        let pane_id = crate::agents::spawn(
            &self.settings(),
            &profile,
            &name,
            &args,
            crate::agents::Placement {
                workspace: workspace_id.as_deref(),
                tab: tab_id.as_deref(),
                caller: self.caller_pane(&context.meta).as_deref(),
                direction: split.as_deref(),
                cwd: cwd.as_deref(),
                pane: in_pane.as_deref(),
            },
        )
        .await?;
        // Pane ids are `{workspace}:{pane}`.
        let ws = workspace_id
            .clone()
            .or_else(|| pane_id.split_once(':').map(|(w, _)| w.to_string()))
            .unwrap_or_default();
        self.registry
            .upsert(
                &pane_id,
                &ws,
                tab_id.as_deref().unwrap_or(""),
                &profile.id,
                &role,
            )
            .await;
        self.registry.set_label(&pane_id, &name).await;

        let mut status = "working".to_string();
        let mut output = String::new();
        let mut wait_error = None;
        if wait_idle {
            // The pane exists either way, so a failed wait is reported, not raised.
            let wait = agent_wait_args(
                &pane_id,
                "idle",
                Some(self.timeout_ms(settings::AGENT_WAIT_MS)),
            );
            match herdr_cli(&as_strs(&wait)).await {
                Ok(_) => {
                    status = "idle".to_string();
                    output = read_pane_text(&pane_id).await.unwrap_or_default();
                    self.registry.set_output(&pane_id, output.clone()).await;
                }
                Err(e) => wait_error = Some(e.message),
            }
        }

        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "pane_id": pane_id,
                "workspace_id": ws,
                "role": role,
                "agent": profile.id,
                "name": name,
                "status": status,
                "output": output,
                "wait_error": wait_error,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    /// The profile driving the agent in `pane` (registered by spawn, or the
    /// agent kind herdr detected); a native default if none matches.
    async fn profile_for_pane(&self, pane: &str) -> herdr_mcp_api::AgentProfile {
        let agent = self
            .registry
            .get(pane)
            .await
            .map(|h| h.agent)
            .unwrap_or_default();
        if agent.is_empty() {
            return herdr_mcp_api::AgentProfile::native("unknown");
        }
        crate::profiles::get(self.persistence.data_dir(), &agent)
            .await
            .unwrap_or_else(|| herdr_mcp_api::AgentProfile::native(&agent))
    }

    #[tool(
        meta = category("A2A"),
        description = "Send a message (text) to another agent and submit it. `target` may be a role, label or pane id. The text may interpolate {{role.output}} / {{pane_id.output}} from the session registry. Works for any agent profile (native or command)."
    )]
    async fn agent_message(
        &self,
        Parameters(AgentMessageParams { target, text }): Parameters<AgentMessageParams>,
    ) -> Result<CallToolResult, McpError> {
        let pane = resolve_target_pane(self, &target).await?;
        let profile = self.profile_for_pane(&pane).await;
        crate::agents::submit(&profile, &pane, &text).await?;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({ "pane_id": pane, "delivered": true }))
                .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("A2A"),
        description = "Prompt an agent and wait for its turn to finish; returns its reply. `target` may be a role, label or pane id. Works for any agent profile: herdr-native agents via `agent prompt --wait`, others by typing the prompt and waiting for herdr-mcp's idle detection. Returns {pane_id, role, agent, output}: output is the reply, also stored as the agent's output ({{role.output}})."
    )]
    async fn agent_prompt(
        &self,
        Parameters(AgentPromptParams {
            target,
            text,
            timeout_ms,
        }): Parameters<AgentPromptParams>,
    ) -> Result<CallToolResult, McpError> {
        let pane = resolve_target_pane(self, &target).await?;
        let profile = self.profile_for_pane(&pane).await;
        let timeout = timeout_ms.unwrap_or_else(|| self.timeout_ms(settings::AGENT_PROMPT_MS));
        let reply =
            crate::agents::prompt(&self.settings(), &profile, &pane, &text, timeout).await?;
        self.registry.set_output(&pane, reply.clone()).await;
        let role = self
            .registry
            .get(&pane)
            .await
            .map(|h| h.role)
            .unwrap_or_default();
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "pane_id": pane,
                "role": role,
                "agent": profile.id,
                "output": reply,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("A2A"),
        description = "Interrupt an agent: stop its current turn (default, the profile's `interrupt` keys, e.g. Esc) or quit it (`hard`, the profile's `stop` keys, e.g. Ctrl-C). `target` may be a role, label or pane id."
    )]
    async fn agent_interrupt(
        &self,
        Parameters(AgentInterruptParams { target, hard }): Parameters<AgentInterruptParams>,
    ) -> Result<CallToolResult, McpError> {
        let pane = resolve_target_pane(self, &target).await?;
        let profile = self.profile_for_pane(&pane).await;
        crate::agents::interrupt(&profile, &pane, hard).await?;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({ "pane_id": pane, "hard": hard }))
                .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("A2A"),
        description = "Read an agent's output and store it as its work product in the registry. Returns {pane_id, role, agent, status, output}. `target` may be a role, label or pane id."
    )]
    async fn agent_read(
        &self,
        Parameters(AgentReadParams {
            target,
            source,
            lines,
        }): Parameters<AgentReadParams>,
    ) -> Result<CallToolResult, McpError> {
        let pane = resolve_target_pane(self, &target).await?;
        let lines_str = lines.map(|n| n.to_string());
        let mut args = vec!["pane", "read", &pane];
        if let Some(ref src) = source {
            args.extend(["--source", src]);
        }
        if let Some(ref n) = lines_str {
            args.extend(["--lines", n]);
        }
        let stored = herdr_cli(&args).await?;
        self.registry.set_output(&pane, stored.clone()).await;
        let handle = self.registry.get(&pane).await;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "pane_id": pane,
                "role": handle.as_ref().map(|h| h.role.clone()).unwrap_or_default(),
                "agent": handle.as_ref().map(|h| h.agent.clone()).unwrap_or_default(),
                "status": handle.as_ref().map(|h| h.status.clone()).unwrap_or_default(),
                "output": stored,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("A2A"),
        description = "Wait for an agent to reach a status (default idle): idle, working, blocked, done or unknown. Blocks until reached or timeout. `target` may be a role, label or pane id."
    )]
    async fn agent_wait(
        &self,
        Parameters(AgentWaitParams {
            target,
            status,
            timeout_ms,
        }): Parameters<AgentWaitParams>,
    ) -> Result<CallToolResult, McpError> {
        let pane = resolve_target_pane(self, &target).await?;
        let args = agent_wait_args(&pane, &status, timeout_ms);
        run_herdr_json(&as_strs(&args)).await
    }

    #[tool(
        meta = category("A2A"),
        description = "List registered agents in the live registry, filtered by workspace id. Each entry has pane_id, role, agent, status, and last captured output."
    )]
    async fn agent_list(
        &self,
        Parameters(AgentListParams { workspace_id }): Parameters<AgentListParams>,
    ) -> Result<CallToolResult, McpError> {
        let handles = if workspace_id.is_empty() {
            // No global list in registry; call herdr for the full picture.
            self.registry.inner_snapshot().await
        } else {
            self.registry.list_for_ws(&workspace_id).await
        };
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({ "agents": handles })).map_err(to_mcp_err)?,
        ]))
    }

    #[tool(meta = category("Variables"), description = "Get a session variable (scoped to a herdr workspace id).")]
    async fn var_get(
        &self,
        Parameters(VarGetParams { session_id, key }): Parameters<VarGetParams>,
    ) -> Result<CallToolResult, McpError> {
        let vars = self
            .persistence
            .load_session_vars(&session_id)
            .await
            .map_err(to_mcp_err)?;
        let value = vars
            .variables
            .get(&key)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "session_id": session_id,
                "key": key,
                "value": value,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("Variables"),
        description = "Set a session variable (scoped to a herdr workspace id). Persisted for chaining across recipe runs."
    )]
    async fn var_set(
        &self,
        Parameters(VarSetParams {
            session_id,
            key,
            value,
        }): Parameters<VarSetParams>,
    ) -> Result<CallToolResult, McpError> {
        // Guard against unresolved `{{ template.session_id }}` literals leaking
        // into the session store as a filename (B3). Canonical session ids are
        // herdr workspace ids; fall back to "w1" (template default).
        let session_id = if session_id.trim().is_empty() || session_id.contains("{{") {
            tracing::debug!("var_set: sanitized session_id {session_id:?} -> w1");
            "w1".to_string()
        } else {
            session_id
        };
        let mut vars = self
            .persistence
            .load_session_vars(&session_id)
            .await
            .map_err(to_mcp_err)?;
        vars.variables.insert(key.clone(), value.clone());
        vars.updated_at = chrono::Utc::now();
        self.persistence
            .save_session_vars(&vars)
            .await
            .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "session_id": session_id,
                "key": key,
                "value": value,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    // ── Recipe templates ───────────────────────────────────────────────

    #[tool(
        meta = category("Templates"),
        description = "List the bundled recipe templates (e.g. dev-watch, git-status, build-and-test). Each has variables you fill in before instantiating."
    )]
    async fn list_templates(
        &self,
        Parameters(_): Parameters<ListTemplatesParams>,
    ) -> Result<CallToolResult, McpError> {
        let templates = tmpl::list_templates();
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "count": templates.len(),
                "templates": templates,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(meta = category("Templates"), description = "Get a single recipe template by id (variables + steps).")]
    async fn get_template(
        &self,
        Parameters(GetTemplateParams { template_id }): Parameters<GetTemplateParams>,
    ) -> Result<CallToolResult, McpError> {
        match tmpl::find_template(&template_id) {
            Some(t) => Ok(CallToolResult::success(vec![
                Content::json(serde_json::json!(t)).map_err(to_mcp_err)?,
            ])),
            None => Err(McpError::invalid_params(
                format!("template not found: {template_id}"),
                None,
            )),
        }
    }

    #[tool(
        meta = category("Templates"),
        description = "Instantiate a template into a runnable recipe and auto-save it to the recipe library. Returns the saved recipe (with its id) ready to run or edit."
    )]
    async fn instantiate_template(
        &self,
        Parameters(InstantiateTemplateParams {
            template_id,
            variables,
            name,
        }): Parameters<InstantiateTemplateParams>,
    ) -> Result<CallToolResult, McpError> {
        let recipe = tmpl::instantiate(&template_id, variables, name).ok_or_else(|| {
            McpError::invalid_params(format!("template not found: {template_id}"), None)
        })?;
        self.persistence
            .save_recipe(&recipe)
            .await
            .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "saved": true,
                "recipe": recipe,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    // ── HTTP & LLM ─────────────────────────────────────────────────────

    #[tool(
        meta = category("HTTP"),
        description = "Send an HTTP request and return {status, ok, headers, body} (the response body parsed as JSON when it is). Send a text body with body or a JSON one with json; url, headers, query and bodies may use {{ }} placeholders in recipes. A non-2xx status is returned, not raised, unless check is true."
    )]
    async fn http_request(
        &self,
        Parameters(HttpRequestParams {
            method,
            url,
            headers,
            query,
            body,
            json,
            check,
            timeout_ms,
        }): Parameters<HttpRequestParams>,
    ) -> Result<CallToolResult, McpError> {
        let body = match (body, json) {
            (Some(_), Some(_)) => return Err(user_err("pass body or json, not both")),
            (Some(text), None) => crate::outbound::Body::Text(text),
            (None, None | Some(serde_json::Value::Null)) => crate::outbound::Body::None,
            (None, Some(json)) => crate::outbound::Body::Json(json),
        };
        let ms = timeout_ms.unwrap_or_else(|| self.timeout_ms(settings::HTTP_REQUEST_MS));
        let out = crate::outbound::send(
            &self.security.guard,
            crate::outbound::Request {
                method: method.as_deref().unwrap_or("GET"),
                url: &url,
                headers: &headers,
                query: &query,
                body,
                timeout: std::time::Duration::from_millis(ms.into()),
            },
        )
        .await
        .map_err(|e| user_err(format!("{e:#}")))?;
        if check && out["ok"] != true {
            return Err(user_err(format!(
                "{url}: HTTP {}: {}",
                out["status"], out["body"]
            )));
        }
        Ok(CallToolResult::success(vec![
            Content::json(out).map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("LLM"),
        description = "Call an OpenAI-compatible chat model (OpenRouter by default; OpenAI, Ollama, LM Studio, … via base_url) and return {text, model, finish_reason, usage}. The key comes from api_key, else the api_key_env environment variable (default OPENROUTER_API_KEY / OPENAI_API_KEY). In recipes, {{step.text}} is the reply."
    )]
    async fn llm_call(
        &self,
        Parameters(LlmCallParams {
            model,
            prompt,
            system,
            messages,
            base_url,
            api_key,
            api_key_env,
            temperature,
            max_tokens,
            extra,
            timeout_ms,
        }): Parameters<LlmCallParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::outbound as out;
        let base = base_url.as_deref().unwrap_or(out::OPENROUTER);
        let key = out::api_key(
            base,
            api_key.as_deref(),
            api_key_env.as_deref(),
            &self.security.config.key_env,
        )
        .map_err(|e| user_err(e.to_string()))?;
        let body = out::chat_body(&out::Chat {
            model: &model,
            system: system.as_deref(),
            prompt: prompt.as_deref(),
            messages: &messages,
            temperature,
            max_tokens,
            extra: extra.as_ref(),
        })
        .map_err(|e| user_err(e.to_string()))?;
        let ms = timeout_ms.unwrap_or_else(|| self.timeout_ms(settings::LLM_CALL_MS));
        let resp = out::api(
            &self.security.guard,
            base,
            "chat/completions",
            key.as_deref(),
            Some(&body),
            std::time::Duration::from_millis(ms.into()),
        )
        .await
        .map_err(|e| user_err(llm_error(e, base, api_key_env.as_deref(), key.is_some())))?;
        let reply = out::chat_reply(&resp).map_err(|e| user_err(e.to_string()))?;
        Ok(CallToolResult::success(vec![
            Content::json(reply).map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("LLM"),
        description = "List the models an OpenAI-compatible API offers ({base_url, models: [{id, name, context_length}]}), OpenRouter by default. Same key rules as llm_call."
    )]
    async fn llm_models(
        &self,
        Parameters(LlmModelsParams {
            base_url,
            api_key,
            api_key_env,
            filter,
        }): Parameters<LlmModelsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::outbound as out;
        let base = base_url.as_deref().unwrap_or(out::OPENROUTER);
        let key = out::api_key(
            base,
            api_key.as_deref(),
            api_key_env.as_deref(),
            &self.security.config.key_env,
        )
        .map_err(|e| user_err(e.to_string()))?;
        let ms = self.timeout_ms(settings::LLM_CALL_MS);
        let resp = out::api(
            &self.security.guard,
            base,
            "models",
            key.as_deref(),
            None,
            std::time::Duration::from_millis(ms.into()),
        )
        .await
        .map_err(|e| user_err(llm_error(e, base, api_key_env.as_deref(), key.is_some())))?;
        let needle = filter.unwrap_or_default().to_lowercase();
        let models: Vec<_> = out::model_list(&resp)
            .into_iter()
            .filter(|m| {
                m["id"]
                    .as_str()
                    .is_some_and(|id| id.to_lowercase().contains(&needle))
            })
            .collect();
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({ "base_url": base, "models": models }))
                .map_err(to_mcp_err)?,
        ]))
    }

    // ── Recipes ────────────────────────────────────────────────────────

    #[tool(
        meta = category("Recipes"),
        description = "List the saved recipes: id, name, description, category, step ids and trigger count. Run one with recipe_run."
    )]
    async fn recipe_list(
        &self,
        Parameters(_): Parameters<RecipeListParams>,
    ) -> Result<CallToolResult, McpError> {
        let recipes = self
            .persistence
            .list_recipes(None)
            .await
            .map_err(to_mcp_err)?;
        let list: Vec<_> = recipes
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "name": r.name,
                    "description": r.description,
                    "category": r.category,
                    "steps": r.steps.iter().map(|s| &s.id).collect::<Vec<_>>(),
                    "triggers": r.triggers.len(),
                })
            })
            .collect();
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({ "recipes": list })).map_err(to_mcp_err)?,
        ]))
    }

    #[tool(meta = category("Recipes"), description = "Get a saved recipe by id: its steps, variables and triggers.")]
    async fn recipe_get(
        &self,
        Parameters(RecipeGetParams { recipe_id }): Parameters<RecipeGetParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = parse_id("recipe_id", &recipe_id)?;
        let recipe = self
            .persistence
            .load_recipe(&id)
            .await
            .map_err(to_mcp_err)?
            .ok_or_else(|| user_err(format!("no recipe {recipe_id}")))?;
        Ok(CallToolResult::success(vec![
            Content::json(recipe).map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("Recipes"),
        description = "Save a recipe. Without recipe_id it creates one (name and steps required); with recipe_id the fields given replace the saved ones. Steps run in order and a failing step stops the run; a step's params may use {{ stepId.path }} (an earlier step's result) and {{ var }} (variables). An optional gate decides what a full run does after the last step: its rules are checked in order and the first whose checks hold ends the run, restarts it from step 1 (a loop; {{ run.pass }} counts passes), fails it, or hands off to another saved recipe (action run, with collection = its id and optional inputs whose {{ }} fill from this run): the other recipe runs as a child run with its own variables, and this run ends as the child ends; max_steps caps the requests one run may execute. Returns the saved recipe."
    )]
    async fn recipe_save(
        &self,
        Parameters(RecipeSaveParams {
            recipe_id,
            name,
            description,
            steps,
            variables,
            triggers,
            category,
            gate,
            trail,
        }): Parameters<RecipeSaveParams>,
    ) -> Result<CallToolResult, McpError> {
        let recipe = match recipe_id {
            Some(raw) => {
                let id = parse_id("recipe_id", &raw)?;
                let req = UpdateRecipeRequest {
                    name,
                    description,
                    steps,
                    variables,
                    triggers,
                    category,
                    gate,
                    trail,
                };
                self.update_recipe(id, req)
                    .await
                    .map_err(to_mcp_err)?
                    .ok_or_else(|| user_err(format!("no recipe {raw}")))?
            }
            None => {
                let (Some(name), Some(steps)) = (name, steps) else {
                    return Err(user_err("a new recipe needs name and steps"));
                };
                let req = CreateRecipeRequest {
                    name,
                    description,
                    steps,
                    variables: variables.unwrap_or_default(),
                    category,
                    triggers: triggers.unwrap_or_default(),
                    gate,
                    trail: trail.unwrap_or_default(),
                };
                self.create_recipe(req).await.map_err(to_mcp_err)?
            }
        };
        Ok(CallToolResult::success(vec![
            Content::json(recipe).map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("Recipes"),
        description = "Run a saved recipe (recipe_id) or ad-hoc steps. Returns the run record at once ({id, status, steps, results, ...}); follow it with run_get, or pass wait: true to get the finished record. variables are added to the run's scope."
    )]
    async fn recipe_run(
        &self,
        Parameters(RecipeRunParams {
            recipe_id,
            steps,
            name,
            session_id,
            variables,
            step_ids,
            wait,
        }): Parameters<RecipeRunParams>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if recipe_id.is_none() && steps.is_none() {
            return Err(user_err("pass recipe_id or steps"));
        }
        let req = herdr_mcp_api::StartRunRequest {
            recipe_id: recipe_id
                .as_deref()
                .map(|raw| parse_id("recipe_id", raw))
                .transpose()?,
            steps,
            name,
            session_id,
            variables,
            step_ids,
        };
        let origin = match self.origin(&context.meta) {
            Origin::Mcp => herdr_mcp_api::RunOrigin::Agent {
                pane_id: herdr_mcp_core::session_context::pane_id(),
            },
            Origin::Run => herdr_mcp_api::RunOrigin::Run {
                run_id: context
                    .meta
                    .0
                    .get(crate::runs::STEP_OF_KEY)
                    .and_then(serde_json::Value::as_str)
                    .and_then(|id| id.parse().ok()),
            },
            Origin::Api => herdr_mcp_api::RunOrigin::Api,
        };
        let (run, done) = crate::runs::start_run(self, req, origin)
            .await
            .map_err(|e| user_err(start_error_to_http(e).1))?;
        let run = if wait {
            done.await.map_err(to_mcp_err)?
        } else {
            run
        };
        Ok(CallToolResult::success(vec![
            Content::json(run).map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("Recipes"),
        description = "Get a run's record: status (running, paused, completed, failed, cancelled, interrupted), each step's state and result, and the error of a failed step. With trail: true, the run's trail as text instead: each event with its time, the params each step sent, why the gate chose its rule, a snapshot of the pane a failed run last used, and, indented under it, the trail of the child run the gate handed off to."
    )]
    async fn run_get(
        &self,
        Parameters(RunGetParams { run_id, trail }): Parameters<RunGetParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = parse_id("run_id", &run_id)?;
        let run = crate::runs::load(self, id)
            .await
            .map_err(to_mcp_err)?
            .ok_or_else(|| user_err(format!("no run {run_id}")))?;
        if trail {
            let text = crate::runs::trail(self, id)
                .await
                .map_err(to_mcp_err)?
                .unwrap_or_default();
            return Ok(CallToolResult::success(vec![Content::text(text)]));
        }
        Ok(CallToolResult::success(vec![
            Content::json(run).map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("Recipes"),
        description = "Pause, resume or cancel a run. Pause takes effect at the next step; cancel also stops the step in flight (e.g. a pane_watch)."
    )]
    async fn run_control(
        &self,
        Parameters(RunControlParams { run_id, action }): Parameters<RunControlParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = parse_id("run_id", &run_id)?;
        let act = herdr_mcp_api::RunAction::parse(&action).ok_or_else(|| {
            user_err(format!(
                "action must be pause, resume or cancel, not {action:?}"
            ))
        })?;
        if !self.runs.control(id, act) {
            return Err(user_err(format!("run {run_id} is not executing")));
        }
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({ "run_id": id, "action": act.as_str() }))
                .map_err(to_mcp_err)?,
        ]))
    }

    // ── Scheduler ──────────────────────────────────────────────────────

    #[tool(
        meta = category("Scheduler"),
        description = "Schedule a persisted recipe to run on a cron expression. Returns the created schedule (with its id). Persisted across restarts."
    )]
    async fn schedule_recipe(
        &self,
        Parameters(ScheduleRecipeParams {
            recipe_id,
            cron_schedule,
            enabled,
        }): Parameters<ScheduleRecipeParams>,
    ) -> Result<CallToolResult, McpError> {
        let recipe_id = uuid::Uuid::parse_str(&recipe_id).map_err(to_mcp_err)?;
        let schedule = ScheduledRecipe {
            id: uuid::Uuid::new_v4(),
            recipe_id,
            cron_schedule: cron_schedule.clone(),
            next_run: None,
            last_run: None,
            enabled,
            created_at: chrono::Utc::now(),
        };
        self.scheduler
            .schedule_one(schedule.clone())
            .await
            .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "scheduled": true,
                "schedule": schedule,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("Scheduler"),
        description = "List all active recipe schedules, with their next run time and enabled state."
    )]
    async fn list_schedules(
        &self,
        Parameters(_): Parameters<ListSchedulesParams>,
    ) -> Result<CallToolResult, McpError> {
        let schedules = self.scheduler.list().await;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "count": schedules.len(),
                "schedules": schedules,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(meta = category("Scheduler"), description = "Delete a recipe schedule by id.")]
    async fn delete_schedule(
        &self,
        Parameters(DeleteScheduleParams { id }): Parameters<DeleteScheduleParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = uuid::Uuid::parse_str(&id).map_err(to_mcp_err)?;
        let removed = self.scheduler.remove(id).await.map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "removed": removed,
                "id": id,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(meta = category("Scheduler"), description = "Enable or disable an existing recipe schedule.")]
    async fn enable_schedule(
        &self,
        Parameters(EnableScheduleParams { id, enabled }): Parameters<EnableScheduleParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = uuid::Uuid::parse_str(&id).map_err(to_mcp_err)?;
        let ok = self
            .scheduler
            .set_enabled(id, enabled)
            .await
            .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "updated": ok,
                "id": id,
                "enabled": enabled,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    // ── Clipboard ───────────────────────────────────────────────────────

    #[tool(
        meta = category("Clipboard"),
        description = "Write text to the system clipboard (the OS clipboard, or the configured [clipboard] commands). Text only."
    )]
    async fn clipboard_set(
        &self,
        Parameters(ClipboardSetParams { text }): Parameters<ClipboardSetParams>,
    ) -> Result<CallToolResult, McpError> {
        let length = text.len();
        crate::clipboard::set(&self.clipboard_config(), text)
            .await
            .map_err(|e| to_mcp_err(format!("{e:#}")))?;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({ "ok": true, "length": length }))
                .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        meta = category("Clipboard"),
        description = "Read the system clipboard's text (the OS clipboard, or the configured [clipboard] commands). Returns an empty string when it holds no text. Text only."
    )]
    async fn clipboard_get(
        &self,
        Parameters(_): Parameters<ClipboardGetParams>,
    ) -> Result<CallToolResult, McpError> {
        let text = crate::clipboard::get(&self.clipboard_config())
            .await
            .map_err(|e| to_mcp_err(format!("{e:#}")))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

#[tool_handler]
impl ServerHandler for HerdrMcpServer {
    /// Every tool call, from any caller: refused if `[security]` says no,
    /// else routed; audited either way.
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let origin = self.origin(&context.meta);
        let name = request.name.to_string();
        let args = request.arguments.clone();
        let result = match self.permit(origin, &name, args.as_ref()).await {
            Ok(()) => {
                let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
                Self::tool_router().call(tcc).await
            }
            Err(e) => Err(e),
        };
        self.security.audit(
            origin,
            &name,
            args.as_ref(),
            match &result {
                Ok(r) => Ok(r.is_error == Some(true)),
                Err(e) => Err(&e.message),
            },
        );
        result
    }

    /// The enabled tools `[mcp] tools` lists (the in-process session, which
    /// the bridge and recipes call through, sees every one).
    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, McpError> {
        let mut tools = self.enabled_tools();
        if !self.in_process {
            tools.retain(|t| self.listed.iter().any(|e| lists(e, t, self.recipes)));
            for tool in &mut tools {
                let mut schema = (*tool.input_schema).clone();
                schema.remove("$schema");
                slim_schema(&mut schema);
                tool.input_schema = std::sync::Arc::new(schema);
            }
        }
        Ok(rmcp::model::ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::new(
            "herdr-mcp",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(format!(
            "{INSTRUCTIONS}\n{}",
            match self.recipes {
                herdr_mcp_core::RecipeAccess::Tools => RECIPES_BY_TOOL,
                herdr_mcp_core::RecipeAccess::Bridge => RECIPES_BY_BRIDGE,
            }
        ))
    }

    /// The agent guide, for clients that read resources.
    async fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListResourcesResult, McpError> {
        use rmcp::model::AnnotateAble;
        let mut guide = rmcp::model::RawResource::new(GUIDE_URI, "agent-guide");
        guide.description = Some(
            "How to drive herdr with these tools: finding panes, waiting, agents, recipes".into(),
        );
        guide.mime_type = Some("text/markdown".into());
        Ok(rmcp::model::ListResourcesResult {
            resources: vec![guide.no_annotation()],
            meta: None,
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        request: rmcp::model::ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResult, McpError> {
        if request.uri != GUIDE_URI {
            return Err(McpError::resource_not_found(
                format!("no resource {}", request.uri),
                None,
            ));
        }
        Ok(rmcp::model::ReadResourceResult::new(vec![
            rmcp::model::ResourceContents::text(GUIDE, GUIDE_URI),
        ]))
    }
}

const GUIDE_URI: &str = "herdr-mcp://agent-guide";
const GUIDE: &str = include_str!("../../../docs/AGENT-GUIDE.md");

/// Sent to every MCP client at connect: what herdr-mcp is for, and the
/// rules herdr's own agent skill teaches, said in these tools' terms.
const INSTRUCTIONS: &str = "\
Control herdr, the terminal multiplexer for coding agents: workspaces → tabs → panes \
(ids like w1, w1:t1, w1:p2; prefer labels, and re-read ids with list_panes after \
closing or moving panes; never guess them).
- Placement: from inside a herdr pane, split_pane and agent_spawn split YOUR pane by \
default (right if it is wide, down if tall) and leave focus where it is. Don't create \
workspaces or tabs unless asked.
- Delegate: agent_spawn, then agent_prompt (sends, waits for the turn to end, returns the \
reply). agent_message only sends. Commands: pane_exec waits and returns exit code and \
output; run_command only queues the line.
- Agent states: idle/done = ready for input; working = busy; blocked = an approval or \
question is on screen: read it (agent_read) and ask the user before answering; unknown \
does not mean finished.
- A timeout does not mean the prompt was lost: read the pane before sending it again.
- Close only panes and tabs you created, unless the user asks.
- If the user can't copy text out of an agent's TUI, point them to the bridge's web page \
(http://localhost:7676 by default): it sends to and reads any pane as plain browser text.
- herdr_api calls any herdr socket method (worktree.*, pane.move, layout, notifications) \
when no tool covers it.
Not every tool is listed (recipe authoring, schedules, templates, variables, HTTP, LLM, \
clipboard): the user can list them under [mcp] tools in herdr-mcp's config. More: the \
herdr-mcp://agent-guide resource.";

/// The recipes rule [`INSTRUCTIONS`] ends with, per `[mcp] recipes`.
const RECIPES_BY_TOOL: &str =
    "- Recipes: recipe_list, recipe_run, then run_get (trail: true explains a failure).";
const RECIPES_BY_BRIDGE: &str = "\
- Recipes (saved tool-call sequences) run through herdr-mcp's HTTP bridge, not MCP tools: \
use the herdr-mcp-recipes skill.";

/// Error code of a call `[security]` refuses (the bridge answers 403).
const FORBIDDEN: rmcp::model::ErrorCode = rmcp::model::ErrorCode(-32001);

fn forbidden(msg: impl Into<std::borrow::Cow<'static, str>>) -> McpError {
    McpError::new(FORBIDDEN, msg, None)
}

impl HerdrMcpServer {
    /// Where a call came from: what the in-process session tagged its
    /// request `_meta` with, else an MCP client.
    fn origin(&self, meta: &rmcp::model::Meta) -> Origin {
        if !self.in_process {
            return Origin::Mcp;
        }
        meta.0
            .get(Origin::META_KEY)
            .and_then(serde_json::Value::as_str)
            .and_then(Origin::parse)
            .unwrap_or(Origin::Api)
    }

    /// The herdr pane an MCP client runs in: this stdio server inherits its
    /// environment. None for the bridge and recipes (the env is the
    /// bridge's own) and outside herdr.
    fn caller_pane(&self, meta: &rmcp::model::Meta) -> Option<String> {
        (self.origin(meta) == Origin::Mcp)
            .then(herdr_mcp_core::session_context::pane_id)
            .flatten()
    }

    /// Every tool not in `[security] disabled-tools`.
    pub fn enabled_tools(&self) -> Vec<rmcp::model::Tool> {
        Self::tool_router()
            .list_all()
            .into_iter()
            .filter(|t| self.security.tool_enabled(&t.name))
            .collect()
    }

    /// Whether `[security]` lets `origin` call `name` with `args`: the tool
    /// must be enabled, and only the bridge API may add triggers or
    /// schedules (or change a recipe that has them) unless
    /// `agent-triggers` is on.
    async fn permit(
        &self,
        origin: Origin,
        name: &str,
        args: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<(), McpError> {
        if !self.security.tool_enabled(name) {
            return Err(forbidden(format!(
                "{name} is disabled by [security] disabled-tools"
            )));
        }
        if origin == Origin::Api || self.security.config.agent_triggers {
            return Ok(());
        }
        let refuse = |what: &str| {
            Err(forbidden(format!(
                "{what}: only the web UI or TUI may, unless [security] agent-triggers = true \
                 (this call came from {})",
                match origin {
                    Origin::Run => "a recipe run",
                    _ => "an MCP client",
                }
            )))
        };
        match name {
            "schedule_recipe" | "enable_schedule" => refuse("scheduling a recipe"),
            "recipe_save" => {
                let arg = |k: &str| args.and_then(|a| a.get(k));
                if arg("triggers")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|t| !t.is_empty())
                {
                    return refuse("adding triggers");
                }
                let Some(id) = arg("recipe_id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|r| r.parse::<uuid::Uuid>().ok())
                else {
                    return Ok(());
                };
                let triggered = self
                    .persistence
                    .load_recipe(&id)
                    .await
                    .map_err(to_mcp_err)?
                    .is_some_and(|r| !r.triggers.is_empty());
                let scheduled = self
                    .persistence
                    .list_schedules()
                    .await
                    .map_err(to_mcp_err)?
                    .iter()
                    .any(|s| s.recipe_id == id);
                if triggered || scheduled {
                    return refuse("changing a recipe with triggers or a schedule");
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// An LLM API error, with a hint naming the key's variable when no key
/// was sent.
fn llm_error(e: anyhow::Error, base: &str, key_env: Option<&str>, had_key: bool) -> String {
    let hint = match key_env.or_else(|| crate::outbound::default_key_env(base)) {
        Some(var) if !had_key => {
            format!(" (no API key: set {var} or pass api_key / api_key_env)")
        }
        _ => String::new(),
    };
    format!("{e:#}{hint}")
}

/// Parse a uuid argument named `what`.
fn parse_id(what: &str, raw: &str) -> Result<uuid::Uuid, McpError> {
    raw.parse()
        .map_err(|_| user_err(format!("{what}: {raw:?} is not a uuid")))
}

/// Map any displayable error into an `McpError` for `#[tool]` methods.
pub(crate) fn to_mcp_err(e: impl std::fmt::Display) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

/// A request the caller can fix (bad target, herdr refused it): code -32000.
pub(crate) fn user_err(msg: impl Into<std::borrow::Cow<'static, str>>) -> McpError {
    McpError::new(rmcp::model::ErrorCode(-32000), msg, None)
}

/// Recursively find a string value for `key` anywhere in a JSON tree.
fn extract_string(value: &serde_json::Value, key: &str) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(v) = map.get(key)
                && let Some(s) = v.as_str()
            {
                return Some(s.to_string());
            }
            for v in map.values() {
                if let Some(found) = extract_string(v, key) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                if let Some(found) = extract_string(v, key) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// Find the `pane_id` field anywhere in a herdr JSON response.
pub(crate) fn extract_pane_id(value: &serde_json::Value) -> Option<String> {
    extract_string(value, "pane_id")
}

// ── herdr 0.9 argv builders ─────────────────────────────────────────────
// Pure, so the exact command lines are pinned by tests.

pub(crate) fn as_strs(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

/// Submit `text` (+ Enter) to an agent: `agent prompt` (0.9 has no `agent send`).
pub(crate) fn agent_submit_args(target: &str, text: &str) -> Vec<String> {
    ["agent", "prompt", target, text].map(String::from).to_vec()
}

/// `agent wait <target> --until <status> [--timeout ms]` (0.9: `--until`,
/// not `--status`; also takes pane ids, replacing `wait agent-status`).
pub(crate) fn agent_wait_args(target: &str, status: &str, timeout_ms: Option<u32>) -> Vec<String> {
    let mut args: Vec<String> = ["agent", "wait", target, "--until", status]
        .map(String::from)
        .to_vec();
    if let Some(ms) = timeout_ms {
        args.extend(["--timeout".to_string(), ms.to_string()]);
    }
    args
}

/// How often `pane_watch` reads its pane, and how many lines it reads.
const WATCH_POLL: std::time::Duration = std::time::Duration::from_millis(500);
const WATCH_LINES: &str = "500";

/// How many times `needle` (or `regex`) occurs in `text`'s lines, and the
/// last line holding an occurrence.
fn watch_matches<'t>(
    text: &'t str,
    needle: &str,
    regex: Option<&Regex>,
) -> (usize, Option<&'t str>) {
    let mut total = 0;
    let mut last = None;
    for line in text.lines() {
        let n = match regex {
            Some(re) => re.find_iter(line).count(),
            None => line.matches(needle).count(),
        };
        if n > 0 {
            total += n;
            last = Some(line);
        }
    }
    (total, last)
}

/// `pane wait-output <pane> (--match T | --regex P) [--source S] [--timeout ms]`
/// (0.9 moved it under `pane`; `--regex` takes the pattern itself).
fn wait_output_args(
    pane: &str,
    pattern: &str,
    regex: bool,
    source: Option<&str>,
    timeout_ms: Option<u32>,
) -> Vec<String> {
    let flag = if regex { "--regex" } else { "--match" };
    let mut args: Vec<String> = ["pane", "wait-output", pane, flag, pattern]
        .map(String::from)
        .to_vec();
    if let Some(src) = source {
        args.extend(["--source".to_string(), src.to_string()]);
    }
    if let Some(ms) = timeout_ms {
        args.extend(["--timeout".to_string(), ms.to_string()]);
    }
    args
}

/// Marker `pane_exec` prints after the command; the echoed command line
/// spells it differently (`%s` placeholders), so only the real output
/// matches [`exec_done_regex`].
fn exec_line(command: &str, token: &str) -> String {
    format!("{command}; printf '\\n%s_%s:%s\\n' HERDR_DONE {token} $?")
}

fn exec_done_regex(token: &str) -> String {
    format!("HERDR_DONE_{token}:[0-9]+")
}

/// The output and exit code of a `pane_exec` run in `screen`: the lines
/// after the echoed command line (or from the top if it scrolled away) up
/// to the end marker.
fn exec_result(screen: &str, token: &str) -> Option<(String, i32)> {
    let done = format!("HERDR_DONE_{token}:");
    let lines: Vec<&str> = screen.lines().collect();
    let end = lines.iter().rposition(|l| l.starts_with(&done))?;
    let code = lines[end][done.len()..].trim().parse().ok()?;
    let echo = format!("HERDR_DONE {token}");
    let start = lines[..end]
        .iter()
        .rposition(|l| l.contains(&echo))
        .map_or(0, |i| i + 1);
    Some((lines[start..end].join("\n").trim_end().to_string(), code))
}

/// The last `n` lines of `text`.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Resolve an a2a target (role, label or pane id) to a pane id. A target
/// the registry doesn't know passes through as a pane id; herdr rejects a
/// bad one.
async fn resolve_target_pane(server: &HerdrMcpServer, target: &str) -> Result<String, McpError> {
    if target.is_empty() {
        return Err(user_err("target is required"));
    }
    Ok(server
        .registry
        .resolve("", target)
        .await
        .unwrap_or_else(|| target.to_string()))
}

/// A pane's last 200 lines (its work product once its agent is idle).
pub(crate) async fn read_pane_text(pane_id: &str) -> Result<String, McpError> {
    herdr_cli(&[
        "pane", "read", pane_id, "--source", "recent", "--lines", "200",
    ])
    .await
}

/// Run `herdr` CLI args and return stdout as a JSON-pretty-printed `CallToolResult`.
async fn run_herdr_json(args: &[&str]) -> Result<CallToolResult, McpError> {
    herdr_json_in(None, args).await
}

/// [`run_herdr_json`] in herdr `session` (`None`: this server's).
async fn herdr_json_in(session: Option<&str>, args: &[&str]) -> Result<CallToolResult, McpError> {
    let output = crate::panes::herdr_in(session, args).await?;

    match serde_json::from_str::<serde_json::Value>(&output) {
        Ok(value) => {
            let content = Content::json(value).map_err(|e| {
                McpError::internal_error(format!("Failed to serialize JSON: {e}"), None)
            })?;
            Ok(CallToolResult::success(vec![content]))
        }
        Err(e) => {
            tracing::debug!("herdr output was not valid JSON: {e}");
            Ok(CallToolResult::success(vec![Content::text(output)]))
        }
    }
}

/// Run `herdr` CLI args in `session` (`None`: this server's) and return
/// stdout as plain text.
async fn herdr_text_in(session: Option<&str>, args: &[&str]) -> Result<CallToolResult, McpError> {
    let output = crate::panes::herdr_in(session, args).await?;
    Ok(CallToolResult::success(vec![Content::text(output)]))
}

/// A typed value as a tool's JSON result.
fn json_result(value: &impl Serialize) -> Result<CallToolResult, McpError> {
    let value = serde_json::to_value(value).map_err(to_mcp_err)?;
    Ok(CallToolResult::success(vec![
        Content::json(value).map_err(to_mcp_err)?,
    ]))
}

/// Execute the `herdr` CLI binary with the given arguments.
pub(crate) async fn herdr_cli(args: &[&str]) -> Result<String, McpError> {
    herdr_cli_at(None, args).await
}

/// [`herdr_cli`] against another session's API socket (`None`: this
/// server's own, exported at startup).
pub(crate) async fn herdr_cli_at(socket: Option<&str>, args: &[&str]) -> Result<String, McpError> {
    tracing::debug!("herdr {}", args.join(" "));

    let binary = herdr_mcp_core::herdr_bin();
    let mut cmd = tokio::process::Command::new(binary);
    if let Some(socket) = socket {
        cmd.env("HERDR_SOCKET_PATH", socket);
    }
    let output = cmd
        .args(args)
        .output()
        .await
        .map_err(|e| McpError::internal_error(format!("Failed to execute herdr: {e}"), None))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let msg = if !stderr.is_empty() {
            stderr.trim().to_string()
        } else if !stdout.is_empty() {
            stdout.trim().to_string()
        } else {
            format!("herdr exited with code {}", output.status)
        };
        Err(user_err(msg))
    }
}

/// The pane whose label is exactly `label`. `Err` is a human-readable
/// diagnostic on no match (listing the labels there are) or ambiguity.
fn resolve_label_in_panes(
    panes: &[herdr_mcp_api::HerdrPane],
    label: &str,
) -> Result<String, String> {
    let labels: Vec<&str> = panes.iter().filter_map(|p| p.label.as_deref()).collect();
    let matched: Vec<&str> = panes
        .iter()
        .filter(|p| p.label.as_deref() == Some(label))
        .map(|p| p.pane_id.as_str())
        .collect();

    match matched.len() {
        0 => {
            let available = if labels.is_empty() {
                "no labeled panes found".to_string()
            } else {
                format!("available labels: {}", labels.join(", "))
            };
            Err(format!("No pane found with label '{label}'. {available}"))
        }
        1 => Ok(matched[0].to_string()),
        _ => Err(format!(
            "Multiple panes found with label '{label}'. Use pane_id instead."
        )),
    }
}

/// Resolve a pane_id from an optional pane_id or label (looked up in
/// this server's session).
async fn resolve_pane_id(
    pane_id: Option<String>,
    label: Option<String>,
) -> Result<String, McpError> {
    resolve_pane_id_in(None, pane_id, label).await
}

/// [`resolve_pane_id`] with the label looked up in `session`.
async fn resolve_pane_id_in(
    session: Option<&str>,
    pane_id: Option<String>,
    label: Option<String>,
) -> Result<String, McpError> {
    match (pane_id, label) {
        (Some(pid), _) => Ok(pid),
        (None, Some(lbl)) => {
            let filter = herdr_mcp_api::PaneFilter {
                session: session.map(str::to_string),
                ..Default::default()
            };
            let panes = crate::panes::panes(&filter).await?;
            resolve_label_in_panes(&panes, &lbl).map_err(user_err)
        }
        (None, None) => Err(user_err("Either pane_id or label is required")),
    }
}

// ── HTTP Bridge ───────────────────────────────────────────────────────

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
};
use herdr_mcp_api::{
    AgentsResponse, AutoVariablesResponse, ConfigResponse, CreateRecipeRequest, ResolveRequest,
    ResolveResponse, SaveVariableRequest, ToolsResponse, UpdateRecipeRequest,
};
use regex::Regex;
use tower_http::services::{ServeDir, ServeFile};

/// Shared state for the Axum HTTP router.
#[derive(Clone)]
struct AppState {
    server: HerdrMcpServer,
}

/// The host of a `host[:port]` authority, lowercased: a bracketed IPv6
/// literal without its brackets, `None` if the brackets don't hold one.
fn authority_host(authority: &str) -> Option<String> {
    match authority.strip_prefix('[') {
        Some(v6) => v6
            .split(']')
            .next()
            .filter(|ip| ip.parse::<std::net::Ipv6Addr>().is_ok())
            .map(str::to_ascii_lowercase),
        None => Some(
            authority
                .rsplit_once(':')
                .map_or(authority, |(name, _port)| name)
                .to_ascii_lowercase(),
        ),
    }
}

fn is_localhost(name: &str) -> bool {
    name == "localhost" || name.ends_with(".localhost")
}

/// Whether a request's `Host` may reach the bridge: `localhost`,
/// `*.localhost`, an IP literal, or the name the bridge is bound to. Any
/// other name is how a DNS-rebinding page would reach this port from a
/// browser, and the bridge can type into panes.
fn host_allowed(host: &str, bind: &str) -> bool {
    authority_host(host).is_some_and(|name| {
        is_localhost(&name)
            || name.parse::<std::net::IpAddr>().is_ok()
            || name == bind.to_ascii_lowercase()
    })
}

/// Whether a browser `Origin` may call the bridge: an http(s) page on
/// `localhost`, `*.localhost`, a loopback IP, or the bind name. Browsers
/// send `Origin` on every cross-origin POST, `no-cors` ones included; a
/// body-less POST needs no preflight, so without this any website could
/// call a tool (`create_workspace` with `{}`). `null` (sandboxed frames,
/// `file:`) is refused. Requests without `Origin` (MCP clients, the TUI,
/// curl) never reach this check.
fn origin_allowed(origin: &str, bind: &str) -> bool {
    let Some(authority) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    authority_host(authority).is_some_and(|name| {
        is_localhost(&name)
            || name
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
            || name == bind.to_ascii_lowercase()
    })
}

/// Build the Axum router: `/api/*` plus the Dioxus web UI — from `web_dir`
/// when set, else the copy built into the binary ([`crate::web`]). Unknown
/// paths fall back to `index.html` so client-side routes resolve. Requests
/// with a foreign `Host` ([`host_allowed`]) or `Origin` ([`origin_allowed`])
/// are refused.
fn http_router(server: HerdrMcpServer, web_dir: Option<&std::path::Path>, bind: &str) -> Router {
    let ui: axum::routing::MethodRouter = match web_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            if !index.is_file() {
                tracing::warn!(
                    "web UI: {} missing; /api/* still available",
                    index.display()
                );
            }
            tracing::info!("serving web UI from {}", dir.display());
            axum::routing::get_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
        }
        None => {
            if !crate::web::embedded() {
                tracing::warn!(
                    "web UI not built into this binary: run `dx bundle --platform web \
                     --release` in crates/herdr-mcp-ui before `cargo build`, or set \
                     http.web-dir. /api/* still available."
                );
            }
            get(crate::web::serve)
        }
    };
    let token = server.security.token.clone();
    let state = AppState { server };

    Router::new()
        .route("/api/health", get(health_handler))
        .route("/api/config", get(config_handler))
        .route("/api/config/clipboard", put(set_clipboard_config_handler))
        .route(
            "/api/settings",
            get(settings_handler).delete(reset_settings_handler),
        )
        .route(
            "/api/settings/{key}",
            put(save_setting_handler).delete(reset_setting_handler),
        )
        .route("/api/tools", get(list_tools_handler))
        .route("/api/tools/{name}", post(call_tool_handler))
        .route("/api/agents", get(agents_handler))
        .route("/api/workspaces", get(workspaces_handler))
        .route(
            "/api/recipes",
            get(list_recipes_handler).post(create_recipe_handler),
        )
        .route(
            "/api/recipes/{id}",
            get(get_recipe_handler)
                .put(update_recipe_handler)
                .delete(delete_recipe_handler),
        )
        .route("/api/herdr/schema", get(herdr_schema_handler))
        .route("/api/herdr/sessions", get(herdr_sessions_handler))
        .route("/api/herdr/panes", get(herdr_panes_handler))
        .route("/api/herdr/events", get(herdr_events_handler))
        .route("/api/templates", get(list_templates_handler))
        .route(
            "/api/templates/{id}/instantiate",
            post(instantiate_template_handler),
        )
        .route("/api/runs", get(list_runs_handler).post(start_run_handler))
        .route("/api/runs/{id}", get(get_run_handler))
        .route("/api/runs/{id}/events", get(run_events_handler))
        .route("/api/runs/{id}/trail", get(run_trail_handler))
        .route("/api/runs/{id}/{action}", post(control_run_handler))
        .route("/api/events", get(events_handler))
        .route("/api/profiles", get(list_profiles_handler))
        .route(
            "/api/profiles/{id}",
            get(get_profile_handler)
                .put(save_profile_handler)
                .delete(delete_profile_handler),
        )
        .route(
            "/api/variables",
            get(list_variables_handler).post(save_variable_handler),
        )
        .route("/api/variables/auto", get(auto_variables_handler))
        .route("/api/variables/resolve", post(resolve_handler))
        .route(
            "/api/variables/{key}",
            get(get_variable_handler).delete(delete_variable_handler),
        )
        .fallback_service(ui)
        .with_state(state)
        .layer(axum::middleware::from_fn({
            let bind = bind.to_string();
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let host = req
                    .headers()
                    .get(axum::http::header::HOST)
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let refused = if !host_allowed(&host, &bind) {
                    Some((StatusCode::FORBIDDEN, format!("host {host:?} not allowed")))
                } else if let Some(o) = req
                    .headers()
                    .get(axum::http::header::ORIGIN)
                    .map(|o| String::from_utf8_lossy(o.as_bytes()).into_owned())
                    .filter(|o| !origin_allowed(o, &bind))
                {
                    Some((StatusCode::FORBIDDEN, format!("origin {o:?} not allowed")))
                } else {
                    token.as_deref().and_then(|t| token_refusal(&req, t))
                };
                async move {
                    match refused {
                        None => next.run(req).await,
                        Some(refusal) => axum::response::IntoResponse::into_response(refusal),
                    }
                }
            }
        }))
}

/// Why `req` may not reach `/api/*` without the bridge token, if it may
/// not: `/api/health` (the liveness probe) and the web UI's own files are
/// open; everything else needs `Authorization: Bearer <token>` or, for
/// `EventSource` streams that can't set headers, `?token=`.
fn token_refusal(req: &axum::extract::Request, token: &str) -> Option<(StatusCode, String)> {
    let path = req.uri().path();
    if !path.starts_with("/api/") || path == "/api/health" {
        return None;
    }
    let bearer = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    let query = req.uri().query().and_then(|q| {
        reqwest::Url::parse(&format!("http://bridge/?{q}"))
            .ok()?
            .query_pairs()
            .find(|(k, _)| k == "token")
            .map(|(_, v)| v.into_owned())
    });
    let given = bearer.map(str::to_string).or(query);
    if given.is_some_and(|g| crate::security::token_matches(&g, token)) {
        return None;
    }
    Some((
        StatusCode::UNAUTHORIZED,
        format!(
            "this bridge needs its token ([security]): send Authorization: Bearer <token>; \
             it is in the data dir's {}",
            herdr_mcp_api::TOKEN_FILE
        ),
    ))
}

/// Start the Axum HTTP server for the web playground. Bind lives until the
/// returned task is aborted or the process exits. The route table is built by
/// [`http_router`]; this is the thin bind + serve wrapper.
pub async fn start_http(
    server: HerdrMcpServer,
    bind: &str,
    port: u16,
    web_dir: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let app = http_router(server, web_dir, bind);

    let listener = tokio::net::TcpListener::bind((bind, port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health_handler() -> &'static str {
    "ok"
}

fn config_response(server: &HerdrMcpServer) -> ConfigResponse {
    ConfigResponse {
        data_dir: server.persistence.data_dir().display().to_string(),
        herdr_socket: server
            .herdr_client
            .get()
            .map(|c| c.socket_path().display().to_string())
            .unwrap_or_default(),
        herdr_bin: herdr_mcp_core::herdr_bin(),
        clipboard: server.clipboard_config(),
        config_path: server.config_path.display().to_string(),
    }
}

fn settings_response(server: &HerdrMcpServer) -> herdr_mcp_api::SettingsResponse {
    herdr_mcp_api::SettingsResponse::new(
        &server.settings(),
        server.config_path.display().to_string(),
        server.settings_warnings.to_vec(),
    )
}

/// `GET /api/settings` — every tunable with its default, bounds and value.
async fn settings_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Json<herdr_mcp_api::SettingsResponse> {
    Json(settings_response(&server))
}

/// `PUT /api/settings/{key}` — validate, write to the config file, apply.
/// 400 (nothing written) for a value out of range, a chord that types text,
/// is reserved or already bound, or an unknown key.
async fn save_setting_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(key): Path<String>,
    Json(value): Json<settings::Value>,
) -> Result<Json<herdr_mcp_api::SettingsResponse>, (StatusCode, String)> {
    write_setting(&server, key, Some(value)).await
}

/// `DELETE /api/settings/{key}` — back to the built-in default.
async fn reset_setting_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<herdr_mcp_api::SettingsResponse>, (StatusCode, String)> {
    write_setting(&server, key, None).await
}

async fn write_setting(
    server: &HerdrMcpServer,
    key: String,
    value: Option<settings::Value>,
) -> Result<Json<herdr_mcp_api::SettingsResponse>, (StatusCode, String)> {
    server
        .save_setting(key, value)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?
        .map_err(bad_request)?;
    Ok(Json(settings_response(server)))
}

/// `DELETE /api/settings` — every setting back to its default (the
/// known-good configuration).
async fn reset_settings_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Result<Json<herdr_mcp_api::SettingsResponse>, (StatusCode, String)> {
    server
        .reset_settings()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    Ok(Json(settings_response(&server)))
}

async fn config_handler(State(AppState { server, .. }): State<AppState>) -> Json<ConfigResponse> {
    Json(config_response(&server))
}

/// `PUT /api/config/clipboard` — persist clipboard commands and apply them
/// live. Both empty switches back to the built-in OS clipboard.
async fn set_clipboard_config_handler(
    State(AppState { server, .. }): State<AppState>,
    Json(cfg): Json<herdr_mcp_core::ClipboardConfig>,
) -> Result<Json<ConfigResponse>, (StatusCode, String)> {
    let blank = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
    let commands = match cfg.commands() {
        Some((copy, paste)) => Some((copy.to_string(), paste.to_string())),
        None if blank(&cfg.copy_command) && blank(&cfg.paste_command) => None,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                "set both copy-command and paste-command, or neither for the built-in clipboard"
                    .into(),
            ));
        }
    };
    server
        .save_clipboard_config(commands)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    Ok(Json(config_response(&server)))
}

async fn list_tools_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Json<ToolsResponse> {
    let tools = server
        .enabled_tools()
        .iter()
        .filter_map(|t| serde_json::to_value(t).ok())
        .collect();
    Json(ToolsResponse { tools })
}

async fn call_tool_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<serde_json::Value>>,
) -> Result<Json<CallToolResult>, (StatusCode, String)> {
    let body = body.map(|j| j.0).unwrap_or(serde_json::json!({}));
    dispatch_tool(&server, &name, body, Origin::Api)
        .await
        .map(Json)
}

/// `GET /api/agents?workspace_id=w1` — canonical pane list from the live
/// `AgentRegistry`. This is the single source of truth the TUI dashboard reads;
/// `list_panes` (MCP tool) shells out to herdr and is reserved as a diagnostic
/// fallback only. Empty `workspace_id` returns the global snapshot.
async fn agents_handler(
    State(AppState { server, .. }): State<AppState>,
    Query(params): Query<WorkspaceQuery>,
) -> Json<AgentsResponse> {
    let agents = match &params.workspace_id {
        Some(ws) if !ws.is_empty() => server.registry.list_for_ws(ws).await,
        _ => server.registry.inner_snapshot().await,
    };
    Json(AgentsResponse { agents })
}

/// `GET /api/workspaces` — passthrough to `herdr workspace list` for the
/// sidecar header. Kept distinct from the registry so the TUI can show
/// workspaces even when no agents are registered yet.
async fn workspaces_handler() -> Json<serde_json::Value> {
    let raw = herdr_cli(&["workspace", "list"])
        .await
        .map(|s| serde_json::from_str::<serde_json::Value>(&s).unwrap_or(serde_json::json!(null)))
        .unwrap_or(serde_json::json!(null));
    Json(raw)
}

fn mcp_err_to_http(e: McpError) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.message.to_string())
}

fn bad_request(e: impl ToString) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}

/// Call a tool by name through rmcp's own router, over the server's
/// in-process MCP session (see [`HerdrMcpServer::tool_session`]) — the HTTP
/// bridge and the run engine take exactly the path an MCP client does.
pub(crate) async fn dispatch_tool(
    server: &HerdrMcpServer,
    name: &str,
    body: serde_json::Value,
    origin: Origin,
) -> Result<CallToolResult, (StatusCode, String)> {
    use rmcp::{ServiceExt, model::ErrorCode, service::ServiceError};
    if !HerdrMcpServer::tool_router().has_route(name) {
        return Err((StatusCode::NOT_FOUND, format!("Unknown tool: {name}")));
    }
    let arguments = match body {
        serde_json::Value::Object(map) => Some(map),
        serde_json::Value::Null => None,
        other => {
            return Err(bad_request(format!(
                "tool arguments must be an object, got {other}"
            )));
        }
    };
    let session = server
        .tool_session
        .get_or_try_init(|| async {
            let (ours, theirs) = tokio::io::duplex(1 << 20);
            let mut this = server.clone();
            this.in_process = true;
            tokio::spawn(async move {
                match this.serve(theirs).await {
                    Ok(running) => {
                        let _ = running.waiting().await;
                    }
                    Err(e) => tracing::error!("in-process MCP server: {e}"),
                }
            });
            ().serve(ours).await
        })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("in-process MCP session: {e}"),
            )
        })?;
    let mut params = rmcp::model::CallToolRequestParams::new(name.to_string());
    params.arguments = arguments;
    let mut meta = rmcp::model::Meta::new();
    meta.0
        .insert(Origin::META_KEY.into(), origin.as_str().into());
    if let Ok(run) = crate::runs::STEP_OF.try_with(|id| *id) {
        meta.0
            .insert(crate::runs::STEP_OF_KEY.into(), run.to_string().into());
    }
    params.meta = Some(meta);
    session.call_tool(params).await.map_err(|e| match e {
        ServiceError::McpError(e) if e.code == ErrorCode::INVALID_PARAMS => bad_request(e.message),
        ServiceError::McpError(e) if e.code == FORBIDDEN => {
            (StatusCode::FORBIDDEN, e.message.into())
        }
        ServiceError::McpError(e) => mcp_err_to_http(e),
        e => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    })
}

// ── Recipe Engine ──────────────────────────────────────────────────────

/// Shared recipe execution core. Seeds `accumulated` from the live agent
/// registry for `session_id` so `{{pane_id.output}}` and `{{role.output}}`
/// resolve to another agent's captured work product.
fn start_error_to_http(e: crate::runs::StartError) -> (StatusCode, String) {
    let code = match e {
        crate::runs::StartError::RecipeNotFound => StatusCode::NOT_FOUND,
        crate::runs::StartError::UnknownStep(_) => StatusCode::BAD_REQUEST,
        crate::runs::StartError::Persist(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, e.to_string())
}

/// A `{{ path }}` placeholder.
static VARIABLE_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"\{\{([^}]+)\}\}").expect("a valid regex literal"));

pub(crate) fn resolve_variables(
    value: &mut serde_json::Value,
    results: &HashMap<String, serde_json::Value>,
) {
    match value {
        serde_json::Value::String(s) => {
            *s = VARIABLE_RE
                .replace_all(s, |caps: &regex::Captures| {
                    let path = caps[1].trim();
                    match resolve_json_path(results, path) {
                        Some(v) => json_value_to_string(&v),
                        None => caps[0].to_string(),
                    }
                })
                .to_string();
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                resolve_variables(v, results);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                resolve_variables(v, results);
            }
        }
        _ => {}
    }
}

/// The `{{ }}` placeholders left in `value` after resolving: ones whose
/// path nothing in scope matched.
pub(crate) fn unresolved_placeholders(value: &serde_json::Value) -> Vec<String> {
    let mut found = Vec::new();
    let mut visit = |s: &str| {
        for m in VARIABLE_RE.find_iter(s) {
            if !found.iter().any(|f| f == m.as_str()) {
                found.push(m.as_str().to_string());
            }
        }
    };
    fn walk(v: &serde_json::Value, visit: &mut impl FnMut(&str)) {
        match v {
            serde_json::Value::String(s) => visit(s),
            serde_json::Value::Array(a) => a.iter().for_each(|v| walk(v, visit)),
            serde_json::Value::Object(m) => m.values().for_each(|v| walk(v, visit)),
            _ => {}
        }
    }
    walk(value, &mut visit);
    found
}

/// Resolve a `{{ }}` path against the run scope:
///
/// - a scope key as-is (`template.session_id`, `result.pane_id`);
/// - `step` alone: the text the step's tool returned;
/// - `step.text`: the same, explicitly;
/// - `step.a.b[0].c`: navigated in the step's result, falling back to the
///   JSON the tool returned as text (`{{exec.output}}`, `{{spawn.pane_id}}`).
pub(crate) fn resolve_json_path(
    root: &HashMap<String, serde_json::Value>,
    path: &str,
) -> Option<serde_json::Value> {
    if let Some(v) = root.get(path) {
        return Some(step_text(v));
    }
    let (step_id, rest) = path.split_once('.')?;
    let step = root.get(step_id)?;
    navigate(step, rest)
        .or_else(|| navigate(&herdr_mcp_api::tool_payload(step)?, rest))
        .or_else(|| (rest == "text").then(|| step_text(step)))
}

/// A tool result as its text content; any other value as-is.
pub(crate) fn step_text(v: &serde_json::Value) -> serde_json::Value {
    if v.get("content").is_some() {
        serde_json::Value::String(herdr_mcp_api::tool_text(v))
    } else {
        v.clone()
    }
}

/// Follow `a.b[0].c` into `value`.
pub(crate) fn navigate(value: &serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = match segment.split_once('[') {
            Some((key, idx)) => {
                let index: usize = idx.strip_suffix(']')?.parse().ok()?;
                let base = if key.is_empty() {
                    current
                } else {
                    current.get(key)?
                };
                base.get(index)?
            }
            None => current.get(segment)?,
        };
    }
    Some(current.clone())
}

/// Convert a JSON value to a string representation suitable for template substitution.
fn json_value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ── Recipe CRUD API ────────────────────────────────────────────────────

async fn list_templates_handler() -> Result<Json<Vec<tmpl::RecipeTemplate>>, (StatusCode, String)> {
    Ok(Json(tmpl::list_templates()))
}

/// `GET /api/herdr/sessions` — every named herdr session.
async fn herdr_sessions_handler()
-> Result<Json<herdr_mcp_api::SessionsResponse>, (StatusCode, String)> {
    let sessions = crate::panes::sessions().await.map_err(bad_gateway)?;
    Ok(Json(herdr_mcp_api::SessionsResponse { sessions }))
}

/// `GET /api/herdr/panes?session=&workspace_id=&tab_id=&agent_status=&focused=&label=`
/// — herdr's panes, narrowed like `list_panes`.
async fn herdr_panes_handler(
    Query(filter): Query<herdr_mcp_api::PaneFilter>,
) -> Result<Json<herdr_mcp_api::PanesResponse>, (StatusCode, String)> {
    let panes = crate::panes::panes(&filter).await.map_err(bad_gateway)?;
    Ok(Json(herdr_mcp_api::PanesResponse { panes }))
}

fn bad_gateway(e: McpError) -> (StatusCode, String) {
    (StatusCode::BAD_GATEWAY, e.message.to_string())
}

/// `GET /api/herdr/schema` — the installed herdr's API schema
/// (`herdr api schema --json`): every socket method and its params.
async fn herdr_schema_handler() -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let text = herdr_cli(&["api", "schema", "--json"])
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.message.to_string()))?;
    serde_json::from_str(&text)
        .map(Json)
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("herdr api schema: {e}")))
}

/// `POST /api/templates/{id}/instantiate` — fill in a template (not saved).
async fn instantiate_template_handler(
    Path(id): Path<String>,
    Json(req): Json<herdr_mcp_api::InstantiateTemplateRequest>,
) -> Result<Json<Recipe>, (StatusCode, String)> {
    tmpl::instantiate(&id, req.variables, req.name)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, format!("template not found: {id}")))
}

async fn list_recipes_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Result<Json<Vec<Recipe>>, (StatusCode, String)> {
    server
        .persistence
        .list_recipes(None)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn create_recipe_handler(
    State(AppState { server, .. }): State<AppState>,
    Json(req): Json<CreateRecipeRequest>,
) -> Result<Json<Recipe>, (StatusCode, String)> {
    server
        .create_recipe(req)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn get_recipe_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Recipe>, (StatusCode, String)> {
    server
        .persistence
        .load_recipe(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "Recipe not found".into()))
}

async fn update_recipe_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<UpdateRecipeRequest>,
) -> Result<Json<Recipe>, (StatusCode, String)> {
    server
        .update_recipe(id, req)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "Recipe not found".to_string()))
}

async fn delete_recipe_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    server
        .persistence
        .delete_recipe(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .then_some(StatusCode::NO_CONTENT)
        .ok_or((StatusCode::NOT_FOUND, "Recipe not found".into()))
}

#[derive(Debug, Deserialize)]
struct WaitQuery {
    #[serde(default)]
    wait: bool,
}

/// `POST /api/runs` — start a run in the background (202 + first record);
/// with `?wait=true`, wait for it to finish (200 + final record).
async fn start_run_handler(
    State(AppState { server, .. }): State<AppState>,
    Query(q): Query<WaitQuery>,
    Json(req): Json<herdr_mcp_api::StartRunRequest>,
) -> Result<(StatusCode, Json<herdr_mcp_api::RunRecord>), (StatusCode, String)> {
    let (run, done) = crate::runs::start_run(&server, req, herdr_mcp_api::RunOrigin::Api)
        .await
        .map_err(start_error_to_http)?;
    if !q.wait {
        return Ok((StatusCode::ACCEPTED, Json(run)));
    }
    let run = done.await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("run task failed: {e}"),
        )
    })?;
    Ok((StatusCode::OK, Json(run)))
}

#[derive(Debug, Deserialize)]
struct ListRunsQuery {
    limit: Option<usize>,
}

/// `GET /api/runs?limit=` — the newest runs (every one without `limit`).
async fn list_runs_handler(
    State(AppState { server, .. }): State<AppState>,
    Query(q): Query<ListRunsQuery>,
) -> Result<Json<Vec<herdr_mcp_api::RunRecord>>, (StatusCode, String)> {
    crate::runs::list(&server, q.limit)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn load_run(
    server: &HerdrMcpServer,
    id: uuid::Uuid,
) -> Result<herdr_mcp_api::RunRecord, (StatusCode, String)> {
    crate::runs::load(server, id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "Run not found".into()))
}

#[derive(Debug, Deserialize)]
struct SinceQuery {
    #[serde(default)]
    since: u64,
}

/// `GET /api/runs/{id}/events?since=N` — the run's events from `since`.
async fn run_events_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
    Query(q): Query<SinceQuery>,
) -> Result<Json<Vec<herdr_mcp_api::RunEvent>>, (StatusCode, String)> {
    let internal = |e: anyhow::Error| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    if server
        .persistence
        .load_run_header(&id)
        .await
        .map_err(internal)?
        .is_none()
    {
        return Err((StatusCode::NOT_FOUND, "Run not found".into()));
    }
    server
        .persistence
        .load_run_events(&id, q.since)
        .await
        .map(Json)
        .map_err(internal)
}

/// `GET /api/profiles` — every agent profile (bundled + `data/profiles`).
async fn list_profiles_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Json<Vec<herdr_mcp_api::AgentProfile>> {
    Json(crate::profiles::list(server.persistence.data_dir()).await)
}

async fn get_profile_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<herdr_mcp_api::AgentProfile>, (StatusCode, String)> {
    crate::profiles::get(server.persistence.data_dir(), &id)
        .await
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, format!("no profile {id:?}")))
}

/// `PUT /api/profiles/{id}` — create or override a profile (400 if invalid).
async fn save_profile_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<String>,
    Json(profile): Json<herdr_mcp_api::AgentProfile>,
) -> Result<Json<herdr_mcp_api::AgentProfile>, (StatusCode, String)> {
    if profile.id != id {
        return Err((
            StatusCode::BAD_REQUEST,
            "body id must match the path".into(),
        ));
    }
    match crate::profiles::save(server.persistence.data_dir(), &profile).await {
        Ok(()) => Ok(Json(profile)),
        Err(crate::profiles::SaveError::Invalid(problems)) => {
            Err((StatusCode::BAD_REQUEST, problems.join("; ")))
        }
        Err(crate::profiles::SaveError::Io(e)) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        }
    }
}

/// `DELETE /api/profiles/{id}` — remove a user profile (a bundled one with
/// the same id comes back). 404 if there is no user file.
async fn delete_profile_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    match crate::profiles::delete(server.persistence.data_dir(), &id).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err((StatusCode::NOT_FOUND, format!("no user profile {id:?}"))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    run: Option<uuid::Uuid>,
    since: Option<u64>,
}

/// `GET /api/events?run={id}&since={seq}` — Server-Sent Events. With `run`,
/// replays that run's log from `since` (or from after the `Last-Event-ID`
/// header, which browsers send on reconnect) and then follows it live;
/// without `run`, every run's new events. Event ids are run `seq`s.
async fn events_handler(
    State(AppState { server, .. }): State<AppState>,
    Query(q): Query<EventsQuery>,
    headers: axum::http::HeaderMap,
) -> axum::response::sse::Sse<
    impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures_util::StreamExt;
    let resume = || {
        headers
            .get("last-event-id")?
            .to_str()
            .ok()?
            .parse::<u64>()
            .ok()
            .map(|seq| seq + 1)
    };
    let since = q.run.and(q.since.or_else(resume));
    let stream = crate::runs::event_stream(server, q.run, since).map(|event| {
        Ok(Event::default()
            .id(event.seq.to_string())
            .json_data(&event)
            .unwrap_or_else(|_| Event::default().comment("unencodable event")))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `GET /api/herdr/events` — Server-Sent Events: herdr's own event stream
/// (`HerdrEvent` JSON per message): the last 200 the subscriber received,
/// oldest first, then each new one. Without a herdr subscriber attached the
/// stream stays empty.
async fn herdr_events_handler(
    State(AppState { server, .. }): State<AppState>,
) -> axum::response::sse::Sse<
    impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    let feed = server.herdr_client.get().map(|c| c.recent_and_live());
    let stream = async_stream::stream! {
        let Some((recent, mut rx)) = feed else { return };
        for event in recent {
            yield Ok(Event::default()
                .json_data(&event)
                .unwrap_or_else(|_| Event::default().comment("unencodable event")));
        }
        loop {
            match rx.recv().await {
                Ok(event) => yield Ok(Event::default()
                    .json_data(&event)
                    .unwrap_or_else(|_| Event::default().comment("unencodable event"))),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    yield Ok(Event::default().comment(format!("skipped {n} events")));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `GET /api/runs/{id}`.
async fn get_run_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<herdr_mcp_api::RunRecord>, (StatusCode, String)> {
    load_run(&server, id).await.map(Json)
}

/// `GET /api/runs/{id}/trail` — the run's trail as text, the trail of the
/// child run its gate handed off to indented under it.
async fn run_trail_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<String, (StatusCode, String)> {
    crate::runs::trail(&server, id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?
        .ok_or((StatusCode::NOT_FOUND, format!("no run {id}")))
}

/// `POST /api/runs/{id}/{pause|resume|cancel}` — 204; 404 unknown run;
/// 409 if it is no longer executing.
async fn control_run_handler(
    State(AppState { server, .. }): State<AppState>,
    Path((id, action)): Path<(uuid::Uuid, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let action = herdr_mcp_api::RunAction::parse(&action).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("unknown run action {action:?}"),
        )
    })?;
    if server.runs.control(id, action) {
        return Ok(StatusCode::NO_CONTENT);
    }
    let run = load_run(&server, id).await?;
    Err((
        StatusCode::CONFLICT,
        format!("run is {:?}, not executing", run.status),
    ))
}

// ── Variables API ──────────────────────────────────────────────────────

/// Flexible save body: the playground sends `{key, value}` while MCP clients
/// may send a full `VariableStore`. Workspace scoping for herdr (`w1`, …)
/// lives in `SessionVars` via `var_set`; `VariableStore.session_id` stays
/// `Option<Uuid>` and is only populated when the caller passes a real UUID.
async fn list_variables_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Result<Json<Vec<crate::persistence::VariableStore>>, (StatusCode, String)> {
    server
        .persistence
        .load_variables(None, None)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// `GET /api/variables/auto[?workspace_id=w1]` — variables resolved on
/// request, never stored: every live herdr pane under its pane id (from
/// `herdr pane list`), with its agent registry handle's non-empty fields laid
/// over it (and the merged pane also under the handle's role), then the
/// workspace's `var_set` session variables when one is given.
async fn auto_variables_handler(
    State(AppState { server, .. }): State<AppState>,
    Query(params): Query<WorkspaceQuery>,
) -> Json<AutoVariablesResponse> {
    let ws = params.workspace_id.filter(|s| !s.trim().is_empty());
    let filter = herdr_mcp_api::PaneFilter {
        workspace_id: ws.clone(),
        ..Default::default()
    };
    let panes: HashMap<String, serde_json::Value> = match crate::panes::panes(&filter).await {
        Ok(panes) => panes
            .into_iter()
            .filter_map(|p| Some((p.pane_id.clone(), serde_json::to_value(p).ok()?)))
            .collect(),
        Err(e) => {
            tracing::debug!("auto variables: herdr pane list: {}", e.message);
            HashMap::new()
        }
    };
    let mut merged = pane_vars(panes, server.registry.seed(ws.as_deref()).await);
    if let Some(ws) = &ws
        && let Ok(bag) = server.persistence.load_session_vars(ws).await
    {
        merged.extend(bag.variables);
    }
    Json(AutoVariablesResponse {
        workspace_id: ws.unwrap_or_default(),
        variables: merged,
    })
}

/// `POST /api/variables/resolve` — `value` resolved in the scope a run of
/// the same recipe, workspace and variables would start with
/// ([`crate::runs::scope`]), so a preview never disagrees with a run.
async fn resolve_handler(
    State(AppState { server, .. }): State<AppState>,
    Json(req): Json<ResolveRequest>,
) -> Result<Json<ResolveResponse>, (StatusCode, String)> {
    let collection = match req.recipe_id {
        Some(id) => {
            server
                .persistence
                .load_recipe(&id)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .ok_or((StatusCode::NOT_FOUND, "Recipe not found".to_string()))?
                .variables
        }
        None => HashMap::new(),
    };
    let ws = crate::runs::run_workspace(req.session_id, &collection, &req.variables);
    let scope = crate::runs::scope(&server, &ws, collection, req.variables).await;
    let mut value = req.value;
    resolve_variables(&mut value, &scope);
    let mut unresolved = Vec::new();
    placeholders(&value, &mut unresolved);
    unresolved.sort();
    unresolved.dedup();
    Ok(Json(ResolveResponse { value, unresolved }))
}

/// Every `{{ path }}` left in `value`'s strings, trimmed.
fn placeholders(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.extend(
            VARIABLE_RE
                .captures_iter(s)
                .map(|c| c[1].trim().to_string()),
        ),
        serde_json::Value::Object(map) => map.values().for_each(|v| placeholders(v, out)),
        serde_json::Value::Array(arr) => arr.iter().for_each(|v| placeholders(v, out)),
        _ => {}
    }
}

/// Live panes (by pane id) with the registry handles (by pane id and role,
/// from `AgentRegistry::seed`) laid over them, non-empty fields winning.
/// Handles of panes herdr no longer lists are left out: the registry keeps
/// them after a pane closes.
fn pane_vars(
    mut panes: HashMap<String, serde_json::Value>,
    handles: HashMap<String, serde_json::Value>,
) -> HashMap<String, serde_json::Value> {
    let mut merged = HashMap::new();
    for (key, handle) in handles {
        let pane_id = handle["pane_id"].as_str().unwrap_or_default();
        let Some(mut obj) = panes.get(pane_id).cloned() else {
            continue;
        };
        if let (Some(obj), Some(fields)) = (obj.as_object_mut(), handle.as_object()) {
            for (k, v) in fields {
                if v.as_str() != Some("") {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
        merged.insert(key, obj);
    }
    for (pane_id, obj) in panes.drain() {
        merged.entry(pane_id).or_insert(obj);
    }
    merged
}

async fn save_variable_handler(
    State(AppState { server, .. }): State<AppState>,
    Json(req): Json<SaveVariableRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if req.key.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "key must not be empty".into()));
    }
    crate::persistence::file_component(&req.key)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let now = chrono::Utc::now();
    let parse_uuid = |s: Option<String>| s.as_deref().and_then(|v| uuid::Uuid::parse_str(v).ok());
    let var = crate::persistence::VariableStore {
        id: req.id.unwrap_or_else(uuid::Uuid::new_v4),
        session_id: parse_uuid(req.session_id),
        execution_id: parse_uuid(req.execution_id),
        key: req.key,
        value: req.value,
        created_at: now,
        updated_at: now,
    };
    server
        .persistence
        .save_variable(&var)
        .await
        .map(|()| StatusCode::CREATED)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn get_variable_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<crate::persistence::VariableStore>, (StatusCode, String)> {
    let vars = server
        .persistence
        .load_variables(None, None)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    vars.into_iter()
        .find(|v| v.key == key)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "Variable not found".into()))
}

async fn delete_variable_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(key): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    crate::persistence::file_component(&key)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    server
        .persistence
        .delete_variable(&key)
        .await
        .map(|deleted| {
            if deleted {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::NOT_FOUND
            }
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use herdr_mcp_core::RecipeAccess;

    /// Every core tool exists (a rename would silently unlist it), the
    /// defaults list exactly those, and a category or name lists more.
    #[test]
    fn mcp_tools_list_core_by_default() {
        let tools = HerdrMcpServer::tool_router().list_all();
        let listed = |entries: &[&str]| -> Vec<String> {
            tools
                .iter()
                .filter(|t| entries.iter().any(|e| lists(e, t, RecipeAccess::Tools)))
                .map(|t| t.name.to_string())
                .collect()
        };
        let defaults = herdr_mcp_core::McpConfig::default().tools;
        let entries: Vec<&str> = defaults.iter().map(String::as_str).collect();
        let core = listed(&entries);
        assert_eq!(core.len(), CORE_TOOLS.len(), "{core:?}");
        let extra = listed(&["core", "scheduler", "llm_call"]);
        assert_eq!(extra.len(), CORE_TOOLS.len() + 5, "{extra:?}");
        assert_eq!(listed(&["all"]).len(), tools.len());
    }

    /// `[mcp] recipes = "bridge"` drops the recipe tools from `core` only.
    #[test]
    fn bridge_recipes_leave_core() {
        let tools = HerdrMcpServer::tool_router().list_all();
        let listed = |entry: &str| -> Vec<String> {
            tools
                .iter()
                .filter(|t| lists(entry, t, RecipeAccess::Bridge))
                .map(|t| t.name.to_string())
                .collect()
        };
        let core = listed("core");
        assert_eq!(core.len(), CORE_TOOLS.len() - CORE_RECIPE_TOOLS.len());
        assert!(!core.iter().any(|t| t == "recipe_run"), "{core:?}");
        assert_eq!(listed("recipe_run"), ["recipe_run"]);
    }

    #[test]
    fn slim_schema_drops_null_from_optional_types() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "label": { "type": ["string", "null"], "description": "d" },
                "type": { "type": "string" },
                "steps": { "type": ["array", "null"], "items": { "type": ["integer", "null"] } },
            },
            "anyOf": [{ "type": ["null", "boolean"] }],
        });
        slim_schema(schema.as_object_mut().unwrap());
        assert_eq!(
            schema,
            json!({
                "type": "object",
                "properties": {
                    "label": { "type": "string", "description": "d" },
                    "type": { "type": "string" },
                    "steps": { "type": "array", "items": { "type": "integer" } },
                },
                "anyOf": [{ "type": "boolean" }],
            })
        );
    }
    use crate::variables::RecipeStep;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::collections::HashMap;

    fn sample_results() -> HashMap<String, serde_json::Value> {
        let mut m = HashMap::new();
        m.insert(
            "step1".to_string(),
            serde_json::json!({
                "result": { "content": ["alpha", "beta"], "text": "hello" },
                "pane_id": "p1"
            }),
        );
        m.insert(
            "step2".to_string(),
            serde_json::json!({ "output": "world", "n": 42 }),
        );
        m
    }

    #[test]
    fn pane_vars_overlays_handles_on_live_panes() {
        let panes = HashMap::from([
            (
                "w1:p1".to_string(),
                json!({"pane_id": "w1:p1", "label": "builder", "cwd": "/w", "agent": "claude"}),
            ),
            ("w1:p2".to_string(), json!({"pane_id": "w1:p2"})),
        ]);
        let handle = json!({"pane_id": "w1:p1", "role": "builder", "agent": "", "output": "ok"});
        let closed = json!({"pane_id": "w1:p9", "role": "gone", "tab_id": "w1:t9"});
        let handles = HashMap::from([
            ("w1:p1".to_string(), handle.clone()),
            ("builder".to_string(), handle),
            ("w1:p9".to_string(), closed.clone()),
            ("gone".to_string(), closed),
        ]);
        let vars = pane_vars(panes, handles);
        let want = json!({
            "pane_id": "w1:p1", "label": "builder", "cwd": "/w", "agent": "claude",
            "role": "builder", "output": "ok"
        });
        assert_eq!(vars["w1:p1"], want);
        assert_eq!(vars["builder"], want);
        assert_eq!(vars["w1:p2"], json!({"pane_id": "w1:p2"}));
        assert_eq!(vars.len(), 3);
    }

    #[test]
    fn test_resolve_json_path_top_level() {
        let root = sample_results();
        let v = resolve_json_path(&root, "step2.output");
        assert_eq!(v, Some(serde_json::json!("world")));
    }

    #[test]
    fn test_resolve_json_path_nested() {
        let root = sample_results();
        let v = resolve_json_path(&root, "step1.result.text");
        assert_eq!(v, Some(serde_json::json!("hello")));
    }

    #[test]
    fn test_resolve_json_path_array_index() {
        let root = sample_results();
        let v = resolve_json_path(&root, "step1.result.content[0]");
        assert_eq!(v, Some(serde_json::json!("alpha")));
    }

    #[test]
    fn test_resolve_json_path_array_nested() {
        let root = sample_results();
        let v = resolve_json_path(&root, "step1.result.content[1]");
        assert_eq!(v, Some(serde_json::json!("beta")));
    }

    #[test]
    fn test_resolve_json_path_missing_returns_none() {
        let root = sample_results();
        assert_eq!(resolve_json_path(&root, "step1.nope"), None);
        assert_eq!(resolve_json_path(&root, "nope.field"), None);
    }

    #[test]
    fn test_resolve_json_path_bare_key_is_the_value() {
        let root = sample_results();
        assert_eq!(
            resolve_json_path(&root, "step2"),
            Some(serde_json::json!({ "output": "world", "n": 42 }))
        );
        assert_eq!(resolve_json_path(&root, "nope"), None);
    }

    /// Tool results read naturally: `{{step}}` / `{{step.text}}` is the
    /// text the tool returned, and fields of JSON returned as text resolve
    /// directly (`{{exec.output}}`). Dotted scope keys resolve as-is.
    #[test]
    fn test_resolve_tool_results_by_text_and_payload() {
        let mut root = HashMap::new();
        root.insert(
            "read".to_string(),
            serde_json::json!({ "content": [{ "type": "text", "text": "line 1\nline 2" }] }),
        );
        root.insert(
            "exec".to_string(),
            serde_json::json!({ "content": [{ "type": "text",
                "text": r#"{"exit_code":0,"output":"ok","pane_id":"w1:p2"}"# }] }),
        );
        root.insert("template.pane".to_string(), serde_json::json!("w1:p3"));
        let get = |p: &str| resolve_json_path(&root, p);
        assert_eq!(get("read"), Some(serde_json::json!("line 1\nline 2")));
        assert_eq!(get("read.text"), get("read"));
        assert_eq!(get("exec.output"), Some(serde_json::json!("ok")));
        assert_eq!(get("exec.exit_code"), Some(serde_json::json!(0)));
        assert_eq!(
            get("exec.content[0].type"),
            Some(serde_json::json!("text")),
            "raw result paths still work"
        );
        assert_eq!(get("template.pane"), Some(serde_json::json!("w1:p3")));
        assert_eq!(get("exec.nope"), None);
    }

    #[test]
    fn test_resolve_variables_simple_substitution() {
        let results = sample_results();
        let mut value = serde_json::json!({ "text": "{{ step2.output }}" });
        resolve_variables(&mut value, &results);
        assert_eq!(value, serde_json::json!({ "text": "world" }));
    }

    #[test]
    fn test_resolve_variables_nested_object() {
        let results = sample_results();
        let mut value = serde_json::json!({ "a": { "b": "{{ step1.result.text }}" } });
        resolve_variables(&mut value, &results);
        assert_eq!(value, serde_json::json!({ "a": { "b": "hello" } }));
    }

    #[test]
    fn test_resolve_variables_array() {
        let results = sample_results();
        let mut value = serde_json::json!(["{{ step2.output }}", "literal"]);
        resolve_variables(&mut value, &results);
        assert_eq!(value, serde_json::json!(["world", "literal"]));
    }

    #[test]
    fn test_resolve_variables_unknown_kept() {
        let results = sample_results();
        let mut value = serde_json::json!({ "text": "{{ unknown.path }}" });
        resolve_variables(&mut value, &results);
        // Unknown variable left unchanged
        assert_eq!(value, serde_json::json!({ "text": "{{ unknown.path }}" }));
    }

    #[test]
    fn test_resolve_variables_whitespace_in_braces() {
        let results = sample_results();
        let mut value = serde_json::json!({ "text": "{{  step2.output  }}" });
        resolve_variables(&mut value, &results);
        assert_eq!(value, serde_json::json!({ "text": "world" }));
    }

    #[test]
    fn test_resolve_variables_multiple_in_string() {
        let results = sample_results();
        let mut value = serde_json::json!({ "text": "{{ step1.result.text }} {{ step2.output }}" });
        resolve_variables(&mut value, &results);
        assert_eq!(value, serde_json::json!({ "text": "hello world" }));
    }

    #[test]
    fn test_resolve_variables_string_value_inserted() {
        let results = sample_results();
        let mut value = serde_json::json!({ "t": "hi {{ step2.output }}" });
        resolve_variables(&mut value, &results);
        assert_eq!(value, serde_json::json!({ "t": "hi world" }));
    }

    #[test]
    fn unresolved_placeholders_lists_each_once() {
        let mut v =
            serde_json::json!({"a": "{{ x }} {{ y.z }}", "b": ["{{ x }}", 1], "c": "{{ ok }}"});
        resolve_variables(
            &mut v,
            &HashMap::from([("ok".to_string(), serde_json::json!("1"))]),
        );
        assert_eq!(unresolved_placeholders(&v), ["{{ x }}", "{{ y.z }}"]);
    }

    #[test]
    fn test_resolve_variables_number_value_inserted() {
        let results = sample_results();
        let mut value = serde_json::json!({ "t": "n={{ step2.n }}" });
        resolve_variables(&mut value, &results);
        assert_eq!(value, serde_json::json!({ "t": "n=42" }));
    }

    #[test]
    fn test_resolve_variables_object_value_to_string() {
        let results = sample_results();
        let mut value = serde_json::json!({ "t": "{{ step1.result }}" });
        resolve_variables(&mut value, &results);
        // Object serializes to its JSON string form
        assert_eq!(
            value,
            serde_json::json!({ "t": "{\"content\":[\"alpha\",\"beta\"],\"text\":\"hello\"}" })
        );
    }

    #[test]
    fn test_resolve_variables_no_substitution() {
        let results = sample_results();
        let mut value = serde_json::json!({ "t": "no placeholders here" });
        resolve_variables(&mut value, &results);
        assert_eq!(value, serde_json::json!({ "t": "no placeholders here" }));
    }

    #[test]
    fn test_json_value_to_string_null() {
        assert_eq!(json_value_to_string(&serde_json::Value::Null), "");
    }

    #[test]
    fn test_json_value_to_string_string() {
        assert_eq!(json_value_to_string(&serde_json::json!("x")), "x");
    }

    #[test]
    fn test_json_value_to_string_number() {
        assert_eq!(json_value_to_string(&serde_json::json!(7)), "7");
    }

    // ── Clipboard passthrough ────────────────────────────────────────────

    /// Build a server with default config (no explicit clipboard commands).
    fn clipboard_test_server() -> HerdrMcpServer {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let pers = Persistence::new(data);
        let pers_arc = std::sync::Arc::new(pers);
        let reg = AgentRegistry::new(pers_arc);
        HerdrMcpServer::new(Persistence::new(tmp.path().join("data2")), reg)
    }

    #[tokio::test]
    async fn agent_spawn_fails_on_unknown_need_before_spawning() {
        let server = clipboard_test_server();
        let args = serde_json::json!({"agent": "shell", "needs": ["ghost"]});
        let (_, err) = dispatch_tool(&server, "agent_spawn", args, Origin::Api)
            .await
            .unwrap_err();
        assert!(err.contains("needs: no agent 'ghost'"), "{err}");
    }

    /// Two saves at once (two rows, or the web UI and the TUI) both land,
    /// live and in the file.
    #[tokio::test]
    async fn concurrent_setting_saves_keep_both() {
        use herdr_mcp_api::settings::{PANE_EXEC_MS, TUI_REFRESH_MS, Value};
        let mut server = clipboard_test_server();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("herdr-mcp.toml");
        server.config_path = path.clone();
        for _ in 0..20 {
            let (a, b) = tokio::join!(
                server.save_setting(PANE_EXEC_MS.into(), Some(Value::Number(5_000))),
                server.save_setting(TUI_REFRESH_MS.into(), Some(Value::Number(1_000))),
            );
            a.unwrap().unwrap();
            b.unwrap().unwrap();
            let live = server.settings();
            assert_eq!(live.number(PANE_EXEC_MS), 5_000);
            assert_eq!(live.number(TUI_REFRESH_MS), 1_000);
            let file: herdr_mcp_core::Config =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(file.settings().0, live);
            server.reset_settings().await.unwrap();
        }
    }

    #[tokio::test]
    async fn save_clipboard_config_writes_file_and_applies_live() {
        let mut server = clipboard_test_server();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("herdr-mcp.toml");
        server.config_path = path.clone();
        std::fs::write(&path, "# keep me\n[http]\nport = 1\n").unwrap();

        server
            .save_clipboard_config(Some(("wl-copy".into(), "wl-paste".into())))
            .await
            .unwrap();

        let live = server.clipboard_config();
        assert_eq!(live.copy_command.as_deref(), Some("wl-copy"));
        assert_eq!(live.paste_command.as_deref(), Some("wl-paste"));
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(file.contains("# keep me") && file.contains("copy-command = \"wl-copy\""));

        // A file that doesn't parse is left alone and the live config is kept.
        std::fs::write(&path, "[http\n").unwrap();
        assert!(
            server
                .save_clipboard_config(Some(("a".into(), "b".into())))
                .await
                .is_err()
        );
        assert_eq!(server.clipboard_config(), live);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[http\n");

        // None clears the override: back to the built-in clipboard.
        std::fs::write(
            &path,
            "[clipboard]\ncopy-command = \"a\"\npaste-command = \"b\"\n",
        )
        .unwrap();
        server.save_clipboard_config(None).await.unwrap();
        assert_eq!(server.clipboard_config().commands(), None);
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("clipboard")
        );
    }

    // The tools run the configured commands (the built-in OS clipboard needs
    // a display, so it is not exercised here).
    #[tokio::test]
    async fn clipboard_tools_use_configured_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let clip = tmp.path().join("clip.txt");
        let mut cfg = herdr_mcp_core::Config::default();
        cfg.clipboard.copy_command = Some(format!("tee {}", clip.display()));
        cfg.clipboard.paste_command = Some(format!("cat {}", clip.display()));
        let reg = AgentRegistry::new(std::sync::Arc::new(Persistence::new(tmp.path().join("d1"))));
        let server =
            HerdrMcpServer::with_config(Persistence::new(tmp.path().join("d2")), reg, &cfg);

        server
            .clipboard_set(Parameters(ClipboardSetParams {
                text: "hello clipboard".into(),
            }))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&clip).unwrap(), "hello clipboard");

        let got = server
            .clipboard_get(Parameters(ClipboardGetParams {}))
            .await
            .unwrap();
        let text = got
            .content
            .iter()
            .find_map(|c| c.as_text())
            .map(|t| t.text.as_str());
        assert_eq!(text, Some("hello clipboard"));
    }

    // ── resolve_label_in_panes tests ──────────────────────────────────

    fn pane(id: &str, label: Option<&str>) -> herdr_mcp_api::HerdrPane {
        serde_json::from_value(serde_json::json!({
            "pane_id": id, "workspace_id": "w1", "tab_id": "w1:t1", "label": label
        }))
        .unwrap()
    }

    #[test]
    fn test_resolve_label_matches_pane_id() {
        let panes = [pane("p0", Some("bar")), pane("p1", Some("foo"))];
        assert_eq!(resolve_label_in_panes(&panes, "foo"), Ok("p1".into()));
    }

    #[test]
    fn test_resolve_label_no_match() {
        let err = resolve_label_in_panes(&[pane("p1", Some("bar"))], "foo").unwrap_err();
        assert!(err.contains("available labels: bar"), "{err}");
    }

    #[test]
    fn test_resolve_label_duplicate() {
        let panes = [pane("p1", Some("foo")), pane("p2", Some("foo"))];
        let err = resolve_label_in_panes(&panes, "foo").unwrap_err();
        assert!(err.contains("Multiple panes"), "{err}");
    }

    #[test]
    fn test_resolve_label_no_labels_at_all() {
        let err = resolve_label_in_panes(&[pane("p1", None)], "foo").unwrap_err();
        assert!(err.contains("no labeled panes found"), "{err}");
        let err = resolve_label_in_panes(&[], "foo").unwrap_err();
        assert!(err.contains("no labeled panes found"), "{err}");
    }

    /// Contract test: the shared `herdr_mcp_api::ApiClient` (used by the web
    /// UI and TUI) round-trips against the real router, so a drift between
    /// server handlers and client types fails here, not in the browser.
    #[test]
    fn tool_is_error_matches_rmcp_wire_format() {
        let err = serde_json::to_value(CallToolResult::error(vec![Content::text("boom")])).unwrap();
        assert!(herdr_mcp_api::tool_is_error(&err));
        assert_eq!(herdr_mcp_api::tool_text(&err), "boom");
        let ok = serde_json::to_value(CallToolResult::success(vec![Content::text("x")])).unwrap();
        assert!(!herdr_mcp_api::tool_is_error(&ok));
    }

    #[test]
    fn host_allowlist_blocks_rebinding_names() {
        for ok in [
            "localhost:7676",
            "LOCALHOST",
            "ui.localhost:1",
            "127.0.0.1:7676",
            "192.168.1.5",
            "[::1]:7676",
            "box:7676",
        ] {
            assert!(host_allowed(ok, "box"), "{ok}");
        }
        for bad in [
            "evil.example:7676",
            "localhost.evil.example",
            "",
            "[evil]:1",
        ] {
            assert!(!host_allowed(bad, "127.0.0.1"), "{bad}");
        }
    }

    #[test]
    fn origin_allowlist_is_loopback_pages_only() {
        for ok in [
            "http://localhost:7676",
            "http://ui.localhost:8080",
            "https://127.0.0.1",
            "http://127.0.0.2:3000",
            "http://[::1]:7676",
            "http://box:7676",
        ] {
            assert!(origin_allowed(ok, "box"), "{ok}");
        }
        for bad in [
            "https://evil.example",
            "http://localhost.evil.example",
            "http://192.168.1.5",
            "http://[2001:db8::1]",
            "null",
            "file://",
            "",
        ] {
            assert!(!origin_allowed(bad, "box"), "{bad}");
        }
    }

    /// The playgrounds group tools by `_meta.category`; none may go missing.
    #[test]
    fn every_tool_has_a_category() {
        for tool in HerdrMcpServer::tool_router().list_all() {
            let json = serde_json::to_value(&tool).unwrap();
            assert!(
                json.pointer("/_meta/category")
                    .and_then(|c| c.as_str())
                    .is_some(),
                "{} has no _meta.category",
                tool.name
            );
        }
    }

    /// Command lines match herdr 0.9's CLI (checked against `herdr … --help`).
    /// Bundled templates only call tools the server has, with params those
    /// tools declare (catches renamed tools and stray or missing fields).
    /// Checked against the schemas: nothing is executed.
    #[test]
    fn bundled_templates_use_real_tools() {
        let tools: HashMap<String, serde_json::Value> = HerdrMcpServer::tool_router()
            .list_all()
            .into_iter()
            .map(|t| {
                let schema = serde_json::Value::Object((*t.input_schema).clone());
                (t.name.to_string(), schema)
            })
            .collect();
        for t in tmpl::list_templates() {
            for step in &t.steps {
                let at = format!("template {} step {}", t.id, step.id);
                let schema = tools
                    .get(&step.tool)
                    .unwrap_or_else(|| panic!("{at}: no tool {}", step.tool));
                let props = schema["properties"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                let params = step.params.as_object().cloned().unwrap_or_default();
                for key in params.keys() {
                    assert!(props.contains_key(key), "{at}: unknown param {key}");
                }
                for req in schema["required"].as_array().into_iter().flatten() {
                    let req = req.as_str().unwrap_or_default();
                    assert!(params.contains_key(req), "{at}: missing param {req}");
                }
            }
        }
    }

    #[test]
    fn pane_exec_captures_output_and_exit_code() {
        let line = exec_line("cargo test", "abc123");
        assert_eq!(
            line,
            r"cargo test; printf '\n%s_%s:%s\n' HERDR_DONE abc123 $?"
        );
        let done = Regex::new(&exec_done_regex("abc123")).unwrap();
        // The echoed command line must not satisfy the wait.
        assert!(!done.is_match(&format!("$ {line}")));

        let screen = format!(
            "$ old stuff\n$ {line}\nrunning 3 tests\ntest result: ok\n\nHERDR_DONE_abc123:101\n$ "
        );
        assert_eq!(
            exec_result(&screen, "abc123"),
            Some(("running 3 tests\ntest result: ok".into(), 101))
        );
        // No trailing newline from the command; echo scrolled out of view.
        let screen = "partial\nHERDR_DONE_abc123:0\n";
        assert_eq!(exec_result(screen, "abc123"), Some(("partial".into(), 0)));
        // Another run's marker doesn't count.
        assert_eq!(exec_result("HERDR_DONE_zzz:0\n", "abc123"), None);
        assert_eq!(tail("a\nb\nc", 2), "b\nc");
    }

    #[test]
    fn herdr_09_argv() {
        assert_eq!(
            agent_submit_args("w1:p2", "hi"),
            ["agent", "prompt", "w1:p2", "hi"]
        );
        assert_eq!(
            agent_wait_args("rev", "idle", Some(5000)),
            [
                "agent",
                "wait",
                "rev",
                "--until",
                "idle",
                "--timeout",
                "5000"
            ]
        );
        assert_eq!(
            wait_output_args("p", "done", false, None, None),
            ["pane", "wait-output", "p", "--match", "done"]
        );
        assert_eq!(
            wait_output_args("p", "ok|err", true, Some("recent"), Some(10)),
            [
                "pane",
                "wait-output",
                "p",
                "--regex",
                "ok|err",
                "--source",
                "recent",
                "--timeout",
                "10"
            ]
        );
    }

    #[test]
    fn watch_matches_counts_occurrences_and_finds_the_last_line() {
        let text = "say DONE when finished\nworking\nDONE\n";
        assert_eq!(watch_matches(text, "DONE", None), (2, Some("DONE")));
        assert_eq!(watch_matches(text, "nope", None), (0, None));
        // Lines are matched one at a time, so `^`/`$` anchor each line.
        let re = Regex::new(r"^DONE$").unwrap();
        assert_eq!(watch_matches(text, "", Some(&re)), (1, Some("DONE")));
    }

    #[tokio::test]
    async fn security_policy_holds_for_the_bridge_runs_and_mcp_clients() {
        use herdr_mcp_api::{ApiClient, ApiError};

        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let config = herdr_mcp_core::Config {
            data_dir: data_dir.clone(),
            security: herdr_mcp_core::SecurityConfig {
                disabled_tools: vec!["run_command".into()],
                require_token: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let registry = AgentRegistry::new(std::sync::Arc::new(Persistence::new(data_dir.clone())));
        let server =
            HerdrMcpServer::with_config(Persistence::new(data_dir.clone()), registry, &config);
        let token = server
            .security
            .token
            .clone()
            .expect("require-token makes one");
        assert_eq!(
            std::fs::read_to_string(data_dir.join(herdr_mcp_api::TOKEN_FILE)).unwrap(),
            token
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = http_router(server.clone(), None, "127.0.0.1");
        tokio::spawn(async move { axum::serve(listener, app).await });
        let status = |r: Result<Vec<serde_json::Value>, ApiError>| match r {
            Err(ApiError::Status { status, .. }) => status,
            other => panic!("{other:?}"),
        };

        // No token: only the health probe answers.
        let anon = ApiClient::new(&base).unwrap();
        anon.health().await.unwrap();
        assert_eq!(status(anon.list_tools().await), 401);
        let wrong = ApiClient::new(&base)
            .unwrap()
            .with_token(Some("nope".into()));
        assert_eq!(status(wrong.list_tools().await), 401);

        let api = ApiClient::new(&base).unwrap().with_token(Some(token));
        let tools = api.list_tools().await.unwrap();
        assert!(tools.iter().any(|t| t["name"] == "var_set"));
        assert!(!tools.iter().any(|t| t["name"] == "run_command"));
        match api
            .call_tool("run_command", &serde_json::json!({ "command": "true" }))
            .await
        {
            Err(ApiError::Status { status: 403, body }) => {
                assert!(body.contains("disabled-tools"), "{body}");
            }
            other => panic!("{other:?}"),
        }

        // Triggers: the bridge API may add them; a recipe run and an MCP
        // client may not.
        let triggers = serde_json::json!([{ "on": "pane.agent_status_changed" }]);
        let saved = api
            .call_tool(
                "recipe_save",
                &serde_json::json!({ "name": "t", "steps": [], "triggers": triggers }),
            )
            .await
            .unwrap();
        let saved = herdr_mcp_api::tool_payload(&saved).unwrap();
        let run = dispatch_tool(
            &server,
            "recipe_run",
            serde_json::json!({
                "steps": [{
                    "id": "s",
                    "tool": "recipe_save",
                    "params": { "recipe_id": saved["id"], "steps": [] },
                }],
                "wait": true,
            }),
            Origin::Api,
        )
        .await
        .unwrap();
        let run = herdr_mcp_api::tool_payload(&serde_json::to_value(run).unwrap()).unwrap();
        assert_eq!(run["status"], "failed", "{run}");
        assert!(run.to_string().contains("agent-triggers"), "{run}");
        let err = server
            .permit(Origin::Mcp, "schedule_recipe", None)
            .await
            .unwrap_err();
        assert!(err.message.contains("an MCP client"), "{}", err.message);
        assert!(server.permit(Origin::Mcp, "var_set", None).await.is_ok());
    }

    #[tokio::test]
    async fn settings_crud_over_http_with_known_good_fallback() {
        use herdr_mcp_api::settings::{PANE_EXEC_MS, Value};
        use herdr_mcp_api::{ApiClient, ApiError};

        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let registry = AgentRegistry::new(std::sync::Arc::new(Persistence::new(data_dir.clone())));
        let mut server = HerdrMcpServer::new(Persistence::new(data_dir), registry);
        let cfg = tmp.path().join("herdr-mcp.toml");
        std::fs::write(&cfg, "# keep me\n").unwrap();
        server.config_path = cfg.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = http_router(server.clone(), None, "127.0.0.1");
        tokio::spawn(async move { axum::serve(listener, app).await });
        let api = ApiClient::new(&format!("http://{addr}")).unwrap();
        let info = |r: &herdr_mcp_api::SettingsResponse, key: &str| {
            r.settings.iter().find(|s| s.key == key).cloned().unwrap()
        };

        // Read: every spec, all on defaults.
        let all = api.settings().await.unwrap();
        assert_eq!(all.settings.len(), herdr_mcp_api::settings::SPECS.len());
        assert!(
            all.settings
                .iter()
                .all(|s| !s.overridden && s.value == s.default)
        );

        // Update: applied live and written to the file.
        let r = api
            .set_setting(PANE_EXEC_MS, &Value::Number(9_000))
            .await
            .unwrap();
        assert!(info(&r, PANE_EXEC_MS).overridden);
        assert_eq!(server.settings().number(PANE_EXEC_MS), 9_000);
        let keys = Value::Keys(vec!["Alt+]".into()]);
        let r = api.set_setting("keys.tab-next", &keys).await.unwrap();
        assert_eq!(
            info(&r, "keys.tab-next").value,
            Value::Keys(vec!["alt+]".into()])
        );
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.contains("# keep me\n"), "{text}");
        assert!(text.contains("pane-exec-ms = 9000"), "{text}");

        // Bad values are refused with the reason, and nothing changes.
        let bad = [
            (PANE_EXEC_MS, Value::Number(1)),
            ("keys.tab-prev", Value::Keys(vec!["alt+]".into()])),
            ("keys.quit", Value::Keys(vec!["q".into()])),
            ("timeouts.nope", Value::Number(5_000)),
        ];
        for (key, value) in bad {
            match api.set_setting(key, &value).await {
                Err(ApiError::Status { status: 400, body }) => assert!(!body.is_empty()),
                other => panic!("{key}: {other:?}"),
            }
        }
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), text);

        // Delete one: back to its default; reset all: the known-good config.
        let r = api.reset_setting(PANE_EXEC_MS).await.unwrap();
        assert!(!info(&r, PANE_EXEC_MS).overridden);
        assert_eq!(server.settings().number(PANE_EXEC_MS), 120_000);
        let r = api.reset_settings().await.unwrap();
        assert!(r.settings.iter().all(|s| !s.overridden));
        assert_eq!(std::fs::read_to_string(&cfg).unwrap().trim(), "# keep me");
    }

    #[tokio::test]
    async fn http_contract_roundtrip_with_api_client() {
        use herdr_mcp_api::{ApiClient, ApiError, CreateRecipeRequest};

        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let registry = AgentRegistry::new(std::sync::Arc::new(Persistence::new(data_dir.clone())));
        let server = HerdrMcpServer::new(Persistence::new(data_dir), registry);
        let web_dir = tmp.path().join("web");
        std::fs::create_dir_all(&web_dir).unwrap();
        std::fs::write(web_dir.join("index.html"), "<html>ui</html>").unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = http_router(server, Some(&web_dir), "127.0.0.1");
        tokio::spawn(async move { axum::serve(listener, app).await });
        let api = ApiClient::new(&format!("http://{addr}")).unwrap();

        // A body-less POST is a CORS "simple request": a website can send it
        // without a preflight, so a foreign Origin is refused outright.
        let http = reqwest::Client::new();
        let tool = format!("http://{addr}/api/tools/list_templates");
        let forged = http.post(&tool).header("origin", "https://evil.example");
        assert_eq!(forged.send().await.unwrap().status(), 403);
        let own = http.post(&tool).header("origin", format!("http://{addr}"));
        assert_eq!(own.send().await.unwrap().status(), 200);
        assert_eq!(http.post(&tool).send().await.unwrap().status(), 200);

        // A fake herdr (nextest runs each test in its own process, so the
        // env var stays here). Its pane labels echo the socket it was given.
        let fake = tmp.path().join("herdr");
        std::fs::write(
            &fake,
            r#"#!/bin/sh
case "$1 $2" in
"session list") echo '{"sessions":[{"name":"default","default":true,"running":true,"socket_path":"/s/d.sock","session_dir":"/s/d"},{"name":"work","default":false,"running":true,"socket_path":"/s/w.sock","session_dir":"/s/w"},{"name":"old","default":false,"running":false,"socket_path":"/s/o.sock","session_dir":"/s/o"}]}' ;;
"pane list") echo "{\"result\":{\"panes\":[{\"pane_id\":\"w1:p1\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\",\"focused\":true,\"agent_status\":\"idle\",\"agent\":\"claude\",\"label\":\"$HERDR_SOCKET_PATH\"},{\"pane_id\":\"w1:p2\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t2\",\"focused\":false,\"agent_status\":\"unknown\"}]}}" ;;
*) echo "fake herdr: $*" >&2; exit 1 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        // SAFETY: this test owns its process (nextest); nothing else reads env concurrently.
        unsafe {
            std::env::set_var("HERDR_BIN", &fake);
            std::env::set_var("HERDR_SOCKET_PATH", "/s/d.sock");
        }

        let sessions = api.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 3);
        assert!(sessions[0].default && !sessions[2].running);
        assert_eq!(sessions.iter().filter(|s| s.current).count(), 1);
        assert!(sessions[0].current);
        let all = api.list_panes(&Default::default()).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].label.as_deref(), Some("/s/d.sock"));
        let filter = |v: serde_json::Value| -> herdr_mcp_api::PaneFilter {
            serde_json::from_value(v).unwrap()
        };
        let idle = api
            .list_panes(&filter(
                serde_json::json!({"agent_status": "idle", "focused": true}),
            ))
            .await
            .unwrap();
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0].agent.as_deref(), Some("claude"));
        assert!(
            api.list_panes(&filter(serde_json::json!({"tab_id": "w1:t9"})))
                .await
                .unwrap()
                .is_empty()
        );
        let work = api
            .list_panes(&filter(serde_json::json!({"session": "work"})))
            .await
            .unwrap();
        assert_eq!(work[0].label.as_deref(), Some("/s/w.sock"));
        for bad in ["old", "nope"] {
            assert!(matches!(
                api.list_panes(&filter(serde_json::json!({"session": bad})))
                    .await,
                Err(ApiError::Status { status: 502, .. })
            ));
        }
        // The tool returns the same typed list.
        let tool = api
            .call_tool("list_panes", &serde_json::json!({"label": "CLAUDE"}))
            .await
            .unwrap();
        let listed: herdr_mcp_api::PanesResponse = herdr_mcp_api::tool_payload(&tool)
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap();
        assert_eq!(listed.panes.len(), 1);

        api.health().await.unwrap();
        let cfg = api.config().await.unwrap();
        assert!(cfg.data_dir.ends_with("data"));
        assert!(!cfg.config_path.is_empty());
        // Validation runs before any file write, so this never touches disk.
        let half = herdr_mcp_api::ClipboardConfig {
            copy_command: Some("  ".into()),
            paste_command: Some("xsel -o".into()),
        };
        assert!(matches!(
            api.set_clipboard_config(&half).await,
            Err(ApiError::Status { status: 400, .. })
        ));
        assert!(!api.list_tools().await.unwrap().is_empty());
        assert!(api.list_agents(None).await.unwrap().is_empty());
        assert!(!api.list_templates().await.unwrap().is_empty());
        let req = herdr_mcp_api::InstantiateTemplateRequest {
            variables: HashMap::from([("pane_id".into(), serde_json::json!("w1:p3"))]),
            name: None,
        };
        let filled = api.instantiate_template("git-status", &req).await.unwrap();
        assert!(filled.steps[0].params.to_string().contains("w1:p3"));
        assert!(
            api.list_recipes()
                .await
                .unwrap()
                .iter()
                .all(|r| r.id != filled.id),
            "instantiate does not save"
        );
        assert!(matches!(
            api.instantiate_template("nope", &req).await,
            Err(ApiError::Status { status: 404, .. })
        ));
        assert_eq!(
            api.auto_variables(Some("w9")).await.unwrap().workspace_id,
            "w9"
        );

        // Variables: 201 / 204 empty bodies must decode as Ok(()).
        let req = herdr_mcp_api::SaveVariableRequest {
            key: "greeting".into(),
            value: serde_json::json!("hi"),
            ..Default::default()
        };
        api.save_variable(&req).await.unwrap();
        assert_eq!(api.get_variable("greeting").await.unwrap().value, "hi");
        assert_eq!(api.list_variables().await.unwrap().len(), 1);
        // Resolve: the scope a run starts with; the request's own variables
        // win, globals fill in, the rest is reported.
        let resolved = api
            .resolve(&ResolveRequest {
                variables: HashMap::from([("who".into(), serde_json::json!("you"))]),
                value: serde_json::json!({ "text": "{{ greeting }} {{ who }}", "n": "{{ later.out }}" }),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(resolved.value["text"], "hi you");
        assert_eq!(resolved.unresolved, ["later.out"]);
        api.delete_variable("greeting").await.unwrap();
        // Path traversal through the key is a client error, not a write.
        let evil = herdr_mcp_api::SaveVariableRequest {
            key: "../escaped".into(),
            ..Default::default()
        };
        assert!(matches!(
            api.save_variable(&evil).await,
            Err(ApiError::Status { status: 400, .. })
        ));
        assert!(!tmp.path().join("data").join("escaped.json").exists());
        assert!(matches!(
            api.delete_variable("../escaped").await,
            Err(ApiError::Status { status: 400, .. })
        ));

        // Runs: start in the background, poll to completion, legacy views.
        let step = |id: &str| herdr_mcp_api::RecipeStep {
            id: id.into(),
            tool: "list_templates".into(),
            params: serde_json::json!({}),
            description: None,
            extract: Vec::new(),
        };
        let started = api
            .start_run(&herdr_mcp_api::StartRunRequest {
                steps: Some(vec![step("a"), step("b")]),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut run = started.clone();
        for _ in 0..200 {
            run = api.get_run(started.id).await.unwrap();
            if run.status.is_finished() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(run.status, herdr_mcp_api::RunStatus::Completed);
        assert_eq!(run.results.len(), 2);
        let events = api.run_events(run.id, 0).await.unwrap();
        assert_eq!(events.len(), 6);
        assert!(events.last().unwrap().kind.is_terminal());
        assert_eq!(api.run_events(run.id, 5).await.unwrap().len(), 1);
        assert!(matches!(
            api.run_events(uuid::Uuid::new_v4(), 0).await,
            Err(ApiError::Status { status: 404, .. })
        ));
        // SSE: replay of a finished run arrives as `id: <seq>` + JSON data.
        let mut sse = reqwest::get(format!("http://{addr}/api/events?run={}&since=4", run.id))
            .await
            .unwrap();
        assert_eq!(
            sse.headers()["content-type"].to_str().unwrap(),
            "text/event-stream"
        );
        let mut body = String::new();
        while !body.contains("\"completed\"") {
            let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), sse.chunk())
                .await
                .expect("SSE stalled")
                .unwrap()
                .expect("SSE ended early");
            body.push_str(&String::from_utf8_lossy(&chunk));
        }
        assert!(body.contains("id: 4") && body.contains("id: 5") && !body.contains("id: 3"));
        assert!(
            api.list_runs(10)
                .await
                .unwrap()
                .iter()
                .any(|r| r.id == run.id)
        );
        assert!(matches!(
            api.control_run(run.id, herdr_mcp_api::RunAction::Cancel)
                .await,
            Err(ApiError::Status { status: 409, .. })
        ));
        assert!(matches!(
            api.get_run(uuid::Uuid::new_v4()).await,
            Err(ApiError::Status { status: 404, .. })
        ));
        // Profiles: bundled list, user override, invalid rejected, delete.
        let profiles = api.list_profiles().await.unwrap();
        assert!(
            profiles
                .iter()
                .any(|p| p.id == "zeroclaw" && !p.is_native())
        );
        assert!(profiles.iter().any(|p| p.id == "hermes" && p.is_native()));
        let mut mine = api.get_profile("shell").await.unwrap();
        mine.id = "python".into();
        mine.command = Some("python3".into());
        api.save_profile(&mine).await.unwrap();
        assert_eq!(api.get_profile("python").await.unwrap(), mine);
        mine.kind = Some("claude".into());
        assert!(matches!(
            api.save_profile(&mine).await,
            Err(ApiError::Status { status: 400, .. })
        ));
        api.delete_profile("python").await.unwrap();
        assert!(matches!(
            api.get_profile("python").await,
            Err(ApiError::Status { status: 404, .. })
        ));
        assert!(matches!(
            api.delete_profile("claude").await,
            Err(ApiError::Status { status: 404, .. })
        ));

        // ?wait=true returns the finished run.
        let waited = api
            .run_and_wait(&herdr_mcp_api::StartRunRequest {
                steps: Some(vec![step("only")]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(waited.status, herdr_mcp_api::RunStatus::Completed);
        assert_eq!(waited.results.len(), 1);
        assert!(matches!(
            api.get_variable("greeting").await,
            Err(ApiError::Status { status: 404, .. })
        ));

        // Recipes CRUD.
        let created = api
            .create_recipe(&CreateRecipeRequest {
                name: "r1".into(),
                description: None,
                steps: vec![RecipeStep {
                    id: "s1".into(),
                    tool: "status".into(),
                    params: serde_json::json!({}),
                    description: None,
                    extract: Vec::new(),
                }],
                variables: HashMap::new(),
                category: None,
                triggers: Vec::new(),
                gate: None,
                trail: Default::default(),
            })
            .await
            .unwrap();
        assert_eq!(api.get_recipe(created.id).await.unwrap().name, "r1");
        let trigger = herdr_mcp_api::RecipeTrigger {
            on: "pane.agent_status_changed".into(),
            when: serde_json::json!({"agent_status": "done"})
                .as_object()
                .cloned()
                .unwrap(),
        };
        let updated = api
            .update_recipe(
                created.id,
                &herdr_mcp_api::UpdateRecipeRequest {
                    name: Some("r2".into()),
                    triggers: Some(vec![trigger.clone()]),
                    category: Some("smoke".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.name, "r2");
        assert_eq!(updated.category.as_deref(), Some("smoke"));
        assert_eq!(
            api.get_recipe(created.id).await.unwrap().triggers,
            vec![trigger]
        );
        assert_eq!(api.list_recipes().await.unwrap().len(), 1);
        api.delete_recipe(created.id).await.unwrap();
        assert!(api.list_recipes().await.unwrap().is_empty());

        // Unknown tool → typed 404, and the SPA fallback still serves the UI.
        assert!(matches!(
            api.call_tool("no_such_tool", &serde_json::json!({})).await,
            Err(ApiError::Status { status: 404, .. })
        ));
        let page = reqwest::get(format!("http://{addr}/playground"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(page, "<html>ui</html>");
    }
}

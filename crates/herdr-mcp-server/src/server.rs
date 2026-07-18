use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::herdr_client::AgentRegistry;
use crate::persistence::Persistence;
use crate::scheduler::Scheduler;
use crate::templates as tmpl;
use crate::variables::{ExecutionResult, ExecutionStatus, Recipe, RecipeStep, ScheduledRecipe};
use herdr_mcp_trim::pfc1::CompressionKey;
use herdr_mcp_trim::pipeline;
use herdr_mcp_trim::policy::TrimPolicy;
use herdr_mcp_trim::stats;

// ── Parameter structs ────────────────────────────────────────────────

/// Parameters for listing tabs.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct ListTabsParams {
    /// Filter by workspace ID (e.g., "1").
    pub workspace_id: Option<String>,
}

/// Parameters for listing panes.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct ListPanesParams {
    /// Filter by workspace ID (e.g., "1").
    pub workspace_id: Option<String>,
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
}

/// Parameters for getting details about a specific agent.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct GetAgentParams {
    /// Agent target: terminal ID, agent name, or pane ID.
    pub target: String,
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
    /// Pane ID to split (alternative to label).
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
    /// Direction to split: "right" or "down".
    pub direction: Option<String>,
    /// Working directory for the new pane.
    pub cwd: Option<String>,
    /// Whether to skip focusing the new pane.
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

/// Parameters for starting an agent in a new pane.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct StartAgentParams {
    /// Name of the agent to start.
    pub name: String,
    /// Arguments to pass to the agent (after '--').
    pub args: Vec<String>,
    /// Working directory for the agent.
    pub cwd: Option<String>,
    /// Workspace ID for the agent.
    pub workspace_id: Option<String>,
    /// Tab ID for the agent.
    pub tab_id: Option<String>,
    /// Split direction: "right" or "down".
    pub split: Option<String>,
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
}

/// Parameters for reading text output from an agent.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct ReadAgentParams {
    /// Agent target: terminal ID, agent name, or pane ID.
    pub target: String,
    /// Source: "visible", "recent", or "recent-unwrapped".
    pub source: Option<String>,
    /// Number of lines to read.
    pub lines: Option<u32>,
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

/// Parameters for sending text to an agent.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct SendAgentParams {
    /// Agent target: terminal ID, agent name, or pane ID.
    pub target: String,
    /// Text to send to the agent.
    pub text: String,
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
    /// Source: "visible" or "recent".
    pub source: Option<String>,
    /// Whether to treat match_text as a regex pattern.
    pub use_regex: Option<bool>,
}

/// Parameters for waiting for a pane's agent status.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct WaitPaneAgentStatusParams {
    /// Pane ID to check (alternative to label).
    pub pane_id: Option<String>,
    /// Pane label (alternative to pane_id).
    pub label: Option<String>,
    /// Target status: "idle", "working", "blocked", "done", "unknown".
    pub status: String,
    /// Timeout in milliseconds.
    pub timeout_ms: Option<u32>,
}

/// Parameters for waiting for an agent's status.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
#[serde(deny_unknown_fields)]
pub struct WaitAgentStatusParams {
    /// Agent target: terminal ID, agent name, or pane ID.
    pub target: String,
    /// Target status: "idle", "working", "blocked", "done", "unknown".
    pub status: String,
    /// Timeout in milliseconds.
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
    pub workspace_id: Option<String>,
    pub tab_id: Option<String>,
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
}

/// Parameters for the `agent_message` a2a step (agent-to-agent message).
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct AgentMessageParams {
    /// Target agent: role or pane id.
    pub target: String,
    /// Text to send to the agent's stream.
    pub text: String,
    /// Optional explicit trim stages applied to `text` before sending. When set,
    /// this overrides the target's per-pane policy. Empty/absent → use policy.
    #[serde(default)]
    pub compress: Option<Vec<String>>,
}

/// Parameters for the `agent_read` a2a step.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct AgentReadParams {
    /// Target agent: role or pane id.
    pub target: String,
    pub source: Option<String>,
    pub lines: Option<u32>,
    /// Decompress a PFC1-compressed read before storing it (default true).
    #[serde(default = "default_true_opt")]
    pub decompress: Option<bool>,
}

/// Parameters for the `agent_wait` a2a step.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct AgentWaitParams {
    /// Target agent: role or pane id.
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

/// Parameters for the `compress` trim tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct CompressParams {
    /// Text to compress.
    pub text: String,
    /// Ordered stage list, e.g. `["caveman:full", "pfc1"]`.
    #[serde(default)]
    pub stages: Vec<String>,
    /// Optional workspace id to attribute the trim stats to (surfaced in
    /// `trim_status` / badge). If omitted, stats are not persisted.
    #[serde(default)]
    pub workspace_id: Option<String>,
}

/// Parameters for the `decompress` trim tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct DecompressParams {
    /// Text to decompress (optionally carrying a PFC1 header).
    pub text: String,
}

/// Parameters for the `list_templates` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct ListTemplatesParams {}

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

// ── Folder-key (PFC1) parameters ──────────────────────────────────────

/// Parameters for the `build_folder_key` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct BuildFolderKeyParams {
    /// Folder to scan recursively for text/markdown files.
    pub folder_path: String,
    /// Minimum term frequency to qualify (default 3).
    #[serde(default)]
    pub min_frequency: Option<usize>,
    /// Minimum term length in bytes (default 4).
    #[serde(default)]
    pub min_length: Option<usize>,
    /// Maximum symbols in the produced key (default 85).
    #[serde(default)]
    pub max_terms: Option<usize>,
    /// Also write the key to the central registry.
    #[serde(default)]
    pub persist_central: Option<bool>,
    /// Fold accepted terms into the persistent master key.
    #[serde(default)]
    pub learn_master: Option<bool>,
}

/// Parameters for the `get_folder_key` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct GetFolderKeyParams {
    /// Folder whose key should be loaded (discovered by walking up the tree).
    pub folder_path: String,
}

/// Parameters for the `list_folder_keys` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct ListFolderKeysParams {}

/// Parameters for the `decompress_with_folder_key` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct DecompressWithFolderKeyParams {
    /// Folder whose key should be used to decompress `text`.
    pub folder_path: String,
    /// PFC1-compressed text (header-less, trusted a2a).
    pub text: String,
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

/// Parameters for the `trim_policy_set` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct TrimPolicySetParams {
    /// Target agent: role or pane id.
    pub target: String,
    /// Policy to attach (stages + direction). Use `null` to clear.
    pub policy: Option<TrimPolicy>,
}

/// Parameters for the `trim_policy_get` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct TrimPolicyGetParams {
    /// Target agent: role or pane id.
    pub target: String,
}

/// Parameters for the `trim_eval` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct TrimEvalParams {
    pub text: String,
    #[serde(default)]
    pub stages: Vec<String>,
}

/// Parameters for the `trim_bench` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct TrimBenchParams {
    /// Path to a corpus file to sweep.
    pub corpus: String,
    /// Stage(s), comma-separated (e.g. `"caveman:full,pfc1"`).
    #[serde(default = "default_bench_level")]
    pub level: String,
}

fn default_bench_level() -> String {
    "caveman:full,pfc1".to_string()
}

/// Parameters for the trim aggregate/diagnose tools.
#[derive(Debug, Default, Deserialize, Serialize, schemars::JsonSchema, Clone, PartialEq)]
#[must_use]
pub struct TrimStatusParams {
    /// Workspace id to scope the query. If omitted, aggregates all workspaces.
    #[serde(default)]
    pub workspace_id: Option<String>,
}

/// Per-pane savings breakdown returned by `trim_status`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaneStatus {
    pub gross_saved_bytes: usize,
    pub net_saved_bytes: usize,
    pub messages_trimmed: u64,
}

/// Aggregate trim savings across a workspace (or all workspaces).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrimStatusResponse {
    pub workspace_net_pct: f64,
    pub workspace_savings_pct: f64,
    pub gross_saved_bytes: usize,
    pub net_saved_bytes: usize,
    pub total_input_bytes: usize,
    pub messages_trimmed: u64,
    pub per_pane: HashMap<String, PaneStatus>,
    pub active_policies: HashMap<String, Vec<String>>,
}

/// A single round-trip sample used by `trim_diagnose`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleRoundtrip {
    pub input: String,
    pub compressed: String,
    pub decompressed: String,
    pub matches: bool,
}

/// End-to-end readiness report returned by `trim_diagnose`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnoseReport {
    pub pipeline_roundtrip_ok: bool,
    pub pfc1_memory_valid: bool,
    pub active_policies: usize,
    pub badge_reachable: bool,
    pub sample_roundtrip: Option<SampleRoundtrip>,
}

fn default_true_bool() -> bool {
    true
}
fn default_true_opt() -> Option<bool> {
    Some(true)
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
    /// Data directory, used by the trim layer for persistent PFC1 memory.
    pub data_dir: std::path::PathBuf,
    /// Resolved clipboard backend config (source of truth: `Config`).
    pub clipboard: herdr_mcp_core::ClipboardConfig,
}

#[tool_router]
impl HerdrMcpServer {
    pub fn new(persistence: Persistence, registry: AgentRegistry) -> Self {
        Self::with_config(persistence, registry, &herdr_mcp_core::Config::default())
    }

    /// Build the server, resolving the clipboard backend from `config`.
    pub fn with_config(
        persistence: Persistence,
        registry: AgentRegistry,
        config: &herdr_mcp_core::Config,
    ) -> Self {
        let data_dir = persistence.data_dir().to_path_buf();
        let persistence = std::sync::Arc::new(persistence);
        let scheduler = Scheduler::new(persistence.clone());
        Self {
            persistence,
            scheduler,
            registry,
            data_dir,
            clipboard: config.clipboard.clone(),
        }
    }

    // ── Trim helpers ─────────────────────────────────────────────────

    /// Apply outbound trim to `text` destined for `pane`. Explicit per-call
    /// `compress` stages win; otherwise the target's per-pane policy is used
    /// (default off). Returns the (possibly compressed) wire text.
    async fn apply_outbound_trim(
        &self,
        pane: &str,
        text: &str,
        explicit: Option<&Vec<String>>,
    ) -> String {
        // 1. Explicit per-call stages win.
        if let Some(specs) = explicit
            && !specs.is_empty()
        {
            let stages = match pipeline::parse_stage_specs(specs) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("trim: {e}");
                    return text.to_string();
                }
            };
            let base = self.base_key_for(None).await;
            let r = pipeline::run(text, &stages, &base);
            self.record_trim_result(pane, &r).await;
            return r.output;
        }
        // 2. Fall back to the target's per-pane policy.
        if let Some(policy) = self.registry.get_trim_policy(pane).await
            && policy.is_active()
        {
            let stages = match policy.parse_stages_with(false) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("trim policy: {e}");
                    return text.to_string();
                }
            };
            let ws = self.registry.get(pane).await.map(|h| h.workspace_id);
            let base = self.base_key_for(ws.as_deref()).await;
            let r = pipeline::run(text, &stages, &base);
            if let Some(last) = r.stages.iter().rev().find_map(|s| s.pfc1_key.clone())
                && let Some(ws) = ws
            {
                self.save_pfc1_key(&ws, &last).await;
            }
            self.record_trim_result(pane, &r).await;
            return r.output;
        }
        text.to_string()
    }

    /// Record a pipeline result against the pane's workspace stats and refresh
    /// its badge. Best-effort: stats I/O or badge push failures are logged,
    /// never fatal. Only records when bytes were actually saved.
    async fn record_trim_result(&self, pane: &str, r: &pipeline::PipelineResult) {
        let Some(ws) = self.registry.get(pane).await.map(|h| h.workspace_id) else {
            return;
        };
        if r.input.len().saturating_sub(r.output.len()) == 0 {
            return; // nothing actually saved; don't pollute accounting
        }
        let mut s = stats::load_stats(&self.data_dir, &ws).await;
        s.record_trim(pane, r.input.len(), r.output.len(), r.total_header_bytes);
        if let Err(e) = stats::save_stats(&self.data_dir, &ws, &s).await {
            tracing::warn!("trim: failed to save stats for {ws}: {e}");
        }
        self.push_badge_for_workspace(&ws).await;
    }

    /// Push a savings-% badge to every pane in `ws` that has an active trim
    /// policy. Best-effort: `herdr pane report-metadata` failures are swallowed.
    async fn push_badge_for_workspace(&self, ws: &str) {
        let s = stats::load_stats(&self.data_dir, ws).await;
        let net_pct = s.savings_pct().round() as i64;
        if net_pct <= 0 {
            return;
        }
        let badge = format!("-{net_pct}%");
        for h in self.registry.list_for_ws(ws).await {
            if let Some(ref policy) = h.trim_policy
                && policy.is_active()
            {
                let _ = herdr_cli(&[
                    "pane",
                    "report-metadata",
                    &h.pane_id,
                    "--source",
                    "herdr-mcp",
                    "--custom-status",
                    &badge,
                    "--ttl-ms",
                    "25000",
                ])
                .await;
            }
        }
    }

    /// Push savings-% badges for every workspace that has trim stats on disk.
    /// Drives the periodic poller so badges survive server restarts.
    async fn push_all_badges(&self) {
        let sessions_dir = self.data_dir.join("sessions");
        let mut entries = match tokio::fs::read_dir(&sessions_dir).await {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("trim poller: cannot read sessions dir: {e}");
                return;
            }
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".trim_stats.json") {
                continue;
            }
            let ws = name.trim_end_matches(".trim_stats.json");
            self.push_badge_for_workspace(ws).await;
        }
    }

    /// Build the PFC1 base key: the default key merged with any persisted
    /// steady-state memory (shared across workspaces in the data dir).
    async fn base_key_for(&self, _ws: Option<&str>) -> CompressionKey {
        let runner = herdr_mcp_trim::runner::PipelineRunner::new(&self.data_dir).await;
        runner.base_key().clone()
    }

    /// Persist a PFC1 key as steady-state memory (union merge).
    async fn save_pfc1_key(&self, _ws: &str, key: &CompressionKey) {
        let path = self.data_dir.join(herdr_mcp_trim::runner::MEMORY_FILE);
        herdr_mcp_trim::runner::save_memory(&path, key).await;
    }

    // ── Discovery ──────────────────────────────────────────────────────

    #[tool(description = "Get overall herdr server status, server status, and client status")]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        run_herdr_json(&["status"]).await
    }

    #[tool(description = "List all workspaces in the current session")]
    async fn list_workspaces(&self) -> Result<CallToolResult, McpError> {
        run_herdr_json(&["workspace", "list"]).await
    }

    #[tool(description = "List tabs, optionally filtered by workspace_id (e.g. '1')")]
    async fn list_tabs(
        &self,
        Parameters(ListTabsParams { workspace_id }): Parameters<ListTabsParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut args = vec!["tab", "list"];
        if let Some(ref wid) = workspace_id {
            args.extend(["--workspace", wid]);
        }
        run_herdr_json(&args).await
    }

    #[tool(description = "List panes, optionally filtered by workspace_id (e.g. '1')")]
    async fn list_panes(
        &self,
        Parameters(ListPanesParams { workspace_id }): Parameters<ListPanesParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut args = vec!["pane", "list"];
        if let Some(ref wid) = workspace_id {
            args.extend(["--workspace", wid]);
        }
        run_herdr_json(&args).await
    }

    #[tool(description = "List all detected agents in the session")]
    async fn list_agents(&self) -> Result<CallToolResult, McpError> {
        run_herdr_json(&["agent", "list"]).await
    }

    #[tool(description = "Get details about a specific pane by pane_id or label")]
    async fn get_pane(
        &self,
        Parameters(GetPaneParams { pane_id, label }): Parameters<GetPaneParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        run_herdr_json(&["pane", "get", &pid]).await
    }

    #[tool(
        description = "Get details about a specific agent by target — terminal ID, agent name, or pane ID"
    )]
    async fn get_agent(
        &self,
        Parameters(GetAgentParams { target }): Parameters<GetAgentParams>,
    ) -> Result<CallToolResult, McpError> {
        run_herdr_json(&["agent", "get", &target]).await
    }

    // ── Lifecycle ──────────────────────────────────────────────────────

    #[tool(description = "Create a new workspace, optionally in a directory with a label")]
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

    #[tool(description = "Split a pane right or down. Returns the new pane's info.")]
    async fn split_pane(
        &self,
        Parameters(SplitPaneParams {
            pane_id,
            label,
            direction,
            cwd,
            no_focus,
        }): Parameters<SplitPaneParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        let mut args = vec!["pane", "split", &pid];
        if let Some(ref dir) = direction {
            args.extend(["--direction", dir]);
        }
        if let Some(ref path) = cwd {
            args.extend(["--cwd", path]);
        }
        if no_focus.unwrap_or(false) {
            args.push("--no-focus");
        }
        run_herdr_json(&args).await
    }

    #[tool(description = "Close a pane by pane_id or label")]
    async fn close_pane(
        &self,
        Parameters(ClosePaneParams { pane_id, label }): Parameters<ClosePaneParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        run_herdr_json(&["pane", "close", &pid]).await
    }

    #[tool(
        description = "Start an agent in a new pane. Pass the agent name and any arguments after '--'"
    )]
    async fn start_agent(
        &self,
        Parameters(StartAgentParams {
            name,
            args,
            cwd,
            workspace_id,
            tab_id,
            split,
        }): Parameters<StartAgentParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut cli = vec!["agent", "start"];
        if let Some(ref path) = cwd {
            cli.extend(["--cwd", path]);
        }
        if let Some(ref wid) = workspace_id {
            cli.extend(["--workspace", wid]);
        }
        if let Some(ref tid) = tab_id {
            cli.extend(["--tab", tid]);
        }
        if let Some(ref dir) = split {
            cli.extend(["--split", dir]);
        }
        cli.push("--");
        cli.push(&name);
        for a in &args {
            cli.push(a);
        }
        run_herdr_json(&cli).await
    }

    // ── Read ───────────────────────────────────────────────────────────

    #[tool(
        description = "Read text output from a pane. Source: visible (current screen), recent (scrollback with wrapping), or recent-unwrapped (scrollback without soft wrapping, best for logs)"
    )]
    async fn read_pane(
        &self,
        Parameters(ReadPaneParams {
            pane_id,
            label,
            source,
            lines,
        }): Parameters<ReadPaneParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        let lines_str = lines.map(|n| n.to_string());
        let mut args = vec!["pane", "read", &pid];
        if let Some(ref src) = source {
            args.extend(["--source", src]);
        }
        if let Some(ref n) = lines_str {
            args.extend(["--lines", n]);
        }
        run_herdr_text(&args).await
    }

    #[tool(
        description = "Read text output from an agent. Source: visible (current screen), recent (scrollback with wrapping), or recent-unwrapped (scrollback without soft wrapping, best for logs)"
    )]
    async fn read_agent(
        &self,
        Parameters(ReadAgentParams {
            target,
            source,
            lines,
        }): Parameters<ReadAgentParams>,
    ) -> Result<CallToolResult, McpError> {
        let lines_str = lines.map(|n| n.to_string());
        let mut args = vec!["agent", "read", &target];
        if let Some(ref src) = source {
            args.extend(["--source", src]);
        }
        if let Some(ref n) = lines_str {
            args.extend(["--lines", n]);
        }
        run_herdr_text(&args).await
    }

    // ── Write ──────────────────────────────────────────────────────────

    #[tool(
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
        description = "Send key presses to a pane. Common keys: Enter, Escape, Tab, Backspace, Ctrl+c, Ctrl+d, ArrowUp, ArrowDown"
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

    #[tool(description = "Send text directly to an agent's stream")]
    async fn send_agent(
        &self,
        Parameters(SendAgentParams { target, text }): Parameters<SendAgentParams>,
    ) -> Result<CallToolResult, McpError> {
        run_herdr_json(&["agent", "send", &target, &text]).await
    }

    // ── Synchronize ────────────────────────────────────────────────────

    #[tool(
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
        let timeout_str = timeout_ms.map(|ms| ms.to_string());
        let mut args = vec!["wait", "output", &pid, "--match", &match_text];
        if let Some(ref ms) = timeout_str {
            args.extend(["--timeout", ms]);
        }
        if let Some(ref src) = source {
            args.extend(["--source", src]);
        }
        if use_regex.unwrap_or(false) {
            args.push("--regex");
        }
        run_herdr_json(&args).await
    }

    #[tool(
        description = "Wait for a pane's agent to reach a specific status. Statuses: idle, working, blocked, done, unknown. Blocks until status reached or timeout."
    )]
    async fn wait_pane_agent_status(
        &self,
        Parameters(WaitPaneAgentStatusParams {
            pane_id,
            label,
            status,
            timeout_ms,
        }): Parameters<WaitPaneAgentStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        let timeout_str = timeout_ms.map(|ms| ms.to_string());
        let mut args = vec!["wait", "agent-status", &pid, "--status", &status];
        if let Some(ref ms) = timeout_str {
            args.extend(["--timeout", ms]);
        }
        run_herdr_json(&args).await
    }

    #[tool(
        description = "Wait for an agent (by target) to reach a specific status. Statuses: idle, working, blocked, done, unknown. Blocks until status reached or timeout."
    )]
    async fn wait_agent_status(
        &self,
        Parameters(WaitAgentStatusParams {
            target,
            status,
            timeout_ms,
        }): Parameters<WaitAgentStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let timeout_str = timeout_ms.map(|ms| ms.to_string());
        let mut args = vec!["agent", "wait", &target, "--status", &status];
        if let Some(ref ms) = timeout_str {
            args.extend(["--timeout", ms]);
        }
        run_herdr_json(&args).await
    }

    // ── A2A primitives ─────────────────────────────────────────────────
    //
    // These amplify herdr's model: a pane *is* an agent, and the live registry
    // (fed by herdr's agent_status_changed events) lets one agent's captured
    // output become another agent's input. Usable both as recipe steps
    // (conductor) and as standalone MCP tools (peer-to-peer).

    #[tool(
        description = "Spawn an agent in a new pane and register it under `role` in the live agent registry. Optionally waits for `needs` targets (role or pane id) to be idle first, then captures the agent's work product. Returns {pane_id, role, agent, status, output}."
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
        }): Parameters<AgentSpawnParams>,
    ) -> Result<CallToolResult, McpError> {
        // Wait for dependencies first (reliable: driven by herdr's agent state).
        let ws_for_resolve = workspace_id.clone().unwrap_or_default();
        for dep in &needs {
            let mut pane = self.registry.resolve(&ws_for_resolve, dep).await;
            if pane.is_none() {
                pane = self.registry.resolve("", dep).await;
            }
            if let Some(pane) = pane {
                let _ = self
                    .wait_agent_status(Parameters(WaitAgentStatusParams {
                        target: pane,
                        status: "idle".into(),
                        timeout_ms: Some(300_000),
                    }))
                    .await;
            }
        }

        let mut cli = vec!["agent", "start"];
        if let Some(ref path) = cwd {
            cli.extend(["--cwd", path]);
        }
        if let Some(ref wid) = workspace_id {
            cli.extend(["--workspace", wid]);
        }
        if let Some(ref tid) = tab_id {
            cli.extend(["--tab", tid]);
        }
        if let Some(ref dir) = split {
            cli.extend(["--split", dir]);
        }
        cli.push("--");
        cli.push(&agent);
        for a in &args {
            cli.push(a);
        }
        let raw = herdr_cli(&cli).await?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({ "raw": raw }));
        let pane_id = extract_pane_id(&value).unwrap_or_default();
        let ws = workspace_id
            .clone()
            .or_else(|| extract_string(&value, "workspace_id"))
            .unwrap_or_default();

        if !pane_id.is_empty() {
            self.registry.upsert(&pane_id, &ws, "", &agent, &role).await;
            if let Some(ref lbl) = label {
                let _ = herdr_cli(&["pane", "rename", &pane_id, lbl]).await;
                self.registry.set_label(&pane_id, lbl).await;
            }
        }

        let mut status = "working".to_string();
        let mut output = String::new();
        if wait_idle && !pane_id.is_empty() {
            let _ = self
                .wait_agent_status(Parameters(WaitAgentStatusParams {
                    target: pane_id.clone(),
                    status: "idle".into(),
                    timeout_ms: Some(300_000),
                }))
                .await;
            status = "idle".to_string();
            output = read_pane_text(&pane_id).await.unwrap_or_default();
            self.registry.set_output(&pane_id, output.clone()).await;
        }

        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "pane_id": pane_id,
                "workspace_id": ws,
                "role": role,
                "agent": agent,
                "status": status,
                "output": output,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        description = "Send a message (text) to another agent's stream. `target` may be a role or pane id. The text may interpolate {{role.output}} / {{pane_id.output}} from the session registry. Optional `compress` lists trim stages (e.g. [\"caveman:full\",\"pfc1\"]) applied to the wire bytes; if absent, the target's per-pane trim policy is used (default off)."
    )]
    async fn agent_message(
        &self,
        Parameters(AgentMessageParams {
            target,
            text,
            compress,
        }): Parameters<AgentMessageParams>,
    ) -> Result<CallToolResult, McpError> {
        let pane = resolve_target_pane(self, &target).await?;
        let wire = self
            .apply_outbound_trim(&pane, &text, compress.as_ref())
            .await;
        run_herdr_json(&["agent", "send", &pane, &wire]).await
    }

    #[tool(
        description = "Read an agent's output and store it as its work product in the registry. Returns {pane_id, role, agent, status, output}. `target` may be a role or pane id. `decompress` (default true) reverses any PFC1 header on the read so {{role.output}} stays byte-faithful."
    )]
    async fn agent_read(
        &self,
        Parameters(AgentReadParams {
            target,
            source,
            lines,
            decompress,
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
        let raw = herdr_cli(&args).await?;

        let do_decompress = decompress.unwrap_or(true);
        let stored = if do_decompress {
            // Try the self-describing header; fall back to the shared server key
            // (compact a2a mode sends header-less payloads).
            let shared = self.base_key_for(None).await;
            pipeline::decompress_pfc1(&raw, Some(&shared))
        } else {
            raw.clone()
        };
        self.registry.set_output(&pane, stored.clone()).await;
        let handle = self.registry.get(&pane).await;
        // Refresh the pane's savings badge (read-only; no stats recorded).
        if let Some(ref h) = handle {
            self.push_badge_for_workspace(&h.workspace_id).await;
        }
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
        description = "Wait for an agent to reach a status (default idle). `target` may be a role or pane id."
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
        self.wait_agent_status(Parameters(WaitAgentStatusParams {
            target: pane,
            status,
            timeout_ms,
        }))
        .await
    }

    #[tool(
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

    #[tool(description = "Get a session variable (scoped to a herdr workspace id).")]
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

    // ── Message-trim tools ─────────────────────────────────────────────
    // Two non-colliding compressors (caveman style + pfc1 phonetic) exposed
    // as MCP tools and usable as recipe steps. See `compressorplan.md`.

    #[tool(
        description = "Compress text via an ordered pipeline of trim stages (e.g. [\"caveman:full\",\"pfc1\"]). Returns input/output sizes, per-stage stats, and the compressed payload with its PFC1 header."
    )]
    async fn compress(
        &self,
        Parameters(CompressParams {
            text,
            stages,
            workspace_id,
        }): Parameters<CompressParams>,
    ) -> Result<CallToolResult, McpError> {
        let parsed = pipeline::parse_stage_specs(&stages).map_err(to_mcp_err)?;
        let runner = herdr_mcp_trim::runner::PipelineRunner::new(&self.data_dir).await;
        let r = runner.run(&text, &parsed).await;
        // Optionally attribute this trim to a workspace's running stats.
        if let Some(ref ws) = workspace_id
            && r.input.len().saturating_sub(r.output.len()) > 0
        {
            let mut s = stats::load_stats(&self.data_dir, ws).await;
            s.record_trim("cli", r.input.len(), r.output.len(), r.total_header_bytes);
            let _ = stats::save_stats(&self.data_dir, ws, &s).await;
            self.push_badge_for_workspace(ws).await;
        }
        let stage_reports: Vec<serde_json::Value> = r
            .stages
            .iter()
            .map(|s| {
                serde_json::json!({
                    "stage": s.stage,
                    "output_len": s.output.len(),
                    "skipped": s.skipped,
                    "stats": s.stats,
                })
            })
            .collect();
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "input_bytes": r.input.len(),
                "output_bytes": r.output.len(),
                "total_savings_bytes": r.total_savings_bytes,
                "total_ratio_pct": r.total_ratio,
                "stages": stage_reports,
                "output": r.output,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        description = "Decompress text produced by `compress`. If the input carries a PFC1 header it is expanded; otherwise the input is returned unchanged."
    )]
    async fn decompress(
        &self,
        Parameters(DecompressParams { text }): Parameters<DecompressParams>,
    ) -> Result<CallToolResult, McpError> {
        let out = pipeline::decompress_pfc1(&text, None);
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "decompressed": out,
                "changed": out != text,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        description = "Attach (or clear with `policy: null`) a message-trim policy to a target agent (role or pane id). Policy = ordered stages + direction (none/outbound/outbound_with_ack). Default off."
    )]
    async fn trim_policy_set(
        &self,
        Parameters(TrimPolicySetParams { target, policy }): Parameters<TrimPolicySetParams>,
    ) -> Result<CallToolResult, McpError> {
        let pane = resolve_target_pane(self, &target).await?;
        self.registry.set_trim_policy(&pane, policy.clone()).await;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "pane_id": pane,
                "policy": policy,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        description = "Read the message-trim policy attached to a target agent (role or pane id). Returns null if none."
    )]
    async fn trim_policy_get(
        &self,
        Parameters(TrimPolicyGetParams { target }): Parameters<TrimPolicyGetParams>,
    ) -> Result<CallToolResult, McpError> {
        let pane = resolve_target_pane(self, &target).await?;
        let policy = self.registry.get_trim_policy(&pane).await;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "pane_id": pane,
                "policy": policy,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        description = "Evaluate trim savings for a single text offline. Returns byte + token estimates and per-stage stats — lets the model verify its own compression."
    )]
    async fn trim_eval(
        &self,
        Parameters(TrimEvalParams { text, stages }): Parameters<TrimEvalParams>,
    ) -> Result<CallToolResult, McpError> {
        let runner = herdr_mcp_trim::runner::PipelineRunner::new(&self.data_dir).await;
        let report = herdr_mcp_trim::eval::trim_eval(&text, &stages, runner.base_key());
        Ok(CallToolResult::success(vec![
            Content::json(report).map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        description = "Sweep a corpus file and report a trim savings distribution for a level (comma-separated stages, e.g. \"caveman:full,pfc1\")."
    )]
    async fn trim_bench(
        &self,
        Parameters(TrimBenchParams { corpus, level }): Parameters<TrimBenchParams>,
    ) -> Result<CallToolResult, McpError> {
        let runner = herdr_mcp_trim::runner::PipelineRunner::new(&self.data_dir).await;
        let report = herdr_mcp_trim::eval::trim_bench(&corpus, &level, runner.base_key());
        Ok(CallToolResult::success(vec![
            Content::json(report).map_err(to_mcp_err)?,
        ]))
    }

    /// Aggregate trim savings across a workspace (or all workspaces). Shared by
    /// the `trim_status` tool and the web dashboard.
    async fn aggregate_trim_status(&self, ws: Option<&str>) -> TrimStatusResponse {
        let mut resp = TrimStatusResponse::default();

        // Determine the workspace set to scan.
        let ws_list: Vec<String> = match ws {
            Some(w) => vec![w.to_string()],
            None => {
                let sessions = self.data_dir.join("sessions");
                let mut list = Vec::new();
                if let Ok(mut entries) = tokio::fs::read_dir(&sessions).await {
                    while let Ok(Some(e)) = entries.next_entry().await {
                        let fname = e.file_name().to_string_lossy().to_string();
                        if fname.ends_with(".trim_stats.json") {
                            list.push(fname.trim_end_matches(".trim_stats.json").to_string());
                        }
                    }
                }
                list
            }
        };

        let mut gross = 0usize;
        let mut net = 0usize;
        let mut total_in = 0usize;
        let mut msgs = 0u64;
        for w in &ws_list {
            let s = stats::load_stats(&self.data_dir, w).await;
            gross = gross.saturating_add(s.gross_saved_bytes);
            net = net.saturating_add(s.net_saved_bytes);
            total_in = total_in.saturating_add(s.total_input_bytes);
            msgs = msgs.saturating_add(s.messages_trimmed);
            for (pane, ps) in &s.per_pane {
                let key = format!("{w}:{pane}");
                let entry = resp.per_pane.entry(key).or_default();
                entry.gross_saved_bytes =
                    entry.gross_saved_bytes.saturating_add(ps.gross_saved_bytes);
                entry.net_saved_bytes = entry.net_saved_bytes.saturating_add(ps.net_saved_bytes);
                entry.messages_trimmed = entry.messages_trimmed.saturating_add(ps.messages_trimmed);
            }
            for h in self.registry.list_for_ws(w).await {
                if let Some(ref policy) = h.trim_policy
                    && policy.is_active()
                {
                    resp.active_policies
                        .insert(h.pane_id.clone(), policy.stages.clone());
                }
            }
        }

        resp.gross_saved_bytes = gross;
        resp.net_saved_bytes = net;
        resp.total_input_bytes = total_in;
        resp.messages_trimmed = msgs;
        resp.workspace_net_pct = if gross > 0 {
            (net as f64 / gross as f64) * 100.0
        } else {
            0.0
        };
        resp.workspace_savings_pct = if total_in > 0 {
            (gross as f64 / total_in as f64) * 100.0
        } else {
            0.0
        };
        resp
    }

    #[tool(
        description = "Aggregate trim savings for a workspace (or all). Returns net/savings %, per-pane breakdown, and active policies."
    )]
    async fn trim_status(
        &self,
        Parameters(TrimStatusParams { workspace_id }): Parameters<TrimStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let resp = self.aggregate_trim_status(workspace_id.as_deref()).await;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::to_value(&resp).map_err(to_mcp_err)?).map_err(to_mcp_err)?,
        ]))
    }

    /// Build the end-to-end readiness report (shared by `trim_diagnose` tool
    /// and the `/api/trim/diagnose` HTTP route).
    async fn build_diagnose_report(&self, ws: Option<&str>) -> DiagnoseReport {
        // 1. Round-trip test: compress then decompress a fixed sample.
        let sample =
            "The quick brown fox jumps over the lazy dog. The fox is quick and the dog is lazy.";
        let stages = pipeline::parse_stage_specs(&["caveman:full".to_string(), "pfc1".to_string()])
            .unwrap_or_default();
        let base = self.base_key_for(None).await;
        let r = pipeline::run(sample, &stages, &base);
        let compressed_with_header = r.stages.iter().any(|s| s.pfc1_key.is_some());
        let decompressed = pipeline::decompress_pfc1(&r.output, Some(&base));
        let roundtrip_ok = if compressed_with_header {
            decompressed == sample
        } else {
            true
        };

        // 2. PFC1 memory file validity.
        let mem_path = self.data_dir.join(herdr_mcp_trim::runner::MEMORY_FILE);
        let pfc1_memory_valid = tokio::fs::read_to_string(&mem_path)
            .await
            .ok()
            .and_then(|c| serde_json::from_str::<CompressionKey>(&c).ok())
            .is_some();

        // 3. Active policy count.
        let active_policies = if let Some(w) = ws {
            self.registry
                .list_for_ws(w)
                .await
                .iter()
                .filter(|h| {
                    h.trim_policy
                        .as_ref()
                        .map(|p| p.is_active())
                        .unwrap_or(false)
                })
                .count()
        } else {
            self.registry
                .inner_snapshot()
                .await
                .iter()
                .filter(|h| {
                    h.trim_policy
                        .as_ref()
                        .map(|p| p.is_active())
                        .unwrap_or(false)
                })
                .count()
        };

        // 4. Badge reachability: at least one registered pane exists.
        let badge_reachable = !self.registry.inner_snapshot().await.is_empty();

        DiagnoseReport {
            pipeline_roundtrip_ok: roundtrip_ok,
            pfc1_memory_valid,
            active_policies,
            badge_reachable,
            sample_roundtrip: Some(SampleRoundtrip {
                input: sample.to_string(),
                compressed: r.output,
                decompressed,
                matches: roundtrip_ok,
            }),
        }
    }

    #[tool(
        description = "End-to-end trim readiness check: pipeline round-trip integrity, PFC1 memory validity, active policy count, and badge reachability."
    )]
    async fn trim_diagnose(
        &self,
        Parameters(TrimStatusParams { workspace_id }): Parameters<TrimStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let report = self.build_diagnose_report(workspace_id.as_deref()).await;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::to_value(&report).map_err(to_mcp_err)?)
                .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(description = "Fire a herdr notification summarizing the session's trim savings.")]
    async fn trim_summary(
        &self,
        Parameters(TrimStatusParams { workspace_id }): Parameters<TrimStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let resp = self.aggregate_trim_status(workspace_id.as_deref()).await;
        let body = format!(
            "Session savings: {:.1}% net ({} messages, {} bytes net saved)",
            resp.workspace_savings_pct, resp.messages_trimmed, resp.net_saved_bytes
        );
        let raw = herdr_cli(&["notification", "show", "herdr-mcp trim", "--body", &body]).await?;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "sent": true,
                "body": body,
                "herdr_output": raw,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(description = "Open a herdr split pane running the live trim dashboard.")]
    async fn trim_dashboard_open(
        &self,
        Parameters(TrimStatusParams { workspace_id }): Parameters<TrimStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut cli = vec!["pane", "split"];
        if let Some(ref w) = workspace_id {
            cli.extend(["--workspace", w]);
        }
        let raw = herdr_cli(&cli).await?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}));
        let pane_id = extract_pane_id(&value).unwrap_or_default();
        if pane_id.is_empty() {
            return Err(McpError {
                code: rmcp::model::ErrorCode(-32000),
                message: "trim: failed to open dashboard pane".into(),
                data: None,
            });
        }
        let data_dir = self.data_dir.display().to_string();
        let cmd = format!("herdr-mcp dashboard --data-dir {data_dir}");
        let _ = herdr_cli(&["pane", "send-text", &pane_id, &cmd]).await;
        let _ = herdr_cli(&["pane", "send-keys", &pane_id, "Enter"]).await;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "pane_id": pane_id,
                "title": "trim-dashboard",
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    // ── Recipe templates ───────────────────────────────────────────────

    #[tool(
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

    #[tool(description = "Get a single recipe template by id (variables + steps).")]
    async fn get_template(
        &self,
        Parameters(GetTemplateParams { template_id }): Parameters<GetTemplateParams>,
    ) -> Result<CallToolResult, McpError> {
        match tmpl::find_template(&template_id) {
            Some(t) => Ok(CallToolResult::success(vec![
                Content::json(serde_json::json!(t)).map_err(to_mcp_err)?,
            ])),
            None => Err(McpError {
                code: rmcp::model::ErrorCode(-32602),
                message: format!("template not found: {template_id}").into(),
                data: None,
            }),
        }
    }

    #[tool(
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
        let recipe = tmpl::instantiate(&template_id, variables, name).ok_or_else(|| McpError {
            code: rmcp::model::ErrorCode(-32602),
            message: format!("template not found: {template_id}").into(),
            data: None,
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

    // ── Scheduler ──────────────────────────────────────────────────────

    #[tool(
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

    #[tool(description = "Delete a recipe schedule by id.")]
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

    #[tool(description = "Enable or disable an existing recipe schedule.")]
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

    // ── Folder-key (PFC1) tools ─────────────────────────────────────────

    #[tool(
        description = "Scan a folder for text/markdown files, learn the domain's most compressible terms + phrases, and write a self-contained PFC1 key to <folder>/.pfc1_key.json. Skips code blocks; reuses the cached key when files are unchanged (mtime)."
    )]
    async fn build_folder_key(
        &self,
        Parameters(BuildFolderKeyParams {
            folder_path,
            min_frequency,
            min_length,
            max_terms,
            persist_central,
            learn_master,
        }): Parameters<BuildFolderKeyParams>,
    ) -> Result<CallToolResult, McpError> {
        let root = std::path::Path::new(&folder_path);
        let mut opts = herdr_mcp_trim::folder_key::FolderKeyOptions::default();
        if let Some(v) = min_frequency {
            opts.min_frequency = v;
        }
        if let Some(v) = min_length {
            opts.min_length = v;
        }
        if let Some(v) = max_terms {
            opts.max_terms = v;
        }
        if let Some(v) = persist_central {
            opts.persist_central = v;
        }
        if let Some(v) = learn_master {
            opts.learn_master = v;
        }
        let key = herdr_mcp_trim::folder_key::build_folder_key(root, &opts, &self.data_dir)
            .await
            .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "built": true,
                "folder": key.folder,
                "seeded_from_master": key.seeded_from_master,
                "stats": key.stats,
                "key": key.key,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        description = "Load the PFC1 folder key for a folder (discovered by walking up the directory tree). Returns null if none exists."
    )]
    async fn get_folder_key(
        &self,
        Parameters(GetFolderKeyParams { folder_path }): Parameters<GetFolderKeyParams>,
    ) -> Result<CallToolResult, McpError> {
        let root = std::path::Path::new(&folder_path);
        // Walk up from the folder (and its parents) to find the nearest key.
        let keys = herdr_mcp_trim::folder_key::discover_folder_keys(root).await;
        let key = keys.into_iter().next();
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "folder": folder_path,
                "key": key,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(description = "List all folder keys in the central registry (data_dir/folder_keys).")]
    async fn list_folder_keys(
        &self,
        Parameters(_): Parameters<ListFolderKeysParams>,
    ) -> Result<CallToolResult, McpError> {
        let keys = herdr_mcp_trim::folder_key::list_central_keys(&self.data_dir).await;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "count": keys.len(),
                "keys": keys,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    #[tool(
        description = "Decompress text using a folder's PFC1 key (header-less, trusted a2a). Returns the input unchanged if no key is found."
    )]
    async fn decompress_with_folder_key(
        &self,
        Parameters(DecompressWithFolderKeyParams { folder_path, text }): Parameters<
            DecompressWithFolderKeyParams,
        >,
    ) -> Result<CallToolResult, McpError> {
        let root = std::path::Path::new(&folder_path);
        let out = herdr_mcp_trim::folder_key::decompress_with_folder_key(root, &text).await;
        Ok(CallToolResult::success(vec![
            Content::json(serde_json::json!({
                "decompressed": out,
                "changed": out != text,
            }))
            .map_err(to_mcp_err)?,
        ]))
    }

    // ── Clipboard ───────────────────────────────────────────────────────

    #[tool(
        description = "Write text to the system clipboard. Requires a clipboard utility on the host (pbcopy on macOS, wl-copy on Wayland, xclip/xsel on X11). Text only."
    )]
    async fn clipboard_set(
        &self,
        Parameters(ClipboardSetParams { text }): Parameters<ClipboardSetParams>,
    ) -> Result<CallToolResult, McpError> {
        let (copy_cmd, _paste_cmd) = detect_clipboard(&self.clipboard)?;
        eprintln!(
            "[clipboard] set: running `{copy_cmd}` ({} chars)",
            text.len()
        );
        match shell_with_stdin(copy_cmd, &text).await {
            Ok(out) => {
                eprintln!("[clipboard] set: ok (stdout={out:?})");
                Ok(CallToolResult::success(vec![
                    Content::json(serde_json::json!({
                        "ok": true,
                        "length": text.len(),
                    }))
                    .map_err(to_mcp_err)?,
                ]))
            }
            Err(e) => {
                eprintln!("[clipboard] set: FAILED: {e}");
                Err(e)
            }
        }
    }

    #[tool(
        description = "Read the current text content of the system clipboard. Requires a clipboard utility on the host (pbpaste on macOS, wl-paste on Wayland, xclip/xsel on X11). Returns empty string if the clipboard contains no text. Text only."
    )]
    async fn clipboard_get(
        &self,
        Parameters(_): Parameters<ClipboardGetParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_copy_cmd, paste_cmd) = detect_clipboard(&self.clipboard)?;
        eprintln!("[clipboard] get: running `{paste_cmd}`");
        match shell_output(paste_cmd).await {
            Ok(text) => {
                eprintln!("[clipboard] get: ok ({} chars)", text.len());
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => {
                eprintln!("[clipboard] get: FAILED: {e}");
                Err(e)
            }
        }
    }
}

#[tool_handler(
    name = "herdr-mcp",
    version = "0.1.0",
    instructions = "Control herdr — a terminal-native agent multiplexer. \
                    Manage workspaces, tabs, and panes; spawn agents; \
                    read output; send text, keys, and commands; \
                    wait for output or agent status changes. \
                    All commands shell out to the local 'herdr' CLI \
                    binary which must be installed and in PATH (https://herdr.dev). \
                    IDs are session-local and may compact when items are closed — \
                    re-read IDs from list commands after structural changes."
)]
impl ServerHandler for HerdrMcpServer {}

// ── Helpers ────────────────────────────────────────────────────────────

/// Map any displayable error into an `McpError` for `#[tool]` methods.
fn to_mcp_err(e: impl std::fmt::Display) -> McpError {
    McpError {
        code: rmcp::model::ErrorCode(-32603),
        message: e.to_string().into(),
        data: None,
    }
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
fn extract_pane_id(value: &serde_json::Value) -> Option<String> {
    extract_string(value, "pane_id")
}

/// Resolve an a2a target (role or pane id) to a concrete pane id.
async fn resolve_target_pane(server: &HerdrMcpServer, target: &str) -> Result<String, McpError> {
    if !target.is_empty()
        && let Some(pane) = server.registry.resolve("", target).await
    {
        return Ok(pane);
    }
    Err(McpError {
        code: rmcp::model::ErrorCode(-32000),
        message: format!("No agent found for target '{target}'").into(),
        data: None,
    })
}

/// Read a pane's recent output via the herdr CLI.
async fn read_pane_text(pane_id: &str) -> anyhow::Result<String> {
    let binary = std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".to_string());
    let output = tokio::process::Command::new(binary)
        .args([
            "pane", "read", pane_id, "--source", "recent", "--lines", "200",
        ])
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run `herdr` CLI args and return stdout as a JSON-pretty-printed `CallToolResult`.
async fn run_herdr_json(args: &[&str]) -> Result<CallToolResult, McpError> {
    let output = herdr_cli(args).await?;

    match serde_json::from_str::<serde_json::Value>(&output) {
        Ok(value) => {
            let content = Content::json(value).map_err(|e| McpError {
                code: rmcp::model::ErrorCode(-32603),
                message: format!("Failed to serialize JSON: {e}").into(),
                data: None,
            })?;
            Ok(CallToolResult::success(vec![content]))
        }
        Err(e) => {
            tracing::debug!("herdr output was not valid JSON: {e}");
            Ok(CallToolResult::success(vec![Content::text(output)]))
        }
    }
}

/// Run `herdr` CLI args and return stdout as plain text.
async fn run_herdr_text(args: &[&str]) -> Result<CallToolResult, McpError> {
    let output = herdr_cli(args).await?;
    Ok(CallToolResult::success(vec![Content::text(output)]))
}

// ── Clipboard helpers ────────────────────────────────────────────────

/// Detect the platform clipboard command pair (copy, paste).
///
/// Resolution order (highest precedence first):
/// 1. `HERDR_MCP_CLIPBOARD_COPY` / `HERDR_MCP_CLIPBOARD_PASTE` env vars (both set).
/// 2. The `clipboard.copy-command` / `clipboard.paste-command` config settings
///    (both set) — lets Linux users force e.g. `xsel` instead of `wl-copy`.
/// 3. Platform auto-detection (pbcopy / wl-copy / xclip / xsel).
///
/// The leaked `&'static str` pairs are used for the process lifetime; this
/// avoids threading an allocator through the call sites.
fn detect_clipboard(
    cfg: &herdr_mcp_core::ClipboardConfig,
) -> Result<(&'static str, &'static str), McpError> {
    if let (Ok(copy), Ok(paste)) = (
        std::env::var("HERDR_MCP_CLIPBOARD_COPY"),
        std::env::var("HERDR_MCP_CLIPBOARD_PASTE"),
    ) && !copy.trim().is_empty()
        && !paste.trim().is_empty()
    {
        // Leak intentionally: the returned pair is 'static and used for the
        // process lifetime; this avoids threading an allocator through the
        // call sites.
        let copy: &'static str = Box::leak(copy.into_boxed_str());
        let paste: &'static str = Box::leak(paste.into_boxed_str());
        return Ok((copy, paste));
    }
    if let (Some(copy), Some(paste)) = (&cfg.copy_command, &cfg.paste_command)
        && !copy.trim().is_empty()
        && !paste.trim().is_empty()
    {
        let copy: &'static str = Box::leak(copy.clone().into_boxed_str());
        let paste: &'static str = Box::leak(paste.clone().into_boxed_str());
        return Ok((copy, paste));
    }
    match std::env::consts::OS {
        "macos" => Ok(("pbcopy", "pbpaste")),
        "windows" => Ok(("clip", "powershell -command Get-Clipboard")),
        "linux" => {
            // Prefer xsel/xclip (work headless) over wl-copy (requires Wayland).
            if command_exists("xsel") {
                Ok(("xsel --clipboard --input", "xsel --clipboard --output"))
            } else if command_exists("xclip") {
                Ok((
                    "xclip -selection clipboard",
                    "xclip -selection clipboard -o",
                ))
            } else if command_exists("wl-copy") {
                Ok(("wl-copy", "wl-paste"))
            } else {
                Err(McpError {
                    code: rmcp::model::ErrorCode(-32603),
                    message:
                        "No clipboard utility found. Install xclip, xsel, or wl-clipboard (e.g. `apt install xclip`).".into(),
                    data: None,
                })
            }
        }
        _ => Err(McpError {
            code: rmcp::model::ErrorCode(-32603),
            message: "Unsupported platform for clipboard operations.".into(),
            data: None,
        }),
    }
}

/// True if `cmd` resolves on PATH.
fn command_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Shell out to a command, piping `stdin_text` to its stdin (clipboard copy).
async fn shell_with_stdin(cmd: &str, stdin_text: &str) -> Result<String, McpError> {
    tracing::debug!("clipboard copy via: {cmd}");
    let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
    let bin = parts[0];
    let args: Vec<&str> = parts
        .get(1)
        .map(|a| a.split_whitespace().collect())
        .unwrap_or_default();

    let mut child = tokio::process::Command::new(bin)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| McpError {
            code: rmcp::model::ErrorCode(-32603),
            message: format!("Clipboard utility '{bin}' not found: {e}").into(),
            data: None,
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(stdin_text.as_bytes())
            .await
            .map_err(|e| McpError {
                code: rmcp::model::ErrorCode(-32603),
                message: format!("Failed to write to clipboard utility stdin: {e}").into(),
                data: None,
            })?;
    }

    let output = child.wait_with_output().await.map_err(|e| McpError {
        code: rmcp::model::ErrorCode(-32603),
        message: format!("Clipboard utility failed: {e}").into(),
        data: None,
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            return Err(McpError {
                code: rmcp::model::ErrorCode(-32000),
                message: format!("Clipboard write failed: {}", stderr.trim()).into(),
                data: None,
            });
        }
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Shell out to a command, capturing stdout (clipboard paste).
async fn shell_output(cmd: &str) -> Result<String, McpError> {
    tracing::debug!("clipboard paste via: {cmd}");
    let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
    let bin = parts[0];
    let args: Vec<&str> = parts
        .get(1)
        .map(|a| a.split_whitespace().collect())
        .unwrap_or_default();

    let output = tokio::process::Command::new(bin)
        .args(&args)
        .output()
        .await
        .map_err(|e| McpError {
            code: rmcp::model::ErrorCode(-32603),
            message: format!("Clipboard utility '{bin}' not found: {e}").into(),
            data: None,
        })?;

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Spawn a background task that refreshes trim badges for all workspaces
/// every 20s. Best-effort: I/O errors are logged, never propagated.
pub fn spawn_trim_poller(server: std::sync::Arc<HerdrMcpServer>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(20));
        loop {
            interval.tick().await;
            server.push_all_badges().await;
        }
    });
}

/// Execute the `herdr` CLI binary with the given arguments.
async fn herdr_cli(args: &[&str]) -> Result<String, McpError> {
    tracing::debug!("herdr {}", args.join(" "));

    let binary = std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".to_string());
    let output = tokio::process::Command::new(binary)
        .args(args)
        .output()
        .await
        .map_err(|e| McpError {
            code: rmcp::model::ErrorCode(-32603),
            message: format!("Failed to execute herdr: {e}").into(),
            data: None,
        })?;

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
        Err(McpError {
            code: rmcp::model::ErrorCode(-32000),
            message: msg.into(),
            data: None,
        })
    }
}

/// Resolve a pane_id from an optional pane_id or label.
/// If pane_id is provided, returns it directly.
/// If label is provided, queries herdr pane list to find the matching pane.
async fn resolve_pane_id(
    pane_id: Option<String>,
    label: Option<String>,
) -> Result<String, McpError> {
    match (pane_id, label) {
        (Some(pid), _) => Ok(pid),
        (None, Some(lbl)) => {
            let output = herdr_cli(&["pane", "list"]).await?;
            let value: serde_json::Value = serde_json::from_str(&output).map_err(|e| McpError {
                code: rmcp::model::ErrorCode(-32603),
                message: format!("Failed to parse pane list: {e}").into(),
                data: None,
            })?;
            let panes = value["result"]["panes"]
                .as_array()
                .ok_or_else(|| McpError {
                    code: rmcp::model::ErrorCode(-32603),
                    message: "Unexpected pane list format".into(),
                    data: None,
                })?;

            let labels: Vec<String> = panes
                .iter()
                .filter_map(|p| p.get("label").and_then(|l| l.as_str()))
                .map(|l| l.to_string())
                .collect();

            let matched: Vec<String> = panes
                .iter()
                .filter(|p| p.get("label").and_then(|l| l.as_str()) == Some(lbl.as_str()))
                .filter_map(|p| p.get("pane_id").and_then(|id| id.as_str()))
                .map(|id| id.to_string())
                .collect();

            match matched.len() {
                0 => {
                    let available = if labels.is_empty() {
                        "no labeled panes found".to_string()
                    } else {
                        format!("available labels: {}", labels.join(", "))
                    };
                    Err(McpError {
                        code: rmcp::model::ErrorCode(-32000),
                        message: format!("No pane found with label '{lbl}'. {available}").into(),
                        data: None,
                    })
                }
                1 => Ok(matched[0].clone()),
                _ => Err(McpError {
                    code: rmcp::model::ErrorCode(-32000),
                    message: format!(
                        "Multiple panes found with label '{lbl}'. Use pane_id instead."
                    )
                    .into(),
                    data: None,
                }),
            }
        }
        (None, None) => Err(McpError {
            code: rmcp::model::ErrorCode(-32000),
            message: "Either pane_id or label is required".into(),
            data: None,
        }),
    }
}

// ── HTTP Bridge ───────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use regex::Regex;
use tower_http::cors::CorsLayer;

/// Shared state for the Axum HTTP router: the server plus the optional cached
/// frontend HTML (single-file build at `dist/index.html`). The bytes are read
/// once at startup and served for every `GET /` and SPA fallback request.
#[derive(Clone)]
struct AppState {
    server: HerdrMcpServer,
    /// Cached bytes of `dist/index.html`, read once at router construction.
    index_html: Option<Arc<Vec<u8>>>,
}

/// Build the Axum router for the web playground. Shared by [`start_http`] and
/// the in-process dashboard that runs the server stack without MCP stdio.
fn http_router(server: HerdrMcpServer) -> Router {
    let index_html = match std::fs::read("./dist/index.html") {
        Ok(bytes) => {
            tracing::info!(
                "serving frontend from ./dist/index.html ({} bytes)",
                bytes.len()
            );
            Some(Arc::new(bytes))
        }
        Err(e) => {
            let hint = format!(
                "frontend not built: ./dist/index.html ({e}); \
                 run `npm run build` in the repo root. /api/* still available."
            );
            tracing::warn!("{hint}");
            None
        }
    };
    let state = AppState { server, index_html };

    Router::new()
        .route("/", get(index_html_handler))
        .route("/api/health", get(health_handler))
        .route("/api/tools", get(list_tools_handler))
        .route("/api/tools/{name}", post(call_tool_handler))
        .route("/api/agents", get(agents_handler))
        .route("/api/workspaces", get(workspaces_handler))
        .route("/api/recipe", post(run_recipe_handler))
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
        .route("/api/recipes/{id}/run", post(run_recipe_by_id_handler))
        .route(
            "/api/variables",
            get(list_variables_handler).post(save_variable_handler),
        )
        .route(
            "/api/variables/{key}",
            get(get_variable_handler).delete(delete_variable_handler),
        )
        .route("/api/executions/{id}", get(get_execution_handler))
        .route("/api/trim/status", get(trim_status_http_handler))
        .route("/api/trim/diagnose", post(trim_diagnose_http_handler))
        .route("/api/trim/summary", post(trim_summary_http_handler))
        .route(
            "/api/trim/dashboard/open",
            post(trim_dashboard_open_http_handler),
        )
        .layer(CorsLayer::permissive())
        .fallback(index_html_handler)
        .with_state(state)
}

/// Start the Axum HTTP server for the web playground. Bind lives until the
/// returned task is aborted or the process exits. The route table is built by
/// [`http_router`]; this is the thin bind + serve wrapper.
pub async fn start_http(server: HerdrMcpServer, port: u16) -> anyhow::Result<()> {
    let app = http_router(server);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn list_tools_handler() -> Json<serde_json::Value> {
    let tools = HerdrMcpServer::tool_router().list_all();
    Json(serde_json::json!({ "tools": tools }))
}

async fn call_tool_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<serde_json::Value>>,
) -> Result<Json<CallToolResult>, (StatusCode, String)> {
    let body = body.map(|j| j.0).unwrap_or(serde_json::json!({}));
    dispatch_tool(&server, &name, body)
        .await
        .map(Json)
        .map_err(|e| (e.0, e.1))
}

/// `GET /api/agents?workspace_id=w1` — canonical pane list from the live
/// `AgentRegistry`. This is the single source of truth the TUI dashboard reads;
/// `list_panes` (MCP tool) shells out to herdr and is reserved as a diagnostic
/// fallback only. Empty `workspace_id` returns the global snapshot.
async fn agents_handler(
    State(AppState { server, .. }): State<AppState>,
    Query(params): Query<TrimStatusParams>,
) -> Json<serde_json::Value> {
    let handles = match &params.workspace_id {
        Some(ws) if !ws.is_empty() => server.registry.list_for_ws(ws).await,
        _ => server.registry.inner_snapshot().await,
    };
    Json(serde_json::json!({ "agents": handles }))
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

/// `GET /` (also SPA fallback) — serves the single-file frontend build, or a
/// plain-text hint when `dist/index.html` is missing.
async fn index_html_handler(
    State(AppState { index_html, .. }): State<AppState>,
) -> (StatusCode, [(&'static str, &'static str); 1], Vec<u8>) {
    match &index_html {
        Some(bytes) => (
            StatusCode::OK,
            [("content-type", "text/html; charset=utf-8")],
            bytes.as_ref().clone(),
        ),
        None => (
            StatusCode::NOT_FOUND,
            [("content-type", "text/plain; charset=utf-8")],
            b"frontend not built \xe2\x80\x94 run `npm run build` in the repo root".to_vec(),
        ),
    }
}

fn mcp_err_to_http(e: McpError) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.message.to_string())
}

fn bad_request(e: impl ToString) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}

/// `GET /api/trim/status?workspace_id=w1` — aggregate trim savings.
async fn trim_status_http_handler(
    State(AppState { server, .. }): State<AppState>,
    Query(params): Query<TrimStatusParams>,
) -> Json<TrimStatusResponse> {
    Json(
        server
            .aggregate_trim_status(params.workspace_id.as_deref())
            .await,
    )
}

/// `POST /api/trim/diagnose` — end-to-end readiness check.
async fn trim_diagnose_http_handler(
    State(AppState { server, .. }): State<AppState>,
    body: Option<Json<serde_json::Value>>,
) -> Json<DiagnoseReport> {
    let ws = body.and_then(|j| {
        j.0.get("workspace_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });
    Json(server.build_diagnose_report(ws.as_deref()).await)
}

/// `POST /api/trim/summary` — fire a savings notification.
async fn trim_summary_http_handler(
    State(AppState { server, .. }): State<AppState>,
    body: Option<Json<serde_json::Value>>,
) -> Json<serde_json::Value> {
    let p: TrimStatusParams = body
        .and_then(|j| serde_json::from_value(j.0).ok())
        .unwrap_or_default();
    let json = match server.trim_summary(Parameters(p)).await {
        Ok(res) => serde_json::to_value(&res).unwrap_or(serde_json::Value::Null),
        Err(e) => serde_json::json!({ "error": e.message.to_string() }),
    };
    Json(json)
}

/// `POST /api/trim/dashboard/open` — open a live dashboard pane.
async fn trim_dashboard_open_http_handler(
    State(AppState { server, .. }): State<AppState>,
    body: Option<Json<serde_json::Value>>,
) -> Json<serde_json::Value> {
    let p: TrimStatusParams = body
        .and_then(|j| serde_json::from_value(j.0).ok())
        .unwrap_or_default();
    let json = match server.trim_dashboard_open(Parameters(p)).await {
        Ok(res) => serde_json::to_value(&res).unwrap_or(serde_json::Value::Null),
        Err(e) => serde_json::json!({ "error": e.message.to_string() }),
    };
    Json(json)
}

/// Dispatch a tool call by name, deserializing the JSON body into the appropriate
/// `Parameters<T>` struct.
async fn dispatch_tool(
    server: &HerdrMcpServer,
    name: &str,
    body: serde_json::Value,
) -> Result<CallToolResult, (StatusCode, String)> {
    match name {
        "status" => server.status().await.map_err(mcp_err_to_http),
        "list_workspaces" => server.list_workspaces().await.map_err(mcp_err_to_http),
        "list_agents" => server.list_agents().await.map_err(mcp_err_to_http),

        "list_tabs" => {
            let p: ListTabsParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .list_tabs(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "list_panes" => {
            let p: ListPanesParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .list_panes(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "get_pane" => {
            let p: GetPaneParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .get_pane(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "get_agent" => {
            let p: GetAgentParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .get_agent(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "create_workspace" => {
            let p: CreateWorkspaceParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .create_workspace(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "create_tab" => {
            let p: CreateTabParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .create_tab(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "split_pane" => {
            let p: SplitPaneParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .split_pane(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "close_pane" => {
            let p: ClosePaneParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .close_pane(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "start_agent" => {
            let p: StartAgentParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .start_agent(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "read_pane" => {
            let p: ReadPaneParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .read_pane(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "read_agent" => {
            let p: ReadAgentParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .read_agent(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "send_text" => {
            let p: SendTextParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .send_text(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "send_keys" => {
            let p: SendKeysParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .send_keys(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "run_command" => {
            let p: RunCommandParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .run_command(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "send_agent" => {
            let p: SendAgentParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .send_agent(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "wait_output" => {
            let p: WaitOutputParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .wait_output(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "wait_pane_agent_status" => {
            let p: WaitPaneAgentStatusParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .wait_pane_agent_status(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "wait_agent_status" => {
            let p: WaitAgentStatusParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .wait_agent_status(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }

        "agent_spawn" => {
            let p: AgentSpawnParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .agent_spawn(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "agent_message" => {
            let p: AgentMessageParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .agent_message(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "agent_read" => {
            let p: AgentReadParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .agent_read(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "agent_wait" => {
            let p: AgentWaitParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .agent_wait(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "agent_list" => {
            let p: AgentListParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .agent_list(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "var_get" => {
            let p: VarGetParams = serde_json::from_value(body).map_err(bad_request)?;
            server.var_get(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "var_set" => {
            let p: VarSetParams = serde_json::from_value(body).map_err(bad_request)?;
            server.var_set(Parameters(p)).await.map_err(mcp_err_to_http)
        }

        // Recipe templates.
        "list_templates" => {
            let p: ListTemplatesParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .list_templates(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "get_template" => {
            let p: GetTemplateParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .get_template(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "instantiate_template" => {
            let p: InstantiateTemplateParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .instantiate_template(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }

        // Message-trim tools.
        "compress" => {
            let p: CompressParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .compress(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "decompress" => {
            let p: DecompressParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .decompress(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "trim_policy_set" => {
            let p: TrimPolicySetParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .trim_policy_set(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "trim_policy_get" => {
            let p: TrimPolicyGetParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .trim_policy_get(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "trim_eval" => {
            let p: TrimEvalParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .trim_eval(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "trim_bench" => {
            let p: TrimBenchParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .trim_bench(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }

        // Aggregation / diagnostics tools.
        "trim_status" => {
            let p: TrimStatusParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .trim_status(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "trim_diagnose" => {
            let p: TrimStatusParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .trim_diagnose(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "trim_summary" => {
            let p: TrimStatusParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .trim_summary(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "trim_dashboard_open" => {
            let p: TrimStatusParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .trim_dashboard_open(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }

        // Scheduler.
        "schedule_recipe" => {
            let p: ScheduleRecipeParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .schedule_recipe(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "list_schedules" => {
            let p: ListSchedulesParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .list_schedules(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "delete_schedule" => {
            let p: DeleteScheduleParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .delete_schedule(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "enable_schedule" => {
            let p: EnableScheduleParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .enable_schedule(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }

        // Folder keys (PFC1).
        "build_folder_key" => {
            let p: BuildFolderKeyParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .build_folder_key(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "get_folder_key" => {
            let p: GetFolderKeyParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .get_folder_key(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "list_folder_keys" => {
            let p: ListFolderKeysParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .list_folder_keys(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "decompress_with_folder_key" => {
            let p: DecompressWithFolderKeyParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server
                .decompress_with_folder_key(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }

        "clipboard_set" => {
            let p: ClipboardSetParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .clipboard_set(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }
        "clipboard_get" => {
            let p: ClipboardGetParams = serde_json::from_value(body).map_err(bad_request)?;
            server
                .clipboard_get(Parameters(p))
                .await
                .map_err(mcp_err_to_http)
        }

        _ => Err((StatusCode::NOT_FOUND, format!("Unknown tool: {name}"))),
    }
}

// ── Recipe Engine ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RecipeRequest {
    #[serde(default)]
    name: Option<String>,
    /// herdr workspace id (e.g. "w1"). Variables are scoped to this workspace so
    /// recipes can chain across its panes/tabs.
    #[serde(default)]
    session_id: Option<String>,
    steps: Vec<RecipeStep>,
}

#[derive(Debug, Serialize)]
struct RecipeResponse {
    results: HashMap<String, serde_json::Value>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Shared recipe execution core. Seeds `accumulated` from the live agent
/// registry for `session_id` so `{{pane_id.output}}` and `{{role.output}}`
/// resolve to another agent's captured work product.
async fn execute_recipe(
    server: &HerdrMcpServer,
    steps: &[RecipeStep],
    session_id: Option<&str>,
) -> RecipeResponse {
    let mut results = HashMap::new();
    let mut accumulated = HashMap::new();

    // Seed agent handles (pane_id/role -> output) for the session workspace.
    if let Some(ws) = session_id {
        for (key, value) in server.registry.seed(ws).await {
            accumulated.insert(key, value);
        }
    }

    let execution_id = uuid::Uuid::new_v4();
    let started_at = chrono::Utc::now();

    for step in steps {
        // Resolve {{ path }} variables in params from accumulated results
        let mut resolved = step.params.clone();
        resolve_variables(&mut resolved, &accumulated);

        match dispatch_tool(server, &step.tool, resolved).await {
            Ok(result) => {
                let json_result = serde_json::to_value(&result).unwrap_or_default();
                let extracted = crate::variables::extract_variables(&json_result);
                accumulated.extend(extracted);
                accumulated.insert(step.id.clone(), json_result.clone());
                // If the step returned an agent handle (pane_id + output),
                // expose it under both the pane id and its role so downstream
                // steps can interpolate `{{pane_id.output}}` or `{{role.output}}`.
                if let Some(obj) = json_result.as_object()
                    && let Some(pane_id) = obj.get("pane_id").and_then(|v| v.as_str())
                {
                    accumulated.insert(pane_id.to_string(), json_result.clone());
                    if let Some(role) = obj.get("role").and_then(|v| v.as_str())
                        && !role.is_empty()
                    {
                        accumulated.insert(role.to_string(), json_result.clone());
                    }
                }
                results.insert(step.id.clone(), json_result);
            }
            Err((code, msg)) => {
                results.insert(
                    step.id.clone(),
                    serde_json::json!({
                        "error": msg,
                        "statusCode": code.as_u16(),
                    }),
                );

                let execution = ExecutionResult {
                    id: execution_id,
                    recipe_id: uuid::Uuid::nil(),
                    started_at,
                    completed_at: Some(chrono::Utc::now()),
                    status: ExecutionStatus::Failed,
                    results: results.clone(),
                    variables: accumulated.clone(),
                    error: Some(msg.clone()),
                };
                let _ = server.persistence.save_execution(&execution).await;

                return RecipeResponse {
                    results,
                    status: "failed".into(),
                    failed_step: Some(step.id.clone()),
                    error: Some(msg),
                };
            }
        }
    }

    let execution = ExecutionResult {
        id: execution_id,
        recipe_id: uuid::Uuid::nil(),
        started_at,
        completed_at: Some(chrono::Utc::now()),
        status: ExecutionStatus::Completed,
        results: results.clone(),
        variables: accumulated.clone(),
        error: None,
    };
    let _ = server.persistence.save_execution(&execution).await;

    RecipeResponse {
        results,
        status: "completed".into(),
        failed_step: None,
        error: None,
    }
}

async fn run_recipe_handler(
    State(AppState { server, .. }): State<AppState>,
    Json(req): Json<RecipeRequest>,
) -> Json<RecipeResponse> {
    let session = req.session_id.as_deref();
    let response = execute_recipe(&server, &req.steps, session).await;
    Json(response)
}
fn resolve_variables(value: &mut serde_json::Value, results: &HashMap<String, serde_json::Value>) {
    match value {
        serde_json::Value::String(s) => {
            let re = Regex::new(r"\{\{([^}]+)\}\}").expect("regex pattern is valid");
            *s = re
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

/// Navigate a dotted path like `step1.result.content[0].text` into a nested JSON tree.
fn resolve_json_path(
    root: &HashMap<String, serde_json::Value>,
    path: &str,
) -> Option<serde_json::Value> {
    let (step_id, rest) = path.split_once('.')?;
    let mut current = root.get(step_id)?.clone();

    if rest.is_empty() {
        return Some(current);
    }

    for segment in rest.split('.') {
        if let Some(idx) = segment.find('[') {
            let key = &segment[..idx];
            let idx_str = &segment[idx + 1..segment.len() - 1];
            let index: usize = idx_str.parse().ok()?;
            current = current.get(key)?.get(index)?.clone();
        } else {
            current = current.get(segment)?.clone();
        }
    }

    Some(current)
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

#[derive(Debug, Deserialize)]
struct CreateRecipeRequest {
    name: String,
    #[serde(default)]
    description: Option<String>,
    steps: Vec<RecipeStepInput>,
    #[serde(default)]
    variables: HashMap<String, serde_json::Value>,
    #[serde(default)]
    category: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RecipeStepInput {
    id: String,
    tool: String,
    params: serde_json::Value,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateRecipeRequest {
    name: Option<String>,
    description: Option<String>,
    steps: Option<Vec<RecipeStepInput>>,
    variables: Option<HashMap<String, serde_json::Value>>,
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
    let now = chrono::Utc::now();
    let recipe = Recipe {
        id: uuid::Uuid::new_v4(),
        name: req.name,
        description: req.description,
        steps: req
            .steps
            .into_iter()
            .map(|s| RecipeStep {
                id: s.id,
                tool: s.tool,
                params: s.params,
                description: s.description,
            })
            .collect(),
        variables: req.variables,
        created_at: now,
        updated_at: now,
        is_template: false,
        category: req.category,
    };

    server
        .persistence
        .save_recipe(&recipe)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(recipe))
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
    let mut recipe = server
        .persistence
        .load_recipe(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "Recipe not found".to_string()))?;

    if let Some(name) = req.name {
        recipe.name = name;
    }
    if let Some(desc) = req.description {
        recipe.description = Some(desc);
    }
    if let Some(steps) = req.steps {
        recipe.steps = steps
            .into_iter()
            .map(|s| RecipeStep {
                id: s.id,
                tool: s.tool,
                params: s.params,
                description: s.description,
            })
            .collect();
    }
    if let Some(vars) = req.variables {
        recipe.variables = vars;
    }
    recipe.updated_at = chrono::Utc::now();

    server
        .persistence
        .save_recipe(&recipe)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(recipe))
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

async fn run_recipe_by_id_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<ExecutionResult>, (StatusCode, String)> {
    let recipe = server
        .persistence
        .load_recipe(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "Recipe not found".to_string()))?;

    let execution_id = uuid::Uuid::new_v4();
    let started_at = chrono::Utc::now();

    let mut results = HashMap::new();
    let mut accumulated = recipe.variables.clone();
    let mut status = ExecutionStatus::Running;
    let mut error_msg: Option<String> = None;

    for step in &recipe.steps {
        let mut resolved = step.params.clone();
        resolve_variables(&mut resolved, &accumulated);

        match dispatch_tool(&server, &step.tool, resolved).await {
            Ok(result) => {
                let json_result = serde_json::to_value(&result).unwrap_or_default();
                let extracted = crate::variables::extract_variables(&json_result);
                accumulated.extend(extracted.clone());
                accumulated.insert(step.id.clone(), json_result.clone());
                results.insert(step.id.clone(), json_result);
            }
            Err((code, msg)) => {
                results.insert(
                    step.id.clone(),
                    serde_json::json!({
                        "error": msg,
                        "statusCode": code.as_u16(),
                    }),
                );
                status = ExecutionStatus::Failed;
                error_msg = Some(msg);
                break;
            }
        }
    }

    if status == ExecutionStatus::Running {
        status = ExecutionStatus::Completed;
    }

    let execution = ExecutionResult {
        id: execution_id,
        recipe_id: id,
        started_at,
        completed_at: Some(chrono::Utc::now()),
        status,
        results: results.clone(),
        variables: accumulated.clone(),
        error: error_msg,
    };

    server
        .persistence
        .save_execution(&execution)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(execution))
}

// ── Variables API ──────────────────────────────────────────────────────

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

async fn save_variable_handler(
    State(AppState { server, .. }): State<AppState>,
    Json(req): Json<crate::persistence::VariableStore>,
) -> Result<StatusCode, (StatusCode, String)> {
    server
        .persistence
        .save_variable(&req)
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
    State(_state): State<AppState>,
    Path(_key): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    Ok(StatusCode::NO_CONTENT)
}

// ── Executions API ────────────────────────────────────────────────────

async fn get_execution_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<ExecutionResult>, (StatusCode, String)> {
    server
        .persistence
        .load_execution(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "Execution not found".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
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
    fn test_resolve_json_path_single_segment_returns_none() {
        // resolve_json_path requires a dotted path (step.field); a bare key returns None.
        let root = sample_results();
        assert_eq!(resolve_json_path(&root, "step2"), None);
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

    // Single test touching process-global env vars so the assertions don't
    // race with each other when tests run in parallel.
    #[tokio::test]
    async fn test_clipboard_env_override_and_roundtrip() {
        let server = clipboard_test_server();
        // --- detection override ---
        unsafe {
            std::env::set_var("HERDR_MCP_CLIPBOARD_COPY", "mycopy");
            std::env::set_var("HERDR_MCP_CLIPBOARD_PASTE", "mypaste");
        }
        let (copy, paste) = detect_clipboard(&server.clipboard).expect("override should be used");
        assert_eq!(copy, "mycopy");
        assert_eq!(paste, "mypaste");

        // Override beats platform detection (even if a real backend exists).
        unsafe {
            std::env::set_var("HERDR_MCP_CLIPBOARD_COPY", "forced-copy");
            std::env::set_var("HERDR_MCP_CLIPBOARD_PASTE", "forced-paste");
        }
        let (copy, _) = detect_clipboard(&server.clipboard).unwrap();
        assert_eq!(copy, "forced-copy");

        // --- roundtrip via a fake backend driven by the override ---
        let tmp = tempfile::tempdir().unwrap();
        let clip = tmp.path().join("clip.txt");
        let copy_sh = tmp.path().join("fake_copy.sh");
        let paste_sh = tmp.path().join("fake_paste.sh");
        std::fs::write(&copy_sh, "#!/bin/sh\ncat > \"$1\"\n").unwrap();
        std::fs::write(&paste_sh, "#!/bin/sh\ncat \"$1\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = 0o755;
            std::fs::set_permissions(&copy_sh, std::fs::Permissions::from_mode(mode)).unwrap();
            std::fs::set_permissions(&paste_sh, std::fs::Permissions::from_mode(mode)).unwrap();
        }

        let copy_cmd = format!("{} {}", copy_sh.display(), clip.display());
        let paste_cmd = format!("{} {}", paste_sh.display(), clip.display());
        unsafe {
            std::env::set_var("HERDR_MCP_CLIPBOARD_COPY", &copy_cmd);
            std::env::set_var("HERDR_MCP_CLIPBOARD_PASTE", &paste_cmd);
        }

        let data_dir = tmp.path().join("data");
        let pers = Persistence::new(data_dir.clone());
        let pers_arc = std::sync::Arc::new(pers);
        let registry = AgentRegistry::new(pers_arc);
        let server = HerdrMcpServer::new(Persistence::new(data_dir), registry);

        let set_res = server
            .clipboard_set(Parameters(ClipboardSetParams {
                text: "hello clipboard".into(),
            }))
            .await;
        assert!(set_res.is_ok(), "clipboard_set should succeed via override");
        // The backend file proves the text was actually written through.
        let written = std::fs::read_to_string(&clip).unwrap();
        assert_eq!(written, "hello clipboard");

        let got = server
            .clipboard_get(Parameters(ClipboardGetParams {}))
            .await
            .expect("clipboard_get should succeed");
        // Extract text via the rmcp API (Content derefs to RawContent).
        let mut text = String::new();
        for c in &got.content {
            if let Some(tc) = c.as_text() {
                text = tc.text.clone();
            }
        }
        assert_eq!(text, "hello clipboard");

        unsafe {
            std::env::remove_var("HERDR_MCP_CLIPBOARD_COPY");
            std::env::remove_var("HERDR_MCP_CLIPBOARD_PASTE");
        }

        // --- config override beats platform detection (env now cleared) ---
        let mut cfg = herdr_mcp_core::Config::default();
        cfg.clipboard.copy_command = Some("cfg-copy".into());
        cfg.clipboard.paste_command = Some("cfg-paste".into());
        let cc_tmp = tempfile::tempdir().unwrap();
        let cc_pers = Persistence::new(cc_tmp.path().join("data"));
        let cc_reg = AgentRegistry::new(std::sync::Arc::new(cc_pers));
        let cfg_server = HerdrMcpServer::with_config(
            Persistence::new(cc_tmp.path().join("data2")),
            cc_reg,
            &cfg,
        );
        let (copy, paste) = detect_clipboard(&cfg_server.clipboard).unwrap();
        assert_eq!(copy, "cfg-copy");
        assert_eq!(paste, "cfg-paste");
    }

    #[test]
    fn detect_clipboard_empty_config_does_not_panic() {
        let cfg = herdr_mcp_core::ClipboardConfig::default();
        // With no env and no config, detection falls through to platform auto-detect.
        // On a real system this returns a tool or an error — but it must not panic.
        let _ = detect_clipboard(&cfg);
    }
}

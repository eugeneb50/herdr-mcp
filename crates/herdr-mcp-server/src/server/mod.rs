use herdr_mcp_api::settings;
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::herdr_client::{AgentRegistry, HerdrClient, SubscriberPhase};
use crate::persistence::Persistence;
use crate::scheduler::Scheduler;
use crate::security::Origin;
use crate::templates as tmpl;
use crate::variables::{Recipe, ScheduledRecipe};
use herdr_mcp_api::{CreateRecipeRequest, UpdateRecipeRequest};
use regex::Regex;
mod engine;
mod http;
mod params;
#[cfg(test)]
mod tests;

pub(crate) use engine::*;
pub use http::start_http;
pub(crate) use http::*;
pub use params::*;

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
                let phase = client.subscriber_state();
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
const GUIDE: &str = include_str!("../../../../docs/AGENT-GUIDE.md");

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

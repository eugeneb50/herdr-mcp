use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    schemars,
    tool, tool_handler, tool_router,
    ErrorData as McpError, ServerHandler,
};
use serde::{Deserialize, Serialize};

// ── Parameter structs ────────────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListTabsParams {
    pub workspace_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListPanesParams {
    pub workspace_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetPaneParams {
    pub pane_id: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetAgentParams {
    pub target: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateWorkspaceParams {
    pub cwd: Option<String>,
    pub label: Option<String>,
    pub no_focus: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateTabParams {
    pub workspace_id: Option<String>,
    pub label: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SplitPaneParams {
    pub pane_id: Option<String>,
    pub label: Option<String>,
    pub direction: Option<String>,
    pub cwd: Option<String>,
    pub no_focus: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClosePaneParams {
    pub pane_id: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StartAgentParams {
    pub name: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub workspace_id: Option<String>,
    pub tab_id: Option<String>,
    pub split: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadPaneParams {
    pub pane_id: Option<String>,
    pub label: Option<String>,
    pub source: Option<String>,
    pub lines: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadAgentParams {
    pub target: String,
    pub source: Option<String>,
    pub lines: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendTextParams {
    pub pane_id: Option<String>,
    pub label: Option<String>,
    pub text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendKeysParams {
    pub pane_id: Option<String>,
    pub label: Option<String>,
    pub keys: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RunCommandParams {
    pub pane_id: Option<String>,
    pub label: Option<String>,
    pub command: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendAgentParams {
    pub target: String,
    pub text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WaitOutputParams {
    pub pane_id: Option<String>,
    pub label: Option<String>,
    pub match_text: String,
    pub timeout_ms: Option<u32>,
    pub source: Option<String>,
    pub use_regex: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WaitPaneAgentStatusParams {
    pub pane_id: Option<String>,
    pub label: Option<String>,
    pub status: String,
    pub timeout_ms: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WaitAgentStatusParams {
    pub target: String,
    pub status: String,
    pub timeout_ms: Option<u32>,
}

// ── Server ───────────────────────────────────────────────────────────

/// The herdr MCP server.
///
/// Stateless — every tool call shells out to the local `herdr` CLI binary.
#[derive(Clone)]
pub struct HerdrMcpServer;

#[tool_router]
impl HerdrMcpServer {
    pub fn new() -> Self {
        Self
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

    #[tool(description = "Get details about a specific agent by target — terminal ID, agent name, or pane ID")]
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
        Parameters(CreateWorkspaceParams { cwd, label, no_focus }): Parameters<CreateWorkspaceParams>,
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

    #[tool(description = "Create a new tab in a workspace, optionally with a label and working directory")]
    async fn create_tab(
        &self,
        Parameters(CreateTabParams { workspace_id, label, cwd }): Parameters<CreateTabParams>,
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
        Parameters(SplitPaneParams { pane_id, label, direction, cwd, no_focus }): Parameters<SplitPaneParams>,
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

    #[tool(description = "Start an agent in a new pane. Pass the agent name and any arguments after '--'")]
    async fn start_agent(
        &self,
        Parameters(StartAgentParams { name, args, cwd, workspace_id, tab_id, split }): Parameters<StartAgentParams>,
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

    #[tool(description = "Read text output from a pane. Source: visible (current screen), recent (scrollback with wrapping), or recent-unwrapped (scrollback without soft wrapping, best for logs)")]
    async fn read_pane(
        &self,
        Parameters(ReadPaneParams { pane_id, label, source, lines }): Parameters<ReadPaneParams>,
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

    #[tool(description = "Read text output from an agent. Source: visible (current screen), recent (scrollback with wrapping), or recent-unwrapped (scrollback without soft wrapping, best for logs)")]
    async fn read_agent(
        &self,
        Parameters(ReadAgentParams { target, source, lines }): Parameters<ReadAgentParams>,
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

    #[tool(description = "Send text to a pane (without pressing Enter). Use run_command to send text+Enter atomically.")]
    async fn send_text(
        &self,
        Parameters(SendTextParams { pane_id, label, text }): Parameters<SendTextParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        run_herdr_json(&["pane", "send-text", &pid, &text]).await
    }

    #[tool(description = "Send key presses to a pane. Common keys: Enter, Escape, Tab, Backspace, Ctrl+c, Ctrl+d, ArrowUp, ArrowDown")]
    async fn send_keys(
        &self,
        Parameters(SendKeysParams { pane_id, label, keys }): Parameters<SendKeysParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        let mut args = vec!["pane", "send-keys", &pid];
        for key in &keys {
            args.push(key);
        }
        run_herdr_json(&args).await
    }

    #[tool(description = "Run a command in a pane (sends text + Enter atomically). Prefer this over send_text + send_keys Enter for commands.")]
    async fn run_command(
        &self,
        Parameters(RunCommandParams { pane_id, label, command }): Parameters<RunCommandParams>,
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

    #[tool(description = "Wait for specific text to appear in a pane. Blocks until matched or timeout. Supports --regex for pattern matching. Returns the matching output on success.")]
    async fn wait_output(
        &self,
        Parameters(WaitOutputParams { pane_id, label, match_text, timeout_ms, source, use_regex }): Parameters<WaitOutputParams>,
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

    #[tool(description = "Wait for a pane's agent to reach a specific status. Statuses: idle, working, blocked, done, unknown. Blocks until status reached or timeout.")]
    async fn wait_pane_agent_status(
        &self,
        Parameters(WaitPaneAgentStatusParams { pane_id, label, status, timeout_ms }): Parameters<WaitPaneAgentStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let pid = resolve_pane_id(pane_id, label).await?;
        let timeout_str = timeout_ms.map(|ms| ms.to_string());
        let mut args = vec!["wait", "agent-status", &pid, "--status", &status];
        if let Some(ref ms) = timeout_str {
            args.extend(["--timeout", ms]);
        }
        run_herdr_json(&args).await
    }

    #[tool(description = "Wait for an agent (by target) to reach a specific status. Statuses: idle, working, blocked, done, unknown. Blocks until status reached or timeout.")]
    async fn wait_agent_status(
        &self,
        Parameters(WaitAgentStatusParams { target, status, timeout_ms }): Parameters<WaitAgentStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let timeout_str = timeout_ms.map(|ms| ms.to_string());
        let mut args = vec!["agent", "wait", &target, "--status", &status];
        if let Some(ref ms) = timeout_str {
            args.extend(["--timeout", ms]);
        }
        run_herdr_json(&args).await
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
        Err(_) => Ok(CallToolResult::success(vec![Content::text(output)])),
    }
}

/// Run `herdr` CLI args and return stdout as plain text.
async fn run_herdr_text(args: &[&str]) -> Result<CallToolResult, McpError> {
    let output = herdr_cli(args).await?;
    Ok(CallToolResult::success(vec![Content::text(output)]))
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
            let panes = value["result"]["panes"].as_array().ok_or_else(|| McpError {
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
                    message: format!("Multiple panes found with label '{lbl}'. Use pane_id instead.").into(),
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

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use regex::Regex;
use tower_http::cors::CorsLayer;

/// Start the Axum HTTP server for the web playground.
pub async fn start_http(server: HerdrMcpServer, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/api/health", get(health_handler))
        .route("/api/tools", get(list_tools_handler))
        .route("/api/tools/{name}", post(call_tool_handler))
        .route("/api/recipe", post(run_recipe_handler))
        .layer(CorsLayer::permissive())
        .with_state(server);

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
    State(server): State<HerdrMcpServer>,
    Path(name): Path<String>,
    body: Option<Json<serde_json::Value>>,
) -> Result<Json<CallToolResult>, (StatusCode, String)> {
    let body = body.map(|j| j.0).unwrap_or(serde_json::json!({}));
    dispatch_tool(&server, &name, body)
        .await
        .map(Json)
        .map_err(|e| (e.0, e.1))
}

fn mcp_err_to_http(e: McpError) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        e.message.to_string(),
    )
}

fn bad_request(e: impl ToString) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
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
            let p: ListTabsParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.list_tabs(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "list_panes" => {
            let p: ListPanesParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.list_panes(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "get_pane" => {
            let p: GetPaneParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.get_pane(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "get_agent" => {
            let p: GetAgentParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.get_agent(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "create_workspace" => {
            let p: CreateWorkspaceParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.create_workspace(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "create_tab" => {
            let p: CreateTabParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.create_tab(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "split_pane" => {
            let p: SplitPaneParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.split_pane(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "close_pane" => {
            let p: ClosePaneParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.close_pane(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "start_agent" => {
            let p: StartAgentParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.start_agent(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "read_pane" => {
            let p: ReadPaneParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.read_pane(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "read_agent" => {
            let p: ReadAgentParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.read_agent(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "send_text" => {
            let p: SendTextParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.send_text(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "send_keys" => {
            let p: SendKeysParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.send_keys(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "run_command" => {
            let p: RunCommandParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.run_command(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "send_agent" => {
            let p: SendAgentParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.send_agent(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "wait_output" => {
            let p: WaitOutputParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.wait_output(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "wait_pane_agent_status" => {
            let p: WaitPaneAgentStatusParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.wait_pane_agent_status(Parameters(p)).await.map_err(mcp_err_to_http)
        }
        "wait_agent_status" => {
            let p: WaitAgentStatusParams =
                serde_json::from_value(body).map_err(bad_request)?;
            server.wait_agent_status(Parameters(p)).await.map_err(mcp_err_to_http)
        }

        _ => Err((StatusCode::NOT_FOUND, format!("Unknown tool: {name}"))),
    }
}

// ── Recipe Engine ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct RecipeRequest {
    #[serde(default)]
    name: Option<String>,
    steps: Vec<RecipeStep>,
}

#[derive(Debug, Deserialize)]
struct RecipeStep {
    id: String,
    tool: String,
    params: serde_json::Value,
    #[serde(default)]
    description: Option<String>,
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

async fn run_recipe_handler(
    State(server): State<HerdrMcpServer>,
    Json(req): Json<RecipeRequest>,
) -> Json<RecipeResponse> {
    let mut results = HashMap::new();
    let mut accumulated = HashMap::new();

    for step in &req.steps {
        // Resolve {{ path }} variables in params from accumulated results
        let mut resolved = step.params.clone();
        resolve_variables(&mut resolved, &accumulated);

        match dispatch_tool(&server, &step.tool, resolved).await {
            Ok(result) => {
                let json_result = serde_json::to_value(&result).unwrap_or_default();
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
                return Json(RecipeResponse {
                    results,
                    status: "failed".into(),
                    failed_step: Some(step.id.clone()),
                    error: Some(msg),
                });
            }
        }
    }

    Json(RecipeResponse {
        results,
        status: "completed".into(),
        failed_step: None,
        error: None,
    })
}

/// Resolve `{{ stepId.result.nested[0].field }}` templates in a JSON value
/// by looking up paths in the accumulated results map.
fn resolve_variables(value: &mut serde_json::Value, results: &HashMap<String, serde_json::Value>) {
    match value {
        serde_json::Value::String(s) => {
            let re = Regex::new(r"\{\{([^}]+)\}\}").unwrap();
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

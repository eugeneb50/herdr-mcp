//! Typed async client over the herdr-mcp HTTP bridge. One client for the web
//! UI (wasm) and the TUI (native). Holds only the base URL — every call
//! resolves from the server (SSoT: registry + `data/` are canonical).

use reqwest::{Method, RequestBuilder, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    AgentHandle, AgentsResponse, AutoVariablesResponse, CreateRecipeRequest, DiagnoseReport,
    ExecutionResult, ExportScriptResponse, PlaygroundSnapshot, ProxyPolicy, Recipe, RecipeRequest,
    RecipeResponse, RecipeTemplate, RunAction, RunRecord, SaveVariableRequest, StartRunRequest,
    ToolsResponse, TrimPolicy, TrimStatusResponse, UpdateRecipeRequest, VariableStore,
};

/// Errors from [`ApiClient`].
#[derive(Debug)]
pub enum ApiError {
    /// The base URL could not be parsed (must be absolute, e.g. `http://127.0.0.1:7676`).
    BadBase(String),
    /// Network / request failure.
    Transport(reqwest::Error),
    /// Bridge answered with a non-2xx status; `body` is its error text.
    Status { status: u16, body: String },
    /// 2xx response whose body didn't match the expected type.
    Decode(serde_json::Error),
    /// The tool ran but reported `is_error`; carries its text.
    Tool(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadBase(e) => write!(f, "invalid bridge URL: {e}"),
            Self::Transport(e) => write!(f, "bridge unreachable: {e}"),
            Self::Status { status, body } => write!(f, "HTTP {status}: {body}"),
            Self::Decode(e) => write!(f, "unexpected response: {e}"),
            Self::Tool(e) => write!(f, "tool error: {e}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        Self::Transport(e)
    }
}

/// Typed client for `/api/*`.
#[derive(Clone, Debug)]
pub struct ApiClient {
    base: Url,
    http: reqwest::Client,
}

impl ApiClient {
    /// Client against an absolute base, e.g. `http://127.0.0.1:7676`. In the
    /// browser pass `window.location.origin` — reqwest rejects relative URLs.
    pub fn new(base: &str) -> Result<Self, ApiError> {
        Self::build(base, reqwest::Client::new())
    }

    /// Like [`ApiClient::new`] with a per-request timeout (native only —
    /// reqwest has no client timeouts on wasm).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_timeout(base: &str, timeout: std::time::Duration) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(ApiError::Transport)?;
        Self::build(base, http)
    }

    fn build(base: &str, http: reqwest::Client) -> Result<Self, ApiError> {
        let base = Url::parse(base).map_err(|e| ApiError::BadBase(format!("{base}: {e}")))?;
        if base.cannot_be_a_base() {
            return Err(ApiError::BadBase(base.to_string()));
        }
        Ok(Self { base, http })
    }

    /// The bridge base URL.
    pub fn base(&self) -> &Url {
        &self.base
    }

    /// Build `{base}/{segments...}` with each segment percent-encoded.
    fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty().extend(segments);
        }
        url
    }

    fn request(&self, method: Method, segments: &[&str]) -> RequestBuilder {
        self.http.request(method, self.url(segments))
    }

    /// Send, fail on non-2xx, return the body text.
    async fn send(req: RequestBuilder) -> Result<String, ApiError> {
        let resp = req.send().await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(ApiError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(body)
    }

    async fn json<T: DeserializeOwned>(req: RequestBuilder) -> Result<T, ApiError> {
        let body = Self::send(req).await?;
        serde_json::from_str(&body).map_err(ApiError::Decode)
    }

    async fn get<T: DeserializeOwned>(&self, segments: &[&str]) -> Result<T, ApiError> {
        Self::json(self.request(Method::GET, segments)).await
    }

    async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        segments: &[&str],
        body: &B,
    ) -> Result<T, ApiError> {
        Self::json(self.request(Method::POST, segments).json(body)).await
    }

    /// POST/DELETE whose success response has no body (201/204).
    async fn no_content(&self, req: RequestBuilder) -> Result<(), ApiError> {
        Self::send(req).await.map(|_| ())
    }

    fn ws_query(req: RequestBuilder, workspace_id: Option<&str>) -> RequestBuilder {
        match workspace_id {
            Some(ws) if !ws.is_empty() => req.query(&[("workspace_id", ws)]),
            _ => req,
        }
    }

    fn ws_body(workspace_id: Option<&str>) -> Value {
        match workspace_id {
            Some(ws) if !ws.is_empty() => serde_json::json!({ "workspace_id": ws }),
            _ => serde_json::json!({}),
        }
    }

    // ── discovery ──────────────────────────────────────────────────────

    /// `GET /api/health` — `Ok` when the bridge answers.
    pub async fn health(&self) -> Result<(), ApiError> {
        Self::send(self.request(Method::GET, &["api", "health"]))
            .await
            .map(|_| ())
    }

    /// `GET /api/config` — runtime config summary (free-form JSON).
    pub async fn config(&self) -> Result<Value, ApiError> {
        self.get(&["api", "config"]).await
    }

    /// `GET /api/tools` — every MCP tool definition with its JSON schema.
    pub async fn list_tools(&self) -> Result<Vec<Value>, ApiError> {
        self.get::<ToolsResponse>(&["api", "tools"])
            .await
            .map(|r| r.tools)
    }

    /// `POST /api/tools/{name}` — invoke a tool; returns the MCP `CallToolResult` JSON.
    pub async fn call_tool(&self, name: &str, params: &Value) -> Result<Value, ApiError> {
        self.post(&["api", "tools", name], params).await
    }

    /// `GET /api/agents[?workspace_id=]` — live registry snapshot.
    pub async fn list_agents(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<Vec<AgentHandle>, ApiError> {
        let req = Self::ws_query(self.request(Method::GET, &["api", "agents"]), workspace_id);
        Self::json::<AgentsResponse>(req).await.map(|r| r.agents)
    }

    /// `GET /api/workspaces` — passthrough of `herdr workspace list` (JSON or null).
    pub async fn list_workspaces(&self) -> Result<Value, ApiError> {
        self.get(&["api", "workspaces"]).await
    }

    /// `clipboard_set` tool — write `text` to the server's clipboard.
    /// A tool-level failure (`is_error`) is returned as an error.
    pub async fn clipboard_set(&self, text: &str) -> Result<(), ApiError> {
        let v = self
            .call_tool("clipboard_set", &serde_json::json!({ "text": text }))
            .await?;
        tool_ok(&v).map(|_| ())
    }

    /// `clipboard_get` tool — the server clipboard's text.
    pub async fn clipboard_get(&self) -> Result<String, ApiError> {
        let v = self
            .call_tool("clipboard_get", &serde_json::json!({}))
            .await?;
        tool_ok(&v).map(tool_text)
    }

    /// `trim_policy_get` tool for a pane id / role / label.
    pub async fn trim_policy_get(&self, target: &str) -> Result<Value, ApiError> {
        self.call_tool("trim_policy_get", &serde_json::json!({ "target": target }))
            .await
    }

    /// `trim_policy_set` tool; `None` clears the policy.
    pub async fn trim_policy_set(
        &self,
        target: &str,
        policy: Option<&TrimPolicy>,
    ) -> Result<Value, ApiError> {
        self.call_tool(
            "trim_policy_set",
            &serde_json::json!({ "target": target, "policy": policy }),
        )
        .await
    }

    /// `proxy_diagnose` tool — CA status, defaults, per-pane policies.
    pub async fn proxy_diagnose(&self) -> Result<Value, ApiError> {
        self.call_tool("proxy_diagnose", &serde_json::json!({}))
            .await
    }

    /// `proxy_startup` tool; `port` overrides the configured listen port.
    pub async fn proxy_startup(&self, port: Option<u16>) -> Result<Value, ApiError> {
        let params = match port {
            Some(p) => serde_json::json!({ "port": p }),
            None => serde_json::json!({}),
        };
        self.call_tool("proxy_startup", &params).await
    }

    /// `proxy_policy_get` tool for a pane id / label.
    pub async fn proxy_policy_get(&self, target: &str) -> Result<Value, ApiError> {
        self.call_tool("proxy_policy_get", &serde_json::json!({ "target": target }))
            .await
    }

    /// `proxy_policy_set` tool.
    pub async fn proxy_policy_set(
        &self,
        target: &str,
        policy: &ProxyPolicy,
    ) -> Result<Value, ApiError> {
        self.call_tool(
            "proxy_policy_set",
            &serde_json::json!({
                "target": target,
                "trim_outbound": policy.trim_outbound,
                "trim_inbound": policy.trim_inbound,
                "stages": policy.stages,
            }),
        )
        .await
    }

    // ── recipes ────────────────────────────────────────────────────────

    /// `POST /api/recipe` — run an ad-hoc recipe.
    pub async fn run_recipe(&self, req: &RecipeRequest) -> Result<RecipeResponse, ApiError> {
        self.post(&["api", "recipe"], req).await
    }

    /// `GET /api/recipes`.
    pub async fn list_recipes(&self) -> Result<Vec<Recipe>, ApiError> {
        self.get(&["api", "recipes"]).await
    }

    /// `POST /api/recipes`.
    pub async fn create_recipe(&self, req: &CreateRecipeRequest) -> Result<Recipe, ApiError> {
        self.post(&["api", "recipes"], req).await
    }

    /// `GET /api/recipes/{id}`.
    pub async fn get_recipe(&self, id: Uuid) -> Result<Recipe, ApiError> {
        self.get(&["api", "recipes", &id.to_string()]).await
    }

    /// `PUT /api/recipes/{id}`.
    pub async fn update_recipe(
        &self,
        id: Uuid,
        req: &UpdateRecipeRequest,
    ) -> Result<Recipe, ApiError> {
        Self::json(
            self.request(Method::PUT, &["api", "recipes", &id.to_string()])
                .json(req),
        )
        .await
    }

    /// `DELETE /api/recipes/{id}` (204).
    pub async fn delete_recipe(&self, id: Uuid) -> Result<(), ApiError> {
        self.no_content(self.request(Method::DELETE, &["api", "recipes", &id.to_string()]))
            .await
    }

    /// `POST /api/recipes/{id}/run`.
    pub async fn run_recipe_by_id(&self, id: Uuid) -> Result<ExecutionResult, ApiError> {
        self.post(
            &["api", "recipes", &id.to_string(), "run"],
            &serde_json::json!({}),
        )
        .await
    }

    /// `GET /api/recipes/{id}/export` — bash script equivalent.
    pub async fn export_recipe(&self, id: Uuid) -> Result<String, ApiError> {
        self.get::<ExportScriptResponse>(&["api", "recipes", &id.to_string(), "export"])
            .await
            .map(|r| r.script)
    }

    /// `GET /api/templates` — bundled recipe templates.
    pub async fn list_templates(&self) -> Result<Vec<RecipeTemplate>, ApiError> {
        self.get(&["api", "templates"]).await
    }

    /// `GET /api/executions/{id}`.
    pub async fn get_execution(&self, id: Uuid) -> Result<ExecutionResult, ApiError> {
        self.get(&["api", "executions", &id.to_string()]).await
    }

    // ── runs ───────────────────────────────────────────────────────────

    /// `POST /api/runs` — start a run in the background; returns its first record.
    pub async fn start_run(&self, req: &StartRunRequest) -> Result<RunRecord, ApiError> {
        self.post(&["api", "runs"], req).await
    }

    /// `GET /api/runs` — every run, newest first.
    pub async fn list_runs(&self) -> Result<Vec<RunRecord>, ApiError> {
        self.get(&["api", "runs"]).await
    }

    /// `GET /api/runs/{id}`.
    pub async fn get_run(&self, id: Uuid) -> Result<RunRecord, ApiError> {
        self.get(&["api", "runs", &id.to_string()]).await
    }

    /// `POST /api/runs/{id}/{pause|resume|cancel}` (204). 409 if the run has
    /// already finished.
    pub async fn control_run(&self, id: Uuid, action: RunAction) -> Result<(), ApiError> {
        self.no_content(self.request(
            Method::POST,
            &["api", "runs", &id.to_string(), action.as_str()],
        ))
        .await
    }

    // ── variables / playground ─────────────────────────────────────────

    /// `GET /api/variables`.
    pub async fn list_variables(&self) -> Result<Vec<VariableStore>, ApiError> {
        self.get(&["api", "variables"]).await
    }

    /// `GET /api/variables/{key}`.
    pub async fn get_variable(&self, key: &str) -> Result<VariableStore, ApiError> {
        self.get(&["api", "variables", key]).await
    }

    /// `POST /api/variables` (201).
    pub async fn save_variable(&self, req: &SaveVariableRequest) -> Result<(), ApiError> {
        self.no_content(self.request(Method::POST, &["api", "variables"]).json(req))
            .await
    }

    /// `DELETE /api/variables/{key}` (204).
    pub async fn delete_variable(&self, key: &str) -> Result<(), ApiError> {
        self.no_content(self.request(Method::DELETE, &["api", "variables", key]))
            .await
    }

    /// `GET /api/variables/auto[?workspace_id=]` — registry + session vars.
    pub async fn auto_variables(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<AutoVariablesResponse, ApiError> {
        let req = Self::ws_query(
            self.request(Method::GET, &["api", "variables", "auto"]),
            workspace_id,
        );
        Self::json(req).await
    }

    /// `GET /api/playground?workspace_id=` — last-used playground values.
    pub async fn get_playground(&self, workspace_id: &str) -> Result<PlaygroundSnapshot, ApiError> {
        let req = Self::ws_query(
            self.request(Method::GET, &["api", "playground"]),
            Some(workspace_id),
        );
        Self::json(req).await
    }

    /// `POST /api/playground` (201).
    pub async fn save_playground(&self, snapshot: &PlaygroundSnapshot) -> Result<(), ApiError> {
        self.no_content(
            self.request(Method::POST, &["api", "playground"])
                .json(snapshot),
        )
        .await
    }

    // ── trim ───────────────────────────────────────────────────────────

    /// `GET /api/trim/status[?workspace_id=]` — aggregate savings.
    pub async fn trim_status(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<TrimStatusResponse, ApiError> {
        let req = Self::ws_query(
            self.request(Method::GET, &["api", "trim", "status"]),
            workspace_id,
        );
        Self::json(req).await
    }

    /// `POST /api/trim/diagnose` — end-to-end readiness check.
    pub async fn trim_diagnose(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<DiagnoseReport, ApiError> {
        self.post(&["api", "trim", "diagnose"], &Self::ws_body(workspace_id))
            .await
    }

    /// `POST /api/trim/summary` — fire a savings notification (tool result JSON).
    pub async fn trim_summary(&self, workspace_id: Option<&str>) -> Result<Value, ApiError> {
        self.post(&["api", "trim", "summary"], &Self::ws_body(workspace_id))
            .await
    }

    /// `POST /api/trim/dashboard/open` — open the dashboard pane (tool result JSON).
    pub async fn trim_dashboard_open(&self, workspace_id: Option<&str>) -> Result<Value, ApiError> {
        self.post(
            &["api", "trim", "dashboard", "open"],
            &Self::ws_body(workspace_id),
        )
        .await
    }
}

/// Concatenated text of an MCP `CallToolResult` JSON: every `content[].text`
/// joined by newlines; a bare JSON string is returned as-is; else empty.
pub fn tool_text(v: &Value) -> String {
    let texts: Vec<&str> = v
        .get("content")
        .and_then(|c| c.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
                .collect()
        })
        .unwrap_or_default();
    if !texts.is_empty() {
        return texts.join("\n");
    }
    v.as_str().map(str::to_string).unwrap_or_default()
}

/// Map a tool result with `is_error: true` to [`ApiError::Tool`].
fn tool_ok(v: &Value) -> Result<&Value, ApiError> {
    if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
        let text = tool_text(v);
        return Err(ApiError::Tool(if text.is_empty() {
            "tool reported is_error".into()
        } else {
            text
        }));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn rejects_relative_base() {
        assert!(matches!(ApiClient::new("/api"), Err(ApiError::BadBase(_))));
    }

    #[test]
    fn builds_urls_with_and_without_trailing_slash() {
        for base in ["http://127.0.0.1:7676", "http://127.0.0.1:7676/"] {
            let c = ApiClient::new(base).unwrap();
            assert_eq!(
                c.url(&["api", "health"]).as_str(),
                "http://127.0.0.1:7676/api/health"
            );
        }
    }

    #[test]
    fn keeps_base_path_prefix() {
        let c = ApiClient::new("http://host:1/prefix/").unwrap();
        assert_eq!(
            c.url(&["api", "tools"]).as_str(),
            "http://host:1/prefix/api/tools"
        );
    }

    #[test]
    fn tool_text_joins_content_items() {
        let v = serde_json::json!({
            "content": [
                { "type": "text", "text": "line one" },
                { "type": "image", "data": "x" },
                { "type": "text", "text": "line two" }
            ],
            "is_error": false
        });
        assert_eq!(tool_text(&v), "line one\nline two");
        assert_eq!(tool_text(&serde_json::json!("bare")), "bare");
        assert_eq!(tool_text(&serde_json::json!({ "content": [] })), "");
    }

    #[test]
    fn tool_ok_surfaces_is_error_text() {
        let bad = serde_json::json!({ "content": [{ "type": "text", "text": "no backend" }], "is_error": true });
        assert!(matches!(tool_ok(&bad), Err(ApiError::Tool(t)) if t == "no backend"));
        let good = serde_json::json!({ "content": [], "is_error": false });
        assert!(tool_ok(&good).is_ok());
    }

    #[test]
    fn percent_encodes_path_segments() {
        let c = ApiClient::new("http://h:1").unwrap();
        assert_eq!(
            c.url(&["api", "variables", "a b/c"]).as_str(),
            "http://h:1/api/variables/a%20b%2Fc"
        );
    }
}

//! Typed async client over the herdr-mcp HTTP bridge. One client for the web
//! UI (wasm) and the TUI (native). Holds only the base URL — every call
//! resolves from the server (SSoT: registry + `data/` are canonical).

use reqwest::{Method, RequestBuilder, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use uuid::Uuid;

use crate::*;

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
    /// The tool ran but reported `isError`; carries its text.
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
    /// Sent as `Authorization: Bearer` when the bridge wants its token.
    token: Option<String>,
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
        Ok(Self {
            base,
            http,
            token: None,
        })
    }

    /// This client, sending `token` (the bridge token) with every request.
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token.filter(|t| !t.trim().is_empty());
        self
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
        let req = self.http.request(method, self.url(segments));
        match &self.token {
            Some(token) => req.bearer_auth(token),
            None => req,
        }
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

    // ── discovery ──────────────────────────────────────────────────────

    /// `GET /api/health` — `Ok` when the bridge answers.
    pub async fn health(&self) -> Result<(), ApiError> {
        Self::send(self.request(Method::GET, &["api", "health"]))
            .await
            .map(|_| ())
    }

    /// `GET /api/config` — the bridge's runtime config.
    pub async fn config(&self) -> Result<ConfigResponse, ApiError> {
        self.get(&["api", "config"]).await
    }

    /// `PUT /api/config/clipboard` — save clipboard commands to the config
    /// file and apply them to the running server. Both must be non-empty.
    pub async fn set_clipboard_config(
        &self,
        cfg: &ClipboardConfig,
    ) -> Result<ConfigResponse, ApiError> {
        Self::json(
            self.request(Method::PUT, &["api", "config", "clipboard"])
                .json(cfg),
        )
        .await
    }

    /// `GET /api/settings` — every tunable (timeouts, limits, TUI keys)
    /// with its default, bounds and value in effect.
    pub async fn settings(&self) -> Result<SettingsResponse, ApiError> {
        self.get(&["api", "settings"]).await
    }

    /// `PUT /api/settings/{key}` — validate, save to the config file and
    /// apply. A bad value is a 400 [`ApiError::Status`] naming the problem.
    pub async fn set_setting(
        &self,
        key: &str,
        value: &crate::settings::Value,
    ) -> Result<SettingsResponse, ApiError> {
        Self::json(
            self.request(Method::PUT, &["api", "settings", key])
                .json(value),
        )
        .await
    }

    /// `DELETE /api/settings/{key}` — back to the built-in default.
    pub async fn reset_setting(&self, key: &str) -> Result<SettingsResponse, ApiError> {
        Self::json(self.request(Method::DELETE, &["api", "settings", key])).await
    }

    /// `DELETE /api/settings` — every setting back to its default.
    pub async fn reset_settings(&self) -> Result<SettingsResponse, ApiError> {
        Self::json(self.request(Method::DELETE, &["api", "settings"])).await
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

    /// `GET /api/herdr/sessions` — every named herdr session.
    pub async fn list_sessions(&self) -> Result<Vec<HerdrSession>, ApiError> {
        self.get::<SessionsResponse>(&["api", "herdr", "sessions"])
            .await
            .map(|r| r.sessions)
    }

    /// `GET /api/herdr/panes` — herdr's panes, narrowed by `filter`.
    pub async fn list_panes(&self, filter: &PaneFilter) -> Result<Vec<HerdrPane>, ApiError> {
        let req = self
            .request(Method::GET, &["api", "herdr", "panes"])
            .query(filter);
        Self::json::<PanesResponse>(req).await.map(|r| r.panes)
    }

    /// `GET /api/workspaces` — passthrough of `herdr workspace list` (JSON or null).
    pub async fn list_workspaces(&self) -> Result<Value, ApiError> {
        self.get(&["api", "workspaces"]).await
    }

    /// `clipboard_set` tool — write `text` to the server's clipboard.
    /// A tool-level failure (`isError`) is returned as an error.
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

    /// `read_pane` tool — a pane's text from `source` (`visible`, `recent`
    /// or `recent-unwrapped`), in `session` or this bridge's own.
    pub async fn read_pane(
        &self,
        pane_id: &str,
        session: Option<&str>,
        source: &str,
        lines: Option<u32>,
    ) -> Result<String, ApiError> {
        let v = self
            .call_tool(
                "read_pane",
                &serde_json::json!({
                    "pane_id": pane_id, "session": session, "source": source, "lines": lines
                }),
            )
            .await?;
        tool_ok(&v).map(tool_text)
    }

    /// Type `text` into a pane: `send_text`, or `run_command` (text + Enter)
    /// when `enter`. The pane is addressed like the tools address it:
    /// `pane_id` and/or `label`, `pane_id` winning when both are set.
    pub async fn send_to_pane(
        &self,
        pane_id: Option<&str>,
        label: Option<&str>,
        text: &str,
        enter: bool,
    ) -> Result<(), ApiError> {
        let (tool, params) = send_call(pane_id, label, text, enter);
        let v = self.call_tool(tool, &params).await?;
        tool_ok(&v).map(|_| ())
    }

    // ── recipes ────────────────────────────────────────────────────────

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

    /// `GET /api/templates` — bundled recipe templates.
    pub async fn list_templates(&self) -> Result<Vec<RecipeTemplate>, ApiError> {
        self.get(&["api", "templates"]).await
    }

    /// `GET /api/herdr/events` — herdr's live event stream (SSE), opened:
    /// read it with `chunk()`, one `data: {HerdrEvent}` line per event. Use
    /// a client without a request timeout, which would end the stream.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn herdr_events(&self) -> Result<reqwest::Response, ApiError> {
        let resp = self
            .request(Method::GET, &["api", "herdr", "events"])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ApiError::Status {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        Ok(resp)
    }

    /// `GET /api/herdr/schema` — the installed herdr's socket API schema.
    pub async fn herdr_schema(&self) -> Result<Value, ApiError> {
        self.get(&["api", "herdr", "schema"]).await
    }

    /// `POST /api/templates/{id}/instantiate` — the template with its
    /// variables filled in, as a recipe (not saved).
    pub async fn instantiate_template(
        &self,
        id: &str,
        req: &InstantiateTemplateRequest,
    ) -> Result<Recipe, ApiError> {
        self.post(&["api", "templates", id, "instantiate"], req)
            .await
    }

    // ── agent profiles ─────────────────────────────────────────────────

    /// `GET /api/profiles`.
    pub async fn list_profiles(&self) -> Result<Vec<AgentProfile>, ApiError> {
        self.get(&["api", "profiles"]).await
    }

    /// `GET /api/profiles/{id}`.
    pub async fn get_profile(&self, id: &str) -> Result<AgentProfile, ApiError> {
        self.get(&["api", "profiles", id]).await
    }

    /// `PUT /api/profiles/{id}` — create or override (400 if invalid).
    pub async fn save_profile(&self, profile: &AgentProfile) -> Result<AgentProfile, ApiError> {
        Self::json(
            self.request(Method::PUT, &["api", "profiles", &profile.id])
                .json(profile),
        )
        .await
    }

    /// `DELETE /api/profiles/{id}` (204) — drop a user profile.
    pub async fn delete_profile(&self, id: &str) -> Result<(), ApiError> {
        self.no_content(self.request(Method::DELETE, &["api", "profiles", id]))
            .await
    }

    // ── runs ───────────────────────────────────────────────────────────

    /// `POST /api/runs` — start a run in the background; returns its first record.
    pub async fn start_run(&self, req: &StartRunRequest) -> Result<RunRecord, ApiError> {
        self.post(&["api", "runs"], req).await
    }

    /// `POST /api/runs?wait=true` — run and wait for the final record.
    pub async fn run_and_wait(&self, req: &StartRunRequest) -> Result<RunRecord, ApiError> {
        Self::json(
            self.request(Method::POST, &["api", "runs"])
                .query(&[("wait", "true")])
                .json(req),
        )
        .await
    }

    /// `GET /api/runs` — every run, newest first.
    pub async fn list_runs(&self) -> Result<Vec<RunRecord>, ApiError> {
        self.get(&["api", "runs"]).await
    }

    /// `GET /api/runs/{id}`.
    pub async fn get_run(&self, id: Uuid) -> Result<RunRecord, ApiError> {
        self.get(&["api", "runs", &id.to_string()]).await
    }

    /// `GET /api/runs/{id}/events?since=` — the run's events from `since`
    /// (inclusive). For live updates use the SSE stream `GET /api/events`.
    pub async fn run_events(&self, id: Uuid, since: u64) -> Result<Vec<RunEvent>, ApiError> {
        Self::json(
            self.request(Method::GET, &["api", "runs", &id.to_string(), "events"])
                .query(&[("since", since)]),
        )
        .await
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

    // ── variables ─────────────────────────────────────────

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

    /// `POST /api/variables/resolve` — `{{ }}` placeholders resolved the way
    /// a run would resolve them.
    pub async fn resolve(&self, req: &ResolveRequest) -> Result<ResolveResponse, ApiError> {
        self.post(&["api", "variables", "resolve"], req).await
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

/// A property's JSON type. Optional fields are `["integer", "null"]` in the
/// schemas rmcp/schemars generate; the non-null member is the type.
pub fn schema_type(prop: &Value) -> &str {
    match prop.get("type") {
        Some(Value::String(t)) => t,
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .unwrap_or("string"),
        _ => "string",
    }
}

/// The JSON a tool returned as text (its first text content parsed), if any.
pub fn tool_payload(v: &Value) -> Option<Value> {
    v.pointer("/content/0/text")
        .and_then(Value::as_str)
        .and_then(|t| serde_json::from_str(t).ok())
}

/// True when an MCP `CallToolResult` JSON reports a tool-level failure
/// (`"isError": true`; rmcp serializes the field camelCase).
pub fn tool_is_error(v: &Value) -> bool {
    v.get("isError").and_then(Value::as_bool).unwrap_or(false)
}

/// The tool call [`ApiClient::send_to_pane`] makes.
fn send_call(
    pane_id: Option<&str>,
    label: Option<&str>,
    text: &str,
    enter: bool,
) -> (&'static str, Value) {
    let (tool, key) = if enter {
        ("run_command", "command")
    } else {
        ("send_text", "text")
    };
    let mut params = serde_json::Map::new();
    if let Some(id) = pane_id {
        params.insert("pane_id".into(), id.into());
    }
    if let Some(l) = label {
        params.insert("label".into(), l.into());
    }
    params.insert(key.into(), text.into());
    (tool, Value::Object(params))
}

/// Map a tool result with `isError: true` to [`ApiError::Tool`].
fn tool_ok(v: &Value) -> Result<&Value, ApiError> {
    if tool_is_error(v) {
        let text = tool_text(v);
        return Err(ApiError::Tool(if text.is_empty() {
            "tool reported isError".into()
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
    fn send_call_picks_tool() {
        assert_eq!(
            send_call(Some("w1:p2"), None, "hi", false),
            (
                "send_text",
                serde_json::json!({"pane_id": "w1:p2", "text": "hi"})
            )
        );
        assert_eq!(
            send_call(None, Some("builder"), "make", true),
            (
                "run_command",
                serde_json::json!({"label": "builder", "command": "make"})
            )
        );
    }

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
            "isError": false
        });
        assert_eq!(tool_text(&v), "line one\nline two");
        assert_eq!(tool_text(&serde_json::json!("bare")), "bare");
        assert_eq!(tool_text(&serde_json::json!({ "content": [] })), "");
    }

    #[test]
    fn tool_ok_surfaces_is_error_text() {
        let bad = serde_json::json!({ "content": [{ "type": "text", "text": "no backend" }], "isError": true });
        assert!(matches!(tool_ok(&bad), Err(ApiError::Tool(t)) if t == "no backend"));
        let good = serde_json::json!({ "content": [], "isError": false });
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

//! The HTTP bridge: router, middleware and `/api/*` handlers.

use super::*;

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
use tower_http::services::{ServeDir, ServeFile};

/// Shared state for the Axum HTTP router.
#[derive(Clone)]
pub(super) struct AppState {
    server: HerdrMcpServer,
}

/// The host of a `host[:port]` authority, lowercased: a bracketed IPv6
/// literal without its brackets, `None` if the brackets don't hold one.
pub(super) fn authority_host(authority: &str) -> Option<String> {
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

pub(super) fn is_localhost(name: &str) -> bool {
    name == "localhost" || name.ends_with(".localhost")
}

/// Whether a request's `Host` may reach the bridge: `localhost`,
/// `*.localhost`, an IP literal, or the name the bridge is bound to. Any
/// other name is how a DNS-rebinding page would reach this port from a
/// browser, and the bridge can type into panes.
pub(super) fn host_allowed(host: &str, bind: &str) -> bool {
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
pub(super) fn origin_allowed(origin: &str, bind: &str) -> bool {
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
pub(super) fn http_router(
    server: HerdrMcpServer,
    web_dir: Option<&std::path::Path>,
    bind: &str,
) -> Router {
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
pub(super) fn token_refusal(
    req: &axum::extract::Request,
    token: &str,
) -> Option<(StatusCode, String)> {
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

pub(super) async fn health_handler() -> &'static str {
    "ok"
}

pub(super) fn config_response(server: &HerdrMcpServer) -> ConfigResponse {
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

pub(super) fn settings_response(server: &HerdrMcpServer) -> herdr_mcp_api::SettingsResponse {
    herdr_mcp_api::SettingsResponse::new(
        &server.settings(),
        server.config_path.display().to_string(),
        server.settings_warnings.to_vec(),
    )
}

/// `GET /api/settings` — every tunable with its default, bounds and value.
pub(super) async fn settings_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Json<herdr_mcp_api::SettingsResponse> {
    Json(settings_response(&server))
}

/// `PUT /api/settings/{key}` — validate, write to the config file, apply.
/// 400 (nothing written) for a value out of range, a chord that types text,
/// is reserved or already bound, or an unknown key.
pub(super) async fn save_setting_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(key): Path<String>,
    Json(value): Json<settings::Value>,
) -> Result<Json<herdr_mcp_api::SettingsResponse>, (StatusCode, String)> {
    write_setting(&server, key, Some(value)).await
}

/// `DELETE /api/settings/{key}` — back to the built-in default.
pub(super) async fn reset_setting_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<herdr_mcp_api::SettingsResponse>, (StatusCode, String)> {
    write_setting(&server, key, None).await
}

pub(super) async fn write_setting(
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
pub(super) async fn reset_settings_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Result<Json<herdr_mcp_api::SettingsResponse>, (StatusCode, String)> {
    server
        .reset_settings()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    Ok(Json(settings_response(&server)))
}

pub(super) async fn config_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Json<ConfigResponse> {
    Json(config_response(&server))
}

/// `PUT /api/config/clipboard` — persist clipboard commands and apply them
/// live. Both empty switches back to the built-in OS clipboard.
pub(super) async fn set_clipboard_config_handler(
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

pub(super) async fn list_tools_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Json<ToolsResponse> {
    let tools = server
        .enabled_tools()
        .iter()
        .filter_map(|t| serde_json::to_value(t).ok())
        .collect();
    Json(ToolsResponse { tools })
}

pub(super) async fn call_tool_handler(
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
pub(super) async fn agents_handler(
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
pub(super) async fn workspaces_handler() -> Json<serde_json::Value> {
    let raw = herdr_cli(&["workspace", "list"])
        .await
        .map(|s| serde_json::from_str::<serde_json::Value>(&s).unwrap_or(serde_json::json!(null)))
        .unwrap_or(serde_json::json!(null));
    Json(raw)
}

pub(super) fn mcp_err_to_http(e: McpError) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.message.to_string())
}

pub(super) fn bad_request(e: impl ToString) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}

pub(super) fn internal(e: impl ToString) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub(super) fn start_error_to_http(e: crate::runs::StartError) -> (StatusCode, String) {
    let code = match e {
        crate::runs::StartError::RecipeNotFound => StatusCode::NOT_FOUND,
        crate::runs::StartError::UnknownStep(_) => StatusCode::BAD_REQUEST,
        crate::runs::StartError::Persist(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, e.to_string())
}

// ── Recipe CRUD API ────────────────────────────────────────────────────

pub(super) async fn list_templates_handler()
-> Result<Json<Vec<tmpl::RecipeTemplate>>, (StatusCode, String)> {
    Ok(Json(tmpl::list_templates()))
}

/// `GET /api/herdr/sessions` — every named herdr session.
pub(super) async fn herdr_sessions_handler()
-> Result<Json<herdr_mcp_api::SessionsResponse>, (StatusCode, String)> {
    let sessions = crate::panes::sessions().await.map_err(bad_gateway)?;
    Ok(Json(herdr_mcp_api::SessionsResponse { sessions }))
}

/// `GET /api/herdr/panes?session=&workspace_id=&tab_id=&agent_status=&focused=&label=`
/// — herdr's panes, narrowed like `list_panes`.
pub(super) async fn herdr_panes_handler(
    Query(filter): Query<herdr_mcp_api::PaneFilter>,
) -> Result<Json<herdr_mcp_api::PanesResponse>, (StatusCode, String)> {
    let panes = crate::panes::panes(&filter).await.map_err(bad_gateway)?;
    Ok(Json(herdr_mcp_api::PanesResponse { panes }))
}

pub(super) fn bad_gateway(e: McpError) -> (StatusCode, String) {
    (StatusCode::BAD_GATEWAY, e.message.to_string())
}

/// `GET /api/herdr/schema` — the installed herdr's API schema
/// (`herdr api schema --json`): every socket method and its params.
pub(super) async fn herdr_schema_handler() -> Result<Json<serde_json::Value>, (StatusCode, String)>
{
    let text = herdr_cli(&["api", "schema", "--json"])
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.message.to_string()))?;
    serde_json::from_str(&text)
        .map(Json)
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("herdr api schema: {e}")))
}

/// `POST /api/templates/{id}/instantiate` — fill in a template (not saved).
pub(super) async fn instantiate_template_handler(
    Path(id): Path<String>,
    Json(req): Json<herdr_mcp_api::InstantiateTemplateRequest>,
) -> Result<Json<Recipe>, (StatusCode, String)> {
    tmpl::instantiate(&id, req.variables, req.name)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, format!("template not found: {id}")))
}

pub(super) async fn list_recipes_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Result<Json<Vec<Recipe>>, (StatusCode, String)> {
    server
        .persistence
        .list_recipes(None)
        .await
        .map(Json)
        .map_err(internal)
}

pub(super) async fn create_recipe_handler(
    State(AppState { server, .. }): State<AppState>,
    Json(req): Json<CreateRecipeRequest>,
) -> Result<Json<Recipe>, (StatusCode, String)> {
    server.create_recipe(req).await.map(Json).map_err(internal)
}

pub(super) async fn get_recipe_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Recipe>, (StatusCode, String)> {
    server
        .persistence
        .load_recipe(&id)
        .await
        .map_err(internal)?
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "Recipe not found".into()))
}

pub(super) async fn update_recipe_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<UpdateRecipeRequest>,
) -> Result<Json<Recipe>, (StatusCode, String)> {
    server
        .update_recipe(id, req)
        .await
        .map_err(internal)?
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "Recipe not found".to_string()))
}

pub(super) async fn delete_recipe_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    server
        .persistence
        .delete_recipe(&id)
        .await
        .map_err(internal)?
        .then_some(StatusCode::NO_CONTENT)
        .ok_or((StatusCode::NOT_FOUND, "Recipe not found".into()))
}

#[derive(Debug, Deserialize)]
pub(super) struct WaitQuery {
    #[serde(default)]
    wait: bool,
}

/// `POST /api/runs` — start a run in the background (202 + first record);
/// with `?wait=true`, wait for it to finish (200 + final record).
pub(super) async fn start_run_handler(
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
pub(super) struct ListRunsQuery {
    limit: Option<usize>,
}

/// `GET /api/runs?limit=` — the newest runs (every one without `limit`).
pub(super) async fn list_runs_handler(
    State(AppState { server, .. }): State<AppState>,
    Query(q): Query<ListRunsQuery>,
) -> Result<Json<Vec<herdr_mcp_api::RunRecord>>, (StatusCode, String)> {
    crate::runs::list(&server, q.limit)
        .await
        .map(Json)
        .map_err(internal)
}

pub(super) async fn load_run(
    server: &HerdrMcpServer,
    id: uuid::Uuid,
) -> Result<herdr_mcp_api::RunRecord, (StatusCode, String)> {
    crate::runs::load(server, id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "Run not found".into()))
}

#[derive(Debug, Deserialize)]
pub(super) struct SinceQuery {
    #[serde(default)]
    since: u64,
}

/// `GET /api/runs/{id}/events?since=N` — the run's events from `since`.
pub(super) async fn run_events_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
    Query(q): Query<SinceQuery>,
) -> Result<Json<Vec<herdr_mcp_api::RunEvent>>, (StatusCode, String)> {
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
pub(super) async fn list_profiles_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Json<Vec<herdr_mcp_api::AgentProfile>> {
    Json(crate::profiles::list(server.persistence.data_dir()).await)
}

pub(super) async fn get_profile_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<herdr_mcp_api::AgentProfile>, (StatusCode, String)> {
    crate::profiles::get(server.persistence.data_dir(), &id)
        .await
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, format!("no profile {id:?}")))
}

/// `PUT /api/profiles/{id}` — create or override a profile (400 if invalid).
pub(super) async fn save_profile_handler(
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
        Err(crate::profiles::SaveError::Io(e)) => Err(internal(e)),
    }
}

/// `DELETE /api/profiles/{id}` — remove a user profile (a bundled one with
/// the same id comes back). 404 if there is no user file.
pub(super) async fn delete_profile_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    match crate::profiles::delete(server.persistence.data_dir(), &id).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err((StatusCode::NOT_FOUND, format!("no user profile {id:?}"))),
        Err(e) => Err(internal(e)),
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct EventsQuery {
    run: Option<uuid::Uuid>,
    since: Option<u64>,
}

/// `GET /api/events?run={id}&since={seq}` — Server-Sent Events. With `run`,
/// replays that run's log from `since` (or from after the `Last-Event-ID`
/// header, which browsers send on reconnect) and then follows it live;
/// without `run`, every run's new events. Event ids are run `seq`s.
pub(super) async fn events_handler(
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

#[derive(Debug, Deserialize)]
pub(super) struct HerdrEventsQuery {
    #[serde(default)]
    level: herdr_mcp_api::EventLevel,
}

/// `GET /api/herdr/events?level=events|reactions|debug` — Server-Sent
/// Events (`HerdrEvent` JSON per message): the recent entries up to
/// `level` (the last 200 of each, oldest first), then each new one. `events`
/// (the default) is herdr's own stream; the higher levels add herdr-mcp's
/// diagnostic entries. Without a herdr subscriber attached the stream stays
/// empty.
pub(super) async fn herdr_events_handler(
    State(AppState { server, .. }): State<AppState>,
    Query(HerdrEventsQuery { level }): Query<HerdrEventsQuery>,
) -> axum::response::sse::Sse<
    impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use herdr_mcp_api::{EventLevel, HerdrEvent};
    let feed = server.herdr_client.get().map(|c| c.recent_and_live(level));
    let sse = |event: &HerdrEvent| {
        Event::default()
            .json_data(event)
            .unwrap_or_else(|_| Event::default().comment("unencodable event"))
    };
    let stream = async_stream::stream! {
        let Some((recent, mut rx)) = feed else { return };
        for event in &recent {
            yield Ok(sse(event));
        }
        loop {
            match rx.recv().await {
                Ok(event) if event.level <= level => yield Ok(sse(&event)),
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    yield Ok(if level >= EventLevel::Reactions {
                        let mut lagged = HerdrEvent::diagnostic(
                            EventLevel::Reactions,
                            "lagged",
                            serde_json::json!({ "listener": "this stream", "skipped": n }),
                        );
                        lagged.at = Some(chrono::Utc::now());
                        sse(&lagged)
                    } else {
                        Event::default().comment(format!("skipped {n} events"))
                    });
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `GET /api/runs/{id}`.
pub(super) async fn get_run_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<herdr_mcp_api::RunRecord>, (StatusCode, String)> {
    load_run(&server, id).await.map(Json)
}

/// `GET /api/runs/{id}/trail` — the run's trail as text, the trail of the
/// child run its gate handed off to indented under it.
pub(super) async fn run_trail_handler(
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
pub(super) async fn control_run_handler(
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
pub(super) async fn list_variables_handler(
    State(AppState { server, .. }): State<AppState>,
) -> Result<Json<Vec<crate::persistence::VariableStore>>, (StatusCode, String)> {
    server
        .persistence
        .load_variables(None, None)
        .await
        .map(Json)
        .map_err(internal)
}

/// `GET /api/variables/auto[?workspace_id=w1]` — variables resolved on
/// request, never stored: every live herdr pane under its pane id (from
/// `herdr pane list`), with its agent registry handle's non-empty fields laid
/// over it (and the merged pane also under the handle's role), then the
/// workspace's `var_set` session variables when one is given.
pub(super) async fn auto_variables_handler(
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
pub(super) async fn resolve_handler(
    State(AppState { server, .. }): State<AppState>,
    Json(req): Json<ResolveRequest>,
) -> Result<Json<ResolveResponse>, (StatusCode, String)> {
    let collection = match req.recipe_id {
        Some(id) => {
            server
                .persistence
                .load_recipe(&id)
                .await
                .map_err(internal)?
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
pub(super) fn placeholders(value: &serde_json::Value, out: &mut Vec<String>) {
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
pub(super) fn pane_vars(
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

pub(super) async fn save_variable_handler(
    State(AppState { server, .. }): State<AppState>,
    Json(req): Json<SaveVariableRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if req.key.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "key must not be empty".into()));
    }
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
        .map_err(internal)
}

pub(super) async fn get_variable_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<crate::persistence::VariableStore>, (StatusCode, String)> {
    let vars = server
        .persistence
        .load_variables(None, None)
        .await
        .map_err(internal)?;

    vars.into_iter()
        .find(|v| v.key == key)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "Variable not found".into()))
}

pub(super) async fn delete_variable_handler(
    State(AppState { server, .. }): State<AppState>,
    Path(key): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
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
        .map_err(internal)
}

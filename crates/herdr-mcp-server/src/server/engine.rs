//! What the run engine and the bridge share: calling a tool through the
//! in-process MCP session and resolving `{{ }}` placeholders.

use super::*;
use axum::http::StatusCode;
use std::collections::HashMap;

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
        e => internal(e),
    })
}

/// A `{{ path }}` placeholder.
pub(crate) static VARIABLE_RE: std::sync::LazyLock<Regex> =
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
pub(crate) fn json_value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

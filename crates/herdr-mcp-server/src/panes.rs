//! herdr's panes and named sessions, fetched from herdr on every call —
//! herdr is their only source of truth. Every caller that needs a pane list
//! (label lookup, agent placement, the dashboard, auto variables, the event
//! subscriber, `list_panes`, `/api/herdr/panes`) goes through [`panes`].

use herdr_mcp_api::{HerdrPane, HerdrSession, PaneFilter, PanesResponse, SessionsResponse};
use rmcp::ErrorData as McpError;
use serde::Deserialize;

use crate::server::{herdr_cli, herdr_cli_at, to_mcp_err, user_err};

/// Every named herdr session (`herdr session list --json`).
/// `current` marks the one whose socket is this server's (`HERDR_SOCKET_PATH`,
/// exported at startup and used by every `herdr` call without a session).
pub async fn sessions() -> Result<Vec<HerdrSession>, McpError> {
    let raw = herdr_cli(&["session", "list", "--json"]).await?;
    let mut sessions = serde_json::from_str::<SessionsResponse>(&raw)
        .map(|r| r.sessions)
        .map_err(|e| to_mcp_err(format!("herdr session list: {e}")))?;
    if let Ok(own) = std::env::var("HERDR_SOCKET_PATH") {
        mark_current(&mut sessions, &own);
    }
    Ok(sessions)
}

fn mark_current(sessions: &mut [HerdrSession], own: &str) {
    let canon = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| p.into());
    let own = canon(own);
    for s in sessions {
        s.current = canon(&s.socket_path) == own;
    }
}

/// The API socket of `session`; `None` (no session named) means this
/// server's own. Fails for an unknown or stopped session.
async fn socket_of(session: Option<&str>) -> Result<Option<String>, McpError> {
    let Some(name) = session.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    pick_socket(&sessions().await?, name)
        .map(Some)
        .map_err(user_err)
}

fn pick_socket(sessions: &[HerdrSession], name: &str) -> Result<String, String> {
    match sessions.iter().find(|s| s.name == name) {
        Some(s) if s.running => Ok(s.socket_path.clone()),
        Some(_) => Err(format!("herdr session '{name}' is not running")),
        None => {
            let known: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
            Err(format!(
                "no herdr session '{name}' (sessions: {})",
                known.join(", ")
            ))
        }
    }
}

/// Run herdr CLI `args` in `session` (`None`: this server's session).
pub async fn herdr_in(session: Option<&str>, args: &[&str]) -> Result<String, McpError> {
    let socket = socket_of(session).await?;
    herdr_cli_at(socket.as_deref(), args).await
}

/// herdr's panes narrowed by `filter`, in herdr's order.
pub async fn panes(filter: &PaneFilter) -> Result<Vec<HerdrPane>, McpError> {
    let mut args = vec!["pane", "list"];
    if let Some(ws) = filter.workspace_id.as_deref().filter(|w| !w.is_empty()) {
        args.extend(["--workspace", ws]);
    }
    let raw = herdr_in(filter.session.as_deref(), &args).await?;
    let panes = parse_panes(&raw).map_err(to_mcp_err)?;
    Ok(panes.into_iter().filter(|p| filter.matches(p)).collect())
}

/// `herdr pane list` stdout → panes (`{"result": {"panes": [...]}}`).
fn parse_panes(raw: &str) -> Result<Vec<HerdrPane>, String> {
    #[derive(Deserialize)]
    struct Envelope {
        result: PanesResponse,
    }
    serde_json::from_str::<Envelope>(raw)
        .map(|e| e.result.panes)
        .map_err(|e| format!("herdr pane list: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_herdr_pane_list() {
        let raw = r#"{"id":"cli:pane:list","result":{"panes":[
            {"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","focused":true,
             "agent_status":"unknown","terminal_id":"t","revision":5,"label":null},
            {"pane_id":"w1:p2","workspace_id":"w1","tab_id":"w1:t2","focused":false,
             "agent_status":"idle","agent":"claude","revision":1}]}}"#;
        let panes = parse_panes(raw).unwrap();
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].label, None);
        assert_eq!(panes[1].agent.as_deref(), Some("claude"));
        assert!(parse_panes(r#"{"error":{"code":"x"}}"#).is_err());
    }

    #[test]
    fn picks_running_session_socket() {
        let s = |name: &str, running: bool| HerdrSession {
            name: name.into(),
            default: name == "default",
            running,
            connection_error: None,
            socket_path: format!("/s/{name}.sock"),
            session_dir: format!("/s/{name}"),
            current: false,
        };
        let mut all = [s("default", true), s("work", true), s("old", false)];
        mark_current(&mut all, "/s/work.sock");
        assert_eq!(all.each_ref().map(|s| s.current), [false, true, false]);
        assert_eq!(pick_socket(&all, "work").unwrap(), "/s/work.sock");
        assert!(
            pick_socket(&all, "old")
                .unwrap_err()
                .contains("not running")
        );
        let err = pick_socket(&all, "nope").unwrap_err();
        assert!(err.contains("default, work, old"), "{err}");
    }
}

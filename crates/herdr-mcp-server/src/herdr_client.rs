//! Live agent registry + herdr event subscriber.
//!
//! In herdr, a pane *is* an agent. herdr detects agents and tracks their
//! lifecycle state, and emits `pane.agent_status_changed` events over its
//! local socket. This module keeps a live `pane_id -> agent` registry fed by
//! that event stream, and exposes it so recipe steps and MCP tools can address
//! agents by role or pane id and pass one agent's output (work product) to the
//! next — the a2a primitive.
//!
//! Transport mirrors llmtrim-herdr's `herdr-rpc`: one newline-terminated JSON
//! request per write, newline-delimited JSON replies. We open a dedicated
//! long-lived socket for the `events.subscribe` stream; one-shot actions go
//! through the `herdr` CLI (see `server::herdr_cli`).

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};

use crate::persistence::Persistence;
use herdr_mcp_api::HerdrEvent;
use herdr_mcp_trim::policy::TrimPolicy;

/// Agent lifecycle state, mirroring herdr's `AgentStatus` (snake_case).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Idle => "idle",
            AgentStatus::Working => "working",
            AgentStatus::Blocked => "blocked",
            AgentStatus::Done => "done",
            AgentStatus::Unknown => "unknown",
        }
    }

    fn parse(s: &str) -> Option<AgentStatus> {
        match s {
            "idle" => Some(AgentStatus::Idle),
            "working" => Some(AgentStatus::Working),
            "blocked" => Some(AgentStatus::Blocked),
            "done" => Some(AgentStatus::Done),
            "unknown" => Some(AgentStatus::Unknown),
            _ => None,
        }
    }
}

/// Wire type, defined once in `herdr-mcp-api`.
pub use herdr_mcp_api::AgentHandle;

/// Thread-safe registry of agents, keyed by herdr pane id.
#[derive(Clone)]
pub struct AgentRegistry {
    inner: Arc<RwLock<HashMap<String, AgentHandle>>>,
    persistence: Arc<Persistence>,
}

impl AgentRegistry {
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            persistence,
        }
    }

    /// Data directory backing persistence (stats, blobs, recipes).
    pub fn data_dir(&self) -> &std::path::Path {
        self.persistence.data_dir()
    }

    /// Register or update an agent handle at the given pane, preserving any
    /// previously captured output/role.
    pub async fn upsert(
        &self,
        pane_id: &str,
        workspace_id: &str,
        tab_id: &str,
        agent: &str,
        role: &str,
    ) {
        let now = chrono::Utc::now().timestamp();
        let mut map = self.inner.write().await;
        let mut handle = map.remove(pane_id).unwrap_or_else(|| AgentHandle {
            pane_id: pane_id.to_string(),
            workspace_id: workspace_id.to_string(),
            tab_id: String::new(),
            agent: String::new(),
            role: String::new(),
            label: String::new(),
            status: String::new(),
            output: String::new(),
            trim_policy: None,
            updated_at: now,
        });
        handle.pane_id = pane_id.to_string();
        handle.workspace_id = workspace_id.to_string();
        if !tab_id.is_empty() {
            handle.tab_id = tab_id.to_string();
        }
        if !agent.is_empty() {
            handle.agent = agent.to_string();
        }
        if !role.is_empty() {
            handle.role = role.to_string();
        }
        handle.updated_at = now;
        map.insert(pane_id.to_string(), handle);
        drop(map);
        self.persist(workspace_id).await;
    }

    pub async fn set_status(&self, pane_id: &str, status: &str) {
        let mut map = self.inner.write().await;
        if let Some(h) = map.get_mut(pane_id) {
            h.status = status.to_string();
            h.updated_at = chrono::Utc::now().timestamp();
        }
    }

    pub async fn set_output(&self, pane_id: &str, output: String) {
        let ws = {
            let mut map = self.inner.write().await;
            if let Some(h) = map.get_mut(pane_id) {
                h.output = output;
                h.updated_at = chrono::Utc::now().timestamp();
                h.workspace_id.clone()
            } else {
                return;
            }
        };
        self.persist(&ws).await;
    }

    pub async fn get(&self, pane_id: &str) -> Option<AgentHandle> {
        self.inner.read().await.get(pane_id).cloned()
    }

    /// Set (or clear, with `None`) the trim policy for a pane. Persisted.
    pub async fn set_trim_policy(&self, pane_id: &str, policy: Option<TrimPolicy>) {
        let ws = {
            let mut map = self.inner.write().await;
            if let Some(h) = map.get_mut(pane_id) {
                h.trim_policy = policy;
                h.updated_at = chrono::Utc::now().timestamp();
                h.workspace_id.clone()
            } else {
                return;
            }
        };
        self.persist(&ws).await;
    }

    /// Read the trim policy for a pane, if any.
    pub async fn get_trim_policy(&self, pane_id: &str) -> Option<TrimPolicy> {
        self.inner
            .read()
            .await
            .get(pane_id)
            .and_then(|h| h.trim_policy.clone())
    }

    /// Set (or clear) the pane label. Persisted.
    pub async fn set_label(&self, pane_id: &str, label: &str) {
        let ws = {
            let mut map = self.inner.write().await;
            if let Some(h) = map.get_mut(pane_id) {
                h.label = label.to_string();
                h.updated_at = chrono::Utc::now().timestamp();
                h.workspace_id.clone()
            } else {
                return;
            }
        };
        self.persist(&ws).await;
        // Note: caller should call `herdr pane rename` via herdr_cli if needed
    }

    /// Resolve a target (role, pane id, or label) to a pane id within a workspace.
    /// When `ws` is empty, resolution spans all workspaces (used by the a2a tools
    /// that address agents purely by role/label).
    pub async fn resolve(&self, ws: &str, target: &str) -> Option<String> {
        let map = self.inner.read().await;
        if map.contains_key(target) {
            return Some(target.to_string());
        }
        map.values()
            .find(|h| {
                (ws.is_empty() || h.workspace_id == ws) && (h.role == target || h.label == target)
            })
            .map(|h| h.pane_id.clone())
    }

    pub async fn list_for_ws(&self, ws: &str) -> Vec<AgentHandle> {
        self.inner
            .read()
            .await
            .values()
            .filter(|h| h.workspace_id == ws)
            .cloned()
            .collect()
    }

    /// Return every registered agent across all workspaces.
    pub async fn inner_snapshot(&self) -> Vec<AgentHandle> {
        self.inner.read().await.values().cloned().collect()
    }

    /// Build the variable seed for a recipe run: `{{pane_id.output}}` and
    /// `{{role.output}}` both resolve to the agent handle.
    pub async fn seed(&self, ws: &str) -> HashMap<String, serde_json::Value> {
        let mut out = HashMap::new();
        for h in self.list_for_ws(ws).await {
            let obj = serde_json::json!({
                "pane_id": h.pane_id,
                "workspace_id": h.workspace_id,
                "tab_id": h.tab_id,
                "agent": h.agent,
                "role": h.role,
                "status": h.status,
                "output": h.output,
            });
            out.insert(h.pane_id.clone(), obj.clone());
            if !h.role.is_empty() {
                out.insert(h.role.clone(), obj);
            }
        }
        out
    }

    /// Persist the agent handles for a workspace to the session blob store.
    async fn persist(&self, ws: &str) {
        let handles: Vec<AgentHandle> = self.list_for_ws(ws).await;
        if let Err(e) = self
            .persistence
            .save_session_blob(ws, "agents", &serde_json::json!(handles))
            .await
        {
            tracing::debug!("failed to persist agent registry for {ws}: {e}");
        }
    }
}

/// Subscriber connection phase — reported by the `status` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubscriberPhase {
    #[default]
    Starting,
    WaitingForSocket,
    Reconnecting,
    Connected,
    Stopped,
}

/// Thread-safe holder for the subscriber phase.
#[derive(Clone, Default)]
pub struct SubscriberState(Arc<tokio::sync::RwLock<SubscriberPhase>>);

impl SubscriberState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self) -> SubscriberPhase {
        *self.0.read().await
    }

    pub async fn set(&self, phase: SubscriberPhase) {
        *self.0.write().await = phase;
    }
}

/// Thin client that owns the registry and runs the event subscriber.
#[derive(Clone)]
pub struct HerdrClient {
    pub registry: AgentRegistry,
    socket_path: std::path::PathBuf,
    reconnect_attempts: u32,
    reconnect_backoff_ms: u64,
    subscriber_state: SubscriberState,
    /// Relays herdr's event stream to in-process listeners (triggers, SSE).
    events: broadcast::Sender<HerdrEvent>,
}

/// herdr events buffered per listener before it lags (and skips ahead).
const EVENT_BUFFER: usize = 256;

impl HerdrClient {
    pub fn new(
        persistence: Arc<Persistence>,
        socket_path: std::path::PathBuf,
        reconnect_attempts: u32,
        reconnect_backoff_ms: u64,
    ) -> Self {
        Self {
            registry: AgentRegistry::new(persistence),
            socket_path,
            reconnect_attempts,
            reconnect_backoff_ms,
            subscriber_state: SubscriberState::new(),
            events: broadcast::channel(EVENT_BUFFER).0,
        }
    }

    /// herdr's events from now on, as the subscriber receives them.
    pub fn events(&self) -> broadcast::Receiver<HerdrEvent> {
        self.events.subscribe()
    }

    /// The herdr API socket this client talks to.
    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    /// One request on herdr's socket API (`{id, method, params}` per line);
    /// returns herdr's response (`{id, result}` or `{id, error}`) as-is.
    /// Streaming methods (`events.subscribe`) return their first message;
    /// the live stream is [`HerdrClient::events`].
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: std::time::Duration,
    ) -> anyhow::Result<serde_json::Value> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let stream = tokio::net::UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| anyhow::anyhow!("herdr socket {}: {e}", self.socket_path.display()))?;
        let (read, mut write) = stream.into_split();
        let req = serde_json::json!({ "id": "herdr-mcp-call", "method": method, "params": params });
        write.write_all(format!("{req}\n").as_bytes()).await?;
        let line = tokio::time::timeout(timeout, BufReader::new(read).lines().next_line())
            .await
            .map_err(|_| anyhow::anyhow!("herdr did not answer {method} within {timeout:?}"))??
            .ok_or_else(|| anyhow::anyhow!("herdr closed the connection"))?;
        Ok(serde_json::from_str(&line)?)
    }

    /// Get the current subscriber phase.
    pub async fn subscriber_state(&self) -> SubscriberPhase {
        self.subscriber_state.get().await
    }

    /// Returns true if a herdr server is listening at our socket.
    ///
    /// Mirrors herdr's `is_server_listening_at()` in `src/server/autodetect.rs`:
    /// checks that the socket file exists AND a UnixStream::connect succeeds.
    /// Returns false for stale sockets (file exists but nobody listening) and
    /// for missing sockets.
    pub async fn is_socket_active(&self) -> bool {
        #[cfg(unix)]
        {
            if !self.socket_path.exists() {
                return false;
            }
            match tokio::net::UnixStream::connect(&self.socket_path).await {
                Ok(_) => true,
                Err(e) => !matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                ),
            }
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// Classify whether an error is a "socket missing" connect-phase error.
    /// Only used to distinguish expected "herdr not running" from unexpected drops.
    fn socket_missing(err: &anyhow::Error) -> bool {
        err.chain().any(|e| {
            e.downcast_ref::<std::io::Error>()
                .map(|ioe| matches!(ioe.kind(), std::io::ErrorKind::NotFound))
                .unwrap_or(false)
        })
    }

    /// Spawn the background event subscriber with smart retry policy.
    ///
    /// Retry behavior:
    /// - Outside herdr + socket missing: one INFO log, then silent DEBUG polling.
    ///   This is the EXPECTED case when herdr isn't running locally.
    /// - Inside herdr + socket missing: bounded WARN for first `reconnect_attempts`
    ///   failures (default 3), then silent DEBUG. This is a genuine problem
    ///   (session server died while we're inside a herdr pane).
    /// - Connected then dropped: WARN with fresh backoff, counter resets on
    ///   non-connect errors so we warn again on the next connection attempt.
    /// - After `reconnect_attempts` are exhausted, continue indefinitely at
    ///   `reconnect_backoff_ms` cadence — never fully gives up.
    pub fn spawn_subscriber(self: &Arc<Self>) {
        // Initial state before spawning
        let client = self.clone();
        // Use a oneshot to set initial state synchronously
        let _ = client
            .subscriber_state
            .0
            .try_write()
            .map(|mut w| *w = SubscriberPhase::Starting);
        tokio::spawn(async move {
            let mut failures: u32 = 0;
            let mut was_connected = false;
            loop {
                // Update state before attempting connection
                if was_connected {
                    client
                        .subscriber_state
                        .set(SubscriberPhase::Reconnecting)
                        .await;
                } else {
                    client
                        .subscriber_state
                        .set(SubscriberPhase::WaitingForSocket)
                        .await;
                }

                match client.run_subscribe_loop().await {
                    Ok(()) => {
                        // Clean shutdown / unsupported platform
                        client.subscriber_state.set(SubscriberPhase::Stopped).await;
                        break;
                    }
                    Err(e) => {
                        let missing = Self::socket_missing(&e);
                        let inside = herdr_mcp_core::session_context::inside_herdr();

                        if missing && !inside && failures == 0 {
                            // OUTSIDE herdr, socket absent — EXPECTED, not an error.
                            tracing::info!(
                                socket = %client.socket_path.display(),
                                "herdr socket not found (herdr not running here); watching for it"
                            );
                        } else if failures < client.reconnect_attempts {
                            if missing && inside {
                                tracing::warn!(
                                    socket = %client.socket_path.display(),
                                    "herdr socket missing while running inside herdr — herdr server may have stopped: {e}"
                                );
                            } else if !missing {
                                tracing::warn!("herdr event subscriber disconnected: {e}");
                            }
                            // missing && !inside after first: DEBUG (quiet)
                        } else {
                            tracing::debug!("herdr event subscriber waiting for socket: {e}");
                        }

                        // Reset failure counter if we had a real connection that dropped
                        // (non-connect-phase error), so we warn again on next connection.
                        if was_connected && !missing {
                            failures = 0;
                        }
                        was_connected = !missing;
                        failures = failures.saturating_add(1);

                        tokio::time::sleep(std::time::Duration::from_millis(
                            client.reconnect_backoff_ms,
                        ))
                        .await;
                    }
                }
            }
        });
    }

    /// One subscription per pane set: herdr reads nothing more on a
    /// connection once it streams, so a new or closed pane (whose status
    /// events need their own subscription) means reconnecting.
    async fn run_subscribe_loop(&self) -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

            loop {
                let request = self.build_subscribe_request().await;
                let stream = tokio::net::UnixStream::connect(&self.socket_path).await?;
                let (read_half, mut write_half) = stream.into_split();
                write_half
                    .write_all(format!("{request}\n").as_bytes())
                    .await?;

                let mut lines = BufReader::new(read_half).lines();
                // First line is the subscribe ack.
                if let Some(line) = lines.next_line().await?
                    && let Ok(ack) = serde_json::from_str::<serde_json::Value>(&line)
                    && ack.get("error").is_some()
                {
                    anyhow::bail!("events.subscribe rejected: {line}");
                }
                self.subscriber_state.set(SubscriberPhase::Connected).await;

                let mut panes_changed = false;
                while let Some(line) = lines.next_line().await? {
                    let Some(event) = HerdrEvent::parse(&line) else {
                        continue;
                    };
                    panes_changed = matches!(event.event.as_str(), "pane.created" | "pane.closed");
                    // No listeners is fine.
                    let _ = self.events.send(event.clone());
                    let client = self.clone();
                    tokio::spawn(async move { client.handle_event(event).await });
                    if panes_changed {
                        break;
                    }
                }
                if !panes_changed {
                    anyhow::bail!("event stream ended");
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = &self.socket_path;
            tracing::info!(
                "herdr event subscriber is only supported on Unix; registry will be populated by explicit tool calls"
            );
            self.subscriber_state.set(SubscriberPhase::Stopped).await;
            Ok(())
        }
    }

    /// Build a `events.subscribe` request: every global event herdr's schema
    /// lists, plus `pane.agent_status_changed` for each live pane (herdr
    /// scopes that one to a pane). As a side effect this seeds the registry
    /// from the live `pane list` so the a2a tools can resolve targets by
    /// pane id / role / label without waiting for the first event.
    async fn build_subscribe_request(&self) -> serde_json::Value {
        let mut subscriptions: Vec<serde_json::Value> = match herdr_schema().await {
            Some(schema) => global_subscriptions(&schema)
                .into_iter()
                .map(|t| serde_json::json!({ "type": t }))
                .collect(),
            None => {
                tracing::debug!("herdr api schema unavailable; subscribing to pane status only");
                Vec::new()
            }
        };

        if let Some(panes) = self.list_panes_json().await {
            for p in &panes {
                let pane_id = p.get("pane_id").and_then(|x| x.as_str()).unwrap_or("");
                if pane_id.is_empty() {
                    continue;
                }
                let ws = p.get("workspace_id").and_then(|x| x.as_str()).unwrap_or("");
                let agent = p.get("agent").and_then(|x| x.as_str()).unwrap_or("");
                let label = p.get("label").and_then(|x| x.as_str()).unwrap_or("");
                let status = p.get("agent_status").and_then(|x| x.as_str()).unwrap_or("");
                self.registry.upsert(pane_id, ws, "", agent, label).await;
                if !label.is_empty() {
                    self.registry.set_label(pane_id, label).await;
                }
                if !status.is_empty() {
                    self.registry.set_status(pane_id, status).await;
                }
                subscriptions.push(serde_json::json!({
                    "type": "pane.agent_status_changed",
                    "pane_id": pane_id
                }));
            }
        }

        serde_json::json!({
            "id": "herdr-mcp-subscribe",
            "method": "events.subscribe",
            "params": { "subscriptions": subscriptions }
        })
    }

    /// Fetch and parse `herdr pane list` -> array of pane objects (best-effort).
    async fn list_panes_json(&self) -> Option<Vec<serde_json::Value>> {
        let binary = herdr_mcp_core::herdr_bin();
        let output = tokio::process::Command::new(binary)
            .args(["pane", "list"])
            .output()
            .await
            .ok()?;
        let raw = String::from_utf8_lossy(&output.stdout);
        let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
        v.get("result")
            .and_then(|r| r.get("panes"))
            .and_then(|p| p.as_array())
            .cloned()
    }

    async fn handle_event(&self, event: HerdrEvent) {
        match event.event.as_str() {
            "pane.agent_status_changed" => {
                let Ok(data) = serde_json::from_value::<StatusData>(event.data) else {
                    return;
                };
                let agent = data.agent.unwrap_or_default();
                self.registry
                    .upsert(&data.pane_id, &data.workspace_id, "", &agent, "")
                    .await;
                if let Some(status) = AgentStatus::parse(&data.agent_status) {
                    self.registry
                        .set_status(&data.pane_id, status.as_str())
                        .await;
                    if status == AgentStatus::Idle {
                        // Capture the work product: pane output at idle.
                        if let Ok(out) = read_pane_output(&data.pane_id).await {
                            self.registry.set_output(&data.pane_id, out).await;
                        }
                    }
                }
            }
            "workspace.closed" => {
                let Some(ws) = event.workspace_id().map(str::to_string) else {
                    return;
                };
                // Summarize the session's trim savings.
                let s = herdr_mcp_trim::stats::load_stats(self.registry.data_dir(), &ws).await;
                if s.messages_trimmed > 0 {
                    let body = format!(
                        "Workspace {ws} closed — trim saved {:.1}% net ({} messages, {} bytes net).",
                        s.savings_pct(),
                        s.messages_trimmed,
                        s.net_saved_bytes
                    );
                    let binary = herdr_mcp_core::herdr_bin();
                    let _ = tokio::process::Command::new(binary)
                        .args(["notification", "show", "herdr-mcp trim", "--body", &body])
                        .output()
                        .await;
                }
            }
            _ => {}
        }
    }
}

/// `pane.agent_status_changed` payload (the fields the registry keeps).
#[derive(Debug, Deserialize)]
struct StatusData {
    pane_id: String,
    workspace_id: String,
    agent_status: String,
    #[serde(default)]
    agent: Option<String>,
}

/// `herdr api schema --json`, if herdr answers.
async fn herdr_schema() -> Option<serde_json::Value> {
    let output = tokio::process::Command::new(herdr_mcp_core::herdr_bin())
        .args(["api", "schema", "--json"])
        .output()
        .await
        .ok()?;
    serde_json::from_slice(&output.stdout).ok()
}

/// Subscription types herdr's schema lists that take no parameters (the
/// global events), e.g. `workspace.created`, `pane.created`.
fn global_subscriptions(schema: &serde_json::Value) -> Vec<String> {
    schema
        .pointer("/schemas/request/$defs/Subscription/oneOf")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter(|s| s.get("required") == Some(&serde_json::json!(["type"])))
        .filter_map(|s| s.pointer("/properties/type/const")?.as_str())
        .map(str::to_string)
        .collect()
}

/// Read a pane's recent output via the herdr CLI. Best-effort; errors are
/// swallowed by callers.
async fn read_pane_output(pane_id: &str) -> anyhow::Result<String> {
    let binary = herdr_mcp_core::herdr_bin();
    let output = tokio::process::Command::new(binary)
        .args([
            "pane", "read", pane_id, "--source", "recent", "--lines", "200",
        ])
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use herdr_mcp_trim::policy::TrimDirection;
    use pretty_assertions::assert_eq;

    #[test]
    fn global_subscriptions_are_the_parameterless_ones() {
        let schema = serde_json::json!({"schemas": {"request": {"$defs": {"Subscription": {"oneOf": [
            {"properties": {"type": {"const": "workspace.created"}}, "required": ["type"]},
            {"properties": {"type": {"const": "pane.created"}}, "required": ["type"]},
            {"properties": {"type": {"const": "pane.agent_status_changed"}, "pane_id": {}},
             "required": ["type", "pane_id"]},
        ]}}}}});
        assert_eq!(
            global_subscriptions(&schema),
            vec!["workspace.created".to_string(), "pane.created".to_string()]
        );
        assert!(global_subscriptions(&serde_json::json!({})).is_empty());
    }

    fn make_persistence() -> Arc<Persistence> {
        // Keep the tempdir alive for the test duration by leaking it; the OS
        // reclaims it when the test process exits.
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        std::mem::forget(tmp);
        Arc::new(p)
    }

    fn make_registry() -> AgentRegistry {
        let p = make_persistence();
        AgentRegistry::new(p)
    }

    #[tokio::test]
    async fn test_upsert_then_get() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "agentA").await;
        let h = reg.get("p1").await.unwrap();
        assert_eq!(h.pane_id, "p1");
        assert_eq!(h.agent, "claude");
    }

    #[tokio::test]
    async fn test_upsert_preserves_role() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "agentA").await;
        let h = reg.get("p1").await.unwrap();
        assert_eq!(h.role, "agentA");
    }

    #[tokio::test]
    async fn test_resolve_by_pane_id() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "").await;
        assert_eq!(reg.resolve("w1", "p1").await, Some("p1".into()));
    }

    #[tokio::test]
    async fn test_resolve_by_role() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "agentA").await;
        assert_eq!(reg.resolve("w1", "agentA").await, Some("p1".into()));
    }

    #[tokio::test]
    async fn test_resolve_by_label() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "agentA").await;
        reg.set_label("p1", "my-label").await;
        assert_eq!(reg.resolve("w1", "my-label").await, Some("p1".into()));
    }

    #[tokio::test]
    async fn test_resolve_empty_ws_spans_all() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "agentA").await;
        assert_eq!(reg.resolve("", "agentA").await, Some("p1".into()));
    }

    #[tokio::test]
    async fn test_resolve_scoped_ws() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "agentA").await;
        reg.upsert("p2", "w2", "", "gpt", "agentA").await;
        // role "agentA" exists in both; w2 scope resolves to p2
        assert_eq!(reg.resolve("w2", "agentA").await, Some("p2".into()));
        // w1 scope resolves to p1
        assert_eq!(reg.resolve("w1", "agentA").await, Some("p1".into()));
    }

    #[tokio::test]
    async fn test_set_status() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "").await;
        reg.set_status("p1", "working").await;
        assert_eq!(reg.get("p1").await.unwrap().status, "working");
    }

    #[tokio::test]
    async fn test_set_output() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "").await;
        reg.set_output("p1", "the work product".into()).await;
        assert_eq!(reg.get("p1").await.unwrap().output, "the work product");
    }

    #[tokio::test]
    async fn test_set_trim_policy_get() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "").await;
        let policy = TrimPolicy {
            stages: vec!["caveman:full".into(), "pfc1".into()],
            direction: TrimDirection::OutboundWithAck,
        };
        reg.set_trim_policy("p1", Some(policy.clone())).await;
        let got = reg.get_trim_policy("p1").await.unwrap();
        assert_eq!(got, policy);
    }

    #[tokio::test]
    async fn test_set_label() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "").await;
        reg.set_label("p1", "labelX").await;
        assert_eq!(reg.get("p1").await.unwrap().label, "labelX");
    }

    #[tokio::test]
    async fn test_list_for_ws() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "").await;
        reg.upsert("p2", "w1", "", "gpt", "").await;
        reg.upsert("p3", "w2", "", "gemini", "").await;
        assert_eq!(reg.list_for_ws("w1").await.len(), 2);
        assert_eq!(reg.list_for_ws("w2").await.len(), 1);
    }

    #[tokio::test]
    async fn test_inner_snapshot() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "").await;
        reg.upsert("p2", "w2", "", "gpt", "").await;
        assert_eq!(reg.inner_snapshot().await.len(), 2);
    }

    #[tokio::test]
    async fn test_seed_outputs() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "agentA").await;
        reg.set_output("p1", "out".into()).await;
        let seed = reg.seed("w1").await;
        assert!(seed.contains_key("p1"));
        assert!(seed.contains_key("agentA"));
        assert_eq!(seed.get("p1").unwrap()["output"], serde_json::json!("out"));
    }

    #[tokio::test]
    async fn test_upsert_update_existing() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "agentA").await;
        reg.upsert("p1", "w1", "", "gpt", "agentB").await;
        let h = reg.get("p1").await.unwrap();
        assert_eq!(h.agent, "gpt");
        assert_eq!(h.role, "agentB");
    }

    #[tokio::test]
    async fn test_get_trim_policy_none_when_unset() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "").await;
        assert!(reg.get_trim_policy("p1").await.is_none());
    }

    #[tokio::test]
    async fn test_clear_trim_policy() {
        let reg = make_registry();
        reg.upsert("p1", "w1", "", "claude", "").await;
        let policy = TrimPolicy {
            stages: vec!["pfc1".into()],
            direction: TrimDirection::Outbound,
        };
        reg.set_trim_policy("p1", Some(policy)).await;
        reg.set_trim_policy("p1", None).await;
        assert!(reg.get_trim_policy("p1").await.is_none());
    }
}

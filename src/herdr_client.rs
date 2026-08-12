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
use tokio::sync::{mpsc, RwLock};

use crate::persistence::Persistence;
use crate::trim::policy::{TrimPolicy, TrimDirection};

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

/// A registered agent: its pane address plus the last captured work product.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHandle {
    pub pane_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub agent: String,
    /// Human-assigned role used for `{{role.output}}` interpolation (e.g. "agentA").
    #[serde(default)]
    pub role: String,
    /// Human-assigned pane label (set via `herdr pane rename`), used for targeting.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub status: String,
    /// Pane output captured at last idle — the agent's work product.
    #[serde(default)]
    pub output: String,
    /// Optional message-trim policy (default off). Persisted as part of the
    /// agent blob, so recipes can opt a pane into compression.
    #[serde(default)]
    pub trim_policy: Option<TrimPolicy>,
    #[serde(default)]
    pub updated_at: i64,
}

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
        let mut handle = map
            .remove(pane_id)
            .unwrap_or_else(|| AgentHandle {
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
                (ws.is_empty() || h.workspace_id == ws)
                    && (h.role == target || h.label == target)
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
}

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
        }
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
        let client = self.clone();
        // Initial state before spawning
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
                    Ok(()) => break, // clean shutdown / unsupported platform
                    Err(e) => {
                        let missing = Self::socket_missing(&e);
                        let inside = crate::session_context::inside_herdr();

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

                        tokio::time::sleep(
                            std::time::Duration::from_millis(client.reconnect_backoff_ms),
                        )
                        .await;
                    }
                }
            }
        });
    }

    async fn run_subscribe_loop(&self) -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

            let stream = tokio::net::UnixStream::connect(&self.socket_path).await?;
            let (read_half, mut write_half) = stream.into_split();

            // Channel to push additional subscribe requests (e.g. when a new
            // workspace appears and we must (re)subscribe its panes).
            let (sub_tx, mut sub_rx) = mpsc::channel::<serde_json::Value>(16);

            // Writer task drains the channel and writes subscribe requests.
            let writer_task = tokio::spawn(async move {
                while let Some(req) = sub_rx.recv().await {
                    if write_half
                        .write_all(format!("{}\n", req).as_bytes())
                        .await
                        .is_err()
                    {
                        break;
                    }
                    let _ = write_half.flush().await;
                }
            });

            // Initial subscription: workspace events (global) + per-pane status
            // events. This also populates the registry from the live pane list.
            let initial = self.build_subscribe_request().await;
            let _ = sub_tx.send(initial).await;

            let mut lines = BufReader::new(read_half).lines();
            // First line is the subscribe ack.
            if let Some(line) = lines.next_line().await? {
                if let Ok(ack) = serde_json::from_str::<serde_json::Value>(&line) {
                    if ack.get("error").is_some() {
                        writer_task.abort();
                        anyhow::bail!("events.subscribe rejected: {line}");
                    }
                }
            }

            // Successfully connected and subscribed
            self.subscriber_state.set(SubscriberPhase::Connected).await;

            while let Some(line) = lines.next_line().await? {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(event) = serde_json::from_str::<SubEvent>(&line) {
                    let client = self.clone();
                    let sub_tx = sub_tx.clone();
                    tokio::spawn(async move {
                        client.handle_event(event, sub_tx).await;
                    });
                }
            }
            writer_task.abort();
            anyhow::bail!("event stream ended")
        }
        #[cfg(not(unix))]
        {
            let _ = &self.socket_path;
            tracing::info!("herdr event subscriber is only supported on Unix; registry will be populated by explicit tool calls");
            Ok(())
        }
    }

    /// Build a `events.subscribe` request for the current set of panes.
    ///
    /// herdr's API requires `params.pane_id: null` for global (workspace-level)
    /// subscriptions, while pane-scoped event types must carry the pane id in
    /// each subscription entry. As a side effect this also seeds the registry
    /// from the live `pane list` so the a2a tools can resolve targets by
    /// pane id / role / label without waiting for the first event.
    async fn build_subscribe_request(&self) -> serde_json::Value {
        let mut pane_subs: Vec<serde_json::Value> = Vec::new();

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
                self.registry
                    .upsert(pane_id, ws, "", agent, label)
                    .await;
                if !label.is_empty() {
                    self.registry.set_label(pane_id, label).await;
                }
                if !status.is_empty() {
                    self.registry.set_status(pane_id, status).await;
                }
                pane_subs.push(serde_json::json!({
                    "type": "pane.agent_status_changed",
                    "pane_id": pane_id
                }));
            }
        }

        let mut subscriptions = vec![
            serde_json::json!({ "type": "workspace.created" }),
            serde_json::json!({ "type": "workspace.closed" }),
        ];
        subscriptions.extend(pane_subs);

        serde_json::json!({
            "id": "herdr-mcp-subscribe",
            "method": "events.subscribe",
            "params": { "pane_id": null, "subscriptions": subscriptions }
        })
    }

    /// Fetch and parse `herdr pane list` -> array of pane objects (best-effort).
    async fn list_panes_json(&self) -> Option<Vec<serde_json::Value>> {
        let binary = std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".to_string());
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

    async fn handle_event(&self, event: SubEvent, sub_tx: mpsc::Sender<serde_json::Value>) {
        match event.event.as_str() {
            "pane.agent_status_changed" => {
                let pane_id = event.data.pane_id;
                let ws = event.data.workspace_id;
                let agent = event.data.agent.clone().unwrap_or_default();
                let label = event.data.label.clone().unwrap_or_default();
                self.registry
                    .upsert(&pane_id, &ws, "", &agent, "")
                    .await;
                if !label.is_empty() {
                    self.registry.set_label(&pane_id, &label).await;
                }
                if let Some(status) = event.data.agent_status.as_deref().and_then(AgentStatus::parse) {
                    let status_str = status.as_str().to_string();
                    self.registry.set_status(&pane_id, &status_str).await;
                    if status == AgentStatus::Idle {
                        // Capture the work product: pane output at idle.
                        let client = self.clone();
                        let pane = pane_id.clone();
                        tokio::spawn(async move {
                            if let Ok(out) = read_pane_output(&pane).await {
                                client.registry.set_output(&pane, out).await;
                            }
                        });
                    }
                }
            }
            "workspace.created" => {
                let ws = event.data.workspace_id;
                if !ws.is_empty() {
                    // A new workspace means new panes: refresh the registry and
                    // (re)subscribe to every pane's status events.
                    let req = self.build_subscribe_request().await;
                    let _ = sub_tx.send(req).await;

                    // Seed a default trim policy for the workspace so all panes inherit it.
                    let default_policy = TrimPolicy {
                        stages: vec!["caveman:full".to_string(), "pfc1".to_string()],
                        direction: TrimDirection::OutboundWithAck,
                    };
                    for handle in self.registry.list_for_ws(&ws).await {
                        if handle.trim_policy.is_none() {
                            self.registry
                                .set_trim_policy(&handle.pane_id, Some(default_policy.clone()))
                                .await;
                        }
                    }
                    tracing::info!("workspace {ws} created; seeded default trim policy");
                }
            }
            "workspace.closed" => {
                let ws = event.data.workspace_id;
                if !ws.is_empty() {
                    // Best-effort: summarize the session's trim savings and
                    // clean up per-workspace trim state.
                    let s = crate::trim::stats::load_stats(self.registry.data_dir(), &ws).await;
                    if s.messages_trimmed > 0 {
                        let body = format!(
                            "Workspace {ws} closed — trim saved {:.1}% net ({} messages, {} bytes net).",
                            s.savings_pct(),
                            s.messages_trimmed,
                            s.net_saved_bytes
                        );
                        let binary =
                            std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".to_string());
                        let _ = tokio::process::Command::new(binary)
                            .args(["notification", "show", "herdr-mcp trim", "--body", &body])
                            .output()
                            .await;
                    }
                    tracing::info!("workspace {ws} closed; cleaned up trim state");
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Deserialize)]
struct SubEvent {
    #[serde(rename = "event")]
    event: String,
    #[serde(rename = "data", default)]
    data: SubData,
}

#[derive(Debug, Default, Deserialize)]
struct SubData {
    #[serde(default)]
    pane_id: String,
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    agent_status: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

/// Read a pane's recent output via the herdr CLI. Best-effort; errors are
/// swallowed by callers.
async fn read_pane_output(pane_id: &str) -> anyhow::Result<String> {
    let binary = std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".to_string());
    let output = tokio::process::Command::new(binary)
        .args(["pane", "read", pane_id, "--source", "recent", "--lines", "200"])
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

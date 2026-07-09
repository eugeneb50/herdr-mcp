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
use tokio::sync::RwLock;

use crate::persistence::Persistence;
use crate::trim::policy::TrimPolicy;

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

    pub async fn remove(&self, pane_id: &str) {
        let mut map = self.inner.write().await;
        if let Some(h) = map.remove(pane_id) {
            drop(map);
            self.persist(&h.workspace_id).await;
        }
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

    /// Resolve a target (role or pane id) to a pane id within a workspace.
    pub async fn resolve(&self, ws: &str, target: &str) -> Option<String> {
        let map = self.inner.read().await;
        if map.contains_key(target) {
            return Some(target.to_string());
        }
        map.values()
            .find(|h| h.workspace_id == ws && h.role == target)
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

    /// Load agent handles for a workspace from disk into the live registry.
    pub async fn load_from_disk(&self, ws: &str) {
        match self.persistence.load_session_blob(ws, "agents").await {
            Ok(Some(value)) => {
                if let Ok(handles) = serde_json::from_value::<Vec<AgentHandle>>(value) {
                    let mut map = self.inner.write().await;
                    for h in handles {
                        map.insert(h.pane_id.clone(), h);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Thin client that owns the registry and runs the event subscriber.
#[derive(Clone)]
pub struct HerdrClient {
    pub registry: AgentRegistry,
    socket_path: std::path::PathBuf,
}

impl HerdrClient {
    pub fn new(persistence: Arc<Persistence>, socket_path: std::path::PathBuf) -> Self {
        Self {
            registry: AgentRegistry::new(persistence),
            socket_path,
        }
    }

    /// Spawn the background event subscriber (reconnects on failure).
    pub fn spawn_subscriber(self: &Arc<Self>) {
        let client = self.clone();
        tokio::spawn(async move {
            loop {
                match client.run_subscribe_loop().await {
                    Ok(()) => break, // clean shutdown (e.g. unsupported platform)
                    Err(e) => {
                        tracing::warn!("herdr event subscriber disconnected: {e}; retrying in 3s");
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    }
                }
            }
        });
    }

    async fn run_subscribe_loop(&self) -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

            let mut stream = tokio::net::UnixStream::connect(&self.socket_path).await?;
            let req = serde_json::json!({
                "id": "herdr-mcp-subscribe",
                "method": "events.subscribe",
                "params": {
                    "subscriptions": [
                        { "type": "pane.agent_status_changed" }
                    ]
                }
            });
            stream
                .write_all(format!("{}\n", req).as_bytes())
                .await?;
            stream.flush().await?;

            let (read_half, _write_half) = stream.into_split();
            let mut lines = BufReader::new(read_half).lines();
            // First line is the subscribe ack.
            if let Some(line) = lines.next_line().await? {
                if let Ok(ack) = serde_json::from_str::<serde_json::Value>(&line) {
                    if ack.get("error").is_some() {
                        anyhow::bail!("events.subscribe rejected: {line}");
                    }
                }
            }

            while let Some(line) = lines.next_line().await? {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(event) = serde_json::from_str::<SubEvent>(&line) {
                    self.handle_event(event).await;
                }
            }
            anyhow::bail!("event stream ended")
        }
        #[cfg(not(unix))]
        {
            let _ = &self.socket_path;
            tracing::info!("herdr event subscriber is only supported on Unix; registry will be populated by explicit tool calls");
            Ok(())
        }
    }

    async fn handle_event(&self, event: SubEvent) {
        if event.event != "pane.agent_status_changed" {
            return;
        }
        let pane_id = event.data.pane_id;
        let ws = event.data.workspace_id;
        let agent = event.data.agent.clone().unwrap_or_default();
        self.registry
            .upsert(&pane_id, &ws, "", &agent, "")
            .await;
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

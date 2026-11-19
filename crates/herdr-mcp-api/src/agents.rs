//! Agent registry wire types.

use serde::{Deserialize, Serialize};

/// A registered agent: its pane address plus the last captured work product.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHandle {
    pub pane_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub agent: String,
    /// The pane's label: the agent's one name, unique in the session.
    /// Targets and `{{label.output}}` use it; herdr keeps it across restarts.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub status: String,
    /// Pane output captured at last idle — the agent's work product.
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub updated_at: i64,
}

/// `GET /api/agents` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsResponse {
    pub agents: Vec<AgentHandle>,
}

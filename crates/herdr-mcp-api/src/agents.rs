//! Agent registry wire types and the per-pane policies stored on them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// When to apply a trim policy to an agent's traffic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrimDirection {
    /// No transform (default).
    #[default]
    None,
    /// Compress outbound text before it crosses the pane boundary.
    Outbound,
    /// Compress outbound; auto-detect + decompress on the next `agent_read`.
    OutboundWithAck,
}

/// A trim policy: an ordered stage list plus a direction.
///
/// Stage parsing lives in `herdr_mcp_trim::policy::TrimPolicyExt` (it needs
/// the pipeline); this crate only carries the data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TrimPolicy {
    /// Ordered stage list, e.g. `["caveman:ultra", "pfc1"]`.
    pub stages: Vec<String>,
    /// When to apply (default `None` = off).
    pub direction: TrimDirection,
}

impl TrimPolicy {
    /// True when this policy should transform outbound text.
    pub fn is_active(&self) -> bool {
        !self.stages.is_empty() && self.direction != TrimDirection::None
    }
}

/// Per-pane HTTPS-intercept proxy policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyPolicy {
    pub trim_outbound: bool,
    pub trim_inbound: bool,
    pub stages: Vec<String>,
}

impl Default for ProxyPolicy {
    fn default() -> Self {
        Self {
            trim_outbound: false,
            trim_inbound: false,
            stages: vec!["caveman:full".into(), "pfc1".into()],
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
    /// Optional per-pane proxy policy (default off). Controls HTTPS
    /// interception trim for this pane's LLM traffic.
    #[serde(default)]
    pub proxy_policy: Option<ProxyPolicy>,
    #[serde(default)]
    pub updated_at: i64,
}

/// `GET /api/agents` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsResponse {
    pub agents: Vec<AgentHandle>,
}

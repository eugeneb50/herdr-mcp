//! Per-pane trim policy stored on an `AgentHandle`.

use serde::{Deserialize, Serialize};

use schemars::JsonSchema;

use crate::trim::pipeline::{parse_stage_spec, parse_stage_specs, StageSpec};

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

    /// Parse `stages` into [`StageSpec`]s, failing on an unknown stage.
    /// PFC1 stages default to `emit_header = true` (self-describing).
    pub fn parse_stages(&self) -> Result<Vec<StageSpec>, String> {
        self.parse_stages_with(true)
    }

    /// Like [`TrimPolicy::parse_stages`] but controls PFC1 header emission.
    /// Trusted a2a passes `false` (both ends share the server key).
    pub fn parse_stages_with(&self, emit_header: bool) -> Result<Vec<StageSpec>, String> {
        let mut out = Vec::new();
        for s in &self.stages {
            let mut spec = parse_stage_spec(s)?;
            if let StageSpec::Pfc1 { emit_header: h } = &mut spec {
                *h = emit_header;
            }
            out.push(spec);
        }
        Ok(out)
    }
}

/// Build a default `OutboundWithAck` policy from a stage list — the common case.
#[allow(dead_code)]
pub fn outbound_with_ack(stages: Vec<String>) -> TrimPolicy {
    TrimPolicy {
        stages,
        direction: TrimDirection::OutboundWithAck,
    }
}

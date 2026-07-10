//! Per-pane trim policy stored on an `AgentHandle`.

use serde::{Deserialize, Serialize};

use schemars::JsonSchema;

use crate::pipeline::{StageSpec, parse_stage_spec};

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
    #[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::StageSpec;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_policy_inactive_when_empty_stages() {
        let p = TrimPolicy {
            stages: vec![],
            direction: TrimDirection::OutboundWithAck,
        };
        assert!(!p.is_active());
    }

    #[test]
    fn test_policy_inactive_when_none_direction() {
        let p = TrimPolicy {
            stages: vec!["pfc1".into()],
            direction: TrimDirection::None,
        };
        assert!(!p.is_active());
    }

    #[test]
    fn test_policy_active_when_stages_and_direction() {
        let p = TrimPolicy {
            stages: vec!["caveman:full".into(), "pfc1".into()],
            direction: TrimDirection::Outbound,
        };
        assert!(p.is_active());
    }

    #[test]
    fn test_parse_stages_default_emit_header() {
        let p = TrimPolicy {
            stages: vec!["pfc1".into()],
            direction: TrimDirection::Outbound,
        };
        let specs = p.parse_stages().unwrap();
        assert_eq!(specs.len(), 1);
        match &specs[0] {
            StageSpec::Pfc1 { emit_header } => {
                assert!(*emit_header, "default emit_header should be true")
            }
            _ => panic!("expected pfc1"),
        }
    }

    #[test]
    fn test_parse_stages_with_header_true() {
        let p = TrimPolicy {
            stages: vec!["pfc1".into()],
            direction: TrimDirection::Outbound,
        };
        let specs = p.parse_stages_with(true).unwrap();
        match &specs[0] {
            StageSpec::Pfc1 { emit_header } => assert!(*emit_header),
            _ => panic!("expected pfc1"),
        }
    }

    #[test]
    fn test_parse_stages_with_header_false() {
        let p = TrimPolicy {
            stages: vec!["pfc1".into()],
            direction: TrimDirection::Outbound,
        };
        let specs = p.parse_stages_with(false).unwrap();
        match &specs[0] {
            StageSpec::Pfc1 { emit_header } => assert!(!*emit_header),
            _ => panic!("expected pfc1"),
        }
    }

    #[test]
    fn test_parse_stages_unknown_stage_errors() {
        let p = TrimPolicy {
            stages: vec!["bogus".into()],
            direction: TrimDirection::Outbound,
        };
        assert!(p.parse_stages().is_err());
    }

    #[test]
    fn test_parse_stages_mixed() {
        let p = TrimPolicy {
            stages: vec!["caveman:full".into(), "pfc1".into()],
            direction: TrimDirection::OutboundWithAck,
        };
        let specs = p.parse_stages().unwrap();
        assert_eq!(specs.len(), 2);
        assert!(matches!(specs[0], StageSpec::Caveman(_)));
        assert!(matches!(specs[1], StageSpec::Pfc1 { .. }));
    }

    #[test]
    fn test_serde_roundtrip() {
        let p = TrimPolicy {
            stages: vec!["caveman:ultra".into(), "pfc1".into()],
            direction: TrimDirection::OutboundWithAck,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: TrimPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}

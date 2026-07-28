use anyhow::Result;
use herdr_mcp_trim::pipeline;
use herdr_mcp_trim::runner::PipelineRunner;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

pub fn trim_body(
    text: &str,
    runner: &PipelineRunner,
    policy: &ProxyPolicy,
    outbound: bool,
) -> Result<String> {
    let active = if outbound {
        policy.trim_outbound
    } else {
        policy.trim_inbound
    };
    if !active || text.is_empty() {
        return Ok(text.to_string());
    }
    let base_key = runner.base_key().clone();
    let stages = pipeline::parse_stage_specs(&policy.stages).unwrap_or_default();
    let result = pipeline::run(text, &stages, &base_key);
    Ok(result.output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_policy_default() {
        let p = ProxyPolicy::default();
        assert!(!p.trim_outbound);
        assert!(!p.trim_inbound);
        assert_eq!(p.stages, vec!["caveman:full", "pfc1"]);
    }

    #[tokio::test]
    async fn test_trim_body_caveman_pfc1_compresses() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = PipelineRunner::new(tmp.path()).await;
        let policy = ProxyPolicy {
            trim_outbound: true,
            trim_inbound: false,
            stages: vec!["caveman:full".into(), "pfc1".into()],
        };
        let input = "the quick brown fox jumps over the lazy dog \
                     because the configuration gateway is slow and the \
                     provisioning framework is also slow but the integration \
                     pipeline remains stable and the implementation strategy \
                     works well across the entire infrastructure";
        let out = trim_body(input, &runner, &policy, true).unwrap();
        assert!(!out.is_empty());
        // Compressed or equal — pipeline may pass through if no savings
        assert!(out.len() <= input.len());
    }

    #[tokio::test]
    async fn test_trim_body_off_policy_passthrough() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = PipelineRunner::new(tmp.path()).await;
        let policy = ProxyPolicy::default();
        let input = "the quick brown fox";
        let out = trim_body(input, &runner, &policy, true).unwrap();
        assert_eq!(out, input);
    }

    #[tokio::test]
    async fn test_trim_body_empty_passthrough() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = PipelineRunner::new(tmp.path()).await;
        let policy = ProxyPolicy {
            trim_outbound: true,
            trim_inbound: true,
            stages: vec!["caveman:full".into()],
        };
        let out = trim_body("", &runner, &policy, true).unwrap();
        assert_eq!(out, "");
    }

    #[tokio::test]
    async fn test_trim_body_inbound_active() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = PipelineRunner::new(tmp.path()).await;
        let policy = ProxyPolicy {
            trim_outbound: false,
            trim_inbound: true,
            stages: vec!["caveman:full".into()],
        };
        let input = "the quick brown fox jumps over the lazy dog because \
                     it is slow and the configuration gateway integration \
                     pipeline implementation infrastructure provisioning";
        let out = trim_body(input, &runner, &policy, false).unwrap();
        assert!(!out.is_empty());
        assert!(out.len() <= input.len());
    }

    #[test]
    fn test_proxy_policy_serde_roundtrip() {
        let p = ProxyPolicy {
            trim_outbound: true,
            trim_inbound: false,
            stages: vec!["pfc1".into()],
        };
        let json = serde_json::to_string(&p).unwrap();
        let p2: ProxyPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(p.trim_outbound, p2.trim_outbound);
        assert_eq!(p.trim_inbound, p2.trim_inbound);
        assert_eq!(p.stages, p2.stages);
    }
}

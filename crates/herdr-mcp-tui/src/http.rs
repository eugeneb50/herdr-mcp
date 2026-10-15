//! Bridge connection for the TUI dashboard. Endpoints and wire types live in
//! `herdr-mcp-api` (shared with the web UI); this module only builds the
//! client for a local port and discovers a live bridge.

use std::time::Duration;

use herdr_mcp_api::{ApiClient, ApiError};

/// Per-request timeout for dashboard calls (tools like `wait_*` can be slow).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Per-port timeout while probing for a bridge, so closed ports fail fast.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

fn base(port: u16) -> String {
    format!("http://localhost:{port}")
}

/// Client for the bridge on `localhost:{port}`.
pub fn client_for(port: u16) -> Result<ApiClient, ApiError> {
    ApiClient::with_timeout(&base(port), REQUEST_TIMEOUT)
}

/// Find a live bridge: tries `initial` (the configured port) first, then the
/// historical defaults. Returns the first port whose `GET /api/health` is 2xx.
pub async fn discover_bridge(initial: u16) -> Option<u16> {
    let mut candidates: Vec<u16> = vec![initial, 8080, 7676];
    candidates.dedup();
    for port in candidates {
        let Ok(probe) = ApiClient::with_timeout(&base(port), PROBE_TIMEOUT) else {
            continue;
        };
        if probe.health().await.is_ok() {
            return Some(port);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Discovery must finish quickly (500ms per candidate port) rather than
    // hanging for the full request timeout.
    #[tokio::test]
    async fn discover_bridge_does_not_hang() {
        let res = tokio::time::timeout(Duration::from_secs(5), discover_bridge(1)).await;
        assert!(res.is_ok(), "discover_bridge hung");
    }

    #[test]
    fn client_for_targets_localhost_port() {
        let c = client_for(7676).unwrap();
        assert_eq!(c.base().as_str(), "http://localhost:7676/");
    }
}

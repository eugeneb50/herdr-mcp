//! Bridge connection for the TUI dashboard. Endpoints and wire types live in
//! `herdr-mcp-api` (shared with the web UI); this module only builds the
//! client for a local port and checks that its bridge is live.

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

/// A client for the bridge on `port` (the configured one) if its
/// `GET /api/health` answers 2xx. No other port is tried: a bridge elsewhere
/// may serve another data dir or herdr session.
pub async fn live_bridge(port: u16) -> Option<ApiClient> {
    let probe = ApiClient::with_timeout(&base(port), PROBE_TIMEOUT).ok()?;
    probe.health().await.ok()?;
    client_for(port).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The probe must finish quickly (500ms) rather than hanging for the full
    // request timeout.
    #[tokio::test]
    async fn live_bridge_does_not_hang() {
        let res = tokio::time::timeout(Duration::from_secs(2), live_bridge(1)).await;
        assert!(matches!(res, Ok(None)), "live_bridge hung or found port 1");
    }

    #[test]
    fn client_for_targets_localhost_port() {
        let c = client_for(7676).unwrap();
        assert_eq!(c.base().as_str(), "http://localhost:7676/");
    }
}

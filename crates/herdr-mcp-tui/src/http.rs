//! Bridge connection for the TUI dashboard. Endpoints and wire types live in
//! `herdr-mcp-api` (shared with the web UI); this module only builds the
//! client for a local port and checks that its bridge is live.

use std::path::Path;
use std::time::Duration;

use herdr_mcp_api::{ApiClient, ApiError};

/// Per-port timeout while probing for a bridge, so closed ports fail fast.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

fn base(port: u16) -> String {
    format!("http://localhost:{port}")
}

/// The bridge token in `data_dir`, if the bridge wrote one (it does when
/// `[security]` makes it required).
pub fn token(data_dir: &Path) -> Option<String> {
    std::fs::read_to_string(data_dir.join(herdr_mcp_api::TOKEN_FILE))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Client for the bridge on `localhost:{port}` serving `data_dir`, each
/// request bounded by `timeout` (the `timeouts.tui-request-ms` setting).
pub fn client_for(port: u16, timeout: Duration, data_dir: &Path) -> Result<ApiClient, ApiError> {
    Ok(ApiClient::with_timeout(&base(port), timeout)?.with_token(token(data_dir)))
}

/// Client for a long-lived stream (`/api/herdr/events`): no request
/// timeout, which would end it.
pub fn stream_client(port: u16, data_dir: &Path) -> Result<ApiClient, ApiError> {
    Ok(ApiClient::new(&base(port))?.with_token(token(data_dir)))
}

/// A client for the bridge on `port` (the configured one) if its
/// `GET /api/health` answers 2xx. No other port is tried: a bridge elsewhere
/// may serve another data dir or herdr session.
pub async fn live_bridge(port: u16, timeout: Duration, data_dir: &Path) -> Option<ApiClient> {
    let probe = ApiClient::with_timeout(&base(port), PROBE_TIMEOUT).ok()?;
    probe.health().await.ok()?;
    client_for(port, timeout, data_dir).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The probe must finish quickly (500ms) rather than hanging for the full
    // request timeout.
    #[tokio::test]
    async fn live_bridge_does_not_hang() {
        let res = tokio::time::timeout(
            Duration::from_secs(2),
            live_bridge(1, Duration::from_secs(30), Path::new("/nonexistent")),
        )
        .await;
        assert!(matches!(res, Ok(None)), "live_bridge hung or found port 1");
    }

    #[test]
    fn client_for_targets_localhost_port() {
        let c = client_for(7676, Duration::from_secs(30), Path::new("/nonexistent")).unwrap();
        assert_eq!(c.base().as_str(), "http://localhost:7676/");
    }
}

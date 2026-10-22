//! Bridge access for the web UI. The typed client and every wire type live in
//! `herdr-mcp-api` (shared with the TUI); this module only locates the bridge.

use dioxus::prelude::*;
pub use herdr_mcp_api::{ApiClient, ApiError};

/// The app-wide client, provided once by `App`. A `Signal` so event closures
/// stay `Copy`; call it (`client()`) to get an owned clone for an async task.
pub fn use_api() -> Signal<ApiClient> {
    use_context()
}

/// Client for the bridge that served this page. The daemon serves the bundle
/// and `/api/*` from one origin (and `dx serve` proxies `/api`), so the page
/// origin is the bridge base. reqwest needs an absolute URL, even on wasm.
pub fn api() -> Result<ApiClient, ApiError> {
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .ok_or_else(|| ApiError::BadBase("window.location.origin unavailable".into()))?;
    ApiClient::new(&origin)
}

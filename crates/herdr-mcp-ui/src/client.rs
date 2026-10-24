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
    Ok(ApiClient::new(&origin)?.with_token(token()))
}

/// Where the page keeps the bridge token.
const TOKEN_KEY: &str = "herdr-mcp-token";

/// The bridge token: taken from a `#token=` link (the bridge prints one
/// when it needs its token) and kept, else the one kept before.
pub fn token() -> Option<String> {
    let location = web_sys::window()?.location();
    if let Some(t) = location
        .hash()
        .ok()
        .and_then(|h| h.strip_prefix("#token=").map(str::to_string))
    {
        crate::local::remember(TOKEN_KEY, &t);
        let _ = location.set_hash("");
    }
    Some(crate::local::remembered(TOKEN_KEY)).filter(|t| !t.is_empty())
}

/// Keep `token` for this page and reload with it.
pub fn set_token(token: &str) {
    crate::local::remember(TOKEN_KEY, token.trim());
    if let Some(w) = web_sys::window() {
        let _ = w.location().reload();
    }
}

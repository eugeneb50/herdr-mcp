//! The web UI built into the binary (see `build.rs`): served when
//! `http.web-dir` is unset, so one `herdr-mcp` file is the whole deployment.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

/// `(path under the bundle, content type, bytes)`; empty when the binary was
/// built without a `dx bundle`.
static ASSETS: &[(&str, &str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

/// Whether this binary carries a web UI.
pub fn embedded() -> bool {
    !ASSETS.is_empty()
}

fn asset(path: &str) -> Option<&'static (&'static str, &'static str, &'static [u8])> {
    ASSETS.iter().find(|(p, _, _)| *p == path)
}

/// Fallback handler: the asset at the request path, else `index.html` so
/// client-side routes resolve.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    match asset(path).or_else(|| asset("index.html")) {
        Some((_, mime, body)) => ([(header::CONTENT_TYPE, *mime)], *body).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "web UI not built into this binary; /api/* is available",
        )
            .into_response(),
    }
}

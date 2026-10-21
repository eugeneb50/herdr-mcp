//! `herdr-mcp-ui` — Dioxus web frontend for herdr-mcp.
//!
//! Thin client over the frozen HTTP bridge (`/api/*`, same origin) via the
//! shared `herdr-mcp-api` client. No server functions, no cached server state.

mod client;
mod components;
mod local;
mod vars;

use components::Route;
use dioxus::prelude::*;

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    // Built once from the page origin and shared via context. The result is
    // fixed for the app's lifetime, so the branch (and the hook inside it)
    // is identical on every render.
    let api = use_hook(|| client::api().map_err(|e| e.to_string()));
    match api {
        Ok(c) => {
            use_context_provider(|| Signal::new(c));
            rsx! {
                document::Stylesheet { href: asset!("/assets/app.css") }
                Router::<Route> {}
            }
        }
        Err(e) => rsx! {
            document::Stylesheet { href: asset!("/assets/app.css") }
            div { class: "alert alert-error", "Cannot locate the herdr-mcp bridge: {e}" }
        },
    }
}

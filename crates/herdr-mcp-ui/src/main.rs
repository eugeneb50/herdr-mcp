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
            // One authenticated call says whether the bridge wants a token
            // this page lacks (or has wrong).
            let probe = c.clone();
            let locked = use_resource(move || {
                let probe = probe.clone();
                async move {
                    matches!(
                        probe.config().await,
                        Err(client::ApiError::Status { status: 401, .. })
                    )
                }
            });
            use_context_provider(|| Signal::new(c));
            rsx! {
                document::Stylesheet { href: asset!("/assets/app.css") }
                if locked() == Some(true) {
                    TokenGate {}
                } else {
                    Router::<Route> {}
                }
            }
        }
        Err(e) => rsx! {
            document::Stylesheet { href: asset!("/assets/app.css") }
            div { class: "alert alert-error", "Cannot locate the herdr-mcp bridge: {e}" }
        },
    }
}

/// Asks for the bridge token: the bridge requires it (`[security]`) and
/// this page has none, or a stale one.
#[component]
fn TokenGate() -> Element {
    let mut token = use_signal(String::new);
    rsx! {
        div { class: "card",
            h2 { class: "card-title", "This bridge needs its token" }
            p {
                "Open the link herdr-mcp printed at startup, or paste the token from "
                code { "bridge.token" }
                " in its data dir."
            }
            input {
                r#type: "password",
                placeholder: "token",
                value: "{token}",
                oninput: move |e| token.set(e.value()),
            }
            button {
                class: "btn btn-primary btn-block",
                disabled: token().trim().is_empty(),
                onclick: move |_| client::set_token(&token()),
                "Use token"
            }
        }
    }
}

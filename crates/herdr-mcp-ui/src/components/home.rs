//! Landing page: hero + install. Static port of `Hero`/`Install`.

use dioxus::prelude::*;

#[component]
pub fn Home() -> Element {
    rsx! {
        section { class: "hero",
            div { class: "eyebrow", "herdr-mcp" }
            h1 { class: "hero-title", "Agents for the multiplexer" }
            p { class: "lede",
                "An MCP server that exposes herdr workspaces, tabs, panes and agents as tools — with a trim pipeline, recipe engine and live dashboard."
            }
            div { class: "hero-actions",
                a { class: "btn btn-primary", href: "/overview", "Open overview" }
                a { class: "btn", href: "/docs", "Read the docs" }
            }
        }
        section { class: "card",
            h2 { class: "card-title", "Install" }
            pre { class: "code-block",
                "cargo install --path crates/herdr-mcp-cli --bin herdr-mcp\nherdr-mcp serve --http 7676"
            }
            p { class: "muted",
                "Requires the server running with HTTP enabled for this UI."
            }
        }
    }
}

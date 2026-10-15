//! Docs page: static reference port of `Documentation`.

use dioxus::prelude::*;

#[component]
pub fn Docs() -> Element {
    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Docs" }
            h1 { class: "page-title", "Documentation" }
            p { class: "lede",
                "Core concepts: workspaces scope variables, panes run agents, recipes chain tools with step-result interpolation."
            }
        }
        section { class: "card",
            h2 { class: "card-title", "Key endpoints" }
            ul { class: "doc-list",
                li { code { "GET /api/agents" } " — live registry pane list" }
                li { code { "POST /api/tools/:name" } " — invoke any MCP tool" }
                li { code { "POST /api/recipe" } " — run a multi-step recipe" }
                li { code { "GET /api/variables/auto" } " — auto-populated pane variables" }
                li { code { "GET /api/trim/status" } " — aggregate trim savings" }
            }
        }
    }
}

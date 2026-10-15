//! Overview page: live agents from the canonical `AgentRegistry` via
//! `GET /api/agents`, workspaces via `GET /api/workspaces`. Port of `Overview`.

use dioxus::prelude::*;

use crate::client::use_api;

fn cell(s: &str) -> &str {
    if s.is_empty() { "—" } else { s }
}

#[component]
pub fn Overview() -> Element {
    let client = use_api();
    let agents = use_resource(move || {
        let client = client();
        async move { client.list_agents(None).await }
    });

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Overview" }
            h1 { class: "page-title", "Live agents" }
            p { class: "lede",
                "Canonical pane list straight from the server AgentRegistry."
            }
        }
        match &*agents.read_unchecked() {
            None => rsx! { p { class: "muted", "Loading agents…" } },
            Some(Err(e)) => rsx! {
                div { class: "alert alert-error",
                    "Could not reach the API: {e} — is the daemon serving --http?"
                }
            },
            Some(Ok(list)) => {
                rsx! {
                    section { class: "card",
                        h2 { class: "card-title",
                            "Agents ({list.len()})"
                        }
                        if list.is_empty() {
                            p { class: "muted", "No agents registered." }
                        } else {
                            table { class: "table",
                                thead {
                                    tr {
                                        th { "pane" }
                                        th { "role" }
                                        th { "agent" }
                                        th { "status" }
                                    }
                                }
                                tbody {
                                    for a in list {
                                        tr {
                                            td { class: "mono", "{cell(&a.pane_id)}" }
                                            td { class: "mono", "{cell(&a.role)}" }
                                            td { "{cell(&a.agent)}" }
                                            td {
                                                span { class: "pill", "{cell(&a.status)}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

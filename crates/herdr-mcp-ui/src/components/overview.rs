//! Overview page: herdr's panes (`GET /api/herdr/panes`) in any running
//! session (`GET /api/herdr/sessions`), filtered by the server, grouped by
//! workspace and tab. In the bridge's own session each pane also shows its
//! registry role (`GET /api/agents`).

use std::collections::HashMap;

use dioxus::prelude::*;
use herdr_mcp_api::{HerdrPane, PaneFilter};

use crate::client::use_api;

const STATUSES: [&str; 5] = ["idle", "working", "blocked", "done", "unknown"];

fn cell(s: &str) -> &str {
    if s.is_empty() { "—" } else { s }
}

fn some(s: String) -> Option<String> {
    Some(s).filter(|s| !s.is_empty())
}

/// Runs of panes sharing a workspace and tab, in herdr's order.
fn by_tab(panes: &[HerdrPane]) -> Vec<(String, Vec<&HerdrPane>)> {
    let mut groups: Vec<(String, Vec<&HerdrPane>)> = Vec::new();
    for p in panes {
        let key = format!("{} / {}", p.workspace_id, p.tab_id);
        match groups.last_mut() {
            Some((k, g)) if *k == key => g.push(p),
            _ => groups.push((key, vec![p])),
        }
    }
    groups
}

#[component]
pub fn Overview() -> Element {
    let client = use_api();
    let mut session: Signal<String> = use_signal(String::new);
    let mut status: Signal<String> = use_signal(String::new);
    let mut label: Signal<String> = use_signal(String::new);
    let mut refresh: Signal<u32> = use_signal(|| 0);

    let sessions = use_resource(move || {
        let client = client();
        async move {
            let _ = refresh.read();
            client.list_sessions().await
        }
    });
    let panes = use_resource(move || {
        let client = client();
        let filter = PaneFilter {
            session: some(session()),
            agent_status: some(status()),
            label: some(label()),
            ..Default::default()
        };
        async move {
            let _ = refresh.read();
            client.list_panes(&filter).await
        }
    });
    // Registry roles exist only for the bridge's own session.
    let roles = use_resource(move || {
        let client = client();
        let own = session.read().is_empty();
        async move {
            let _ = refresh.read();
            let agents = if own {
                client.list_agents(None).await.unwrap_or_default()
            } else {
                Vec::new()
            };
            agents
                .into_iter()
                .filter(|a| !a.role.is_empty())
                .map(|a| (a.pane_id, a.role))
                .collect::<HashMap<_, _>>()
        }
    });
    let roles = roles.read_unchecked().clone().unwrap_or_default();

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Overview" }
            h1 { class: "page-title", "Panes" }
            p { class: "lede",
                "Every pane herdr reports, in any running session. Pick another session to look inside it; writes still go to this bridge's own."
            }
        }
        div { class: "form-row",
            div { class: "form-col",
                label { class: "form-label", "Session" }
                select {
                    class: "input",
                    value: "{session}",
                    onchange: move |e| session.set(e.value()),
                    match &*sessions.read_unchecked() {
                        Some(Ok(list)) => rsx! {
                            for s in list.iter().filter(|s| s.running) {
                                option {
                                    key: "{s.name}",
                                    // The bridge's own session is the empty value.
                                    value: if s.current { "" } else { "{s.name}" },
                                    if s.current { "{s.name} (this bridge)" } else { "{s.name}" }
                                }
                            }
                        },
                        _ => rsx! { option { value: "", "this bridge's session" } },
                    }
                }
            }
            div { class: "form-col",
                label { class: "form-label", "Status" }
                select {
                    class: "input",
                    value: "{status}",
                    onchange: move |e| status.set(e.value()),
                    option { value: "", "any" }
                    for s in STATUSES {
                        option { key: "{s}", value: "{s}", "{s}" }
                    }
                }
            }
            div { class: "form-col",
                label { class: "form-label", "Label or agent" }
                input {
                    class: "input",
                    r#type: "text",
                    placeholder: "Filter panes...",
                    value: "{label}",
                    oninput: move |e| label.set(e.value()),
                }
            }
            button {
                class: "btn tab-refresh",
                onclick: move |_| *refresh.write() += 1,
                "↻"
            }
        }
        match &*panes.read_unchecked() {
            None => rsx! { p { class: "muted", "Loading panes…" } },
            Some(Err(e)) => rsx! {
                div { class: "alert alert-error", "Could not list panes: {e}" }
            },
            Some(Ok(list)) if list.is_empty() => rsx! {
                section { class: "card",
                    h2 { class: "card-title", "Panes (0)" }
                    p { class: "muted", "No panes match." }
                }
            },
            Some(Ok(list)) => rsx! {
                for (group, rows) in by_tab(list) {
                    section { class: "card", key: "{group}",
                        h2 { class: "card-title mono", "{group} ({rows.len()})" }
                        table { class: "table",
                            thead {
                                tr {
                                    th { "pane" }
                                    th { "label" }
                                    th { "agent" }
                                    th { "status" }
                                    th { "role" }
                                    th { "cwd" }
                                }
                            }
                            tbody {
                                for p in rows {
                                    tr { key: "{p.pane_id}",
                                        td { class: "mono",
                                            if p.focused { "● {p.pane_id}" } else { "{p.pane_id}" }
                                        }
                                        td { "{cell(p.label.as_deref().unwrap_or_default())}" }
                                        td { "{cell(p.agent.as_deref().unwrap_or_default())}" }
                                        td { span { class: "pill", "{cell(&p.agent_status)}" } }
                                        td { class: "mono",
                                            "{cell(roles.get(&p.pane_id).map(String::as_str).unwrap_or_default())}"
                                        }
                                        td { class: "mono muted", "{cell(p.cwd.as_deref().unwrap_or_default())}" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_runs_of_the_same_tab() {
        let p = |id: &str, tab: &str| -> HerdrPane {
            serde_json::from_value(serde_json::json!({
                "pane_id": id, "workspace_id": "w1", "tab_id": tab
            }))
            .unwrap()
        };
        let panes = [p("a", "t1"), p("b", "t1"), p("c", "t2")];
        let groups = by_tab(&panes);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "w1 / t1");
        assert_eq!(groups[0].1.len(), 2);
    }
}

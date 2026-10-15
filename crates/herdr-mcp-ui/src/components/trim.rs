//! Trim dashboard: aggregate savings, per-pane bars, active policies,
//! diagnose/summary actions. Port of `TrimDashboard` (polls every 5 s server-side
//! via re-fetch; a refresh button re-runs the resource).

use dioxus::prelude::*;

use crate::client::{fmt_bytes, use_api};

#[component]
pub fn Trim() -> Element {
    let client = use_api();
    let mut refresh: Signal<u32> = use_signal(|| 0);
    let status = use_resource(move || {
        let client = client();
        async move {
            // Re-subscribe on refresh: reading the signal tracks it.
            let _ = refresh.read();
            client.trim_status(None).await
        }
    });
    let mut toast: Signal<Option<String>> = use_signal(|| None);

    let run_action = move |action: &'static str| {
        let client = client();
        spawn(async move {
            let res = match action {
                "diagnose" => client.trim_diagnose(None).await.map(|_| ()),
                "summary" => client.trim_summary(None).await.map(|_| ()),
                "open" => client.trim_dashboard_open(None).await.map(|_| ()),
                _ => return,
            };
            match res {
                Ok(_) => toast.set(Some(format!("{action}: ok"))),
                Err(e) => toast.set(Some(format!("{action} failed: {e}"))),
            }
        });
    };

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow eyebrow-live", "Trim Dashboard" }
            h1 { class: "page-title", "Message-trim savings" }
            p { class: "lede",
                "Live aggregate of bytes saved by the caveman + PFC1 pipeline."
            }
        }
        if let Some(msg) = toast.read().as_deref() {
            div { class: "alert", "{msg}" }
        }
        match &*status.read_unchecked() {
            None => rsx! { p { class: "muted", "Loading trim status…" } },
            Some(Err(e)) => rsx! {
                div { class: "alert alert-error",
                    "Could not reach the trim API: {e}"
                }
            },
            Some(Ok(v)) => {
                let max_net = v.per_pane.values().map(|p| p.net_saved_bytes).max().unwrap_or(1);
                let savings = v.workspace_savings_pct;
                rsx! {
                    div { class: "btn-row",
                        button { class: "btn", onclick: move |_| run_action("diagnose"), "▶ Run Diagnostics" }
                        button { class: "btn", onclick: move |_| run_action("summary"), "▶ Send Summary" }
                        button { class: "btn", onclick: move |_| refresh += 1, "↻ Refresh" }
                    }
                    div { class: "stat-grid",
                        div { class: "stat",
                            div { class: "stat-label", "Savings %" }
                            div { class: "stat-value accent",
                                if savings > 0.0 { "-{savings:.1}%" } else { "—" }
                            }
                        }
                        div { class: "stat",
                            div { class: "stat-label", "Net saved" }
                            div { class: "stat-value", "{fmt_bytes(v.net_saved_bytes)}B" }
                        }
                        div { class: "stat",
                            div { class: "stat-label", "Gross saved" }
                            div { class: "stat-value", "{fmt_bytes(v.gross_saved_bytes)}B" }
                        }
                        div { class: "stat",
                            div { class: "stat-label", "Messages" }
                            div { class: "stat-value", "{v.messages_trimmed}" }
                        }
                    }
                    section { class: "card",
                        h2 { class: "card-title", "Per-pane" }
                        if v.per_pane.is_empty() {
                            p { class: "muted", "No trimmed panes yet." }
                        } else {
                                div { class: "bar-list",
                                    for (pane, p) in v.per_pane.iter() {
                                        {
                                            let net = p.net_saved_bytes;
                                            let pct = ((net as f64 / max_net.max(1) as f64) * 100.0).max(2.0) as u64;
                                            rsx! {
                                                div { class: "bar-row",
                                                    span { class: "mono bar-label", "{pane}" }
                                                    div { class: "bar-track",
                                                        div { class: "bar-fill", style: "width: {pct}%" }
                                                    }
                                                    span { class: "mono bar-value", "{fmt_bytes(net)}B" }
                                                }
                                            }
                                        }
                                    }
                                }
                        }
                    }
                    section { class: "card",
                        h2 { class: "card-title", "Active policies" }
                        if v.active_policies.is_empty() {
                            p { class: "muted", "No active trim policies." }
                        } else {
                                ul { class: "policy-list",
                                    for (pane, stages) in v.active_policies.iter() {
                                        li {
                                            span { class: "mono", "{pane} " }
                                            for s in stages.iter() {
                                                span { class: "pill", "{s}" }
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

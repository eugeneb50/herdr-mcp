//! Send page, the landing page: type text into a pane picked by label —
//! `send_text`, or `run_command` (text + Enter). Panes come from
//! `GET /api/herdr/panes` (this bridge's session, labeled ones first).

use dioxus::prelude::*;
use herdr_mcp_api::HerdrPane;

use crate::client::use_api;

/// Browser-local key for the last pane sent to (its label, else its id).
const LAST_PANE: &str = "herdr-mcp.send.pane";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Labeled panes first, each group in herdr's order.
fn ordered(mut panes: Vec<HerdrPane>) -> Vec<HerdrPane> {
    panes.sort_by_key(|p| p.label.is_none());
    panes
}

/// `builder · claude · /home/me/app`, or the pane id for an unlabeled shell.
fn option_text(p: &HerdrPane) -> String {
    let head = p.label.clone().unwrap_or_else(|| p.pane_id.clone());
    [Some(head), p.agent.clone(), p.cwd.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The pane to preselect: the remembered one (by label, then id), else the first.
fn initial(panes: &[HerdrPane], remembered: Option<&str>) -> Option<String> {
    let hit = remembered.and_then(|r| {
        panes
            .iter()
            .find(|p| p.label.as_deref() == Some(r))
            .or_else(|| panes.iter().find(|p| p.pane_id == r))
    });
    hit.or(panes.first()).map(|p| p.pane_id.clone())
}

#[component]
pub fn Send() -> Element {
    let client = use_api();
    let mut selected: Signal<String> = use_signal(String::new);
    let mut text: Signal<String> = use_signal(String::new);
    let mut enter: Signal<bool> = use_signal(|| true);
    let mut status: Signal<Option<Result<String, String>>> = use_signal(|| None);
    let mut busy: Signal<bool> = use_signal(|| false);
    let mut refresh: Signal<u32> = use_signal(|| 0);

    let panes = use_resource(move || {
        let client = client();
        async move {
            let _ = refresh.read();
            let panes = ordered(client.list_panes(&Default::default()).await?);
            let remembered = storage().and_then(|s| s.get_item(LAST_PANE).ok().flatten());
            // Keep the current pick across refreshes while it still exists.
            let current = selected.peek().clone();
            if !panes.iter().any(|p| p.pane_id == current) {
                selected.set(initial(&panes, remembered.as_deref()).unwrap_or_default());
            }
            Ok::<_, herdr_mcp_api::ApiError>(panes)
        }
    });

    let mut send = move || {
        let body = text();
        if body.is_empty() || busy() {
            return;
        }
        let pane = match &*panes.read_unchecked() {
            Some(Ok(list)) => list.iter().find(|p| p.pane_id == selected()).cloned(),
            _ => return,
        };
        let Some(pane) = pane else {
            status.set(Some(Err("pick a pane first".into())));
            return;
        };
        let name = pane.label.clone().unwrap_or_else(|| pane.pane_id.clone());
        let press_enter = enter();
        let client = client();
        busy.set(true);
        spawn(async move {
            match client.send_to_pane(&pane.pane_id, &body, press_enter).await {
                Ok(()) => {
                    if let Some(s) = storage() {
                        let _ = s.set_item(LAST_PANE, &name);
                    }
                    let how = if press_enter { " + Enter" } else { "" };
                    status.set(Some(Ok(format!(
                        "sent {} chars{how} to {name} ({})",
                        body.chars().count(),
                        pane.pane_id
                    ))));
                    text.set(String::new());
                }
                Err(e) => status.set(Some(Err(format!("send failed: {e}")))),
            }
            busy.set(false);
        });
    };

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Send" }
            h1 { class: "page-title", "Send to pane" }
            p { class: "lede",
                "Type into a herdr pane by its label. Enter sends, Shift+Enter adds a line."
            }
        }
        section { class: "card",
            div { class: "form-row",
                div { class: "form-col",
                    label { class: "form-label", "Pane" }
                    match &*panes.read_unchecked() {
                        None => rsx! { p { class: "muted", "Loading panes…" } },
                        Some(Err(e)) => rsx! {
                            div { class: "alert alert-error", "Could not list panes: {e}" }
                        },
                        Some(Ok(list)) if list.is_empty() => rsx! {
                            p { class: "muted", "herdr reports no panes." }
                        },
                        Some(Ok(list)) => rsx! {
                            select {
                                class: "input",
                                value: "{selected}",
                                onchange: move |e| selected.set(e.value()),
                                for p in list {
                                    option {
                                        key: "{p.pane_id}",
                                        value: "{p.pane_id}",
                                        selected: *selected.read() == p.pane_id,
                                        "{option_text(p)}"
                                    }
                                }
                            }
                        },
                    }
                }
                button {
                    class: "btn tab-refresh",
                    title: "Reload panes",
                    onclick: move |_| *refresh.write() += 1,
                    "↻"
                }
            }
            div { class: "form-block",
                label { class: "form-label", "Text" }
                textarea {
                    class: "input textarea",
                    rows: "4",
                    autofocus: true,
                    placeholder: "run the tests again",
                    value: "{text}",
                    oninput: move |e| text.set(e.value()),
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter && !e.modifiers().shift() {
                            e.prevent_default();
                            send();
                        }
                    },
                }
            }
            div { class: "btn-row",
                label { class: "check-row",
                    input {
                        r#type: "checkbox",
                        checked: enter(),
                        onchange: move |_| enter.toggle(),
                    }
                    span { class: "muted", "press Enter after" }
                }
                button {
                    class: "btn btn-primary",
                    disabled: busy() || text.read().is_empty(),
                    onclick: move |_| send(),
                    if busy() { "Sending…" } else { "Send" }
                }
            }
            match &*status.read() {
                Some(Ok(msg)) => rsx! { div { class: "alert", "{msg}" } },
                Some(Err(msg)) => rsx! { div { class: "alert alert-error", "{msg}" } },
                None => rsx! {},
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, label: Option<&str>) -> HerdrPane {
        serde_json::from_value(serde_json::json!({
            "pane_id": id, "workspace_id": "w1", "tab_id": "w1:t1", "label": label,
            "agent": "claude"
        }))
        .unwrap()
    }

    #[test]
    fn labeled_first_and_remembered_pick() {
        let panes = ordered(vec![pane("w1:p1", None), pane("w1:p2", Some("builder"))]);
        assert_eq!(panes[0].pane_id, "w1:p2");
        assert_eq!(option_text(&panes[0]), "builder · claude");
        assert_eq!(option_text(&panes[1]), "w1:p1 · claude");
        assert_eq!(initial(&panes, Some("builder")).as_deref(), Some("w1:p2"));
        assert_eq!(initial(&panes, Some("w1:p1")).as_deref(), Some("w1:p1"));
        assert_eq!(initial(&panes, Some("gone")).as_deref(), Some("w1:p2"));
        assert_eq!(initial(&[], None), None);
    }
}

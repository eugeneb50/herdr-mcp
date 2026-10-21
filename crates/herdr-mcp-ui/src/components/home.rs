//! Home page, the landing page: send text to a pane, getting-started notes,
//! and the live pane list. The pane is addressed like the tool runner
//! addresses it — `pane_id` and/or `label`, typed or picked from the panes
//! and names the auto variables hold (`GET /api/variables/auto`), which also
//! fill the pane list. The sent text stays in the box for re-sending; with
//! highlighting on, a `<pre>` drawn under the transparent textarea colors it.

use dioxus::prelude::*;
use herdr_mcp_api::HerdrPane;

use super::highlight::tokens;
use super::pane_options::{PaneOptions, pane_list};
use crate::client::use_api;
use crate::vars::auto_panes;

/// Browser-local keys for the last target sent to, as typed.
const LAST_PANE_ID: &str = "herdr-mcp.send.pane_id";
const LAST_LABEL: &str = "herdr-mcp.send.label";
/// Browser-local "highlight code" toggle, `"1"` when on.
const HIGHLIGHT: &str = "herdr-mcp.send.highlight";
/// The highlight layer, scrolled along with the textarea.
const HIGHLIGHT_ID: &str = "send-highlight";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn remembered(key: &str) -> String {
    storage()
        .and_then(|s| s.get_item(key).ok().flatten())
        .unwrap_or_default()
}

fn remember(key: &str, value: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(key, value);
    }
}

/// A field as a tool parameter: trimmed, absent when blank.
fn param(s: &str) -> Option<&str> {
    Some(s.trim()).filter(|s| !s.is_empty())
}

/// Whether `p` is the pane the form targets (`pane_id` wins, as in the tools).
fn targeted(p: &HerdrPane, pane_id: &str, label: &str) -> bool {
    match (param(pane_id), param(label)) {
        (Some(id), _) => p.pane_id == id,
        (None, Some(l)) => p.label.as_deref() == Some(l),
        (None, None) => false,
    }
}

#[component]
pub fn Home() -> Element {
    let client = use_api();
    let mut pane_id: Signal<String> = use_signal(|| remembered(LAST_PANE_ID));
    let mut label: Signal<String> = use_signal(|| remembered(LAST_LABEL));
    let mut text: Signal<String> = use_signal(String::new);
    let mut enter: Signal<bool> = use_signal(|| true);
    let mut highlight: Signal<bool> = use_signal(|| remembered(HIGHLIGHT) == "1");
    let mut status: Signal<Option<Result<String, String>>> = use_signal(|| None);
    let mut busy: Signal<bool> = use_signal(|| false);
    let mut refresh: Signal<u32> = use_signal(|| 0);

    let auto = use_resource(move || {
        let client = client();
        async move {
            let _ = refresh.read();
            client.auto_variables(None).await.map(|a| a.variables)
        }
    });
    let vars = match &*auto.read() {
        Some(Ok(vars)) => vars.clone(),
        _ => Default::default(),
    };
    let panes = auto_panes(&vars);

    let mut send = move || {
        let body = text();
        let (id, lbl) = (pane_id(), label());
        if body.is_empty() || busy() {
            return;
        }
        if param(&id).is_none() && param(&lbl).is_none() {
            status.set(Some(Err("set a pane_id or label first".into())));
            return;
        }
        let press_enter = enter();
        let client = client();
        busy.set(true);
        spawn(async move {
            match client
                .send_to_pane(param(&id), param(&lbl), &body, press_enter)
                .await
            {
                Ok(()) => {
                    remember(LAST_PANE_ID, &id);
                    remember(LAST_LABEL, &lbl);
                    let how = if press_enter { " + Enter" } else { "" };
                    let to = param(&id).or(param(&lbl)).unwrap_or_default();
                    status.set(Some(Ok(format!(
                        "sent {} chars{how} to {to}",
                        body.chars().count()
                    ))));
                    *refresh.write() += 1;
                }
                Err(e) => status.set(Some(Err(format!("send failed: {e}")))),
            }
            busy.set(false);
        });
    };

    let dash = |s: &Option<String>| s.clone().unwrap_or_else(|| "—".into());

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Home" }
            h1 { class: "page-title", "Send to a pane" }
            p { class: "lede",
                "Address a pane by pane_id or label — type it, pick it, or click a pane below. Enter sends, Shift+Enter adds a line."
            }
            details { class: "details",
                summary { "Getting started" }
                pre { class: "code-block",
                    "cargo install --path crates/herdr-mcp-cli --bin herdr-mcp\nherdr-mcp serve --http 7676"
                }
                ul { class: "doc-list",
                    li { code { "POST /api/tools/:name" } " — invoke any MCP tool" }
                    li { code { "POST /api/runs?wait=true" } " — run a recipe (saved or ad-hoc steps) and get the result" }
                    li { code { "GET /api/variables/auto" } " — live panes and agents as variables" }
                    li { code { "GET /api/herdr/panes" } " — herdr panes (session/status/label filters)" }
                    li { code { "GET /api/trim/status" } " — aggregate trim savings" }
                }
            }
        }
        PaneOptions { auto: vars }
        section { class: "card",
            div { class: "form-row",
                div { class: "form-col",
                    label { class: "field-head", code { class: "mono accent", "pane_id" } }
                    input {
                        class: "input",
                        r#type: "text",
                        list: pane_list("pane_id"),
                        placeholder: "w1:p2",
                        value: "{pane_id}",
                        oninput: move |e| pane_id.set(e.value()),
                    }
                }
                div { class: "form-col",
                    label { class: "field-head",
                        code { class: "mono accent", "label" }
                        span { class: "field-desc", "used when pane_id is empty" }
                    }
                    input {
                        class: "input",
                        r#type: "text",
                        list: pane_list("label"),
                        placeholder: "builder",
                        value: "{label}",
                        oninput: move |e| label.set(e.value()),
                    }
                }
            }
            div { class: if highlight() { "editor editor-hl" } else { "editor" },
                if highlight() {
                    pre { id: HIGHLIGHT_ID, class: "input textarea hl-layer", "aria-hidden": "true",
                        for (kind , run) in tokens(&text.read()) {
                            span { class: kind.class(), "{run}" }
                        }
                        // A trailing newline needs a following char to take up a line.
                        " "
                    }
                }
            textarea {
                class: "input textarea",
                rows: "3",
                spellcheck: !highlight(),
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
                onscroll: move |e| {
                    if let Some(layer) = web_sys::window()
                        .and_then(|w| w.document())
                        .and_then(|d| d.get_element_by_id(HIGHLIGHT_ID))
                    {
                        layer.set_scroll_top(e.scroll_top() as i32);
                        layer.set_scroll_left(e.scroll_left() as i32);
                    }
                },
            }
            }
            div { class: "send-row",
                label { class: "check-row",
                    input {
                        r#type: "checkbox",
                        checked: enter(),
                        onchange: move |_| enter.toggle(),
                    }
                    span { class: "muted", "press Enter after" }
                }
                label { class: "check-row",
                    input {
                        r#type: "checkbox",
                        checked: highlight(),
                        onchange: move |_| {
                            highlight.toggle();
                            remember(HIGHLIGHT, if highlight() { "1" } else { "" });
                        },
                    }
                    span { class: "muted", "highlight code" }
                }
                match &*status.read() {
                    Some(Ok(msg)) => rsx! { span { class: "send-status", "{msg}" } },
                    Some(Err(msg)) => rsx! { span { class: "send-status send-error", "{msg}" } },
                    None => rsx! {},
                }
                button {
                    class: "btn",
                    disabled: text.read().is_empty(),
                    onclick: move |_| {
                        text.set(String::new());
                        status.set(None);
                    },
                    "Clear"
                }
                button {
                    class: "btn btn-primary",
                    disabled: busy() || text.read().is_empty(),
                    onclick: move |_| send(),
                    if busy() { "Sending…" } else { "Send" }
                }
            }
        }
        section { class: "card",
            div { class: "card-head",
                h2 { class: "card-title", "Panes · {panes.len()}" }
                span { class: "field-desc", "click a pane to target it" }
                button {
                    class: "btn tab-refresh",
                    title: "Reload panes",
                    onclick: move |_| *refresh.write() += 1,
                    "↻"
                }
            }
            match &*auto.read() {
                None => rsx! { p { class: "muted", "Loading panes…" } },
                Some(Err(e)) => rsx! {
                    div { class: "alert alert-error", "Could not load the auto variables: {e}" }
                },
                Some(Ok(_)) if panes.is_empty() => rsx! {
                    p { class: "muted", "herdr reports no panes." }
                },
                Some(Ok(_)) => rsx! {
                    table { class: "table",
                        thead {
                            tr {
                                th { "label" }
                                th { "pane_id" }
                                th { "agent" }
                                th { "status" }
                                th { "cwd" }
                            }
                        }
                        tbody {
                            for p in panes.iter().cloned() {
                                tr {
                                    key: "{p.pane_id}",
                                    class: if targeted(&p, &pane_id.read(), &label.read()) { "row-pick row-sel" } else { "row-pick" },
                                    onclick: move |_| {
                                        pane_id.set(p.pane_id.clone());
                                        label.set(String::new());
                                    },
                                    td { "{dash(&p.label)}" }
                                    td { code { class: "mono", "{p.pane_id}" } }
                                    td { "{dash(&p.agent)}" }
                                    td { span { class: "pill", "{p.agent_status}" } }
                                    td { class: "muted cell-path", title: "{dash(&p.cwd)}", "{dash(&p.cwd)}" }
                                }
                            }
                        }
                    }
                },
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
    fn target_match() {
        let (unlabeled, builder) = (pane("w1:p1", None), pane("w1:p2", Some("builder")));
        assert!(targeted(&builder, "", " builder "));
        assert!(targeted(&unlabeled, "w1:p1", "builder"));
        assert!(!targeted(&builder, "w1:p1", "builder"));
        assert!(!targeted(&unlabeled, "", ""));
        assert_eq!(param("  "), None);
    }
}

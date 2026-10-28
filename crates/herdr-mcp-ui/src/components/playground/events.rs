//! herdr's event stream (`GET /api/herdr/events`, SSE: the server's last
//! 200 events, then live ones): what recipe triggers can fire on, newest
//! first.

use dioxus::prelude::*;
use herdr_mcp_api::HerdrEvent;
use wasm_bindgen::{JsCast, closure::Closure};

/// Events kept on screen.
const KEEP: usize = 200;

/// An `EventSource` on the bridge's `path`, calling `on_message` with each
/// message's data and `on_open` on every (re)connect. Close it on drop.
pub fn sse(
    path: &str,
    mut on_message: impl FnMut(String) + 'static,
    on_open: impl FnMut() + 'static,
) -> Option<web_sys::EventSource> {
    // EventSource can't send headers: the token goes in the query.
    let url = match crate::client::token() {
        Some(t) => format!("{path}?token={t}"),
        None => path.to_string(),
    };
    let es = web_sys::EventSource::new(&url).ok()?;
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |msg: web_sys::MessageEvent| {
            if let Some(text) = msg.data().as_string() {
                on_message(text);
            }
        });
    let on_open = Closure::<dyn FnMut()>::new(on_open);
    es.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    es.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    // They live as long as the page's EventSource.
    on_message.forget();
    on_open.forget();
    Some(es)
}

#[component]
pub fn HerdrEvents() -> Element {
    let mut events: Signal<Vec<HerdrEvent>> = use_signal(Vec::new);
    let mut filter: Signal<String> = use_signal(String::new);
    let mut source: Signal<Option<web_sys::EventSource>> = use_signal(|| None);

    use_hook(move || {
        let es = sse(
            "/api/herdr/events",
            move |text| {
                if let Ok(ev) = serde_json::from_str::<HerdrEvent>(&text) {
                    let mut w = events.write();
                    w.insert(0, ev);
                    w.truncate(KEEP);
                }
            },
            // Every (re)connect starts with the server's recent events.
            move || events.write().clear(),
        );
        source.set(es);
    });
    use_drop(move || {
        if let Some(es) = source.peek().as_ref() {
            es.close();
        }
    });

    let needle = filter.read().clone();
    let shown: Vec<HerdrEvent> = events
        .read()
        .iter()
        .filter(|e| needle.is_empty() || e.event.contains(&needle))
        .cloned()
        .collect();

    rsx! {
        p { class: "field-desc",
            "herdr's last 200 events, then live ones. A recipe trigger fires on the event name, optionally filtered by values in its data (e.g. "
            code { "agent_status=done" }
            "); the run gets the data as "
            code { "{{{{herdr.…}}}}" }
            "."
        }
        div { class: "btn-row",
            input {
                class: "input",
                r#type: "text",
                placeholder: "Filter by event name…",
                value: "{filter}",
                oninput: move |e| filter.set(e.value()),
            }
            button { class: "btn", onclick: move |_| events.write().clear(), "Clear" }
        }
        if shown.is_empty() {
            p { class: "muted", "Waiting for herdr events… (create a pane, switch tabs, or run an agent)" }
        }
        for (i, ev) in shown.into_iter().enumerate() {
            div { key: "{i}", class: "card",
                div { class: "response-head",
                    code { class: "mono accent", "{ev.event}" }
                    if let Some(ws) = ev.workspace_id() {
                        span { class: "pill", "{ws}" }
                    }
                }
                pre { class: "mono", "{serde_json::to_string_pretty(&ev.data).unwrap_or_default()}" }
            }
        }
    }
}

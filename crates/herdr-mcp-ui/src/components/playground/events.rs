//! herdr's event stream (`GET /api/herdr/events?level=`, SSE: the server's
//! recent entries, then live ones): what recipe triggers can fire on, newest
//! first, plus herdr-mcp's own diagnostic entries at the higher levels.

use dioxus::prelude::*;
use herdr_mcp_api::{EventLevel, HerdrEvent};
use wasm_bindgen::{JsCast, closure::Closure};

/// Entries kept on screen (diagnostics included).
const KEEP: usize = 500;

/// An `EventSource` on the bridge's `path`, calling `on_message` with each
/// message's data and `on_open` on every (re)connect. Close it on drop.
pub fn sse(
    path: &str,
    mut on_message: impl FnMut(String) + 'static,
    on_open: impl FnMut() + 'static,
) -> Option<web_sys::EventSource> {
    // EventSource can't send headers: the token goes in the query.
    let url = match crate::client::token() {
        Some(t) => {
            let join = if path.contains('?') { '&' } else { '?' };
            format!("{path}{join}token={t}")
        }
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
    let mut level: Signal<EventLevel> = use_signal(EventLevel::default);

    // A new level is a new stream, which replays that level's recent entries.
    use_effect(move || {
        let path = format!("/api/herdr/events?level={}", level().as_str());
        if let Some(old) = source.peek().as_ref() {
            old.close();
        }
        events.write().clear();
        let es = sse(
            &path,
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
            "herdr's recent events, then live ones; the level adds what herdr-mcp did about them. A recipe trigger fires on the event name, optionally filtered by values in its data (e.g. "
            code { "agent_status=done" }
            "); the run gets the data as "
            code { "{{{{herdr.…}}}}" }
            "."
        }
        div { class: "btn-row",
            select {
                class: "input input-narrow",
                title: "events: herdr's own. reactions: + what herdr-mcp did about them (subscriber, triggers fired or skipped, agent status). debug: + every trigger checked and why it didn't match.",
                onchange: move |e| {
                    if let Some(l) = EventLevel::parse(&e.value()) {
                        level.set(l);
                    }
                },
                for l in EventLevel::ALL {
                    option { value: l.as_str(), selected: l == level(), "{l.as_str()}" }
                }
            }
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
                    if let Some(at) = ev.at {
                        span { class: "muted mono", {at.format("%H:%M:%S").to_string()} }
                    }
                    code {
                        class: if ev.level == EventLevel::Events { "mono accent" } else { "mono muted" },
                        "{ev.event}"
                    }
                    if ev.level != EventLevel::Events {
                        span { class: "pill", "{ev.level.as_str()}" }
                    }
                    if let Some(ws) = ev.workspace_id() {
                        span { class: "pill", "{ws}" }
                    }
                }
                pre { class: "mono", "{serde_json::to_string_pretty(&ev.data).unwrap_or_default()}" }
            }
        }
    }
}

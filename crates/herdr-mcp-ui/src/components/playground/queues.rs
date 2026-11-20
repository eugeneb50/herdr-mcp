use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use herdr_mcp_api::{
    ApiError, Uuid,
    recipe::QueueListQuery,
    settings::{Settings, UI_QUEUES_POLL_MS},
};
use serde_json::Value;
use std::collections::BTreeMap;

use super::queue_rows::{actions, item_id, selected_row};
use crate::{client::use_api, components::Route};

const ITEM_LIMIT: u32 = 100;

fn queue_link(queue: Option<String>, item: Option<String>) -> Route {
    Route::Playground {
        tab: Some("queues".into()),
        queue,
        item,
        run: None,
    }
}

fn field<'a>(value: &'a Value, name: &str) -> &'a str {
    value[name].as_str().unwrap_or("—")
}

fn utc(value: &Value) -> String {
    value
        .as_i64()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(
            || "—".into(),
            |time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        )
}

struct QueueRow {
    name: String,
    queued: i64,
    running: i64,
    dead: i64,
}

#[component]
pub(super) fn Queues(
    queue: Option<String>,
    item: Option<String>,
    mut status: Signal<String>,
) -> Element {
    let api = use_api();
    let mut interval = use_signal(|| Settings::default().number(UI_QUEUES_POLL_MS) as u32);
    let mut action_pending = use_signal(|| false);
    let mut action_message = use_signal(|| None::<Result<String, String>>);

    let list_queue = queue.clone();
    let detail_item = item.clone();
    let mut snapshot = use_resource(move || {
        let client = api();
        let queue = list_queue.clone();
        let item = detail_item.clone();
        let filter = status();
        async move {
            if let Ok(settings) = client.settings().await {
                interval.set(settings.overrides().number(UI_QUEUES_POLL_MS) as u32);
            }
            let depths = async {
                let mut queues = BTreeMap::new();
                for recipe in client.list_recipes().await? {
                    if recipe.queue.is_some() {
                        queues.insert(
                            recipe.id.to_string(),
                            QueueRow {
                                name: recipe.name,
                                queued: 0,
                                running: 0,
                                dead: 0,
                            },
                        );
                    }
                }
                for depth in client.queue_depths().await? {
                    let Some(id) = depth["queue"].as_str() else {
                        continue;
                    };
                    // Raw mailboxes are outside the collection queue view.
                    if Uuid::parse_str(id).is_err() {
                        continue;
                    }
                    let row = queues.entry(id.to_owned()).or_insert_with(|| QueueRow {
                        name: id.to_owned(),
                        queued: 0,
                        running: 0,
                        dead: 0,
                    });
                    row.queued = depth["queued"].as_i64().unwrap_or_default();
                    row.running = depth["running"].as_i64().unwrap_or_default();
                    row.dead = depth["dead"].as_i64().unwrap_or_default();
                }
                Ok::<_, ApiError>(queues)
            }
            .await;
            let rows = match queue {
                Some(queue) => {
                    client
                        .queue_list(
                            &queue,
                            &QueueListQuery {
                                status: (!filter.is_empty()).then_some(filter),
                                limit: Some(ITEM_LIMIT),
                            },
                        )
                        .await
                }
                None => Ok(Vec::new()),
            };
            let detail = match item {
                Some(id) => Some(client.queue_item(&id).await),
                None => None,
            };
            (depths, rows, detail)
        }
    });
    use_future(move || async move {
        loop {
            let delay = *interval.peek();
            TimeoutFuture::new(delay).await;
            let visible = web_sys::window()
                .and_then(|w| w.document())
                .is_some_and(|doc| !doc.hidden());
            if visible && !snapshot.pending() && !*action_pending.peek() {
                snapshot.restart();
            }
        }
    });

    let act = move |(id, retry): (String, bool)| {
        if *action_pending.peek() {
            return;
        }
        action_pending.set(true);
        action_message.set(None);
        let client = api();
        spawn(async move {
            let reply = if retry {
                client.queue_retry(&id).await
            } else {
                client.queue_cancel(&id).await
            };
            action_message.set(Some(
                reply
                    .map(|item| format!("Item status: {}", field(&item, "status")))
                    .map_err(|error| error.to_string()),
            ));
            action_pending.set(false);
            snapshot.restart();
        });
    };

    let data = snapshot.read();
    let Some((depths, rows, detail)) = data.as_ref() else {
        return rsx! { p { role: "status", "Loading queues…" } };
    };
    let active_row = rows
        .as_ref()
        .ok()
        .and_then(|rows| selected_row(rows, item.as_deref()));
    rsx! {
        div { class: "btn-row",
            button { class: "btn", onclick: move |_| snapshot.restart(), "Refresh queues" }
            if *snapshot.state().read() == UseResourceState::Pending {
                span { class: "muted", role: "status", "Refreshing…" }
            }
        }
        if let Ok(queues) = depths {
            div { class: "card",
                if queues.is_empty() { p { "No collection queues configured." } }
                for (id, row) in queues {
                    Link {
                        key: "{id}",
                        class: if queue.as_deref() == Some(id) { "tree-item tree-sel" } else { "tree-item" },
                        to: queue_link(Some(id.clone()), None),
                        span { class: "tree-name", "{row.name}" }
                        span { class: "muted", "{row.queued} queued · {row.running} running · {row.dead} dead" }
                    }
                }
            }
        } else if let Err(error) = depths {
            p { role: "alert", class: "send-error", "Queue counts unavailable. {error}" }
        }
        if let Some(queue) = &queue {
            div { class: "runs",
                div { class: "card",
                    label {
                        "Status "
                        select { class: "input input-narrow", value: status(), onchange: move |e| status.set(e.value()),
                            option { value: "", "All statuses" }
                            for choice in ["queued", "running", "done", "dead", "cancelled"] {
                                option { value: choice, "{choice}" }
                            }
                        }
                    }
                    match rows {
                        Err(error) => rsx! { p { role: "alert", class: "send-error", "Queue items unavailable. {error}" } },
                        Ok(rows) => rsx! {
                            p { class: "muted", "{rows.len()} items · up to {ITEM_LIMIT}" }
                            if rows.is_empty() { p { "No items match this filter." } }
                            for (index, row) in rows.iter().enumerate() {
                                if let Some(id) = item_id(row) {
                                    Link {
                                        key: "{id}",
                                        class: if active_row == Some(index) { "tree-item tree-sel run-row" } else { "tree-item run-row" },
                                        to: queue_link(Some(queue.clone()), Some(id.to_owned())),
                                        span { class: "pill", {field(row, "status")} }
                                        span { class: "tree-name mono", "{id}" }
                                        span { class: "muted", {field(row, "requester")} " · attempt " {row["attempts"].to_string()} }
                                        span { class: "muted", {utc(&row["enqueued_at"])} }
                                    }
                                }
                            }
                        },
                    }
                }
                div { class: "card",
                    match detail {
                        None => rsx! { p { class: "muted", "Pick an item to read its history." } },
                        Some(Err(error)) => rsx! { p { role: "alert", class: "send-error", "Item unavailable. {error}" } },
                        Some(Ok(detail)) => rsx! {
                            ItemHistory { item: detail.item.clone(), events: detail.events.clone(), pending: action_pending(), on_action: act }
                        },
                    }
                    if let Some(message) = action_message() {
                        match message {
                            Ok(text) => rsx! { p { role: "status", "{text}" } },
                            Err(error) => rsx! { p { role: "alert", class: "send-error", "{error}" } },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ItemHistory(
    item: Value,
    events: Vec<Value>,
    pending: bool,
    on_action: EventHandler<(String, bool)>,
) -> Element {
    let id = item_id(&item).unwrap_or_default().to_owned();
    let status = field(&item, "status");
    let (cancel, retry) = actions(status);
    let retry_id = id.clone();
    let cancel_id = id.clone();
    rsx! {
        h2 { "Item {id}" }
        p { "{status} · attempt " {item["attempts"].to_string()} " of " {item["max_attempts"].to_string()} }
        div { class: "btn-row",
            if cancel && !id.is_empty() {
                button { class: "btn", disabled: pending, onclick: move |_| on_action.call((cancel_id.clone(), false)), "Cancel item" }
            }
            if retry && !id.is_empty() {
                button { class: "btn", disabled: pending, onclick: move |_| on_action.call((retry_id.clone(), true)), "Retry item" }
            }
            if let Some(run) = item["run_id"].as_str().and_then(|id| Uuid::parse_str(id).ok()) {
                Link { to: super::run_link(run), "Open run" }
            }
        }
        h3 { "Item history" }
        if events.is_empty() { p { "No events yet." } }
        ul {
            for event in &events {
                li {
                    key: "{event[\"seq\"]}",
                    strong { {field(event, "kind")} }
                    span { class: "muted", " · " {utc(&event["at"])} }
                    if let Some(attempt) = event["data"]["attempt"].as_u64() { " · attempt {attempt}" }
                    if let Some(claim) = event["data"]["claim"].as_u64() { " · claim {claim}" }
                    if let Some(error) = event["data"]["error"].as_str() { p { "{error}" } }
                }
            }
        }
    }
}

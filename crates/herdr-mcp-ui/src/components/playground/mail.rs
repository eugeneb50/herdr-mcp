//! Mail Playground tab (P0): read-only activity. A filtered capsule list and
//! the selected capsule's event timeline, read through the typed bridge client.
//! Every wire type is owned by `herdr-mcp-api`; this module keeps no copy.

use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use herdr_mcp_api::{
    Uuid,
    mail::{MailItemResponse, MailListQuery},
    settings::{Settings, UI_QUEUES_POLL_MS},
};
use serde_json::Value;

use super::queue_rows::{
    Cause, Group, bridge_unreachable, buttons, event_note, field, fmt_ms, item_id, refetch,
    selected_row_by, utc_ms,
};
use crate::{client::use_api, components::Route};

const ITEM_LIMIT: u32 = 100;
const VIEW: &str = "activity";

type MailRow = MailItemResponse;
/// The filtered page and the open capsule's history, read in one tick.
type Tick = (
    Result<Vec<MailRow>, String>,
    Option<Result<MailRow, String>>,
);

/// Blank filters stay unset: the bridge must never see an empty label/status.
pub(super) fn list_query(to: &str, status: &str) -> MailListQuery {
    MailListQuery {
        to: Some(to.trim().to_owned()).filter(|label| !label.is_empty()),
        status: Some(status.to_owned()).filter(|status| !status.is_empty()),
        limit: Some(ITEM_LIMIT),
    }
}

/// The URL owns the view and the selection; the component is keyed on both.
pub(super) fn mail_link(view: &str, to: Option<String>, item: Option<String>) -> Route {
    Route::Playground {
        tab: Some("mail".into()),
        queue: None,
        item: None,
        run: None,
        mail_view: Some(view.to_owned()),
        mail_to: to,
        mail_item: item,
    }
}

/// Selection follows the item id, so a refresh may reorder or replace rows.
pub(super) fn selected_row(rows: &[MailRow], selected: Option<&str>) -> Option<usize> {
    selected_row_by(rows, selected, |row| item_id(&row.item))
}

/// The mailbox label is the suffix of the canonical queue name.
pub(super) fn label(item: &Value) -> &str {
    item["queue"]
        .as_str()
        .and_then(|queue| queue.strip_prefix("mail-"))
        .unwrap_or("—")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Empty {
    None,
    Filtered,
    NoSelection,
}

pub(super) fn empty_state(rows: &[MailRow], selected: Option<&str>) -> Empty {
    if rows.is_empty() {
        Empty::Filtered
    } else if selected.is_none() {
        Empty::NoSelection
    } else {
        Empty::None
    }
}

/// Mail-specific event annotations; attempt/claim come from `queue_rows`.
pub(super) fn event_details(event: &Value) -> Option<String> {
    let data = &event["data"];
    let mut notes = Vec::new();
    if let Some(delivery) = data["delivery"].as_str() {
        notes.push(format!("delivery {delivery}"));
    }
    if let Some(reply) = data["reply"].as_str() {
        notes.push(format!("reply {reply}"));
    }
    if let Some(submitted) = data["submitted"].as_bool() {
        notes.push(format!("submitted {submitted}"));
    }
    (!notes.is_empty()).then(|| notes.join(" · "))
}

pub(super) fn last_event_at(events: &[Value]) -> Option<i64> {
    events.iter().filter_map(|event| event["at"].as_i64()).max()
}

fn restart(cause: Cause, mut meta: Resource<Option<String>>, mut tick: Resource<Tick>) {
    if refetch(cause) == Group::All {
        meta.restart();
    }
    tick.restart();
}

#[component]
pub(super) fn Mail(
    to: Option<String>,
    item: Option<String>,
    mut status: Signal<String>,
) -> Element {
    let api = use_api();
    let interval = use_signal(|| Settings::default().number(UI_QUEUES_POLL_MS) as u32);
    let navigator = use_navigator();

    // Settings are read on mount and on manual refresh, never on a poll tick.
    let meta = use_resource(move || {
        let client = api();
        let mut interval = interval;
        async move {
            let settings = client.settings().await;
            let error = settings.as_ref().err().map(ToString::to_string);
            if let Ok(settings) = &settings {
                interval.set(settings.overrides().number(UI_QUEUES_POLL_MS) as u32);
            }
            error
        }
    });

    let tick_to = to.clone();
    let detail_item = item.clone();
    let tick = use_resource(move || {
        let client = api();
        let to = tick_to.clone();
        let item = detail_item.clone();
        let filter = status();
        async move {
            let rows = client
                .mail_items(&list_query(&to.unwrap_or_default(), &filter))
                .await
                .map_err(|error| error.to_string());
            let detail = match item {
                Some(id) => Some(
                    client
                        .mail_item(&id)
                        .await
                        .map_err(|error| error.to_string()),
                ),
                None => None,
            };
            (rows, detail)
        }
    });

    let refresh = move |cause: Cause| restart(cause, meta, tick);
    let poll_refresh = refresh;
    use_future(move || async move {
        loop {
            let delay = *interval.peek();
            TimeoutFuture::new(delay).await;
            let visible = web_sys::window()
                .and_then(|w| w.document())
                .is_some_and(|doc| !doc.hidden());
            if visible && !tick.pending() {
                poll_refresh(Cause::Poll);
            }
        }
    });

    let metadata = meta.read();
    let snapshot = tick.read();
    let Some(settings_error) = metadata.as_ref() else {
        return rsx! { p { role: "status", "Loading mail…" } };
    };
    let Some((rows, detail)) = snapshot.as_ref() else {
        return rsx! { p { role: "status", "Loading mail…" } };
    };
    let mut attempts = vec![
        Some(
            settings_error
                .as_ref()
                .map_or(Ok(()), |error| Err(error.clone())),
        ),
        Some(rows.as_ref().map(|_| ()).map_err(Clone::clone)),
    ];
    if let Some(detail) = detail {
        attempts.push(Some(detail.as_ref().map(|_| ()).map_err(Clone::clone)));
    }
    if bridge_unreachable(&attempts) {
        return rsx! {
            div { class: "btn-row",
                button { class: "btn", onclick: move |_| refresh(Cause::Manual), "Refresh mail" }
            }
            p { role: "alert", class: "send-error", "Bridge unreachable — no mail data could be read." }
        };
    }
    let active_row = rows
        .as_ref()
        .ok()
        .and_then(|rows| selected_row(rows, item.as_deref()));
    let empty = rows
        .as_ref()
        .map_or(Empty::None, |rows| empty_state(rows, item.as_deref()));
    let change_to = to.clone();
    rsx! {
        div { class: "btn-row",
            button { class: "btn", onclick: move |_| refresh(Cause::Manual), "Refresh mail" }
            if *tick.state().read() == UseResourceState::Pending {
                span { class: "muted", role: "status", "Refreshing…" }
            }
        }
        if let Some(error) = settings_error {
            p { role: "alert", class: "send-error", "Settings read failed. {error}" }
        }
        div { class: "runs",
            div { class: "card",
                div { class: "btn-row",
                    label {
                        "To "
                        input {
                            class: "input input-narrow",
                            r#type: "search",
                            placeholder: "mailbox label",
                            value: to.clone().unwrap_or_default(),
                            onchange: move |event| {
                                let value = event.value();
                                let to = (!value.trim().is_empty()).then(|| value.trim().to_owned());
                                navigator.push(mail_link(VIEW, to, None));
                            },
                        }
                    }
                    label {
                        "Status "
                        select { class: "input input-narrow", value: status(), onchange: move |e| status.set(e.value()),
                            option { value: "", "All statuses" }
                            for choice in ["queued", "running", "done", "dead", "cancelled"] {
                                option { value: choice, "{choice}" }
                            }
                        }
                    }
                }
                match rows {
                    Err(error) => rsx! { p { role: "alert", class: "send-error", "Mail items unavailable. {error}" } },
                    Ok(rows) => rsx! {
                        p { class: "muted", "{rows.len()} items · up to {ITEM_LIMIT}" }
                        if empty == Empty::Filtered { p { "No capsules match this filter." } }
                        for (index, row) in rows.iter().enumerate() {
                            if let Some(id) = item_id(&row.item) {
                                Link {
                                    key: "{id}",
                                    class: if active_row == Some(index) { "tree-item tree-sel run-row" } else { "tree-item run-row" },
                                    to: mail_link(VIEW, change_to.clone(), Some(id.to_owned())),
                                    span { class: "pill", {field(&row.item, "status")} }
                                    span { class: "tree-name mono", "{id}" }
                                    span { class: "muted", "{label(&row.item)} · " {field(&row.item["payload"], "task")} }
                                    span { class: "muted", {utc_ms(&row.item["enqueued_at"])} }
                                }
                            }
                        }
                    },
                }
            }
            div { class: "card",
                match detail {
                    None => rsx! { p { class: "muted", "Pick a capsule to read its events." } },
                    Some(Err(error)) => rsx! { p { role: "alert", class: "send-error", "Capsule unavailable. {error}" } },
                    Some(Ok(detail)) => rsx! { CapsuleDetail { detail: detail.clone() } },
                }
            }
        }
    }
}

#[component]
fn CapsuleDetail(detail: MailRow) -> Element {
    let item = &detail.item;
    let payload = &item["payload"];
    let id = item_id(item).unwrap_or_default().to_owned();
    let run = item["run_id"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok());
    let controls = buttons(field(item, "status"), &id, run);
    let events = &detail.events;
    let last = fmt_ms(last_event_at(events));
    rsx! {
        h2 { "Capsule " {field(payload, "id")} }
        p {
            span { class: "pill", {field(item, "status")} }
            " · capsule " {field(payload, "status")} " · " {label(item)}
            " · attempt " {item["attempts"].to_string()} " of " {item["max_attempts"].to_string()}
        }
        p { class: "muted", "from " {field(payload, "from")} " · task " {field(payload, "task")} }
        if let Some(re) = payload["re"].as_str() { p { class: "muted", "reply to {re}" } }
        if let Some(delivery) = item["delivery"].as_str() { p { class: "muted", "delivery {delivery}" } }
        p { class: "muted", "enqueued " {utc_ms(&item["enqueued_at"])} " · last event {last}" }
        if let Some(run) = controls.run { Link { to: super::run_link(run), "Open run" } }
        h3 { "Event timeline" }
        if events.is_empty() { p { "No events yet." } }
        ul {
            for event in events {
                li {
                    key: "{event[\"seq\"]}",
                    strong { {field(event, "kind")} }
                    span { class: "muted", " · " {utc_ms(&event["at"])} }
                    if let Some(note) = event_note(event) { span { class: "muted", " · {note}" } }
                    if let Some(details) = event_details(event) { span { class: "muted", " · {details}" } }
                    if let Some(error) = event["data"]["error"].as_str() { p { "{error}" } }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mail_selection(route: &Route) -> (Option<String>, Option<String>, Option<String>) {
        match route {
            Route::Playground {
                mail_view,
                mail_to,
                mail_item,
                ..
            } => (mail_view.clone(), mail_to.clone(), mail_item.clone()),
            _ => (None, None, None),
        }
    }

    fn row(id: &str) -> MailRow {
        MailRow {
            item: json!({"id": id, "queue": "mail-lead"}),
            events: vec![],
        }
    }

    #[test]
    fn mail_link_round_trips_view_and_selection() {
        let linked = mail_link(VIEW, Some("lead".into()), Some("item-1".into()));
        assert_eq!(
            mail_selection(&linked),
            (
                Some(VIEW.into()),
                Some("lead".into()),
                Some("item-1".into())
            )
        );
        let Route::Playground { tab, .. } = &linked else {
            panic!("mail link must be a playground route");
        };
        assert_eq!(tab.as_deref(), Some("mail"));
        let plain = mail_link(VIEW, None, None);
        assert_eq!(mail_selection(&plain), (Some(VIEW.into()), None, None));
    }

    #[test]
    fn selected_mail_row_follows_id_across_refresh() {
        let a = [row("a"), row("b")];
        let b = [row("b"), row("a")];
        assert_eq!(selected_row(&a, Some("a")), Some(0));
        assert_eq!(selected_row(&b, Some("a")), Some(1));
        assert_eq!(selected_row(&[row("b")], Some("a")), None);
        assert_eq!(selected_row(&a, None), None);
    }

    #[test]
    fn mail_actions_follow_queue_status() {
        // P0 is read-only; P1's retry/cancel reuse the shared queue mapping.
        for status in ["queued", "running"] {
            assert_eq!(super::super::queue_rows::actions(status), (true, false));
        }
        for status in ["dead", "cancelled"] {
            assert_eq!(super::super::queue_rows::actions(status), (false, true));
        }
        for status in ["done", "unknown"] {
            assert_eq!(super::super::queue_rows::actions(status), (false, false));
        }
    }

    #[test]
    fn utc_ms_formats_epoch_millis() {
        assert_eq!(utc_ms(&json!(0)), "1970-01-01 00:00:00 UTC");
        assert_eq!(
            utc_ms(&json!(1_700_000_000_000_i64)),
            "2023-11-14 22:13:20 UTC"
        );
        assert_eq!(utc_ms(&json!(null)), "—");
        assert_eq!(fmt_ms(None), "—");
    }

    #[test]
    fn mail_list_query_omits_blank_filters() {
        let all = list_query("  ", "");
        assert_eq!(all.to, None);
        assert_eq!(all.status, None);
        assert_eq!(all.limit, Some(ITEM_LIMIT));
        let filtered = list_query(" lead ", "dead");
        assert_eq!(filtered.to.as_deref(), Some("lead"));
        assert_eq!(filtered.status.as_deref(), Some("dead"));
    }

    #[test]
    fn empty_state_distinguishes_filter_and_selection() {
        assert_eq!(empty_state(&[], None), Empty::Filtered);
        assert_eq!(empty_state(&[row("a")], None), Empty::NoSelection);
        assert_eq!(empty_state(&[row("a")], Some("a")), Empty::None);
    }

    #[test]
    fn event_details_formats_mail_fields() {
        assert_eq!(event_details(&json!({"data": {}})), None);
        assert_eq!(
            event_details(&json!({"data": {
                "delivery": "d-1", "reply": "r-1", "submitted": true
            }}))
            .as_deref(),
            Some("delivery d-1 · reply r-1 · submitted true")
        );
        assert_eq!(
            last_event_at(&[json!({"at": 3}), json!({"at": 9})]),
            Some(9)
        );
        assert_eq!(last_event_at(&[]), None);
    }
}

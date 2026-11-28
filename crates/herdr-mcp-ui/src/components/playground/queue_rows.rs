use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use herdr_mcp_api::Uuid;
use serde_json::Value;

pub(super) fn item_id(item: &Value) -> Option<&str> {
    item["id"].as_str().filter(|id| !id.is_empty())
}

/// Selection follows an id, not a row index, so a refresh can reorder rows.
pub(super) fn selected_row_by<T>(
    rows: &[T],
    selected: Option<&str>,
    id: impl Fn(&T) -> Option<&str>,
) -> Option<usize> {
    let selected = selected?;
    rows.iter().position(|row| id(row) == Some(selected))
}

pub(super) fn selected_row(rows: &[Value], selected: Option<&str>) -> Option<usize> {
    selected_row_by(rows, selected, item_id)
}

/// A JSON string field, or an em dash when absent.
pub(super) fn field<'a>(value: &'a Value, name: &str) -> &'a str {
    value[name].as_str().unwrap_or("—")
}

/// Epoch milliseconds in UTC, or an em dash for a missing/invalid value.
pub(super) fn fmt_ms(value: Option<i64>) -> String {
    value
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(
            || "—".into(),
            |time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        )
}

pub(super) fn utc_ms(value: &Value) -> String {
    fmt_ms(value.as_i64())
}

/// The attempt/claim annotations shared by queue and mail event timelines.
pub(super) fn event_note(event: &Value) -> Option<String> {
    let data = &event["data"];
    let mut notes = Vec::new();
    if let Some(attempt) = data["attempt"].as_u64() {
        notes.push(format!("attempt {attempt}"));
    }
    if let Some(claim) = data["claim"].as_u64() {
        notes.push(format!("claim {claim}"));
    }
    (!notes.is_empty()).then(|| notes.join(" · "))
}

/// Whether the item can be cancelled and whether it can be retried.
pub(super) fn actions(status: &str) -> (bool, bool) {
    (
        matches!(status, "queued" | "running"),
        matches!(status, "dead" | "cancelled"),
    )
}

/// What a refresh re-reads. `Tick` is the filtered items plus the open item's
/// history; `All` also re-reads settings, collections and queue depths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Group {
    Tick,
    All,
}

/// Why a refresh was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cause {
    Poll,
    Action,
    Manual,
}

/// A poll tick re-reads only items and the open item's detail. Settings and
/// the collection list are read on mount and queue change; actions and an
/// explicit refresh re-read everything.
pub(super) fn refetch(cause: Cause) -> Group {
    match cause {
        Cause::Poll => Group::Tick,
        Cause::Action | Cause::Manual => Group::All,
    }
}

/// The poll loop shared by the queues, mail activity and mail health views.
/// `busy` covers whatever a view must not refresh over (a read or an action).
pub(super) fn use_poll(
    interval: Signal<u32>,
    busy: impl Fn() -> bool + Clone + 'static,
    refresh: impl Fn(Cause) + Clone + 'static,
) {
    use_future(move || {
        let busy = busy.clone();
        let refresh = refresh.clone();
        async move {
            loop {
                TimeoutFuture::new(*interval.peek()).await;
                let visible = web_sys::window()
                    .and_then(|w| w.document())
                    .is_some_and(|doc| !doc.hidden());
                if visible && !busy() {
                    refresh(Cause::Poll);
                }
            }
        }
    });
}

/// One connection state: only when every read attempted in a refresh failed
/// is the bridge unreachable. `None` means that read was not attempted.
pub(super) fn bridge_unreachable(results: &[Option<Result<(), String>>]) -> bool {
    let mut attempted = false;
    for result in results.iter().flatten() {
        attempted = true;
        if result.is_ok() {
            return false;
        }
    }
    attempted
}

/// The controls an item's detail renders. A run link needs a parseable id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Buttons {
    pub(super) cancel: bool,
    pub(super) retry: bool,
    pub(super) run: Option<Uuid>,
}

pub(super) fn buttons(status: &str, id: &str, run: Option<Uuid>) -> Buttons {
    let (cancel, retry) = actions(status);
    Buttons {
        cancel: cancel && !id.is_empty(),
        retry: retry && !id.is_empty(),
        run,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_only_nonempty_string_ids() {
        assert_eq!(item_id(&json!({"id": "item-1"})), Some("item-1"));
        for item in [json!({}), json!({"id": ""}), json!({"id": 42})] {
            assert_eq!(item_id(&item), None);
        }
    }

    #[test]
    fn selected_row_follows_id_across_refresh() {
        let a = json!({"id": "a"});
        let b = json!({"id": "b"});
        assert_eq!(selected_row(&[a.clone(), b.clone()], Some("a")), Some(0));
        assert_eq!(selected_row(&[b.clone(), a], Some("a")), Some(1));
        assert_eq!(selected_row(&[b], Some("a")), None);
    }

    #[test]
    fn event_note_lists_attempt_and_claim() {
        assert_eq!(event_note(&json!({"data": {}})), None);
        assert_eq!(
            event_note(&json!({"data": {"attempt": 2, "claim": 7}})).as_deref(),
            Some("attempt 2 · claim 7")
        );
    }

    #[test]
    fn actions_follow_queue_status() {
        for status in ["queued", "running"] {
            assert_eq!(actions(status), (true, false));
        }
        for status in ["dead", "cancelled"] {
            assert_eq!(actions(status), (false, true));
        }
        for status in ["done", "unknown"] {
            assert_eq!(actions(status), (false, false));
        }
    }

    #[test]
    fn poll_tick_rereads_only_items_and_open_detail() {
        assert_eq!(refetch(Cause::Poll), Group::Tick);
        assert_eq!(refetch(Cause::Action), Group::All);
        assert_eq!(refetch(Cause::Manual), Group::All);
    }

    #[test]
    fn one_connection_line_only_when_every_read_fails() {
        let err = || Err::<(), String>("down".into());
        assert!(bridge_unreachable(&[Some(err()), Some(err()), Some(err())]));
        assert!(!bridge_unreachable(&[
            Some(err()),
            Some(Ok(())),
            Some(err())
        ]));
        // No item selected: its detail was not attempted, but the attempted
        // reads still all failed.
        assert!(bridge_unreachable(&[Some(err()), Some(err()), None]));
        assert!(!bridge_unreachable(&[None, None]));
        assert!(!bridge_unreachable(&[]));
    }

    #[test]
    fn buttons_follow_status_id_and_run() {
        let none = buttons("queued", "", None);
        assert_eq!(
            none,
            Buttons {
                cancel: false,
                retry: false,
                run: None
            }
        );
        assert_eq!(
            buttons("queued", "i-1", None),
            Buttons {
                cancel: true,
                retry: false,
                run: None
            }
        );
        assert_eq!(
            buttons("dead", "i-1", None),
            Buttons {
                cancel: false,
                retry: true,
                run: None
            }
        );
        let run = Uuid::parse_str("00000000-0000-0000-0000-000000000001").ok();
        assert_eq!(
            buttons("done", "i-1", run),
            Buttons {
                cancel: false,
                retry: false,
                run
            }
        );
    }
}

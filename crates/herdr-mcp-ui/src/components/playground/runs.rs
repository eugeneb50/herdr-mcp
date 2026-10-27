//! Past and live runs: a list to pick from, and the picked run's trail (what
//! each step sent and got, the gate's checks, errors, pane snapshots).

use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use herdr_mcp_api::{RunAction, RunEvent, RunRecord, RunStatus, Uuid};

use crate::client::use_api;

/// The fastest the list (and a live run's trail) is re-read while run
/// events stream in.
const THROTTLE_MS: u32 = 300;

/// Counts the run events the bridge streams (`GET /api/events`), and each
/// (re)connect, when some may have been missed: what the list and a live
/// run's trail re-read on.
#[derive(Clone, Copy)]
struct Seen(Signal<u64>);

fn use_run_events() -> Seen {
    let mut seen = use_signal(|| 0u64);
    let mut source: Signal<Option<web_sys::EventSource>> = use_signal(|| None);
    use_hook(move || {
        let es = super::events::sse(
            "/api/events",
            move |_| *seen.write() += 1,
            move || *seen.write() += 1,
        );
        source.set(es);
    });
    use_drop(move || {
        if let Some(es) = source.peek().as_ref() {
            es.close();
        }
    });
    Seen(seen)
}

/// Waits until `seen` moves past `read` (at most every [`THROTTLE_MS`]),
/// then records it.
async fn changed(seen: Seen, read: &mut Option<u64>) {
    loop {
        let now = *seen.0.peek();
        if *read != Some(now) {
            *read = Some(now);
            return;
        }
        TimeoutFuture::new(THROTTLE_MS).await;
    }
}

fn status_class(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Completed => "pill pill-ok",
        RunStatus::Failed | RunStatus::Interrupted => "pill pill-error",
        RunStatus::Running | RunStatus::Paused => "pill pill-live",
        RunStatus::Cancelled => "pill",
    }
}

/// When a run started, UTC.
fn started(r: &RunRecord) -> String {
    r.started_at.format("%m-%d %H:%M:%S").to_string()
}

/// The collection filter's value for a run: its recipe id, or "adhoc".
fn group(r: &RunRecord) -> String {
    r.recipe_id
        .map_or_else(|| "adhoc".into(), |id| id.to_string())
}

#[component]
pub fn Runs() -> Element {
    let client = use_api();
    let mut runs: Signal<Vec<RunRecord>> = use_signal(Vec::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut pick: Signal<Option<Uuid>> = use_signal(|| None);
    let mut failed_only = use_signal(|| false);
    let mut collection = use_signal(String::new);
    let seen = use_run_events();
    use_context_provider(|| seen);

    use_future(move || async move {
        let client = client();
        let mut read = None;
        loop {
            changed(seen, &mut read).await;
            match client
                .list_runs(herdr_mcp_api::playground::RUNS_SHOWN)
                .await
            {
                Ok(list) => {
                    runs.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("cannot list runs: {e}"))),
            }
        }
    });

    let all = runs.read().clone();
    let mut groups: Vec<(String, String)> = Vec::new();
    for r in &all {
        let g = group(r);
        if !groups.iter().any(|(k, _)| *k == g) {
            let name = match r.recipe_id {
                Some(_) => r.name.clone().unwrap_or_else(|| "collection".into()),
                None => "ad-hoc runs".into(),
            };
            groups.push((g, name));
        }
    }
    groups.sort_by_key(|(_, n)| n.to_lowercase());
    let want = collection.read().clone();
    let shown: Vec<RunRecord> = all
        .iter()
        .filter(|r| want.is_empty() || group(r) == want)
        .filter(|r| {
            !failed_only() || matches!(r.status, RunStatus::Failed | RunStatus::Interrupted)
        })
        .cloned()
        .collect();
    let picked = pick().and_then(|id| all.iter().find(|r| r.id == id).cloned());
    let child_runs: Vec<(Uuid, String)> = picked
        .as_ref()
        .map(|p| {
            all.iter()
                .filter(|r| r.origin.parent() == Some(p.id))
                .map(|r| (r.id, r.title().to_string()))
                .collect()
        })
        .unwrap_or_default();

    rsx! {
        p { class: "field-desc",
            "Every run of a collection, request send, schedule or trigger, newest first. Pick one to read its trail: what each request sent and got, why the gate chose its rule, errors, the last lines of the pane a failed run used, and, indented under it, the trail of the collection its gate handed off to. How long runs are kept is set per collection (Run history) or in Settings › runs."
        }
        div { class: "btn-row",
            select {
                class: "input input-narrow",
                onchange: move |e| collection.set(e.value()),
                option { value: "", selected: want.is_empty(), "All collections" }
                for (k, n) in groups.iter() {
                    option { value: "{k}", selected: *k == want, "{n}" }
                }
            }
            label { class: "muted",
                input {
                    r#type: "checkbox",
                    checked: failed_only(),
                    onchange: move |e| failed_only.set(e.checked()),
                }
                " failed only"
            }
            span { class: "muted", "{shown.len()} of {all.len()}" }
        }
        if let Some(e) = error() {
            p { class: "send-status send-error", "{e}" }
        }
        div { class: "runs",
            div { class: "card",
                if shown.is_empty() {
                    p { class: "muted", "No runs yet." }
                }
                for r in shown.iter().cloned() {
                    button {
                        key: "{r.id}",
                        class: if pick() == Some(r.id) { "tree-item tree-sel run-row" } else { "tree-item run-row" },
                        onclick: move |_| pick.set(Some(r.id)),
                        span { class: status_class(r.status), "{r.status.as_str()}" }
                        span { class: "tree-name", "{r.title()}" }
                        span { class: "muted mono", "{started(&r)}" }
                    }
                }
            }
            match picked {
                Some(run) => rsx! {
                    RunDetail {
                        key: "{run.id}",
                        run,
                        child_runs,
                        on_open: move |id| pick.set(Some(id)),
                    }
                },
                None => rsx! {
                    div { class: "card", p { class: "muted", "Pick a run to see its trail." } }
                },
            }
        }
    }
}

/// `child_runs`: the runs this one started (its gate's hand-off, `recipe_run`
/// steps); `on_open` picks one, or the parent.
#[component]
fn RunDetail(
    run: RunRecord,
    child_runs: Vec<(Uuid, String)>,
    on_open: EventHandler<Uuid>,
) -> Element {
    let client = use_api();
    let seen: Seen = use_context();
    let id = run.id;
    let mut events: Signal<Vec<RunEvent>> = use_signal(Vec::new);
    // With the trail of the child run its gate handed off to.
    let mut trail = use_signal(String::new);
    let mut msg: Signal<Option<Result<String, String>>> = use_signal(|| None);

    // Re-read the log until it ends (a finished run's log never changes).
    use_future(move || async move {
        let client = client();
        let mut read = None;
        loop {
            changed(seen, &mut read).await;
            if let (Ok(list), Ok(text)) =
                (client.run_events(id, 0).await, client.run_trail(id).await)
            {
                let done = list.iter().any(|e| e.kind.is_terminal());
                events.set(list);
                trail.set(text);
                if done {
                    break;
                }
            }
        }
    });

    let list = events.read().clone();
    let trail = trail();
    let pane = run.pane(&list);
    let live = !run.status.is_finished();

    let copy = {
        let trail = trail.clone();
        move |_| {
            let Some(write) =
                web_sys::window().map(|w| w.navigator().clipboard().write_text(&trail))
            else {
                msg.set(Some(Err("no clipboard in this browser".into())));
                return;
            };
            spawn(async move {
                msg.set(Some(
                    match wasm_bindgen_futures::JsFuture::from(write).await {
                        Ok(_) => Ok("trail copied".into()),
                        Err(_) => Err("the browser denied clipboard access".into()),
                    },
                ));
            });
        }
    };
    let rerun = {
        let req = run.rerun();
        move |_| {
            let client = client();
            let req = req.clone();
            spawn(async move {
                match client.start_run(&req).await {
                    Ok(r) => msg.set(Some(Ok(format!("started run {}", r.id)))),
                    Err(e) => msg.set(Some(Err(format!("run failed to start: {e}")))),
                }
            });
        }
    };
    let open_pane = {
        let pane = pane.clone();
        move |_| {
            let Some(pane) = pane.clone() else { return };
            let client = client();
            spawn(async move {
                msg.set(Some(
                    herdr_mcp_api::playground::focus_pane(&client, &pane).await,
                ));
            });
        }
    };
    let cancel = move |_| {
        let client = client();
        spawn(async move {
            if let Err(e) = client.control_run(id, RunAction::Cancel).await {
                msg.set(Some(Err(format!("cancel failed: {e}"))));
            }
        });
    };

    rsx! {
        div { class: "card",
            div { class: "response-head",
                span { class: status_class(run.status), "{run.status.as_str()}" }
                span { class: "mono", "{run.title()}" }
                span { class: "muted", "{run.summary()} · by {run.origin}" }
            }
            div { class: "btn-row",
                button { class: "btn", onclick: copy, "Copy trail" }
                if let Some(p) = pane.clone() {
                    button {
                        class: "btn",
                        title: "Focus this pane in herdr: agents keep their sessions, so its scrollback shows what happened",
                        onclick: open_pane,
                        "Open pane {p}"
                    }
                }
                if let Some(parent) = run.origin.parent() {
                    button {
                        class: "btn",
                        title: "The run whose gate or step started this one",
                        onclick: move |_| on_open.call(parent),
                        "Open parent run"
                    }
                }
                for (child, name) in child_runs.into_iter() {
                    button {
                        key: "{child}",
                        class: "btn",
                        title: "A run this one started",
                        onclick: move |_| on_open.call(child),
                        "Open {name}"
                    }
                }
                button { class: "btn", onclick: rerun, "Run again" }
                if live {
                    button { class: "btn link-danger", onclick: cancel, "Cancel" }
                }
                match &*msg.read() {
                    Some(Ok(m)) => rsx! { span { class: "send-status", "{m}" } },
                    Some(Err(m)) => rsx! { span { class: "send-status send-error", "{m}" } },
                    None => rsx! {},
                }
            }
            pre { class: "mono trail", "{trail}" }
        }
    }
}

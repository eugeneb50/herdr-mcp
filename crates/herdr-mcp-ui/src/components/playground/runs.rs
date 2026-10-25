//! Past and live runs: a list to pick from, and the picked run's trail (what
//! each step sent and got, the gate's checks, errors, pane snapshots).

use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use herdr_mcp_api::{RunAction, RunEvent, RunRecord, RunStatus, Uuid, trail_text};

use crate::client::use_api;

/// How often the list (and a live run's trail) is re-read.
const POLL_MS: u32 = 2_000;

fn status_class(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Completed => "pill pill-ok",
        RunStatus::Failed | RunStatus::Interrupted => "pill pill-error",
        RunStatus::Running | RunStatus::Paused => "pill pill-live",
        RunStatus::Cancelled => "pill",
    }
}

fn status_name(s: RunStatus) -> String {
    format!("{s:?}").to_lowercase()
}

fn run_name(r: &RunRecord) -> String {
    r.name.clone().unwrap_or_else(|| "ad-hoc run".into())
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

    use_future(move || async move {
        let client = client();
        loop {
            match client.list_runs().await {
                Ok(list) => {
                    runs.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("cannot list runs: {e}"))),
            }
            TimeoutFuture::new(POLL_MS).await;
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

    rsx! {
        p { class: "field-desc",
            "Every run of a collection, request send, schedule or trigger, newest first. Pick one to read its trail: what each request sent and got, why the gate chose its rule, errors, and the last lines of the pane a failed run used. How long runs are kept is set per collection (Run history) or in Settings › runs."
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
                        span { class: status_class(r.status), "{status_name(r.status)}" }
                        span { class: "tree-name", "{run_name(&r)}" }
                        span { class: "muted mono", "{started(&r)}" }
                    }
                }
            }
            match picked {
                Some(run) => rsx! { RunDetail { key: "{run.id}", run } },
                None => rsx! {
                    div { class: "card", p { class: "muted", "Pick a run to see its trail." } }
                },
            }
        }
    }
}

#[component]
fn RunDetail(run: RunRecord) -> Element {
    let client = use_api();
    let id = run.id;
    let mut events: Signal<Vec<RunEvent>> = use_signal(Vec::new);
    let mut msg: Signal<Option<Result<String, String>>> = use_signal(|| None);

    // Re-read the log until it ends (a finished run's log never changes).
    use_future(move || async move {
        let client = client();
        loop {
            if let Ok(list) = client.run_events(id, 0).await {
                let done = list.iter().any(|e| e.kind.is_terminal());
                events.set(list);
                if done {
                    break;
                }
            }
            TimeoutFuture::new(POLL_MS).await;
        }
    });

    let list = events.read().clone();
    let trail = trail_text(&run, &list);
    let pane = run.pane(&list);
    let live = !run.status.is_finished();

    let copy = {
        let trail = trail.clone();
        move |_| {
            let copied = web_sys::window()
                .map(|w| w.navigator().clipboard().write_text(&trail))
                .is_some();
            msg.set(Some(if copied {
                Ok("trail copied".into())
            } else {
                Err("no clipboard in this browser".into())
            }));
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
                let args = serde_json::json!({
                    "method": "pane.focus",
                    "params": { "pane_id": pane },
                });
                match client.call_tool("herdr_api", &args).await {
                    Ok(r) if !herdr_mcp_api::tool_is_error(&r) => {
                        msg.set(Some(Ok(format!("focused pane {pane} in herdr"))));
                    }
                    Ok(r) => msg.set(Some(Err(herdr_mcp_api::tool_text(&r)))),
                    Err(e) => msg.set(Some(Err(format!("cannot focus {pane}: {e}")))),
                }
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
                span { class: status_class(run.status), "{status_name(run.status)}" }
                span { class: "mono", "{run_name(&run)}" }
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

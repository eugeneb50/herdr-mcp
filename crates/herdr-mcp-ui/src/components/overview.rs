//! Overview page, the observation deck: herdr's panes (`GET /api/herdr/panes`)
//! in any running session (`GET /api/herdr/sessions`), filtered by the server,
//! as a collapsible workspace → tab tree under the deck. In the bridge's own
//! session each pane also shows its registry role (`GET /api/agents`). Ticking a pane watches it: the
//! deck shows each watched pane's screen or scrollback (`read_pane`), re-read
//! with the pane list on every tick of the refresh interval.

use std::collections::{BTreeSet, HashMap};

use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use herdr_mcp_api::{HerdrPane, PaneFilter};

use crate::client::use_api;
use crate::local::{remember, remembered};

const STATUSES: [&str; 5] = ["idle", "working", "blocked", "done", "unknown"];

/// Browser-local watched panes: a JSON list of `[session, pane_id]`, the
/// session empty for this bridge's own.
const WATCHED: &str = "herdr-mcp.overview.watched";
/// Browser-local refresh interval (ms, `0` = paused) and read source.
const EVERY: &str = "herdr-mcp.overview.every";
const SOURCE: &str = "herdr-mcp.overview.source";

const INTERVALS: [(&str, &str); 4] = [
    ("1000", "1s"),
    ("2000", "2s"),
    ("5000", "5s"),
    ("0", "paused"),
];
/// `read_pane` sources: the current screen, or the scrollback's tail.
const SOURCES: [(&str, &str); 2] = [("visible", "screen"), ("recent", "scrollback")];
/// Scrollback lines a deck card reads.
const SCROLLBACK: u32 = 200;

/// A watched pane: its session (empty = this bridge's) and pane id.
type Watch = (String, String);

fn load_watched() -> Vec<Watch> {
    serde_json::from_str(&remembered(WATCHED)).unwrap_or_default()
}

fn save_watched(w: &[Watch]) {
    remember(WATCHED, &serde_json::to_string(w).unwrap_or_default());
}

fn page_hidden() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .is_some_and(|d| d.hidden())
}

/// Text as a terminal card shows it: trailing blank lines dropped.
fn screen(text: &str) -> &str {
    text.trim_end()
}

fn cell(s: &str) -> &str {
    if s.is_empty() { "—" } else { s }
}

fn some(s: String) -> Option<String> {
    Some(s).filter(|s| !s.is_empty())
}

/// Panes as a workspace → tab tree, each level in herdr's first-seen order.
type Tree<'a> = Vec<(&'a str, Vec<(&'a str, Vec<&'a HerdrPane>)>)>;

fn by_workspace(panes: &[HerdrPane]) -> Tree<'_> {
    let mut tree: Tree = Vec::new();
    for p in panes {
        let ws = match tree.iter().position(|(w, _)| *w == p.workspace_id) {
            Some(i) => i,
            None => {
                tree.push((&p.workspace_id, Vec::new()));
                tree.len() - 1
            }
        };
        let tabs = &mut tree[ws].1;
        match tabs.iter_mut().find(|(t, _)| *t == p.tab_id) {
            Some((_, rows)) => rows.push(p),
            None => tabs.push((&p.tab_id, vec![p])),
        }
    }
    tree
}

fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

#[component]
pub fn Overview() -> Element {
    let client = use_api();
    let mut session: Signal<String> = use_signal(String::new);
    let mut status: Signal<String> = use_signal(String::new);
    let mut label: Signal<String> = use_signal(String::new);
    let mut refresh: Signal<u32> = use_signal(|| 0);
    let mut watched: Signal<Vec<Watch>> = use_signal(load_watched);
    let mut every: Signal<String> = use_signal(|| {
        Some(remembered(EVERY))
            .filter(|v| INTERVALS.iter().any(|(ms, _)| ms == v))
            .unwrap_or_else(|| "2000".into())
    });
    let mut source: Signal<String> = use_signal(|| {
        Some(remembered(SOURCE))
            .filter(|v| SOURCES.iter().any(|(s, _)| s == v))
            .unwrap_or_else(|| "visible".into())
    });

    // The deck's clock: bumps `refresh` every interval while the page is
    // visible and something is watched, so the pane list and every watched
    // pane are re-read together.
    use_future(move || async move {
        loop {
            let ms: u32 = every.peek().parse().unwrap_or(0);
            TimeoutFuture::new(if ms == 0 { 500 } else { ms }).await;
            if ms > 0 && !watched.peek().is_empty() && !page_hidden() {
                *refresh.write() += 1;
            }
        }
    });

    let mut toggle = move |w: Watch| {
        let mut list = watched.write();
        match list.iter().position(|x| *x == w) {
            Some(i) => {
                list.remove(i);
            }
            None => list.push(w),
        }
        save_watched(&list);
    };

    let sessions = use_resource(move || {
        let client = client();
        async move {
            let _ = refresh.read();
            client.list_sessions().await
        }
    });
    let panes = use_resource(move || {
        let client = client();
        let filter = PaneFilter {
            session: some(session()),
            agent_status: some(status()),
            label: some(label()),
            ..Default::default()
        };
        async move {
            let _ = refresh.read();
            client.list_panes(&filter).await
        }
    });
    // Registry roles exist only for the bridge's own session.
    let roles = use_resource(move || {
        let client = client();
        let own = session.read().is_empty();
        async move {
            let _ = refresh.read();
            let agents = if own {
                client.list_agents(None).await.unwrap_or_default()
            } else {
                Vec::new()
            };
            agents
                .into_iter()
                .filter(|a| !a.role.is_empty())
                .map(|a| (a.pane_id, a.role))
                .collect::<HashMap<_, _>>()
        }
    });
    let roles = roles.read_unchecked().clone().unwrap_or_default();
    // Each watched pane's herdr record, from its session's unfiltered list.
    let known = use_resource(move || {
        let client = client();
        let sessions: BTreeSet<String> = watched.read().iter().map(|(s, _)| s.clone()).collect();
        async move {
            let _ = refresh.read();
            let mut known: HashMap<Watch, HerdrPane> = HashMap::new();
            for s in sessions {
                let filter = PaneFilter {
                    session: some(s.clone()),
                    ..Default::default()
                };
                for p in client.list_panes(&filter).await.unwrap_or_default() {
                    known.insert((s.clone(), p.pane_id.clone()), p);
                }
            }
            known
        }
    });
    let known = known.read_unchecked().clone().unwrap_or_default();
    let here = session();

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Overview" }
            h1 { class: "page-title", "Observation deck" }
            p { class: "lede",
                "Every pane herdr reports, in any running session. Tick panes to watch them live on the deck; pick another session to look inside it. Watching only reads — writes still go to this bridge's own session."
            }
        }
        div { class: "form-row",
            div { class: "form-col",
                label { class: "form-label", "Session" }
                select {
                    class: "input",
                    value: "{session}",
                    onchange: move |e| session.set(e.value()),
                    match &*sessions.read_unchecked() {
                        Some(Ok(list)) => rsx! {
                            for s in list.iter().filter(|s| s.running) {
                                option {
                                    key: "{s.name}",
                                    // The bridge's own session is the empty value.
                                    value: if s.current { "" } else { "{s.name}" },
                                    if s.current { "{s.name} (this bridge)" } else { "{s.name}" }
                                }
                            }
                        },
                        _ => rsx! { option { value: "", "this bridge's session" } },
                    }
                }
            }
            div { class: "form-col",
                label { class: "form-label", "Status" }
                select {
                    class: "input",
                    value: "{status}",
                    onchange: move |e| status.set(e.value()),
                    option { value: "", "any" }
                    for s in STATUSES {
                        option { key: "{s}", value: "{s}", "{s}" }
                    }
                }
            }
            div { class: "form-col",
                label { class: "form-label", "Label or agent" }
                input {
                    class: "input",
                    r#type: "text",
                    placeholder: "Filter panes...",
                    value: "{label}",
                    oninput: move |e| label.set(e.value()),
                }
            }
            div { class: "form-col",
                label { class: "form-label", "Deck shows" }
                select {
                    class: "input",
                    value: "{source}",
                    onchange: move |e| {
                        remember(SOURCE, &e.value());
                        source.set(e.value());
                    },
                    for (v, name) in SOURCES {
                        option { key: "{v}", value: "{v}", "{name}" }
                    }
                }
            }
            div { class: "form-col",
                label { class: "form-label", "Refresh" }
                select {
                    class: "input",
                    value: "{every}",
                    onchange: move |e| {
                        remember(EVERY, &e.value());
                        every.set(e.value());
                    },
                    for (ms, name) in INTERVALS {
                        option { key: "{ms}", value: "{ms}", "{name}" }
                    }
                }
            }
            button {
                class: "btn tab-refresh",
                onclick: move |_| *refresh.write() += 1,
                "↻"
            }
        }
        if watched.read().is_empty() {
            section { class: "card",
                h2 { class: "card-title", "Deck" }
                p { class: "muted", "Tick a pane below to watch it here." }
            }
        } else {
            div { class: "deck",
                for w in watched.read().iter().cloned() {
                    Watcher {
                        key: "{w.0}/{w.1}",
                        pane: known.get(&w).cloned(),
                        session: w.0.clone(),
                        pane_id: w.1.clone(),
                        refresh,
                        source,
                        on_close: move |_| toggle(w.clone()),
                    }
                }
            }
        }
        match &*panes.read_unchecked() {
            None => rsx! { p { class: "muted", "Loading panes…" } },
            Some(Err(e)) => rsx! {
                div { class: "alert alert-error", "Could not list panes: {e}" }
            },
            Some(Ok(list)) if list.is_empty() => rsx! {
                section { class: "card",
                    h2 { class: "card-title", "Panes (0)" }
                    p { class: "muted", "No panes match." }
                }
            },
            Some(Ok(list)) => rsx! {
                section { class: "card",
                    h2 { class: "card-title", "Panes ({list.len()})" }
                    for (ws, tabs) in by_workspace(list) {
                        details { class: "tree", key: "{ws}", open: true,
                            summary {
                                span { class: "mono tree-name", "{ws}" }
                                span { class: "muted",
                                    " · {plural(tabs.len(), \"tab\")} · {plural(tabs.iter().map(|(_, r)| r.len()).sum(), \"pane\")}"
                                }
                            }
                            for (tab, rows) in tabs {
                                details { class: "tree tree-tab", key: "{tab}", open: true,
                                    summary {
                                        span { class: "mono tree-name", "{tab}" }
                                        span { class: "muted", " · {plural(rows.len(), \"pane\")}" }
                                    }
                                    table { class: "table",
                                        thead {
                                            tr {
                                                th { "watch" }
                                                th { "pane" }
                                                th { "label" }
                                                th { "agent" }
                                                th { "status" }
                                                th { "role" }
                                                th { "cwd" }
                                            }
                                        }
                                        tbody {
                                            for p in rows {
                                                tr {
                                                    key: "{p.pane_id}",
                                                    class: "row-pick",
                                                    onclick: {
                                                        let w = (here.clone(), p.pane_id.clone());
                                                        move |_| toggle(w.clone())
                                                    },
                                                    td {
                                                        input {
                                                            r#type: "checkbox",
                                                            checked: watched.read().contains(&(here.clone(), p.pane_id.clone())),
                                                        }
                                                    }
                                                    td { class: "mono",
                                                        if p.focused { "● {p.pane_id}" } else { "{p.pane_id}" }
                                                    }
                                                    td { "{cell(p.label.as_deref().unwrap_or_default())}" }
                                                    td { "{cell(p.agent.as_deref().unwrap_or_default())}" }
                                                    td { span { class: "pill", "{cell(&p.agent_status)}" } }
                                                    td { class: "mono",
                                                        "{cell(roles.get(&p.pane_id).map(String::as_str).unwrap_or_default())}"
                                                    }
                                                    td { class: "mono muted", "{cell(p.cwd.as_deref().unwrap_or_default())}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        }
    }
}

/// One deck card: a watched pane's text, re-read whenever `refresh` or
/// `source` changes. `pane` is its herdr record when the pane still exists.
#[component]
fn Watcher(
    pane: Option<HerdrPane>,
    session: String,
    pane_id: String,
    refresh: ReadSignal<u32>,
    source: ReadSignal<String>,
    on_close: EventHandler<()>,
) -> Element {
    let client = use_api();
    let mut wide: Signal<bool> = use_signal(|| false);
    let text = use_resource({
        let (session, pane_id) = (session.clone(), pane_id.clone());
        move || {
            let client = client();
            let (session, pane_id) = (session.clone(), pane_id.clone());
            let src = source();
            async move {
                let _ = refresh.read();
                let lines = (src == "recent").then_some(SCROLLBACK);
                client
                    .read_pane(&pane_id, some(session).as_deref(), &src, lines)
                    .await
                    .map_err(|e| e.to_string())
            }
        }
    });
    let name = pane
        .as_ref()
        .and_then(|p| p.label.clone().or_else(|| p.agent.clone()))
        .unwrap_or_default();

    rsx! {
        section { class: if wide() { "card deck-card deck-wide" } else { "card deck-card" },
            div { class: "card-head",
                h2 { class: "card-title mono", "{pane_id}" }
                if !name.is_empty() {
                    span { "{name}" }
                }
                if let Some(p) = &pane {
                    span { class: "pill", "{cell(&p.agent_status)}" }
                } else {
                    span { class: "pill send-error", "gone" }
                }
                if !session.is_empty() {
                    span { class: "pill muted", "{session}" }
                }
                div { class: "deck-actions",
                    button {
                        class: "btn",
                        title: if wide() { "Shrink" } else { "Widen" },
                        onclick: move |_| wide.toggle(),
                        if wide() { "⤡" } else { "⤢" }
                    }
                    button {
                        class: "btn",
                        title: "Stop watching",
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }
            }
            match &*text.read_unchecked() {
                None => rsx! { p { class: "muted", "Reading…" } },
                Some(Err(e)) => rsx! { div { class: "alert alert-error", "{e}" } },
                Some(Ok(t)) => rsx! {
                    // column-reverse keeps the scroll pinned to the newest line.
                    div { class: "deck-screen",
                        pre { class: "mono", "{screen(t)}" }
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nests_tabs_under_workspaces_in_first_seen_order() {
        let p = |id: &str, ws: &str, tab: &str| -> HerdrPane {
            serde_json::from_value(serde_json::json!({
                "pane_id": id, "workspace_id": ws, "tab_id": tab
            }))
            .unwrap()
        };
        let panes = [
            p("a", "w2", "w2:t1"),
            p("b", "w1", "w1:t1"),
            p("c", "w2", "w2:t2"),
            p("d", "w2", "w2:t1"),
        ];
        let tree = by_workspace(&panes);
        // One line per tab: "workspace tab: panes".
        let shape: Vec<String> = tree
            .iter()
            .flat_map(|(w, tabs)| {
                tabs.iter().map(move |(t, rows)| {
                    let ids: Vec<&str> = rows.iter().map(|p| p.pane_id.as_str()).collect();
                    format!("{w} {t}: {}", ids.join(","))
                })
            })
            .collect();
        assert_eq!(shape, ["w2 w2:t1: a,d", "w2 w2:t2: c", "w1 w1:t1: b"]);
    }

    #[test]
    fn screen_drops_trailing_blank_lines() {
        assert_eq!(screen("$ make\nok\n\n   \n"), "$ make\nok");
    }
}

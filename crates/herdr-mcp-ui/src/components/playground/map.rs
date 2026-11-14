//! The map: every collection, what starts it (herdr event triggers, cron
//! schedules) and the collections its run nodes start, coloured by each
//! one's latest run as runs stream in. Links into a deleted collection end
//! at a "missing" box. Pick a collection to read its plan.

use std::collections::HashMap;

use dioxus::prelude::*;
use herdr_mcp_api::playground::{
    MapLink, MapSource, NODE_H, NODE_W, RUNS_SHOWN, map_columns, map_links, schedules,
};
use herdr_mcp_api::{Recipe, RunRecord, RunStatus, StartRunRequest, Uuid, tool_payload};
use serde_json::json;

use super::runs::{changed, use_run_events};
use crate::client::use_api;

const GAP_X: f32 = 110.0;
const GAP_Y: f32 = 28.0;

/// A box on the map.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Spot {
    Source(MapSource),
    Collection(Uuid),
}

fn status_class(s: Option<RunStatus>) -> &'static str {
    match s {
        Some(RunStatus::Running | RunStatus::Paused) => " node-running",
        Some(RunStatus::Completed) => " node-done",
        Some(RunStatus::Failed | RunStatus::Interrupted) => " node-failed",
        Some(RunStatus::Cancelled) => " node-cancelled",
        None => "",
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

#[component]
pub fn Map() -> Element {
    let client = use_api();
    let mut recipes: Signal<Vec<Recipe>> = use_signal(Vec::new);
    let mut sched: Signal<Vec<(Uuid, String, bool)>> = use_signal(Vec::new);
    let mut runs: Signal<Vec<RunRecord>> = use_signal(Vec::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut picked: Signal<Option<Uuid>> = use_signal(|| None);
    let seen = use_run_events();

    // Collections and schedules once, runs on every run event.
    use_future(move || async move {
        let client = client();
        match client.list_recipes().await {
            Ok(list) => recipes.set(list.into_iter().filter(|r| !r.is_template).collect()),
            Err(e) => error.set(Some(format!("cannot load collections: {e}"))),
        }
        match client.call_tool("list_schedules", &json!({})).await {
            Ok(v) => sched.set(tool_payload(&v).map(|p| schedules(&p)).unwrap_or_default()),
            Err(e) => error.set(Some(format!("cannot load schedules: {e}"))),
        }
        let mut read = None;
        loop {
            changed(seen, &mut read).await;
            if let Ok(list) = client.list_runs(RUNS_SHOWN).await {
                runs.set(list);
            }
        }
    });

    let recipes = recipes();
    let links = map_links(&recipes, &sched.read());
    let cols = map_columns(&recipes, &links);
    let names: HashMap<Uuid, String> = recipes.iter().map(|r| (r.id, r.name.clone())).collect();
    // Runs come newest first: the first per collection is its latest.
    let mut latest: HashMap<Uuid, RunRecord> = HashMap::new();
    for r in runs.read().iter() {
        if let Some(id) = r.recipe_id {
            latest.entry(id).or_insert_with(|| r.clone());
        }
    }

    // Sources in column 0, collections by their column from 1, missing
    // collections in a column after the rest.
    let mut at: HashMap<Spot, (f32, f32)> = HashMap::new();
    let mut rows: HashMap<usize, usize> = HashMap::new();
    let mut place = |spot: Spot, col: usize, at: &mut HashMap<Spot, (f32, f32)>| {
        if at.contains_key(&spot) {
            return;
        }
        let row = rows.entry(col).or_default();
        at.insert(
            spot,
            (
                col as f32 * (NODE_W + GAP_X),
                *row as f32 * (NODE_H + GAP_Y),
            ),
        );
        *row += 1;
    };
    for l in &links {
        if !matches!(l.from, MapSource::Collection(_)) {
            place(Spot::Source(l.from.clone()), 0, &mut at);
        }
    }
    for r in &recipes {
        place(Spot::Collection(r.id), cols[&r.id] + 1, &mut at);
    }
    let last = cols.values().max().map_or(1, |c| c + 2);
    for l in &links {
        if !names.contains_key(&l.to) {
            place(Spot::Collection(l.to), last, &mut at);
        }
    }
    let width = at.values().map(|p| p.0).fold(0.0, f32::max) + NODE_W + 20.0;
    let height = at.values().map(|p| p.1).fold(0.0, f32::max) + NODE_H + 90.0;

    let edges: Vec<(String, String, bool)> = links
        .iter()
        .filter_map(|l| {
            let from = match &l.from {
                MapSource::Collection(id) => Spot::Collection(*id),
                s => Spot::Source(s.clone()),
            };
            let (a, b) = (at.get(&from)?, at.get(&Spot::Collection(l.to))?);
            let (x1, y1, x2, y2) = (a.0 + NODE_W, a.1 + NODE_H / 2.0, b.0, b.1 + NODE_H / 2.0);
            let back = x2 <= a.0;
            let d = if back {
                let drop = 60.0 + (y1 - y2).abs() * 0.2;
                format!(
                    "M{x1} {y1} C{} {} {} {} {x2} {y2}",
                    x1 + 60.0,
                    y1.max(y2) + drop,
                    x2 - 60.0,
                    y1.max(y2) + drop
                )
            } else {
                let dx = ((x2 - x1) / 2.0).max(40.0);
                format!("M{x1} {y1} C{} {y1} {} {y2} {x2} {y2}", x1 + dx, x2 - dx)
            };
            let key = format!("{:?}>{}:{}", l.from, l.to, l.label);
            Some((key, d, back))
        })
        .collect();

    let boxes: Vec<(Spot, (f32, f32))> = at.into_iter().collect();

    rsx! {
        p { class: "field-desc",
            "What starts each collection: herdr events (triggers), cron schedules, and other collections' run nodes. Boxes take the colour of the collection's latest run; dashed links loop back. Pick a collection for its plan."
        }
        if let Some(e) = error() {
            p { class: "send-status send-error", "{e}" }
        }
        if recipes.is_empty() {
            p { class: "muted", "No collections yet." }
        }
        div { class: "map-wrap",
            svg {
                class: "map",
                width: "{width}",
                height: "{height}",
                for (key, d, back) in edges {
                    path { key: "{key}", class: if back { "edge edge-loop" } else { "edge" }, d: "{d}" }
                }
                for (spot, (x, y)) in boxes {
                    {
                        let (class, title, sub, pick) = match &spot {
                            Spot::Source(MapSource::Trigger(on)) => ("node node-start".to_string(), "⚡ trigger".to_string(), on.clone(), None),
                            Spot::Source(MapSource::Schedule(cron)) => ("node node-start".to_string(), "⏱ schedule".to_string(), cron.clone(), None),
                            Spot::Source(MapSource::Collection(_)) => unreachable!("collections are placed as collections"),
                            Spot::Collection(id) => match names.get(id) {
                                Some(name) => {
                                    let run = latest.get(id);
                                    let sub = run.map_or_else(|| "no runs".to_string(), |r| r.status.as_str().to_string());
                                    let sel = if picked() == Some(*id) { " node-sel" } else { "" };
                                    let class = format!("node node-run{}{sel}", status_class(run.map(|r| r.status)));
                                    (class, name.clone(), sub, Some(*id))
                                }
                                None => ("node node-fail".to_string(), "missing".to_string(), "deleted collection".to_string(), None),
                            },
                        };
                        let key = match &spot {
                            Spot::Source(s) => format!("{s:?}"),
                            Spot::Collection(id) => id.to_string(),
                        };
                        rsx! {
                            g {
                                key: "{key}",
                                class: "{class}",
                                onclick: move |_| {
                                    if pick.is_some() {
                                        picked.set(pick);
                                    }
                                },
                                rect { x: "{x}", y: "{y}", width: "{NODE_W}", height: "{NODE_H}", rx: "8" }
                                text { class: "node-title", x: "{x + 12.0}", y: "{y + 22.0}", "{clip(&title, 22)}" }
                                text { class: "node-sub", x: "{x + 12.0}", y: "{y + 40.0}", "{clip(&sub, 26)}" }
                            }
                        }
                    }
                }
            }
        }
        if let Some(recipe) = picked().and_then(|id| recipes.iter().find(|r| r.id == id).cloned()) {
            Plan {
                key: "{recipe.id}",
                links: links.iter().filter(|l| l.to == recipe.id || l.from == MapSource::Collection(recipe.id)).cloned().collect::<Vec<_>>(),
                latest: latest.get(&recipe.id).cloned(),
                names,
                recipe,
            }
        }
    }
}

/// A collection's plan: what starts it, what it starts, what it does, how
/// its latest run went.
#[component]
fn Plan(
    recipe: Recipe,
    links: Vec<MapLink>,
    latest: Option<RunRecord>,
    names: HashMap<Uuid, String>,
) -> Element {
    let client = use_api();
    let mut msg: Signal<Option<Result<String, String>>> = use_signal(|| None);
    let id = recipe.id;
    let name = |id: &Uuid| {
        names
            .get(id)
            .cloned()
            .unwrap_or_else(|| "missing collection".into())
    };
    let starts: Vec<String> = links
        .iter()
        .filter(|l| l.to == id)
        .map(|l| match &l.from {
            MapSource::Trigger(on) if l.label.is_empty() => format!("⚡ {on}"),
            MapSource::Trigger(on) => format!("⚡ {on} when {}", l.label),
            MapSource::Schedule(cron) if l.label == "off" => format!("⏱ {cron} (off)"),
            MapSource::Schedule(cron) => format!("⏱ {cron}"),
            MapSource::Collection(from) => format!("⇢ {} (node {})", name(from), l.label),
        })
        .collect();
    let hands: Vec<String> = links
        .iter()
        .filter(|l| l.from == MapSource::Collection(id))
        .map(|l| format!("⇢ {} (node {})", name(&l.to), l.label))
        .collect();
    let problems = recipe.flow.problems();
    let kinds = {
        let mut count: Vec<(&str, usize)> = Vec::new();
        for n in &recipe.flow.nodes {
            match count.iter_mut().find(|(k, _)| *k == n.kind.name()) {
                Some((_, c)) => *c += 1,
                None => count.push((n.kind.name(), 1)),
            }
        }
        count
            .into_iter()
            .map(|(k, c)| format!("{c} {k}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let run_now = move |_| {
        let client = client();
        spawn(async move {
            let req = StartRunRequest {
                recipe_id: Some(id),
                ..Default::default()
            };
            msg.set(Some(match client.start_run(&req).await {
                Ok(r) => Ok(format!("started run {}", r.id)),
                Err(e) => Err(format!("run failed to start: {e}")),
            }));
        });
    };

    rsx! {
        div { class: "card",
            div { class: "response-head",
                span { class: "response-title", "{recipe.name}" }
                span { class: "muted", "{kinds}" }
            }
            div { class: "var-group-title", "Started by" }
            if starts.is_empty() {
                p { class: "muted", "Nothing: it runs when sent from the playground or an agent." }
            }
            for s in starts {
                div { key: "{s}", class: "mono", "{s}" }
            }
            div { class: "var-group-title", "Starts" }
            if hands.is_empty() {
                p { class: "muted", "No other collection." }
            }
            for h in hands {
                div { key: "{h}", class: "mono", "{h}" }
            }
            for p in problems {
                p { key: "{p}", class: "send-status send-error", "{p}" }
            }
            div { class: "var-group-title", "Latest run" }
            match latest {
                Some(r) => rsx! { p { class: "mono", "{r.status.as_str()} · {r.summary()} · by {r.origin}" } },
                None => rsx! { p { class: "muted", "None kept." } },
            }
            div { class: "btn-row",
                button { class: "btn btn-primary", onclick: run_now, "Run now" }
                match &*msg.read() {
                    Some(Ok(m)) => rsx! { span { class: "send-status", "{m}" } },
                    Some(Err(m)) => rsx! { span { class: "send-status send-error", "{m}" } },
                    None => rsx! {},
                }
            }
        }
    }
}

//! A collection's flow settings: the gate (end / restart / fail / run
//! another collection after the last request) and how long its runs are
//! kept.

use dioxus::prelude::*;
use herdr_mcp_api::gate::{inputs_text, parse_inputs, rule_text};
use herdr_mcp_api::playground::{display_value, parse_value};
use herdr_mcp_api::settings::{KEEP_DAYS, KEEP_RUNS, SNAPSHOT_LINES};
use herdr_mcp_api::{Check, Gate, GateAction, GateRule, Op, RunTrail, Uuid};
use serde_json::Value;

use crate::client::use_api;

fn action_kind(a: &GateAction) -> &'static str {
    match a {
        GateAction::End => "end",
        GateAction::Restart { .. } => "restart",
        GateAction::Fail { .. } => "fail",
        GateAction::Run { .. } => "run",
    }
}

/// The gate's rules, checked in order after the last request; the first
/// match ends, restarts, fails or hands the run off to one of the other
/// `collections`. `paths` are suggestions for a check's path (the
/// requests' ids, `run.pass`).
#[component]
pub fn GateEditor(
    gate: Signal<Gate>,
    collections: Vec<(Uuid, String)>,
    paths: Vec<String>,
) -> Element {
    let rules = gate.read().rules.clone();
    let max_steps = gate.read().max_steps;
    rsx! {
        div { class: "card",
            div { class: "var-group-title", "Gate · after the last request" }
            p { class: "field-desc",
                "Rules are checked in order once every request ran; the first that holds decides: "
                b { "end" }
                ", "
                b { "restart" }
                " from the first request (a loop), or "
                b { "fail" }
                ". A check reads a path like a placeholder: "
                code { "list.panes[0].pane_id" }
                ", a variable, or "
                code { "run.pass" }
                " (1 on the first pass). No rule matching completes the run. Sending one request never uses the gate."
            }
            datalist { id: "gate-paths",
                for p in paths.iter() {
                    option { value: "{p}" }
                }
            }
            for (i, rule) in rules.iter().cloned().enumerate() {
                GateRuleRow {
                    key: "{i}",
                    gate,
                    index: i,
                    rule,
                    last: i + 1 == rules.len(),
                    collections: collections.clone(),
                }
            }
            div { class: "btn-row",
                button {
                    class: "btn",
                    onclick: move |_| {
                        gate.write().rules.push(GateRule {
                            checks: vec![Check {
                                path: String::new(),
                                op: Op::Exists,
                                value: Value::Null,
                            }],
                            any: false,
                            then: GateAction::End,
                        });
                    },
                    "+ Rule"
                }
                if !rules.is_empty() {
                    label { class: "form-label", "Max requests per run" }
                    input {
                        class: "input input-narrow",
                        r#type: "number",
                        min: "1",
                        title: "Every pass of a restart loop counts; a run that would go over fails.",
                        value: "{max_steps}",
                        onchange: move |e| {
                            if let Ok(n) = e.value().trim().parse::<u32>() {
                                gate.write().max_steps = n.max(1);
                            }
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn GateRuleRow(
    gate: Signal<Gate>,
    index: usize,
    rule: GateRule,
    last: bool,
    collections: Vec<(Uuid, String)>,
) -> Element {
    let i = index;
    let kind = action_kind(&rule.then);
    let mut inputs_error = use_signal(|| None::<String>);
    let (param, target) = match &rule.then {
        GateAction::End => (String::new(), None),
        GateAction::Restart { delay_ms } => (delay_ms.to_string(), None),
        GateAction::Fail { message } => (message.clone(), None),
        GateAction::Run { collection, inputs } => (inputs_text(inputs), Some(*collection)),
    };
    let preview = rule_text(&rule, &collections);
    let first = collections.first().map(|(id, _)| *id);
    rsx! {
        div { class: "gate-rule",
            div { class: "btn-row",
                span { class: "pill", "{i + 1}" }
                select {
                    class: "input input-narrow",
                    onchange: move |e| gate.write().rules[i].any = e.value() == "any",
                    option { value: "all", selected: !rule.any, "all of" }
                    option { value: "any", selected: rule.any, "any of" }
                }
                span { class: "muted", if rule.checks.is_empty() { "always" } else { "these hold" } }
                span { class: "spacer" }
                button {
                    class: "link-btn",
                    disabled: i == 0,
                    onclick: move |_| gate.write().rules.swap(i, i - 1),
                    "↑"
                }
                button {
                    class: "link-btn",
                    disabled: last,
                    onclick: move |_| gate.write().rules.swap(i, i + 1),
                    "↓"
                }
                button {
                    class: "link-btn link-danger",
                    onclick: move |_| {
                        gate.write().rules.remove(i);
                    },
                    "✕"
                }
            }
            for (j, check) in rule.checks.iter().cloned().enumerate() {
                div { key: "{j}", class: "gate-check",
                    input {
                        class: "input mono",
                        r#type: "text",
                        list: "gate-paths",
                        placeholder: "path, e.g. read.text",
                        value: "{check.path}",
                        oninput: move |e| gate.write().rules[i].checks[j].path = e.value(),
                    }
                    select {
                        class: "input input-narrow",
                        onchange: move |e| {
                            if let Some(op) = Op::ALL.into_iter().find(|o| o.name() == e.value()) {
                                let mut g = gate.write();
                                let c = &mut g.rules[i].checks[j];
                                c.op = op;
                                if !op.takes_value() {
                                    c.value = Value::Null;
                                }
                            }
                        },
                        for op in Op::ALL {
                            option { value: "{op.name()}", selected: op == check.op, "{op.name()}" }
                        }
                    }
                    if check.op.takes_value() {
                        input {
                            class: "input mono",
                            r#type: "text",
                            placeholder: "value",
                            value: "{display_value(&check.value)}",
                            oninput: move |e| gate.write().rules[i].checks[j].value = parse_value(e.value().trim()),
                        }
                    }
                    button {
                        class: "link-btn link-danger",
                        onclick: move |_| {
                            gate.write().rules[i].checks.remove(j);
                        },
                        "✕"
                    }
                }
            }
            div { class: "btn-row",
                button {
                    class: "link-btn",
                    onclick: move |_| {
                        gate.write().rules[i].checks.push(Check {
                            path: String::new(),
                            op: Op::Eq,
                            value: Value::Null,
                        });
                    },
                    "+ check"
                }
                span { class: "muted", "then" }
                select {
                    class: "input input-narrow",
                    onchange: move |e| {
                        gate.write().rules[i].then = match e.value().as_str() {
                            "restart" => GateAction::Restart { delay_ms: 1000 },
                            "fail" => GateAction::Fail { message: String::new() },
                            "run" => match first {
                                Some(collection) => GateAction::Run { collection, inputs: Default::default() },
                                None => return,
                            },
                            _ => GateAction::End,
                        };
                    },
                    option { value: "end", selected: kind == "end", "end" }
                    option { value: "restart", selected: kind == "restart", "restart" }
                    option { value: "fail", selected: kind == "fail", "fail" }
                    option {
                        value: "run",
                        selected: kind == "run",
                        disabled: first.is_none(),
                        title: "Hand off to another collection: it runs as a child run with its own variables, and this run ends as it ends",
                        "run collection"
                    }
                }
                match kind {
                    "restart" => rsx! {
                        input {
                            class: "input input-narrow",
                            r#type: "number",
                            min: "0",
                            title: "Delay before the next pass, in ms",
                            value: "{param}",
                            onchange: move |e| {
                                let delay_ms = e.value().trim().parse().unwrap_or(0);
                                gate.write().rules[i].then = GateAction::Restart { delay_ms };
                            },
                        }
                        span { class: "muted", "ms" }
                    },
                    "fail" => rsx! {
                        input {
                            class: "input",
                            r#type: "text",
                            placeholder: "message (optional)",
                            value: "{param}",
                            oninput: move |e| gate.write().rules[i].then = GateAction::Fail { message: e.value() },
                        }
                    },
                    "run" => rsx! {
                        select {
                            class: "input input-narrow",
                            onchange: move |e| {
                                if let (Ok(id), GateAction::Run { collection, .. }) = (e.value().parse::<Uuid>(), &mut gate.write().rules[i].then) {
                                    *collection = id;
                                }
                            },
                            if let Some(id) = rule.then.missing_collection(&collections) {
                                option { value: "{id}", selected: true, "missing collection" }
                            }
                            for (id, name) in collections.iter() {
                                option { value: "{id}", selected: target == Some(*id), "{name}" }
                            }
                        }
                        if rule.then.missing_collection(&collections).is_some() {
                            span {
                                class: "send-status send-error",
                                "No other saved collection has this id (deleted?): pick one, or the run fails here."
                            }
                        }
                        input {
                            class: "input mono",
                            r#type: "text",
                            placeholder: "inputs: key=value … ({{{{path}}}} reads this run)",
                            title: "Variables the child run starts with, over its own and the globals; values may use {{{{placeholders}}}} from this run",
                            value: "{param}",
                            onchange: move |e| match parse_inputs(&e.value()) {
                                Ok(parsed) => {
                                    inputs_error.set(None);
                                    if let GateAction::Run { inputs, .. } = &mut gate.write().rules[i].then {
                                        *inputs = parsed;
                                    }
                                }
                                Err(err) => inputs_error.set(Some(err)),
                            },
                        }
                    },
                    _ => rsx! {},
                }
            }
            if let Some(err) = inputs_error() {
                p { class: "send-status send-error", "{err}" }
            }
            code { class: "mono muted gate-preview", "{preview}" }
        }
    }
}

/// How long this collection's runs are kept and how many pane lines a
/// failed run saves; blank fields use the global `runs.*` settings.
#[component]
pub fn TrailEditor(trail: Signal<RunTrail>) -> Element {
    let client = use_api();
    let globals = use_resource(move || {
        let client = client();
        async move { client.settings().await.ok().map(|s| s.overrides()) }
    });
    let global = move |key: &str| {
        globals
            .read()
            .clone()
            .flatten()
            .map(|s| s.number(key))
            .unwrap_or(0)
    };
    let hint = |n: u64, none: &str| {
        if n == 0 {
            format!("global: {none}")
        } else {
            format!("global: {n}")
        }
    };
    let field = |v: Option<u32>| v.map(|n| n.to_string()).unwrap_or_default();
    let parse = |text: String| text.trim().parse::<u32>().ok();
    let t = *trail.read();
    rsx! {
        div { class: "card",
            div { class: "var-group-title", "Run history" }
            p { class: "field-desc",
                "How long this collection's runs are kept in the Runs tab, and how many lines of the pane a failed run last used are saved into its trail. Blank uses the global setting (Settings › runs); 0 means no limit."
            }
            div { class: "form-row",
                div { class: "form-col",
                    label { class: "form-label", "Keep last runs" }
                    input {
                        class: "input",
                        r#type: "number",
                        min: "0",
                        placeholder: "{hint(global(KEEP_RUNS), \"keep all\")}",
                        value: "{field(t.keep_runs)}",
                        onchange: move |e| trail.write().keep_runs = parse(e.value()),
                    }
                }
                div { class: "form-col",
                    label { class: "form-label", "Keep days" }
                    input {
                        class: "input",
                        r#type: "number",
                        min: "0",
                        placeholder: "{hint(global(KEEP_DAYS), \"forever\")}",
                        value: "{field(t.keep_days)}",
                        onchange: move |e| trail.write().keep_days = parse(e.value()),
                    }
                }
                div { class: "form-col",
                    label { class: "form-label", "Snapshot lines" }
                    input {
                        class: "input",
                        r#type: "number",
                        min: "0",
                        placeholder: "{hint(global(SNAPSHOT_LINES), \"none\")}",
                        value: "{field(t.snapshot_lines)}",
                        onchange: move |e| trail.write().snapshot_lines = parse(e.value()),
                    }
                }
            }
        }
    }
}

//! Editors for a flow's non-tool nodes (start, branch, run, wait, fail) and
//! how long a collection's runs are kept.

use dioxus::prelude::*;
use herdr_mcp_api::flow::{inputs_text, parse_inputs};
use herdr_mcp_api::playground::{display_value, parse_value};
use herdr_mcp_api::settings::{KEEP_DAYS, KEEP_RUNS, SNAPSHOT_LINES};
use herdr_mcp_api::{Check, Node, NodeKind, Op, Rule, RunTrail, Uuid};
use serde_json::Value;

use crate::client::use_api;

/// A start, branch, run, wait or fail node. `paths` suggest a check's path
/// (the flow's node ids, `run.pass`); `collections` are the ones a run node
/// may start.
#[component]
pub fn NodeEditor(
    node: Node,
    collections: Vec<(Uuid, String)>,
    paths: Vec<String>,
    on_save: EventHandler<Node>,
    on_delete: EventHandler<()>,
) -> Element {
    let mut draft = use_signal(|| node.clone());
    let mut inputs = use_signal(|| match &node.kind {
        NodeKind::Run { inputs, .. } => inputs_text(inputs),
        _ => String::new(),
    });
    let mut error = use_signal(|| None::<String>);
    let start = matches!(node.kind, NodeKind::Start);
    let d = draft.read().clone();

    let save = move |_| {
        let mut next = draft.read().clone();
        next.id = next.id.trim().to_string();
        if let NodeKind::Run { inputs: parsed, .. } = &mut next.kind {
            match parse_inputs(&inputs.read()) {
                Ok(v) => *parsed = v,
                Err(e) => {
                    error.set(Some(e));
                    return;
                }
            }
        }
        error.set(None);
        on_save.call(next);
    };

    rsx! {
        div { class: "card",
            div { class: "var-group-title", "{d.kind.name()} node" }
            if start {
                p { class: "field-desc",
                    "Where every run begins. Wire its port to the first node; several edges run their nodes in parallel."
                }
            } else {
                div { class: "form-row",
                    div { class: "form-col",
                        label { class: "form-label", "Node" }
                        input {
                            class: "input mono",
                            r#type: "text",
                            value: "{d.id}",
                            oninput: move |e| draft.write().id = e.value(),
                        }
                    }
                    div { class: "form-col",
                        label { class: "form-label", "Description" }
                        input {
                            class: "input",
                            r#type: "text",
                            placeholder: "optional",
                            value: "{d.description.clone().unwrap_or_default()}",
                            oninput: move |e| draft.write().description = Some(e.value()).filter(|v| !v.trim().is_empty()),
                        }
                    }
                }
            }
            match d.kind.clone() {
                NodeKind::Branch { rules } => rsx! {
                    BranchRules { draft, rules, paths }
                },
                NodeKind::Run { collection, .. } => rsx! {
                    p { class: "field-desc",
                        "Runs another collection as a child run with its own variables, the globals and these inputs, and waits for it. Its result is "
                        code { "{{run_id, status, error, variables}}" }
                        "; port error when the child does not complete."
                    }
                    div { class: "form-row",
                        select {
                            class: "input",
                            onchange: move |e| {
                                if let (Ok(id), NodeKind::Run { collection, .. }) = (e.value().parse::<Uuid>(), &mut draft.write().kind) {
                                    *collection = id;
                                }
                            },
                            if !collections.iter().any(|(id, _)| *id == collection) {
                                option { value: "{collection}", selected: true, "missing collection" }
                            }
                            for (id, name) in collections.iter() {
                                option { value: "{id}", selected: *id == collection, "{name}" }
                            }
                        }
                        input {
                            class: "input mono",
                            r#type: "text",
                            placeholder: "inputs: key=value … ({{{{path}}}} reads this run)",
                            value: "{inputs}",
                            oninput: move |e| inputs.set(e.value()),
                        }
                    }
                },
                NodeKind::Wait { ms } => rsx! {
                    div { class: "btn-row",
                        label { class: "form-label", "Wait" }
                        input {
                            class: "input input-narrow",
                            r#type: "number",
                            min: "0",
                            value: "{ms}",
                            onchange: move |e| {
                                if let NodeKind::Wait { ms } = &mut draft.write().kind {
                                    *ms = e.value().trim().parse().unwrap_or(0);
                                }
                            },
                        }
                        span { class: "muted", "ms" }
                    }
                },
                NodeKind::Fail { message } => rsx! {
                    input {
                        class: "input",
                        r#type: "text",
                        placeholder: "message (empty: the error names the node)",
                        value: "{message}",
                        oninput: move |e| {
                            if let NodeKind::Fail { message } = &mut draft.write().kind {
                                *message = e.value();
                            }
                        },
                    }
                },
                NodeKind::Start | NodeKind::Tool { .. } => rsx! {},
            }
            if !start {
                div { class: "btn-row",
                    button { class: "btn btn-primary", onclick: save, "Save" }
                    button { class: "btn link-danger", onclick: move |_| on_delete.call(()), "Delete" }
                }
            }
            if let Some(e) = error() {
                p { class: "send-status send-error", "{e}" }
            }
        }
    }
}

fn rules_mut(node: &mut Node) -> &mut Vec<Rule> {
    match &mut node.kind {
        NodeKind::Branch { rules } => rules,
        _ => unreachable!("BranchRules edits a branch node"),
    }
}

/// A branch's rules: the first that holds fires its port (rule 1 → port
/// `1`), none fires `else`. Only the last rule can go, so the others keep
/// their ports.
#[component]
fn BranchRules(draft: Signal<Node>, rules: Vec<Rule>, paths: Vec<String>) -> Element {
    let last = rules.len();
    rsx! {
        p { class: "field-desc",
            "A check reads a path like a placeholder: "
            code { "list.panes[0].pane_id" }
            ", a variable, or "
            code { "run.pass" }
            " (1 on the first pass, +1 per loop). A rule with no checks always holds."
        }
        datalist { id: "check-paths",
            for p in paths.iter() {
                option { value: "{p}" }
            }
        }
        for (i, rule) in rules.iter().cloned().enumerate() {
            div { key: "{i}", class: "rule-card",
                div { class: "btn-row",
                    span { class: "pill", "port {i + 1}" }
                    select {
                        class: "input input-narrow",
                        onchange: move |e| rules_mut(&mut draft.write())[i].any = e.value() == "any",
                        option { value: "all", selected: !rule.any, "all of" }
                        option { value: "any", selected: rule.any, "any of" }
                    }
                    span { class: "spacer" }
                    if i + 1 == last {
                        button {
                            class: "link-btn link-danger",
                            title: "Its port's edges go too",
                            onclick: move |_| {
                                rules_mut(&mut draft.write()).pop();
                            },
                            "✕"
                        }
                    }
                }
                for (j, check) in rule.checks.iter().cloned().enumerate() {
                    div { key: "{j}", class: "rule-check",
                        input {
                            class: "input mono",
                            r#type: "text",
                            list: "check-paths",
                            placeholder: "path, e.g. read.text",
                            value: "{check.path}",
                            oninput: move |e| rules_mut(&mut draft.write())[i].checks[j].path = e.value(),
                        }
                        select {
                            class: "input input-narrow",
                            onchange: move |e| {
                                if let Some(op) = Op::ALL.into_iter().find(|o| o.name() == e.value()) {
                                    let mut node = draft.write();
                                    let c = &mut rules_mut(&mut node)[i].checks[j];
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
                                oninput: move |e| rules_mut(&mut draft.write())[i].checks[j].value = parse_value(e.value().trim()),
                            }
                        }
                        button {
                            class: "link-btn link-danger",
                            onclick: move |_| {
                                rules_mut(&mut draft.write())[i].checks.remove(j);
                            },
                            "✕"
                        }
                    }
                }
                button {
                    class: "link-btn",
                    onclick: move |_| {
                        rules_mut(&mut draft.write())[i].checks.push(Check {
                            path: String::new(),
                            op: Op::Eq,
                            value: Value::Null,
                        });
                    },
                    "+ check"
                }
            }
        }
        button {
            class: "btn",
            onclick: move |_| rules_mut(&mut draft.write()).push(Rule { checks: Vec::new(), any: false }),
            "+ Rule"
        }
        p { class: "muted", "No rule holding fires port else." }
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

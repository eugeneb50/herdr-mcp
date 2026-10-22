//! Pieces of a collection (a saved recipe) the playground reuses: filling
//! in a template, the trigger editor, and a finished run's results.

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::HashMap;

use super::forms::ResponseViewer;
use crate::client::use_api;
use crate::components::pane_options::pane_list;
use herdr_mcp_api::{InstantiateTemplateRequest, RecipeStep, RecipeTemplate, RecipeTrigger};

/// A finished run (`RunRecord`): its status, then each step's result in
/// step order.
#[component]
pub fn RunResults(data: Value, order: Vec<String>) -> Element {
    let status = data
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let error = data
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string);
    let results = data.get("results").cloned().unwrap_or(Value::Null);
    let ran: Vec<(usize, String, Value)> = order
        .iter()
        .enumerate()
        .filter_map(|(i, id)| Some((i + 1, id.clone(), results.get(id)?.clone())))
        .collect();
    let ok = status == "completed";
    rsx! {
        div { class: "card",
            div { class: "response-head",
                span { class: "response-title", "Run" }
                span { class: if ok { "pill" } else { "pill pill-error" }, "{status}" }
            }
            if let Some(e) = error {
                div { class: "alert alert-error", "{e}" }
            }
            for (n, id, result) in ran {
                div { key: "{id}",
                    div { class: "form-label", "#{n} {id}" }
                    match result.get("error").and_then(Value::as_str) {
                        Some(e) => rsx! { div { class: "alert alert-error", "{e}" } },
                        None => rsx! { ResponseViewer { data: result.clone() } },
                    }
                }
            }
        }
    }
}

/// Variables of a template, filled in before it becomes a new collection.
/// The server does the substitution (`/api/templates/{id}/instantiate`);
/// pane fields suggest the live panes (the Playground's `LivePaneOptions`).
#[component]
pub fn TemplateForm(
    template: RecipeTemplate,
    on_load: EventHandler<(String, Vec<RecipeStep>)>,
    on_cancel: EventHandler<()>,
) -> Element {
    let client = use_api();
    let mut values: Signal<HashMap<String, String>> = use_signal(|| {
        template
            .variables
            .iter()
            .map(|v| {
                let default = v.default_value.as_ref().map(crate::vars::display_value);
                (v.name.clone(), default.unwrap_or_default())
            })
            .collect()
    });
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let missing = template.variables.iter().any(|v| {
        v.required
            && values
                .read()
                .get(&v.name)
                .is_none_or(|s| s.trim().is_empty())
    });
    let id = template.id.clone();
    let name = template.name.clone();
    let load = move |_| {
        let client = client();
        let (id, name) = (id.clone(), name.clone());
        let variables = values
            .read()
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        spawn(async move {
            let req = InstantiateTemplateRequest {
                variables,
                name: Some(name),
            };
            match client.instantiate_template(&id, &req).await {
                Ok(recipe) => on_load.call((recipe.name, recipe.steps)),
                Err(e) => error.set(Some(format!("Could not fill in template: {e}"))),
            }
        });
    };
    rsx! {
        div { class: "card",
            h3 { class: "card-title", "{template.name}" }
            p { class: "field-desc", "{template.description}" }
            for var in template.variables.iter().cloned() {
                div { key: "{var.name}", class: "field",
                    label { class: "field-head",
                        code { class: "mono accent", "{var.name}" }
                        if var.required {
                            span { class: "required", "required" }
                        }
                    }
                    p { class: "field-desc", "{var.description}" }
                    input {
                        class: "input",
                        r#type: "text",
                        list: pane_list(&var.name),
                        value: "{values.read().get(&var.name).cloned().unwrap_or_default()}",
                        oninput: move |e| {
                            values.write().insert(var.name.clone(), e.value());
                        },
                    }
                }
            }
            if let Some(e) = error.read().as_deref() {
                div { class: "alert alert-error", "{e}" }
            }
            div { class: "btn-row",
                button {
                    class: "btn btn-primary",
                    disabled: missing,
                    onclick: load,
                    "Create collection"
                }
                button { class: "btn", onclick: move |_| on_cancel.call(()), "Cancel" }
            }
        }
    }
}

/// `when` as the editor shows it: `agent_status=done, pane.tab_id=w1:t1`.
fn when_text(when: &serde_json::Map<String, Value>) -> String {
    when.iter()
        .map(|(k, v)| format!("{k}={}", crate::vars::display_value(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Inverse of [`when_text`]: values that parse as JSON (`true`, `3`) stay
/// typed, anything else is a string.
fn parse_when(text: &str) -> serde_json::Map<String, Value> {
    text.split(',')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let v = v.trim();
            let value = serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_string()));
            Some((k.trim().to_string(), value))
        })
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

/// herdr events a trigger can name: every subscription type in the
/// installed herdr's schema.
fn event_names(schema: &Value) -> Vec<String> {
    schema
        .pointer("/schemas/request/$defs/Subscription/oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.pointer("/properties/type/const")?.as_str())
        .map(str::to_string)
        .collect()
}

/// The recipe's triggers: herdr events that start a run of it (once saved).
#[component]
pub fn TriggerEditor(
    triggers: Vec<RecipeTrigger>,
    on_change: EventHandler<Vec<RecipeTrigger>>,
) -> Element {
    let client = use_api();
    let names = use_resource(move || {
        let client = client();
        async move {
            client
                .herdr_schema()
                .await
                .map(|s| event_names(&s))
                .unwrap_or_default()
        }
    });
    rsx! {
        div { class: "card",
            div { class: "var-group-title", "Triggers" }
            p { class: "field-desc",
                "Run this collection when herdr emits an event (see the herdr Events tab). Requests read the event as "
                code { "{{{{herdr.pane_id}}}}" }
                " etc. Filters are "
                code { "path=value" }
                " pairs on the event data."
            }
            datalist { id: "herdr-events",
                for n in names.read().clone().unwrap_or_default() {
                    option { value: "{n}" }
                }
            }
            for (i, t) in triggers.iter().cloned().enumerate() {
                div { key: "{i}", class: "btn-row",
                    input {
                        class: "input",
                        r#type: "text",
                        list: "herdr-events",
                        placeholder: "pane.agent_status_changed",
                        value: "{t.on}",
                        oninput: {
                            let triggers = triggers.clone();
                            move |e: FormEvent| {
                                let mut next = triggers.clone();
                                next[i].on = e.value();
                                on_change.call(next);
                            }
                        },
                    }
                    input {
                        class: "input",
                        r#type: "text",
                        placeholder: "agent_status=done",
                        value: "{when_text(&t.when)}",
                        onchange: {
                            let triggers = triggers.clone();
                            move |e: FormEvent| {
                                let mut next = triggers.clone();
                                next[i].when = parse_when(&e.value());
                                on_change.call(next);
                            }
                        },
                    }
                    button {
                        class: "link-btn link-danger",
                        onclick: {
                            let triggers = triggers.clone();
                            move |_| {
                                let mut next = triggers.clone();
                                next.remove(i);
                                on_change.call(next);
                            }
                        },
                        "✕"
                    }
                }
            }
            button {
                class: "btn",
                onclick: {
                    let triggers = triggers.clone();
                    move |_| {
                        let mut next = triggers.clone();
                        next.push(RecipeTrigger {
                            on: "pane.agent_status_changed".into(),
                            when: serde_json::Map::new(),
                        });
                        on_change.call(next);
                    }
                },
                "+ Add Trigger"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_round_trips_through_text() {
        let when = parse_when("agent_status=done, pane.tab_id = w1:t1, focused=true");
        assert_eq!(when["agent_status"], "done");
        assert_eq!(when["pane.tab_id"], "w1:t1");
        assert_eq!(when["focused"], true);
        assert_eq!(parse_when(&when_text(&when)), when);
        assert!(parse_when("").is_empty());
    }
}

//! Builders for two request kinds: `http_request` (an endpoint builder:
//! method, URL, query, headers, body) and `llm_call` (a wizard: provider and
//! key, model, prompt). Both edit the same raw values [`super::forms::DynamicForm`]
//! does, so a request can switch between builder and plain fields.

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::HashMap;

use crate::client::use_api;
use herdr_mcp_api::playground::{
    BodyMode, CUSTOM_PROVIDER, HTTP_METHODS, KeyMode, PROVIDERS, json_body_error, kv_json, kv_rows,
    load_models, model_choices, pick_provider, provider_of, with_value as with,
};

// Hints shown in inputs (constants: rsx would read `{{ }}` in a literal as
// an escaped brace).
const URL_HINT: &str = "https://api.example.com/v1/items/{{id}}";
const VALUE_HINT: &str = "value or {{var}}";
const TOKEN_HINT: &str = "Bearer {{token}}";
const JSON_HINT: &str = r#"{"name": "{{name}}"}"#;
const PROMPT_HINT: &str = "Summarise this: {{read.output}}";

/// Name/value rows for a JSON-object param (`query`, `headers`).
#[component]
fn KvRows(
    title: &'static str,
    add: &'static str,
    key_hint: &'static str,
    value_hint: &'static str,
    initial: String,
    on_change: EventHandler<String>,
) -> Element {
    let mut rows = use_signal(|| kv_rows(&initial));
    let commit = move || on_change.call(kv_json(&rows.read()));
    rsx! {
        div { class: "form-block",
            div { class: "form-label", "{title}" }
            for (i, (k, v)) in rows.read().iter().cloned().enumerate() {
                div { key: "{i}", class: "kv-row",
                    input {
                        class: "input mono",
                        r#type: "text",
                        placeholder: key_hint,
                        value: "{k}",
                        oninput: move |e| {
                            rows.write()[i].0 = e.value();
                            commit();
                        },
                    }
                    input {
                        class: "input mono",
                        r#type: "text",
                        placeholder: value_hint,
                        value: "{v}",
                        oninput: move |e| {
                            rows.write()[i].1 = e.value();
                            commit();
                        },
                    }
                    button {
                        class: "link-btn link-danger",
                        onclick: move |_| {
                            rows.write().remove(i);
                            commit();
                        },
                        "✕"
                    }
                }
            }
            button {
                class: "btn",
                onclick: move |_| rows.write().push((String::new(), String::new())),
                "{add}"
            }
        }
    }
}

/// The `http_request` endpoint builder.
#[component]
pub fn HttpBuilder(
    values: HashMap<String, String>,
    on_change: EventHandler<HashMap<String, String>>,
) -> Element {
    let method = values
        .get("method")
        .cloned()
        .unwrap_or_else(|| "GET".into());
    let url = values.get("url").cloned().unwrap_or_default();
    let mode = BodyMode::of(&values);
    let body = mode
        .key()
        .and_then(|k| values.get(k))
        .cloned()
        .unwrap_or_default();
    let json_error = json_body_error(&values);
    let check = values.get("check").is_some_and(|c| c == "true");

    let v = values.clone();
    let set = move |key: &'static str, raw: String| on_change.call(with(&v, key, raw));
    let v = values.clone();
    let set_mode = move |next: BodyMode| on_change.call(next.apply(&v));

    rsx! {
        div { class: "form-block",
            div { class: "form-label", "Endpoint" }
            div { class: "endpoint-row",
                select {
                    class: "input mono",
                    onchange: {
                        let set = set.clone();
                        move |e: Event<FormData>| set("method", e.value())
                    },
                    for m in HTTP_METHODS {
                        option { value: m, selected: method.eq_ignore_ascii_case(m), "{m}" }
                    }
                }
                input {
                    class: "input mono",
                    r#type: "text",
                    placeholder: URL_HINT,
                    value: "{url}",
                    oninput: {
                        let set = set.clone();
                        move |e: Event<FormData>| set("url", e.value())
                    },
                }
            }
        }
        KvRows {
            title: "Query",
            add: "+ Query param",
            key_hint: "name",
            value_hint: VALUE_HINT,
            initial: values.get("query").cloned().unwrap_or_default(),
            on_change: {
                let set = set.clone();
                move |raw| set("query", raw)
            },
        }
        KvRows {
            title: "Headers",
            add: "+ Header",
            key_hint: "Authorization",
            value_hint: TOKEN_HINT,
            initial: values.get("headers").cloned().unwrap_or_default(),
            on_change: {
                let set = set.clone();
                move |raw| set("headers", raw)
            },
        }
        div { class: "form-block",
            div { class: "form-label", "Body" }
            div { class: "response-tabs body-modes",
                for m in BodyMode::ALL {
                    button {
                        key: "{m.label()}",
                        class: if mode == m { "mini-tab mini-tab-active" } else { "mini-tab" },
                        onclick: {
                            let set_mode = set_mode.clone();
                            move |_| set_mode(m)
                        },
                        "{m.label()}"
                    }
                }
            }
            if let Some(key) = mode.key() {
                textarea {
                    class: "input textarea",
                    rows: "6",
                    placeholder: if mode == BodyMode::Json { JSON_HINT } else { "text body" },
                    value: "{body}",
                    oninput: {
                        let set = set.clone();
                        move |e: Event<FormData>| set(key, e.value())
                    },
                }
                if let Some(e) = json_error {
                    p { class: "field-desc", style: "color:#f87171", "Not JSON yet: {e}" }
                }
            }
        }
        label { class: "check-row",
            input {
                r#type: "checkbox",
                checked: check,
                onchange: {
                    let set = set.clone();
                    move |_| set("check", if check { String::new() } else { "true".into() })
                },
            }
            span { class: "muted", "Fail on a non-2xx status (stops a collection run)" }
        }
    }
}

/// The `llm_call` wizard: provider and key, model, prompt.
#[component]
pub fn LlmWizard(
    values: HashMap<String, String>,
    on_change: EventHandler<HashMap<String, String>>,
) -> Element {
    let client = use_api();
    let mut step = use_signal(|| {
        if values.get("model").is_some_and(|m| !m.is_empty()) {
            3
        } else {
            1
        }
    });
    let mut models: Signal<Vec<Value>> = use_signal(Vec::new);
    let mut model_filter = use_signal(String::new);
    let mut loading = use_signal(|| false);
    let mut load_error: Signal<Option<String>> = use_signal(|| None);
    let mut custom = use_signal(|| false);

    let get = |k: &str| values.get(k).cloned().unwrap_or_default();
    let base_url = get("base_url");
    let provider_i = if custom() {
        CUSTOM_PROVIDER
    } else {
        provider_of(&base_url)
    };
    let provider = &PROVIDERS[provider_i];
    let api_key = get("api_key");
    let key_env = get("api_key_env");
    let key_mode = KeyMode::of(&values, provider);
    let model = get("model");
    let system = get("system");
    let prompt = get("prompt");
    let temperature = get("temperature");
    let max_tokens = get("max_tokens");

    let v = values.clone();
    let set = move |key: &'static str, raw: String| on_change.call(with(&v, key, raw));
    let v = values.clone();
    let choose_provider = move |i: usize| {
        custom.set(i == CUSTOM_PROVIDER);
        on_change.call(pick_provider(&v, i));
    };
    let v = values.clone();
    let set_key_mode = move |mode: KeyMode| on_change.call(mode.apply(&v, provider));
    let v = values.clone();
    let fetch_models = move |_| {
        let client = client();
        let v = v.clone();
        spawn(async move {
            loading.set(true);
            load_error.set(None);
            match load_models(&client, &v).await {
                Ok(list) => models.set(list),
                Err(e) => load_error.set(Some(e)),
            }
            loading.set(false);
        });
    };

    let shown = model_choices(&models.read(), &model_filter.read());

    rsx! {
        div { class: "response-tabs wizard-steps",
            for (n, label) in [(1, "1 · Provider"), (2, "2 · Model"), (3, "3 · Prompt")] {
                button {
                    key: "{n}",
                    class: if step() == n { "mini-tab mini-tab-active" } else { "mini-tab" },
                    onclick: move |_| step.set(n),
                    "{label}"
                }
            }
        }
        if step() == 1 {
            div { class: "form-block",
                div { class: "form-label", "Provider" }
                div { class: "provider-grid",
                    for (i, p) in PROVIDERS.iter().enumerate() {
                        button {
                            key: "{p.name}",
                            class: if i == provider_i { "provider provider-sel" } else { "provider" },
                            onclick: {
                                let mut choose_provider = choose_provider.clone();
                                move |_| choose_provider(i)
                            },
                            div { class: "provider-name", "{p.name}" }
                            div { class: "field-desc", "{p.hint}" }
                        }
                    }
                }
                if provider_i == CUSTOM_PROVIDER {
                    input {
                        class: "input mono",
                        r#type: "text",
                        placeholder: "https://my-server/v1",
                        value: "{base_url}",
                        oninput: {
                            let set = set.clone();
                            move |e: Event<FormData>| set("base_url", e.value())
                        },
                    }
                }
            }
            div { class: "form-block",
                div { class: "form-label", "API key" }
                for m in KeyMode::ALL {
                    label { key: "{m.label()}", class: "check-row",
                        input {
                            r#type: "radio",
                            name: "keymode",
                            checked: key_mode == m,
                            onchange: {
                                let set_key_mode = set_key_mode.clone();
                                move |_| set_key_mode(m)
                            },
                        }
                        span { "{m.label()}" }
                    }
                }
                if key_mode == KeyMode::Env {
                    input {
                        class: "input mono",
                        r#type: "text",
                        placeholder: provider.key_env.unwrap_or("LLM_API_KEY"),
                        value: "{key_env}",
                        oninput: {
                            let set = set.clone();
                            move |e: Event<FormData>| set("api_key_env", e.value())
                        },
                    }
                    p { class: "field-desc",
                        "Read from herdr-mcp's environment when the request runs; blank uses the placeholder."
                    }
                } else if key_mode == KeyMode::Key {
                    input {
                        class: "input mono",
                        r#type: "text",
                        value: "{api_key}",
                        oninput: {
                            let set = set.clone();
                            move |e: Event<FormData>| set("api_key", e.value())
                        },
                    }
                    p { class: "field-desc",
                        "Keep the key in a variable (Variables page, e.g. llm_api_key) and refer to it as {{{{llm_api_key}}}}: a pasted key is saved in the collection."
                    }
                }
            }
            button { class: "btn", onclick: move |_| step.set(2), "Next: model →" }
        }
        if step() == 2 {
            div { class: "form-block",
                div { class: "form-label", "Model" }
                input {
                    class: "input mono",
                    r#type: "text",
                    placeholder: if provider.name == "OpenRouter" { "openai/gpt-4o-mini" } else { "model id" },
                    value: "{model}",
                    oninput: {
                        let set = set.clone();
                        move |e: Event<FormData>| set("model", e.value())
                    },
                }
                div { class: "endpoint-row",
                    button {
                        class: "btn",
                        disabled: loading(),
                        onclick: fetch_models,
                        if loading() { "Loading…" } else { "Load models from {provider.name}" }
                    }
                    input {
                        class: "input",
                        r#type: "text",
                        placeholder: "filter",
                        value: "{model_filter}",
                        oninput: move |e| model_filter.set(e.value()),
                    }
                }
                if let Some(e) = load_error() {
                    p { class: "field-desc", style: "color:#f87171", "{e}" }
                }
                if !shown.is_empty() {
                    select {
                        class: "input mono",
                        size: "10",
                        onchange: {
                            let set = set.clone();
                            move |e: Event<FormData>| set("model", e.value())
                        },
                        for (id, ctx) in shown {
                            option { key: "{id}", value: "{id}", selected: id == model, "{id}   {ctx}" }
                        }
                    }
                }
            }
            button { class: "btn", onclick: move |_| step.set(3), "Next: prompt →" }
        }
        if step() == 3 {
            div { class: "form-block",
                div { class: "form-label", "System (optional)" }
                textarea {
                    class: "input textarea",
                    rows: "2",
                    value: "{system}",
                    oninput: {
                        let set = set.clone();
                        move |e: Event<FormData>| set("system", e.value())
                    },
                }
            }
            div { class: "form-block",
                div { class: "form-label", "Prompt" }
                textarea {
                    class: "input textarea",
                    rows: "6",
                    placeholder: PROMPT_HINT,
                    value: "{prompt}",
                    oninput: {
                        let set = set.clone();
                        move |e: Event<FormData>| set("prompt", e.value())
                    },
                }
            }
            div { class: "form-row",
                div { class: "form-col",
                    label { class: "form-label", "Temperature" }
                    input {
                        class: "input",
                        r#type: "number",
                        step: "0.1",
                        placeholder: "model default",
                        value: "{temperature}",
                        oninput: {
                            let set = set.clone();
                            move |e: Event<FormData>| set("temperature", e.value())
                        },
                    }
                }
                div { class: "form-col",
                    label { class: "form-label", "Max tokens" }
                    input {
                        class: "input",
                        r#type: "number",
                        placeholder: "model default",
                        value: "{max_tokens}",
                        oninput: {
                            let set = set.clone();
                            move |e: Event<FormData>| set("max_tokens", e.value())
                        },
                    }
                }
            }
            p { class: "field-desc",
                "{model} via {provider.name}. Send to try it; later requests read the reply as {{{{this-request.text}}}}."
            }
        }
    }
}

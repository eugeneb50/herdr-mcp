//! Builders for two request kinds: `http_request` (an endpoint builder:
//! method, URL, query, headers, body) and `llm_call` (a wizard: provider and
//! key, model, prompt). Both edit the same raw values [`super::forms::DynamicForm`]
//! does, so a request can switch between builder and plain fields.

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::HashMap;

use crate::client::use_api;
use herdr_mcp_api::{ResolveRequest, tool_is_error, tool_payload};

// Hints shown in inputs (constants: rsx would read `{{ }}` in a literal as
// an escaped brace).
const URL_HINT: &str = "https://api.example.com/v1/items/{{id}}";
const VALUE_HINT: &str = "value or {{var}}";
const TOKEN_HINT: &str = "Bearer {{token}}";
const JSON_HINT: &str = r#"{"name": "{{name}}"}"#;
const PROMPT_HINT: &str = "Summarise this: {{read.output}}";

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// `{"k": "v"}` as editable rows (non-string values as JSON).
fn rows_of(raw: &str) -> Vec<(String, String)> {
    serde_json::from_str::<serde_json::Map<String, Value>>(raw)
        .map(|m| {
            m.into_iter()
                .map(|(k, v)| {
                    let v = match v {
                        Value::String(s) => s,
                        other => other.to_string(),
                    };
                    (k, v)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Rows back to a JSON object string; rows without a name are dropped,
/// and no rows at all is "" (the param is left out).
fn rows_json(rows: &[(String, String)]) -> String {
    let map: serde_json::Map<String, Value> = rows
        .iter()
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), Value::String(v.clone())))
        .collect();
    if map.is_empty() {
        String::new()
    } else {
        Value::Object(map).to_string()
    }
}

/// `values` with `key` set to `raw` ("" removes it).
fn with(values: &HashMap<String, String>, key: &str, raw: String) -> HashMap<String, String> {
    let mut next = values.clone();
    if raw.is_empty() {
        next.remove(key);
    } else {
        next.insert(key.to_string(), raw);
    }
    next
}

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
    let mut rows = use_signal(|| rows_of(&initial));
    let commit = move || on_change.call(rows_json(&rows.read()));
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
    let mode = if values.contains_key("json") {
        "json"
    } else if values.contains_key("body") {
        "text"
    } else {
        "none"
    };
    let body = values
        .get(if mode == "json" { "json" } else { "body" })
        .cloned()
        .unwrap_or_default();
    let json_error = (mode == "json" && !body.trim().is_empty() && !body.contains("{{"))
        .then(|| serde_json::from_str::<Value>(&body).err())
        .flatten();
    let check = values.get("check").is_some_and(|c| c == "true");

    let v = values.clone();
    let set = move |key: &'static str, raw: String| on_change.call(with(&v, key, raw));
    let v = values.clone();
    let set_mode = move |next: String| {
        let current = v
            .get("json")
            .or_else(|| v.get("body"))
            .cloned()
            .unwrap_or_default();
        let mut out = v.clone();
        out.remove("json");
        out.remove("body");
        match next.as_str() {
            "json" => {
                out.insert(
                    "json".into(),
                    if current.is_empty() {
                        "{}".into()
                    } else {
                        current
                    },
                );
            }
            "text" => {
                out.insert("body".into(), current);
            }
            _ => {}
        }
        on_change.call(out);
    };

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
                    for m in METHODS {
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
                for (m, label) in [("none", "None"), ("json", "JSON"), ("text", "Text")] {
                    button {
                        key: "{m}",
                        class: if mode == m { "mini-tab mini-tab-active" } else { "mini-tab" },
                        onclick: {
                            let set_mode = set_mode.clone();
                            move |_| set_mode(m.to_string())
                        },
                        "{label}"
                    }
                }
            }
            if mode != "none" {
                textarea {
                    class: "input textarea",
                    rows: "6",
                    placeholder: if mode == "json" { JSON_HINT } else { "text body" },
                    value: "{body}",
                    oninput: {
                        let set = set.clone();
                        move |e: Event<FormData>| set(if mode == "json" { "json" } else { "body" }, e.value())
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

/// An OpenAI-compatible provider preset.
struct Provider {
    name: &'static str,
    base_url: &'static str,
    key_env: Option<&'static str>,
    hint: &'static str,
}

const PROVIDERS: [Provider; 5] = [
    Provider {
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        key_env: Some("OPENROUTER_API_KEY"),
        hint: "Hundreds of models behind one key; ids look like openai/gpt-4o-mini.",
    },
    Provider {
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        key_env: Some("OPENAI_API_KEY"),
        hint: "OpenAI's own API; ids look like gpt-4o-mini.",
    },
    Provider {
        name: "Ollama",
        base_url: "http://localhost:11434/v1",
        key_env: None,
        hint: "Local models; no key.",
    },
    Provider {
        name: "LM Studio",
        base_url: "http://localhost:1234/v1",
        key_env: None,
        hint: "Local models; no key.",
    },
    Provider {
        name: "Custom",
        base_url: "",
        key_env: None,
        hint: "Any OpenAI-compatible server: give its base URL (…/v1).",
    },
];

/// The preset `base_url` belongs to (OpenRouter when unset, as the tool
/// defaults to it), else Custom.
fn provider_of(base_url: &str) -> &'static Provider {
    let base = base_url.trim().trim_end_matches('/');
    let base = if base.is_empty() {
        PROVIDERS[0].base_url
    } else {
        base
    };
    PROVIDERS
        .iter()
        .find(|p| !p.base_url.is_empty() && p.base_url == base)
        .unwrap_or(&PROVIDERS[4])
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
    let provider = if custom() {
        &PROVIDERS[4]
    } else {
        provider_of(&base_url)
    };
    let api_key = get("api_key");
    let key_env = get("api_key_env");
    let key_mode = if !api_key.is_empty() {
        "key"
    } else if !key_env.is_empty() || provider.key_env.is_some() {
        "env"
    } else {
        "none"
    };
    let model = get("model");
    let system = get("system");
    let prompt = get("prompt");
    let temperature = get("temperature");
    let max_tokens = get("max_tokens");

    let v = values.clone();
    let set = move |key: &'static str, raw: String| on_change.call(with(&v, key, raw));
    let v = values.clone();
    let pick_provider = move |i: usize| {
        let p = &PROVIDERS[i];
        custom.set(i == 4);
        let mut next = with(&v, "base_url", p.base_url.to_string());
        next.remove("api_key_env");
        next.remove("api_key");
        on_change.call(next);
    };
    let v = values.clone();
    let set_key_mode = move |mode: &'static str| {
        let mut next = v.clone();
        next.remove("api_key");
        next.remove("api_key_env");
        if mode == "key" {
            next.insert("api_key".into(), "{{llm_api_key}}".into());
        } else if mode == "env" && provider.key_env.is_none() {
            next.insert("api_key_env".into(), "LLM_API_KEY".into());
        }
        on_change.call(next);
    };
    let v = values.clone();
    let load_models = move |_| {
        let mut args = serde_json::Map::new();
        for k in ["base_url", "api_key", "api_key_env"] {
            if let Some(val) = v.get(k).filter(|s| !s.is_empty()) {
                args.insert(k.into(), Value::String(val.clone()));
            }
        }
        let client = client();
        spawn(async move {
            loading.set(true);
            load_error.set(None);
            // A {{variable}} key resolves like it would in a run.
            let args = match client
                .resolve(&ResolveRequest {
                    value: Value::Object(args.clone()),
                    ..Default::default()
                })
                .await
            {
                Ok(r) => r.value,
                Err(_) => Value::Object(args),
            };
            match client.call_tool("llm_models", &args).await {
                Ok(r) if tool_is_error(&r) => {
                    let text = r
                        .pointer("/content/0/text")
                        .and_then(Value::as_str)
                        .unwrap_or("failed");
                    load_error.set(Some(text.to_string()));
                }
                Ok(r) => {
                    let list = tool_payload(&r)
                        .and_then(|p| p["models"].as_array().cloned())
                        .unwrap_or_default();
                    models.set(list);
                }
                Err(e) => load_error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    };

    let needle = model_filter.read().to_lowercase();
    let shown: Vec<(String, String)> = models
        .read()
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let ctx = m["context_length"]
                .as_u64()
                .map(|n| format!("{}k ctx", n / 1000))
                .unwrap_or_default();
            id.to_lowercase().contains(&needle).then_some((id, ctx))
        })
        .take(200)
        .collect();

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
                            class: if std::ptr::eq(p, provider) { "provider provider-sel" } else { "provider" },
                            onclick: {
                                let mut pick_provider = pick_provider.clone();
                                move |_| pick_provider(i)
                            },
                            div { class: "provider-name", "{p.name}" }
                            div { class: "field-desc", "{p.hint}" }
                        }
                    }
                }
                if provider.name == "Custom" {
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
                for (m, label) in [("env", "From an environment variable"), ("key", "From a variable (or pasted)"), ("none", "No key")] {
                    label { key: "{m}", class: "check-row",
                        input {
                            r#type: "radio",
                            name: "keymode",
                            checked: key_mode == m,
                            onchange: {
                                let set_key_mode = set_key_mode.clone();
                                move |_| set_key_mode(m)
                            },
                        }
                        span { "{label}" }
                    }
                }
                if key_mode == "env" {
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
                } else if key_mode == "key" {
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
                        onclick: load_models,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_round_trip_and_drop_blank_names() {
        let rows = rows_of(r#"{"a":"1","n":2}"#);
        assert_eq!(rows, [("a".into(), "1".into()), ("n".into(), "2".into())]);
        let mut rows = rows;
        rows.push((" ".into(), "x".into()));
        assert_eq!(rows_json(&rows), r#"{"a":"1","n":"2"}"#);
        assert_eq!(rows_json(&[]), "");
    }

    #[test]
    fn providers_are_found_by_base_url() {
        assert_eq!(provider_of("").name, "OpenRouter");
        assert_eq!(provider_of("https://api.openai.com/v1/").name, "OpenAI");
        assert_eq!(provider_of("http://box:8000/v1").name, "Custom");
    }
}

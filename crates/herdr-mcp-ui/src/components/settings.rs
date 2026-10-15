//! Settings page: runtime config rows, env overrides, clipboard round-trip
//! test. Port of `Settings` (+ TUI clipboard panel behavior via bridge tools).

use dioxus::prelude::*;
use serde_json::Value;

use crate::client::use_api;

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

/// Extract clipboard text from a `clipboard_get` tool result
/// (`content[].text` concatenated). Port of `parse_clipboard_text`.
pub fn parse_clipboard_text(v: &Value) -> String {
    let mut out = String::new();
    if let Some(items) = v.get("content").and_then(|c| c.as_array()) {
        for item in items {
            if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
    }
    out
}

#[component]
pub fn Settings() -> Element {
    let client = use_api();
    let config = use_resource(move || {
        let client = client();
        async move { client.config().await }
    });
    let mut clip_text: Signal<String> = use_signal(String::new);
    let mut clip_result: Signal<Option<String>> = use_signal(|| None);

    let test_clipboard = move |_: ()| {
        let text = clip_text.read().clone();
        let client = client();
        spawn(async move {
            let set_res = client
                .call_tool("clipboard_set", &serde_json::json!({ "text": text }))
                .await;
            match set_res {
                Err(e) => clip_result.set(Some(format!("set failed: {e}"))),
                Ok(_) => match client
                    .call_tool("clipboard_get", &serde_json::json!({}))
                    .await
                {
                    Err(e) => clip_result.set(Some(format!("get failed: {e}"))),
                    Ok(v) => {
                        let back = parse_clipboard_text(&v);
                        if back == text {
                            clip_result.set(Some("round-trip ok".to_string()));
                        } else {
                            clip_result.set(Some(format!("mismatch: got {back:?}")));
                        }
                    }
                },
            }
        });
    };

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Settings" }
            h1 { class: "page-title", "Settings" }
        }
        section { class: "card",
            h2 { class: "card-title", "Runtime" }
            match &*config.read_unchecked() {
                None => rsx! { p { class: "muted", "Loading…" } },
                Some(Err(e)) => rsx! { p { class: "muted", "Server unavailable: {e}" } },
                Some(Ok(v)) => rsx! {
                    div { class: "rows",
                        SettingRow { label: "HTTP Port", value: str_field(v, "http_port").unwrap_or("...".into()) }
                        SettingRow { label: "Data Directory", value: str_field(v, "data_dir").unwrap_or("...".into()) }
                        SettingRow { label: "herdr Socket", value: str_field(v, "herdr_socket").unwrap_or("(not set)".into()) }
                        SettingRow { label: "herdr Binary", value: str_field(v, "herdr_bin").unwrap_or("...".into()) }
                    }
                    h2 { class: "card-title", "Environment Overrides" }
                    div { class: "rows",
                        EnvRow { name: "HERDR_SOCKET_PATH", value: str_field(v, "herdr_socket") }
                        EnvRow { name: "HERDR_MCP_DATA_DIR", value: str_field(v, "data_dir") }
                        EnvRow { name: "HERDR_MCP_HTTP_PORT", value: str_field(v, "http_port") }
                        EnvRow { name: "HERDR_BIN", value: str_field(v, "herdr_bin") }
                    }
                },
            }
        }
        section { class: "card",
            h2 { class: "card-title", "Clipboard backend test" }
            p { class: "muted",
                "Writes through the bridge clipboard_set tool and reads back via clipboard_get."
            }
            div { class: "form-row",
                input {
                    class: "input",
                    r#type: "text",
                    placeholder: "probe text",
                    value: "{clip_text}",
                    oninput: move |e| clip_text.set(e.value()),
                }
                button { class: "btn btn-primary", onclick: move |_| test_clipboard(()), "Test" }
            }
            if let Some(r) = clip_result.read().as_deref() {
                p { class: "mono", "{r}" }
            }
        }
    }
}

#[component]
fn SettingRow(label: &'static str, value: String) -> Element {
    rsx! {
        div { class: "row",
            span { class: "row-label", "{label}" }
            span { class: "mono", "{value}" }
        }
    }
}

#[component]
fn EnvRow(name: &'static str, value: Option<String>) -> Element {
    rsx! {
        div { class: "row",
            span { class: "mono row-label", "{name}" }
            match value {
                Some(v) => rsx! { span { class: "mono accent", "[set] {v}" } },
                None => rsx! { span { class: "mono muted", "[not set]" } },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clipboard_text_from_content_array() {
        let v = serde_json::json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "text", "text": "world" }
            ]
        });
        assert_eq!(parse_clipboard_text(&v), "hello\nworld");
    }

    #[test]
    fn parse_clipboard_text_empty_when_no_text() {
        let v = serde_json::json!({ "content": [{ "type": "image" }] });
        assert_eq!(parse_clipboard_text(&v), "");
    }
}

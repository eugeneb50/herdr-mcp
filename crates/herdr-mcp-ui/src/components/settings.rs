//! Settings page: runtime config rows, clipboard backend commands (saved to
//! the config file and applied live) and a clipboard round-trip test. Same
//! bridge calls as the TUI Settings tab.

use dioxus::prelude::*;
use herdr_mcp_api::ClipboardConfig;

use crate::client::use_api;

#[component]
pub fn Settings() -> Element {
    let client = use_api();
    let mut config = use_resource(move || {
        let client = client();
        async move { client.config().await }
    });
    // Unsaved edits; `None` shows the server's current command.
    let mut copy_edit: Signal<Option<String>> = use_signal(|| None);
    let mut paste_edit: Signal<Option<String>> = use_signal(|| None);
    let mut save_result: Signal<Option<String>> = use_signal(|| None);
    let mut clip_text: Signal<String> = use_signal(String::new);
    let mut clip_result: Signal<Option<String>> = use_signal(|| None);

    let current = move || -> ClipboardConfig {
        match &*config.read() {
            Some(Ok(c)) => c.clipboard.clone(),
            _ => ClipboardConfig::default(),
        }
    };

    let save_clipboard = move |_: ()| {
        let live = current();
        let cfg = ClipboardConfig {
            copy_command: copy_edit.read().clone().or(live.copy_command),
            paste_command: paste_edit.read().clone().or(live.paste_command),
        };
        let client = client();
        spawn(async move {
            match client.set_clipboard_config(&cfg).await {
                Ok(c) => {
                    save_result.set(Some(format!("saved to {} (active now)", c.config_path)));
                    copy_edit.set(None);
                    paste_edit.set(None);
                    config.restart();
                }
                Err(e) => save_result.set(Some(format!("save failed: {e}"))),
            }
        });
    };

    let test_clipboard = move |_: ()| {
        let text = clip_text.read().clone();
        let client = client();
        spawn(async move {
            let res = match client.clipboard_set(&text).await {
                Err(e) => format!("set failed: {e}"),
                Ok(()) => match client.clipboard_get().await {
                    Err(e) => format!("get failed: {e}"),
                    Ok(back) if back == text => "round-trip ok".to_string(),
                    Ok(back) => format!("mismatch: got {back:?}"),
                },
            };
            clip_result.set(Some(res));
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
                Some(Ok(c)) => rsx! {
                    div { class: "rows",
                        SettingRow { label: "Data Directory", value: c.data_dir.clone() }
                        SettingRow { label: "herdr Socket", value: c.herdr_socket.clone() }
                        SettingRow { label: "herdr Binary", value: c.herdr_bin.clone() }
                        SettingRow { label: "Config File", value: c.config_path.clone() }
                    }
                },
            }
        }
        section { class: "card",
            h2 { class: "card-title", "Clipboard backend" }
            p { class: "muted",
                "Commands used by clipboard_set / clipboard_get. Empty on the server means platform auto-detect. Save writes the [clipboard] table of the config file and applies immediately (HERDR_MCP_CLIPBOARD_* env vars still win)."
            }
            div { class: "form-row",
                input {
                    class: "input",
                    r#type: "text",
                    placeholder: "copy command (e.g. wl-copy)",
                    value: copy_edit.read().clone().or(current().copy_command).unwrap_or_default(),
                    oninput: move |e| copy_edit.set(Some(e.value())),
                }
                input {
                    class: "input",
                    r#type: "text",
                    placeholder: "paste command (e.g. wl-paste)",
                    value: paste_edit.read().clone().or(current().paste_command).unwrap_or_default(),
                    oninput: move |e| paste_edit.set(Some(e.value())),
                }
                button { class: "btn btn-primary", onclick: move |_| save_clipboard(()), "Save" }
            }
            if let Some(r) = save_result.read().as_deref() {
                p { class: "mono", "{r}" }
            }
            h2 { class: "card-title", "Round-trip test" }
            p { class: "muted",
                "Writes through clipboard_set and reads back via clipboard_get, using the active commands."
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

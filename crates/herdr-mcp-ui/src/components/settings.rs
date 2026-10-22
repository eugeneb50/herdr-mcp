//! Settings page: the options registry (keybindings, timeouts, limits;
//! `/api/settings`), runtime config rows, clipboard backend commands (saved
//! to the config file and applied live) and a clipboard round-trip test.
//! Same bridge calls as the TUI Settings tab.

use std::collections::HashMap;

use dioxus::prelude::*;
use herdr_mcp_api::settings::{Kind, Value};
use herdr_mcp_api::{ApiError, ClipboardConfig, SettingInfo, SettingsResponse};

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
        Options {}
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
                "clipboard_set / clipboard_get use the built-in OS clipboard unless both commands are set (e.g. xsel). Save writes the [clipboard] table of the config file and applies immediately; save both empty to go back to the built-in clipboard."
            }
            div { class: "form-row",
                input {
                    class: "input",
                    r#type: "text",
                    placeholder: "copy command (empty: built-in)",
                    value: copy_edit.read().clone().or(current().copy_command).unwrap_or_default(),
                    oninput: move |e| copy_edit.set(Some(e.value())),
                }
                input {
                    class: "input",
                    r#type: "text",
                    placeholder: "paste command (empty: built-in)",
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

/// The settings registry: every knob with its default and bounds; Save
/// writes the config file's `[settings.*]` and applies it, Reset goes back
/// to the default. The bridge refuses bad values and says why.
#[component]
fn Options() -> Element {
    let client = use_api();
    let mut settings = use_resource(move || {
        let client = client();
        async move { client.settings().await }
    });
    // Unsaved edits by key; a key's row shows the server's value otherwise.
    let mut edits: Signal<HashMap<String, String>> = use_signal(HashMap::new);
    // Last outcome by key ("" = the reset-all button).
    let mut notes: Signal<HashMap<String, Result<String, String>>> = use_signal(HashMap::new);

    let mut settle = move |key: String, reply: Result<SettingsResponse, ApiError>, done: String| {
        let note = match reply {
            Ok(r) => {
                edits.write().remove(&key);
                settings.set(Some(Ok(r)));
                Ok(done)
            }
            Err(ApiError::Status { body, .. }) => Err(body),
            Err(e) => Err(e.to_string()),
        };
        notes.write().insert(key, note);
    };
    let save = move |info: SettingInfo| {
        let text = edits
            .read()
            .get(&info.key)
            .cloned()
            .unwrap_or_else(|| info.value.to_string());
        let value = match Value::parse(info.kind, &text) {
            Ok(v) => v,
            Err(e) => {
                notes.write().insert(info.key.clone(), Err(e));
                return;
            }
        };
        let client = client();
        spawn(async move {
            let reply = client.set_setting(&info.key, &value).await;
            settle(info.key.clone(), reply, "saved, active now".into());
        });
    };
    let reset = move |key: String| {
        let client = client();
        spawn(async move {
            let reply = client.reset_setting(&key).await;
            settle(key.clone(), reply, "back on its default".into());
        });
    };
    let reset_all = move |_| {
        let client = client();
        spawn(async move {
            let reply = client.reset_settings().await;
            notes.write().clear();
            edits.write().clear();
            settle(
                String::new(),
                reply,
                "every setting is back on its default".into(),
            );
        });
    };

    rsx! {
        section { class: "card",
            div { class: "card-head",
                h2 { class: "card-title", "Options" }
                button { class: "link-btn link-danger", onclick: reset_all, "Reset all to defaults" }
            }
            p { class: "muted",
                "Keybindings (TUI), timeouts and limits. Save writes the [settings.*] tables of the config file and applies at once; Reset puts a setting back on its built-in default, the known-good configuration. A bad value in the file falls back to the default too."
            }
            if let Some(note) = notes.read().get("") {
                Note { note: note.clone() }
            }
            match &*settings.read_unchecked() {
                None => rsx! { p { class: "muted", "Loading…" } },
                Some(Err(e)) => rsx! { p { class: "muted", "Server unavailable: {e}" } },
                Some(Ok(r)) => rsx! {
                    for w in r.warnings.iter() {
                        div { class: "alert alert-error", "{w}" }
                    }
                    table { class: "table",
                        thead {
                            tr {
                                th { "Setting" }
                                th { "Value" }
                                th { "Default" }
                                th {}
                            }
                        }
                        tbody {
                            for (i, info) in r.settings.iter().enumerate() {
                                OptionRow {
                                    key: "{info.key}",
                                    first_in_section: i == 0 || r.settings[i - 1].section != info.section,
                                    info: info.clone(),
                                    text: edits.read().get(&info.key).cloned().unwrap_or_else(|| info.value.to_string()),
                                    note: notes.read().get(&info.key).cloned(),
                                    on_edit: move |(key, text): (String, String)| {
                                        edits.write().insert(key, text);
                                    },
                                    on_save: save,
                                    on_reset: reset,
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

#[component]
fn OptionRow(
    first_in_section: bool,
    info: SettingInfo,
    text: String,
    note: Option<Result<String, String>>,
    on_edit: EventHandler<(String, String)>,
    on_save: EventHandler<SettingInfo>,
    on_reset: EventHandler<String>,
) -> Element {
    let unit = if info.kind == Kind::Millis { " ms" } else { "" };
    let bounds = match info.kind {
        Kind::Keys => "comma-separated chords, e.g. alt+right, ctrl+tab".to_string(),
        _ => format!("{}–{}{unit}", info.min, info.max),
    };
    let restart = if info.restart {
        " · applies on restart"
    } else {
        ""
    };
    let key = info.key.clone();
    let reset_key = info.key.clone();
    let saved = info.clone();
    let clicked = info.clone();
    rsx! {
        if first_in_section {
            tr {
                td { class: "accent mono", colspan: "4", "[{info.section}]" }
            }
        }
        tr {
            td {
                div {
                    if info.overridden {
                        span { class: "accent", "● " }
                    }
                    "{info.label}"
                }
                p { class: "field-desc", "{info.help}" }
                p { class: "field-desc mono", "{info.key} · {bounds}{restart}" }
            }
            td {
                input {
                    class: "input mono",
                    r#type: "text",
                    value: "{text}",
                    oninput: move |e| on_edit.call((key.clone(), e.value())),
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter {
                            on_save.call(saved.clone());
                        }
                    },
                }
                if let Some(note) = note {
                    Note { note }
                }
            }
            td { class: "mono muted", "{info.default}{unit}" }
            td {
                div { class: "btn-row",
                    button { class: "btn btn-primary", onclick: move |_| on_save.call(clicked.clone()), "Save" }
                    button {
                        class: "link-btn",
                        disabled: !info.overridden,
                        onclick: move |_| on_reset.call(reset_key.clone()),
                        "Reset"
                    }
                }
            }
        }
    }
}

#[component]
fn Note(note: Result<String, String>) -> Element {
    match note {
        Ok(msg) => rsx! { p { class: "field-resolved", "✓ {msg}" } },
        Err(msg) => rsx! { p { class: "field-desc pill-error", "✗ {msg}" } },
    }
}

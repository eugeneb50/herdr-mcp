//! Settings tab: the Options registry (keybindings, timeouts, limits;
//! `GET/PUT/DELETE /api/settings`), herdr-mcp runtime settings + herdr
//! sidecar context profiler, and the clipboard backend configuration.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use herdr_mcp_api::settings;

use crate::App;
use crate::theme::{accent_style, dim_style, focused_panel_block, panel_block, selected_style};
use crate::truncate;

pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    // Editing a clipboard field — feed all keys to textarea.
    if app.settings.clipboard.editing {
        return handle_clipboard_edit(app, code, mods).await;
    }
    if app.settings.options.editing {
        return handle_option_edit(app, code, mods).await;
    }

    // Tab/BackTab cycle frames: options (2) → runtime (0) → clipboard (1).
    const ORDER: [usize; 3] = [2, 0, 1];
    let at = ORDER
        .iter()
        .position(|f| *f == app.settings.focused_frame)
        .unwrap_or(0);
    match code {
        KeyCode::Tab => {
            app.settings.focused_frame = ORDER[(at + 1) % ORDER.len()];
            return Ok(true);
        }
        KeyCode::BackTab => {
            app.settings.focused_frame = ORDER[(at + ORDER.len() - 1) % ORDER.len()];
            return Ok(true);
        }
        _ => {}
    }

    match app.settings.focused_frame {
        0 => handle_frame_runtime(app, code).await,
        1 => handle_frame_clipboard(app, code, mods).await,
        2 => handle_frame_options(app, code, mods).await,
        _ => Ok(false),
    }
}

/// Frame 2: Options — ↑/↓ pick, Enter edit, Delete reset to default,
/// Shift+Delete reset everything.
async fn handle_frame_options(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    let count = settings::SPECS.len();
    let opts = &mut app.settings.options;
    match code {
        KeyCode::Up => opts.selected = opts.selected.saturating_sub(1),
        KeyCode::Down => opts.selected = (opts.selected + 1).min(count.saturating_sub(1)),
        KeyCode::Home => opts.selected = 0,
        KeyCode::End => opts.selected = count.saturating_sub(1),
        KeyCode::Enter => {
            let Some(spec) = settings::SPECS.get(opts.selected) else {
                return Ok(true);
            };
            let current = app
                .knobs
                .value(spec.key)
                .map(|v| v.to_string())
                .unwrap_or_default();
            let opts = &mut app.settings.options;
            opts.edit_area = tui_textarea::TextArea::new(vec![current]);
            opts.edit_area.set_block(crate::theme::panel_block(
                " Edit: Enter saves, Esc cancels ",
            ));
            opts.edit_area.move_cursor(tui_textarea::CursorMove::End);
            opts.editing = true;
            opts.msg.clear();
        }
        KeyCode::Delete if mods.contains(KeyModifiers::SHIFT) => {
            let reply = match &app.http {
                Some(http) => Some(http.reset_settings().await),
                None => None,
            };
            settle(app, reply, "every setting is back on its default");
        }
        KeyCode::Delete => {
            let Some(spec) = settings::SPECS.get(opts.selected) else {
                return Ok(true);
            };
            let reply = match &app.http {
                Some(http) => Some(http.reset_setting(spec.key).await),
                None => None,
            };
            settle(
                app,
                reply,
                &format!("{} is back on its default", spec.label),
            );
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Keys while editing an option's value.
async fn handle_option_edit(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    match code {
        KeyCode::Esc => app.settings.options.editing = false,
        KeyCode::Enter => {
            let Some(spec) = settings::SPECS.get(app.settings.options.selected) else {
                app.settings.options.editing = false;
                return Ok(true);
            };
            let text = app.settings.options.edit_area.lines().join(" ");
            let value = match settings::Value::parse(spec.kind, &text) {
                Ok(v) => v,
                Err(e) => {
                    app.settings.options.msg = format!("✗ {e}");
                    return Ok(true);
                }
            };
            let reply = match &app.http {
                Some(http) => Some(http.set_setting(spec.key, &value).await),
                None => None,
            };
            if settle(app, reply, &format!("{} saved (active now)", spec.label)) {
                app.settings.options.editing = false;
            }
        }
        KeyCode::Char(c)
            if mods.contains(KeyModifiers::CONTROL) && matches!(c, 'c' | 'v' | 'x') =>
        {
            return Ok(false);
        }
        _ => {
            if let Some(input) = crate::to_textarea_input(code, mods) {
                app.settings.options.edit_area.input(input);
            }
        }
    }
    Ok(true)
}

/// Apply a settings write's answer: `true` if it was taken. The bridge's
/// refusal (out of range, chord taken…) becomes the frame's message.
fn settle(
    app: &mut App,
    reply: Option<Result<herdr_mcp_api::SettingsResponse, herdr_mcp_api::ApiError>>,
    done: &str,
) -> bool {
    let msg = match reply {
        None => "✗ HTTP bridge not connected".to_string(),
        Some(Ok(response)) => {
            app.apply_settings(response);
            app.settings.options.msg = format!("✓ {done}");
            return true;
        }
        Some(Err(herdr_mcp_api::ApiError::Status { body, .. })) => format!("✗ {body}"),
        Some(Err(e)) => format!("✗ {e}"),
    };
    app.settings.options.msg = msg;
    false
}

/// Frame 0: Runtime settings (env vars) — Up/Down navigation.
async fn handle_frame_runtime(app: &mut App, code: KeyCode) -> Result<bool> {
    match code {
        KeyCode::Up => {
            app.settings.selected = app.settings.selected.saturating_sub(1);
            Ok(true)
        }
        KeyCode::Down => {
            if app.settings.selected + 1 < 10 {
                app.settings.selected += 1;
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Frame 1: Clipboard settings — Up/Down for field focus, Left/Right for preset, actions.
async fn handle_frame_clipboard(app: &mut App, code: KeyCode, _mods: KeyModifiers) -> Result<bool> {
    match code {
        // Up/Down cycle clipboard field focus: 0=preset, 1=copy, 2=paste
        KeyCode::Up => {
            if app.settings.clipboard.field_focus > 0 {
                app.settings.clipboard.field_focus -= 1;
            } else {
                app.settings.clipboard.field_focus = 2;
            }
            Ok(true)
        }
        KeyCode::Down => {
            if app.settings.clipboard.field_focus < 2 {
                app.settings.clipboard.field_focus += 1;
            } else {
                app.settings.clipboard.field_focus = 0;
            }
            Ok(true)
        }
        // Left/Right in preset list cycles presets
        KeyCode::Left | KeyCode::Right => {
            if app.settings.clipboard.field_focus == 0 {
                let presets = crate::BackendPreset::ALL.len();
                match code {
                    KeyCode::Right => {
                        app.settings.clipboard.preset_index =
                            (app.settings.clipboard.preset_index + 1) % presets;
                    }
                    KeyCode::Left => {
                        app.settings.clipboard.preset_index =
                            if app.settings.clipboard.preset_index > 0 {
                                app.settings.clipboard.preset_index - 1
                            } else {
                                presets - 1
                            };
                    }
                    _ => {}
                }
                // When preset changes, update command fields unless Custom
                let preset = crate::BackendPreset::ALL[app.settings.clipboard.preset_index];
                if !matches!(preset, crate::BackendPreset::Custom) {
                    let (copy, paste) = preset.commands();
                    app.settings.clipboard.copy_cmd = copy.to_string();
                    app.settings.clipboard.paste_cmd = paste.to_string();
                }
            }
            Ok(true)
        }
        // Enter on copy/paste field starts inline editing
        KeyCode::Enter => {
            if app.settings.clipboard.field_focus > 0 {
                let field = app.settings.clipboard.field_focus - 1; // 0=copy, 1=paste
                app.settings.clipboard.editing = true;
                app.settings.clipboard.edit_field = field;
                let text = if field == 0 {
                    &app.settings.clipboard.copy_cmd
                } else {
                    &app.settings.clipboard.paste_cmd
                };
                app.settings.clipboard.edit_area = tui_textarea::TextArea::new(vec![text.clone()]);
                app.settings
                    .clipboard
                    .edit_area
                    .set_block(crate::theme::panel_block(" Edit (Esc to finish) "));
                Ok(true)
            } else {
                Ok(false)
            }
        }
        // Test clipboard round-trip
        KeyCode::Char('t') => {
            run_clipboard_test(app).await;
            Ok(true)
        }
        // Save clipboard config to file
        KeyCode::Char('s') => {
            save_clipboard_config(app).await;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Handle keys while editing a clipboard field (inline textarea).
async fn handle_clipboard_edit(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    match code {
        KeyCode::Esc => {
            app.settings.clipboard.editing = false;
            Ok(true)
        }
        KeyCode::Char(c)
            if mods.contains(KeyModifiers::CONTROL) && matches!(c, 'c' | 'v' | 'x') =>
        {
            Ok(false)
        }
        _ => {
            if let Some(input) = crate::to_textarea_input(code, mods) {
                app.settings.clipboard.edit_area.input(input);
                let new_text = app.settings.clipboard.edit_area.lines().join("\n");
                if app.settings.clipboard.edit_field == 0 {
                    app.settings.clipboard.copy_cmd = new_text;
                } else {
                    app.settings.clipboard.paste_cmd = new_text;
                }
            }
            Ok(true)
        }
    }
}

/// Round-trips a nonce through the server's clipboard tools, so it tests the
/// commands the server is using now: unsaved edits need a Save (`s`) first.
async fn run_clipboard_test(app: &mut App) {
    // Generate a nonce for this test run
    let nonce = format!("herdr-mcp-clip-test-{}", uuid::Uuid::new_v4().simple());
    let http = match &app.http {
        Some(h) => h.clone(),
        None => {
            app.settings.clipboard.test_msg =
                "HTTP bridge not connected — see herdr-mcp.log in the data dir".into();
            return;
        }
    };
    app.settings.clipboard.test_msg = "testing...".into();
    // Copy
    match http.clipboard_set(&nonce).await {
        Ok(_) => {}
        Err(e) => {
            app.settings.clipboard.test_msg = format!("clipboard_set failed: {e}");
            return;
        }
    }
    // Paste
    match http.clipboard_get().await {
        Ok(got) => {
            if got == nonce {
                let clip = &app.settings.clipboard;
                let edited = herdr_mcp_api::ClipboardConfig {
                    copy_command: Some(clip.copy_cmd.clone()),
                    paste_command: Some(clip.paste_cmd.clone()),
                };
                let unsaved = app
                    .config
                    .as_ref()
                    .is_some_and(|c| c.clipboard.commands() != edited.commands());
                let note = if unsaved {
                    " (active commands; press s to apply edits)"
                } else {
                    ""
                };
                app.settings.clipboard.test_msg =
                    format!("✓ OK — round-trip {} chars{note}", nonce.len());
            } else {
                app.settings.clipboard.test_msg =
                    format!("✗ mismatch: wrote {}, got {}", nonce, got);
            }
        }
        Err(e) => {
            app.settings.clipboard.test_msg = format!("clipboard_get failed: {e}");
        }
    }
}

async fn save_clipboard_config(app: &mut App) {
    let Some(http) = app.http.clone() else {
        app.settings.clipboard.save_msg = "HTTP bridge not connected".into();
        return;
    };
    let cfg = herdr_mcp_api::ClipboardConfig {
        copy_command: Some(app.settings.clipboard.copy_cmd.clone()),
        paste_command: Some(app.settings.clipboard.paste_cmd.clone()),
    };
    match http.set_clipboard_config(&cfg).await {
        Ok(config) => {
            app.settings.clipboard.save_msg =
                format!("Saved -> {} (active now)", config.config_path);
            app.set_config(config);
        }
        Err(e) => {
            app.settings.clipboard.save_msg = format!("Save failed: {e}");
        }
    }
}

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let header = Paragraph::new(Line::from(vec![
        Span::styled("Settings", accent_style().add_modifier(Modifier::BOLD)),
        Span::styled(
            "  options · runtime · clipboard   (Tab next panel, ↑/↓ nav)",
            dim_style(),
        ),
    ]));
    frame.render_widget(header, Rect::new(area.x, area.y, area.width, 1));

    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );

    // Left: the options registry. Right: runtime, env/sidecar, clipboard.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(body);
    render_options(frame, cols[0], app);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Length(15),
            Constraint::Min(8),
        ])
        .split(cols[1]);
    render_runtime(frame, right[0], app);
    render_env_and_sidecar(frame, right[1], app);
    render_clipboard(frame, right[2], app);
}

/// The settings registry: one row per spec, grouped by section; the
/// highlighted row's help, bounds and default underneath.
fn render_options(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let focused = app.settings.focused_frame == 2;
    let title = " Options: Enter edit · Del default · Shift+Del all defaults ";
    let block = if focused {
        focused_panel_block(title)
    } else {
        panel_block(title)
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let opts = &app.settings.options;
    let label_w = settings::SPECS
        .iter()
        .map(|s| s.label.len())
        .max()
        .unwrap_or(0);
    let mut lines: Vec<Line> = Vec::new();
    let mut section = "";
    for (i, spec) in settings::SPECS.iter().enumerate() {
        if spec.section() != section {
            section = spec.section();
            lines.push(Line::from(Span::styled(
                format!("[{section}]"),
                accent_style(),
            )));
        }
        let value = app
            .knobs
            .value(spec.key)
            .map(|v| v.to_string())
            .unwrap_or_default();
        let unit = if spec.kind == settings::Kind::Millis {
            " ms"
        } else {
            ""
        };
        let mark = if app.knobs.is_set(spec.key) { "*" } else { " " };
        let style = if focused && i == opts.selected {
            selected_style()
        } else {
            Style::default()
        };
        lines.push(
            Line::from(vec![
                Span::styled(format!(" {mark}{:>label_w$}  ", spec.label), dim_style()),
                Span::styled(format!("{value}{unit}"), style),
            ])
            .style(style),
        );
    }

    let help_h = 5u16;
    let list_h = inner.height.saturating_sub(help_h);
    // Keep the highlighted row on screen: its line index counts headers.
    let row_line = settings::SPECS
        .iter()
        .take(opts.selected + 1)
        .enumerate()
        .filter(|(i, s)| *i == 0 || settings::SPECS[i - 1].section() != s.section())
        .count()
        + opts.selected;
    let scroll = row_line.saturating_sub(usize::from(list_h.saturating_sub(1)));
    frame.render_widget(
        Paragraph::new(lines).scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        Rect::new(inner.x, inner.y, inner.width, list_h),
    );

    let help_area = Rect::new(
        inner.x,
        inner.y + list_h,
        inner.width,
        help_h.min(inner.height),
    );
    if opts.editing {
        frame.render_widget(
            &opts.edit_area,
            Rect::new(help_area.x, help_area.y, help_area.width, 3),
        );
        frame.render_widget(
            Paragraph::new(Span::styled(opts.msg.clone(), warn_or_ok(&opts.msg))),
            Rect::new(help_area.x, help_area.y + 3, help_area.width, 2),
        );
        return;
    }
    let Some(spec) = settings::SPECS.get(opts.selected) else {
        return;
    };
    let bounds = match spec.kind {
        settings::Kind::Keys => "comma-separated chords, e.g. alt+right, ctrl+tab".to_string(),
        _ => format!("{}..={}", spec.min, spec.max),
    };
    let default = settings::Settings::default()
        .value(spec.key)
        .map(|v| v.to_string())
        .unwrap_or_default();
    let restart = if spec.restart {
        "  (applies on restart)"
    } else {
        ""
    };
    let help = vec![
        Line::from(Span::styled(
            format!("{}{restart}", spec.key),
            accent_style(),
        )),
        Line::from(Span::styled(spec.help, dim_style())),
        Line::from(Span::styled(
            format!("default {default} · {bounds}"),
            dim_style(),
        )),
        Line::from(Span::styled(opts.msg.clone(), warn_or_ok(&opts.msg))),
    ];
    frame.render_widget(Paragraph::new(help).wrap(Wrap { trim: true }), help_area);
}

fn warn_or_ok(msg: &str) -> Style {
    if msg.starts_with('✗') {
        crate::theme::warn_style()
    } else {
        accent_style()
    }
}

fn render_clipboard(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let block = if app.settings.focused_frame == 1 {
        focused_panel_block(" Clipboard backend ")
    } else {
        panel_block(" Clipboard backend ")
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Clear hit-areas — rebuilt every frame.
    app.clipboard_preset_rects.clear();
    app.clipboard_btn_rects.clear();
    app.clipboard_field_rects.clear();

    let clip = &app.settings.clipboard;
    let mut lines: Vec<Line> = Vec::new();

    // Bridge status pill
    let bridge_style = if app.bridge_connected {
        accent_style()
    } else {
        dim_style()
    };
    let bridge_text = if app.bridge_connected {
        "Bridge: connected"
    } else {
        "Bridge: not connected — see herdr-mcp.log in the data dir"
    };
    lines.push(Line::from(Span::styled(bridge_text, bridge_style)));

    // Config file path
    lines.push(Line::from(Span::styled(
        format!(
            "Config file: {}",
            app.config
                .as_ref()
                .map_or("(bridge not connected)", |c| c.config_path.as_str())
        ),
        dim_style(),
    )));

    lines.push(Line::from(""));

    // Backend presets (radio-like)
    lines.push(Line::from(Span::styled("Backend:", accent_style())));
    let preset_start_y = inner.y + lines.len() as u16;
    for (i, preset) in crate::BackendPreset::ALL.iter().enumerate() {
        let selected = clip.preset_index == i && clip.field_focus == 0;
        let marker = if selected { "● " } else { "○ " };
        let label = format!("{} {}", marker, preset.label());
        let style = if selected {
            selected_style()
        } else {
            Style::default()
        };
        // Store hit-area for mouse click
        app.clipboard_preset_rects.push(Rect::new(
            inner.x,
            preset_start_y + i as u16,
            inner.width,
            1,
        ));
        lines.push(Line::from(Span::styled(format!("  {}", label), style)));
    }

    lines.push(Line::from(""));

    // Command fields
    let copy_label = "Copy command";
    let paste_label = "Paste command";
    let copy_focus = clip.field_focus == 1;
    let paste_focus = clip.field_focus == 2;

    let copy_style = if copy_focus {
        selected_style()
    } else {
        Style::default()
    };
    let paste_style = if paste_focus {
        selected_style()
    } else {
        Style::default()
    };

    // If editing, show the TextArea; else show static line
    let copy_y = inner.y + lines.len() as u16;
    if clip.editing && clip.edit_field == 0 {
        frame.render_widget(&clip.edit_area, Rect::new(inner.x, copy_y, inner.width, 3));
        // Push dummy lines to advance y for next element
        lines.push(Line::from(""));
        lines.push(Line::from(""));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  (editing — Esc to finish)",
            dim_style(),
        )));
    } else {
        app.clipboard_field_rects
            .push(Rect::new(inner.x, copy_y, inner.width, 1));
        lines.push(Line::from(vec![
            Span::styled(format!("{:>16} ", copy_label), dim_style()),
            Span::styled(&clip.copy_cmd, copy_style),
        ]));
    }

    let paste_y = inner.y + lines.len() as u16;
    if clip.editing && clip.edit_field == 1 {
        frame.render_widget(&clip.edit_area, Rect::new(inner.x, paste_y, inner.width, 3));
        lines.push(Line::from(""));
        lines.push(Line::from(""));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  (editing — Esc to finish)",
            dim_style(),
        )));
    } else {
        app.clipboard_field_rects
            .push(Rect::new(inner.x, paste_y, inner.width, 1));
        lines.push(Line::from(vec![
            Span::styled(format!("{:>16} ", paste_label), dim_style()),
            Span::styled(&clip.paste_cmd, paste_style),
        ]));
    }

    lines.push(Line::from(""));

    // Test + Save buttons (clickable)
    let btn_y = inner.y + lines.len() as u16;
    let test_label = " [t] Test ";
    let save_label = " [s] Save ";
    let hint = "   [Tab] cycle field   [←/→] or click preset";
    let test_btn_style = if app.bridge_connected {
        accent_style()
    } else {
        dim_style()
    };
    let save_btn_style = accent_style();
    lines.push(Line::from(vec![
        Span::styled(test_label, test_btn_style),
        Span::styled(save_label, save_btn_style),
        Span::styled(hint, dim_style()),
    ]));
    // Store hit-areas
    app.clipboard_btn_rects
        .push(Rect::new(inner.x + 1, btn_y, test_label.len() as u16, 1));
    app.clipboard_btn_rects.push(Rect::new(
        inner.x + 1 + test_label.len() as u16,
        btn_y,
        save_label.len() as u16,
        1,
    ));

    // Test outcome
    if !clip.test_msg.is_empty() {
        let test_style = if clip.test_msg.starts_with("✓") {
            accent_style()
        } else if clip.test_msg.starts_with("✗") {
            Style::default().fg(ratatui::style::Color::Red)
        } else {
            dim_style()
        };
        lines.push(Line::from(Span::styled(
            format!("  {}", clip.test_msg),
            test_style,
        )));
    }

    // Save outcome
    if !clip.save_msg.is_empty() {
        let save_style = if clip.save_msg.starts_with("Saved") {
            accent_style()
        } else {
            Style::default().fg(ratatui::style::Color::Red)
        };
        lines.push(Line::from(Span::styled(
            format!("  {}", clip.save_msg),
            save_style,
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_runtime(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = if app.settings.focused_frame == 0 {
        focused_panel_block(" Runtime ")
    } else {
        panel_block(" Runtime ")
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // What the server actually uses (GET /api/config).
    let (herdr_socket, herdr_bin) = app
        .config
        .as_ref()
        .map(|c| (c.herdr_socket.clone(), c.herdr_bin.clone()))
        .unwrap_or_default();
    let lines = vec![
        row(
            app.settings.selected == 0,
            "HTTP port".into(),
            app.settings.http_port.to_string(),
        ),
        row(
            app.settings.selected == 1,
            "Data directory".into(),
            truncate(&app.settings.data_dir, 40),
        ),
        row(
            app.settings.selected == 2,
            "herdr socket".into(),
            truncate(&herdr_socket, 40),
        ),
        row(app.settings.selected == 3, "herdr binary".into(), herdr_bin),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_env_and_sidecar(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = if app.settings.focused_frame == 0 {
        focused_panel_block(" Environment & sidecar ")
    } else {
        panel_block(" Environment & sidecar ")
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let env_rows = [
        ("HERDR_SOCKET_PATH", 4),
        ("HERDR_MCP_DATA_DIR", 5),
        ("HERDR_MCP_HTTP_PORT", 6),
        ("HERDR_MCP_CONFIG", 7),
        ("HERDR_BIN", 8),
    ];
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from("Overrides:"));
    for (name, sel) in env_rows {
        let set = std::env::var(name).is_ok();
        let mark = if set { "[✓]" } else { "[ ]" };
        let value = if set {
            truncate(&std::env::var(name).unwrap_or_default(), 30)
        } else {
            "(unset)".into()
        };
        let style = if app.settings.selected == sel {
            selected_style()
        } else {
            Style::default()
        };
        let mark_style = if set { accent_style() } else { dim_style() };
        lines.push(
            Line::from(vec![
                Span::styled(format!("{mark} "), mark_style),
                Span::styled(format!("{:>20} ", name), dim_style()),
                Span::styled(value, style),
            ])
            .style(style),
        );
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Sidecar profiler:"));
    lines.push(row(
        app.settings.selected == 9,
        "workspace".into(),
        app.herdr
            .workspace()
            .unwrap_or("(unknown — no focused pane)")
            .into(),
    ));
    lines.push(raw_row(format!(
        "  session:   {}",
        app.herdr.session_name()
    )));
    lines.push(raw_row(format!(
        "  pane:      {}",
        app.herdr
            .focused()
            .map_or("(unknown)", |p| p.pane_id.as_str())
    )));
    lines.push(raw_row(format!("  panes:     {}", app.herdr.panes.len())));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn row(selected: bool, label: String, value: String) -> Line<'static> {
    let style = if selected {
        selected_style()
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled(format!("{:>20} ", label), dim_style()),
        Span::styled(value, style),
    ])
    .style(style)
}

fn raw_row(text: String) -> Line<'static> {
    Line::from(Span::styled(text, dim_style()))
}

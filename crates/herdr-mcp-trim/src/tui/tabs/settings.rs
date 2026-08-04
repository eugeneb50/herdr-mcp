//! Settings tab: herdr-mcp runtime settings + herdr sidecar context profiler +
//! clipboard backend configuration.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::tui::App;
use crate::tui::theme::{accent_style, dim_style, focused_panel_block, panel_block, selected_style};
use crate::tui::truncate;

pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    // Editing a clipboard field — feed all keys to textarea.
    if app.settings.clipboard.editing {
        return handle_clipboard_edit(app, code, mods).await;
    }

    // Tab/BackTab cycle frames (2 frames: runtime=0, clipboard=1).
    match code {
        KeyCode::Tab => {
            app.settings.focused_frame = (app.settings.focused_frame + 1) % 2;
            return Ok(true);
        }
        KeyCode::BackTab => {
            app.settings.focused_frame = (app.settings.focused_frame + 1) % 2;
            return Ok(true);
        }
        _ => {}
    }

    match app.settings.focused_frame {
        0 => handle_frame_runtime(app, code).await,
        1 => handle_frame_clipboard(app, code, mods).await,
        _ => Ok(false),
    }
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
                let presets = crate::tui::BackendPreset::ALL.len();
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
                let preset = crate::tui::BackendPreset::ALL[app.settings.clipboard.preset_index];
                if !matches!(preset, crate::tui::BackendPreset::Custom) {
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
                    .set_block(crate::tui::theme::panel_block(" Edit (Esc to finish) "));
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
            if let Some(input) = crate::tui::to_textarea_input(code, mods) {
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

async fn run_clipboard_test(app: &mut App) {
    let copy = &app.settings.clipboard.copy_cmd;
    let paste = &app.settings.clipboard.paste_cmd;
    if copy.is_empty() || paste.is_empty() {
        app.settings.clipboard.test_msg = "copy/paste commands empty".into();
        return;
    }
    // Generate a nonce for this test run
    let nonce = format!("herdr-mcp-clip-test-{}", uuid::Uuid::new_v4().simple());
    let http = match &app.http {
        Some(h) => h.clone(),
        None => {
            app.settings.clipboard.test_msg =
                "HTTP bridge not connected — start herdr-mcp serve --http 7676".into();
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
                app.settings.clipboard.test_msg =
                    format!("✓ OK — round-trip {} chars", nonce.len());
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
    let copy = &app.settings.clipboard.copy_cmd;
    let paste = &app.settings.clipboard.paste_cmd;
    let path = if app.settings.clipboard.config_path.is_empty() {
        herdr_mcp_core::config_file_or_default()
    } else {
        std::path::PathBuf::from(&app.settings.clipboard.config_path)
    };
    match herdr_mcp_core::upsert_clipboard_in_file(&path, copy, paste) {
        Ok(_) => {
            app.settings.clipboard.save_msg = format!(
                "Saved -> {} ({} bytes)",
                path.display(),
                copy.len() + paste.len()
            );
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
            "  runtime + clipboard config   (↑/↓ nav, Tab clipboard fields)",
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

    // Top row: runtime + env/sidecar (original layout)
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(Rect::new(
            body.x,
            body.y,
            body.width,
            body.height.saturating_sub(14),
        ));

    render_runtime(frame, top[0], app);
    render_env_and_sidecar(frame, top[1], app);

    // Bottom: clipboard config panel
    let clipboard_area = Rect::new(
        body.x,
        top[0].y + top[0].height,
        body.width,
        body.height.saturating_sub(top[0].height).min(14),
    );
    render_clipboard(frame, clipboard_area, app);
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
        "Bridge: not connected — start herdr-mcp serve --http 7676"
    };
    lines.push(Line::from(Span::styled(bridge_text, bridge_style)));

    // Config file path
    lines.push(Line::from(Span::styled(
        format!("Config file: {}", clip.config_path),
        dim_style(),
    )));

    lines.push(Line::from(""));

    // Backend presets (radio-like)
    lines.push(Line::from(Span::styled("Backend:", accent_style())));
    let preset_start_y = inner.y + lines.len() as u16;
    for (i, preset) in crate::tui::BackendPreset::ALL.iter().enumerate() {
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

    let herdr_bin = std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".to_string());
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
            truncate(&app.settings.herdr_socket, 40),
        ),
        row(app.settings.selected == 3, "HERDR_BIN".into(), herdr_bin),
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
    match &app.herdr.workspace {
        Some(ws) => lines.push(row(
            app.settings.selected == 9,
            "workspace".into(),
            ws.clone(),
        )),
        None => lines.push(row(
            app.settings.selected == 9,
            "workspace".into(),
            "(unknown — not inside herdr?)".into(),
        )),
    }
    lines.push(raw_row(format!(
        "  pane:      {}",
        app.herdr.pane_id.as_deref().unwrap_or("(unknown)")
    )));
    lines.push(raw_row(format!("  panes:     {}", app.herdr.pane_count)));

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

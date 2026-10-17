//! Send tab, the first tab: type text into a pane picked by label —
//! `send_text`, or `run_command` (text + Enter). Panes are the dashboard's
//! live list, labeled ones first.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};

use crate::theme::{accent_style, dim_style, focused_panel_block, panel_block};
use crate::{App, HerdrPane};

/// Panes in the order the tab lists them: labeled first, each group in
/// herdr's order. A view over `app.herdr.panes`, built per call.
pub fn targets(panes: &[HerdrPane]) -> Vec<&HerdrPane> {
    let mut out: Vec<&HerdrPane> = panes.iter().collect();
    out.sort_by_key(|p| p.label.is_empty());
    out
}

fn pane_line(p: &HerdrPane) -> String {
    let head = if p.label.is_empty() {
        p.pane_id.as_str()
    } else {
        p.label.as_str()
    };
    [Some(head), p.agent.as_deref(), Some(p.cwd.as_str())]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // pane list
            Constraint::Length(5), // text box
            Constraint::Length(4), // options + status
        ])
        .split(area);

    let list = targets(&app.herdr.panes);
    let selected = app.send.pane_index.min(list.len().saturating_sub(1));
    let block = panel_block(&format!(
        " Pane — {} ({}) · ↑/↓ pick ",
        app.herdr.session_name(),
        list.len()
    ));
    if list.is_empty() {
        let mut lines = vec![Line::from("herdr reports no panes.")];
        if let Some(err) = &app.herdr.last_error {
            lines.push(Line::from(format!("Last refresh error: {err}")));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(block)
                .wrap(Wrap { trim: false }),
            chunks[0],
        );
    } else {
        let items: Vec<ListItem> = list
            .iter()
            .map(|p| {
                let dot = if p.focused { "● " } else { "  " };
                ListItem::new(format!("{dot}{}", pane_line(p)))
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(selected));
        let widget = List::new(items).block(block).highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(widget, chunks[0], &mut state);
    }

    let block = focused_panel_block(" Text · Enter send · Esc clear ");
    let inner = block.inner(chunks[1]);
    frame.render_widget(block, chunks[1]);
    frame.render_widget(&app.send.text, inner);

    let check = if app.send.enter { "[x]" } else { "[ ]" };
    let lines = vec![
        Line::from(vec![
            Span::styled(format!("{check} press Enter after"), accent_style()),
            Span::styled("   Ctrl+t toggle", dim_style()),
        ]),
        Line::from(app.send.msg.clone()),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(" Send "))
            .wrap(Wrap { trim: false }),
        chunks[2],
    );
}

/// Keyboard handler: the text box always has focus.
pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let count = app.herdr.panes.len();
    match code {
        KeyCode::Up => {
            app.send.pane_index = app.send.pane_index.saturating_sub(1);
            Ok(true)
        }
        KeyCode::Down => {
            if count > 0 {
                app.send.pane_index = (app.send.pane_index + 1).min(count - 1);
            }
            Ok(true)
        }
        KeyCode::Enter => {
            send(app).await;
            Ok(true)
        }
        KeyCode::Esc => {
            app.send.text = crate::TextArea::default();
            Ok(true)
        }
        KeyCode::Char('t') if ctrl => {
            app.send.enter = !app.send.enter;
            Ok(true)
        }
        KeyCode::Tab | KeyCode::BackTab => Ok(true),
        // Global Ctrl keys (clipboard, tab switch, refresh, quit) pass through.
        _ if ctrl || mods.contains(KeyModifiers::ALT) => Ok(false),
        KeyCode::Char(_)
        | KeyCode::Backspace
        | KeyCode::Delete
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Home
        | KeyCode::End => {
            app.feed_active_textarea(code, mods);
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn send(app: &mut App) {
    let text = app.send.text.lines().join("\n");
    if text.is_empty() {
        app.send.msg = "type something to send".into();
        return;
    }
    if let Err(msg) = app.herdr.writable() {
        app.send.msg = msg;
        return;
    }
    let Some(http) = app.http.clone() else {
        app.send.msg = "bridge not connected".into();
        return;
    };
    let list = targets(&app.herdr.panes);
    let Some(pane) = list.get(app.send.pane_index).copied() else {
        app.send.msg = "no pane to send to".into();
        return;
    };
    let name = if pane.label.is_empty() {
        pane.pane_id.clone()
    } else {
        pane.label.clone()
    };
    let pane_id = pane.pane_id.clone();
    let enter = app.send.enter;
    match http.send_to_pane(&pane_id, &text, enter).await {
        Ok(()) => {
            let how = if enter { " + Enter" } else { "" };
            app.send.msg = format!(
                "sent {} chars{how} to {name} ({pane_id})",
                text.chars().count()
            );
            app.send.text = crate::TextArea::default();
        }
        Err(e) => app.send.msg = format!("send failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, label: &str) -> HerdrPane {
        HerdrPane {
            pane_id: id.into(),
            label: label.into(),
            agent: Some("claude".into()),
            cwd: "/w".into(),
            ..Default::default()
        }
    }

    #[test]
    fn labeled_panes_come_first() {
        let panes = [
            pane("w1:p1", ""),
            pane("w1:p2", "builder"),
            pane("w1:p3", ""),
        ];
        let order: Vec<&str> = targets(&panes).iter().map(|p| p.pane_id.as_str()).collect();
        assert_eq!(order, ["w1:p2", "w1:p1", "w1:p3"]);
        assert_eq!(pane_line(&panes[1]), "builder · claude · /w");
        assert_eq!(pane_line(&panes[0]), "w1:p1 · claude · /w");
    }
}

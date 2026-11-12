//! The Events tab: herdr's event stream (`GET /api/herdr/events`, SSE: the
//! server's recent entries, then live ones), what collection triggers fire
//! on, newest first. `Tab` cycles the level: herdr's events only, plus
//! herdr-mcp's reactions to them, plus debug (every trigger checked). The stream is read by its own task, which reconnects when the
//! bridge goes away; the main loop drains what it got with
//! [`EventsState::drain`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_mcp_api::{EventLevel, HerdrEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use tokio::sync::mpsc;

use crate::App;
use crate::theme::{accent_style, dim_style, focused_panel_block, panel_block, selected_style};

/// Entries kept (diagnostics included).
const KEEP: usize = 500;

#[derive(Default)]
pub struct EventsState {
    /// Newest first.
    pub events: Vec<HerdrEvent>,
    /// Shows only events whose name contains this.
    pub filter: String,
    /// The picked event among the shown ones.
    pub index: usize,
    /// What the stream shows; changing it restarts the stream.
    pub level: EventLevel,
    /// From the stream task: an event, or `None` when it (re)connects and
    /// the server's recent events follow.
    rx: Option<mpsc::UnboundedReceiver<Option<HerdrEvent>>>,
    /// The bridge the stream reads, once started.
    bridge: Option<(u16, PathBuf)>,
    pub(crate) list: ListState,
    pub(crate) list_inner: Rect,
}

impl EventsState {
    /// Start reading the stream from the bridge on `port`, once.
    pub fn start(&mut self, port: u16, data_dir: &Path) {
        if self.rx.is_some() {
            return;
        }
        self.bridge = Some((port, data_dir.to_path_buf()));
        let level = self.level;
        // No request timeout: it would end the stream.
        let Ok(client) = crate::http::stream_client(port, data_dir) else {
            return;
        };
        let (tx, rx) = mpsc::unbounded_channel();
        self.rx = Some(rx);
        tokio::spawn(async move {
            loop {
                if let Ok(mut resp) = client.herdr_events(level).await {
                    if tx.send(None).is_err() {
                        return;
                    }
                    let mut buf: Vec<u8> = Vec::new();
                    while let Ok(Some(chunk)) = resp.chunk().await {
                        buf.extend_from_slice(&chunk);
                        while let Some(end) = buf.iter().position(|b| *b == b'\n') {
                            let line: Vec<u8> = buf.drain(..=end).collect();
                            if let Some(ev) = parse_line(&String::from_utf8_lossy(&line))
                                && tx.send(Some(ev)).is_err()
                            {
                                return;
                            }
                        }
                    }
                }
                if tx.is_closed() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    /// Take the events that arrived; `true` if there were any.
    pub fn drain(&mut self) -> bool {
        let Some(rx) = self.rx.as_mut() else {
            return false;
        };
        let mut new = Vec::new();
        let mut reset = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Some(ev) => new.push(ev),
                // A new connection replays what this list already has.
                None => {
                    reset = true;
                    new.clear();
                }
            }
        }
        if reset {
            self.events.clear();
            self.index = 0;
        }
        if new.is_empty() {
            return reset;
        }
        // Keep the picked event picked as new ones arrive above it (the
        // top stays the top: it follows the newest).
        let shown = new.iter().filter(|e| self.passes(e)).count();
        new.reverse();
        self.events.splice(0..0, new);
        self.events.truncate(KEEP);
        if self.index > 0 {
            self.index = (self.index + shown).min(self.shown().len().saturating_sub(1));
        }
        true
    }

    /// Show the next level: a new stream, which replays that level's recent
    /// entries. (The old task ends at its next send.)
    pub fn cycle_level(&mut self) {
        self.level = self.level.next();
        self.rx = None;
        self.events.clear();
        self.index = 0;
        if let Some((port, dir)) = self.bridge.clone() {
            self.start(port, &dir);
        }
    }

    fn passes(&self, e: &HerdrEvent) -> bool {
        self.filter.is_empty() || e.event.contains(&self.filter)
    }

    /// The events the filter lets through.
    pub fn shown(&self) -> Vec<&HerdrEvent> {
        self.events.iter().filter(|e| self.passes(e)).collect()
    }
}

/// One SSE line: the event in a `data:` line, else `None`.
fn parse_line(line: &str) -> Option<HerdrEvent> {
    let data = line.trim_end().strip_prefix("data:")?;
    serde_json::from_str(data.trim()).ok()
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let ev = &mut app.events;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);
    let b = focused_panel_block(&format!(" herdr events · {} ", ev.level.as_str()));
    let inner = b.inner(cols[0]);
    frame.render_widget(b, cols[0]);
    let filter = if ev.filter.is_empty() {
        Line::styled(
            "type to filter by event name  Esc clear  Tab level  Ctrl+D clear list",
            dim_style(),
        )
    } else {
        Line::from(vec![
            Span::styled("filter: ", dim_style()),
            Span::raw(ev.filter.clone()),
            Span::styled("▌", accent_style()),
        ])
    };
    frame.render_widget(
        Paragraph::new(filter),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let list_area = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    );
    let shown = ev.shown();
    let picked = shown.get(ev.index).map(|e| (*e).clone());
    if shown.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "Waiting for herdr events… (create a pane, switch tabs, or run an agent)",
                dim_style(),
            ))
            .wrap(Wrap { trim: false }),
            list_area,
        );
    }
    let items: Vec<ListItem> = shown
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let on = i == ev.index;
            let name = if e.level == EventLevel::Events {
                accent_style()
            } else {
                dim_style()
            };
            let mut spans = vec![Span::styled(if on { "▸ " } else { "  " }, accent_style())];
            if let Some(at) = e.at {
                spans.push(Span::styled(
                    format!("{} ", at.format("%H:%M:%S")),
                    dim_style(),
                ));
            }
            spans.push(Span::styled(e.event.clone(), name));
            if let Some(ws) = e.workspace_id() {
                spans.push(Span::styled(format!("  {ws}"), dim_style()));
            }
            let line = Line::from(spans);
            ListItem::new(if on {
                line.style(selected_style())
            } else {
                line
            })
        })
        .collect();
    ev.list.select(Some(ev.index));
    frame.render_stateful_widget(List::new(items), list_area, &mut ev.list);
    ev.list_inner = list_area;

    let b = panel_block(" Event data ");
    let inner = b.inner(cols[1]);
    frame.render_widget(b, cols[1]);
    let mut lines = vec![Line::styled(
        "A collection trigger fires on the event name, optionally filtered by values in its data (agent_status=done); the run reads the data as {{herdr.…}}. Edit triggers in the web UI. Tab: reactions adds what herdr-mcp did about each event (subscriber, triggers fired or skipped, agent status); debug adds every trigger checked and why it didn't match.",
        dim_style(),
    )];
    if let Some(e) = picked {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            e.event.clone(),
            accent_style().add_modifier(Modifier::BOLD),
        ));
        let mut meta = vec![e.level.as_str().to_string()];
        if let Some(at) = e.at {
            meta.push(at.format("%Y-%m-%d %H:%M:%S%.3f UTC").to_string());
        }
        lines.push(Line::styled(meta.join(" · "), dim_style()));
        lines.extend(
            serde_json::to_string_pretty(&e.data)
                .unwrap_or_default()
                .lines()
                .map(|l| Line::from(l.to_string())),
        );
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// ↑/↓ pick an event, typing filters by name, Esc clears the filter, Tab
/// cycles the level, Ctrl+D clears the list.
pub fn handle_key(ev: &mut EventsState, code: KeyCode, mods: KeyModifiers) -> bool {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let len = ev.shown().len();
    match code {
        KeyCode::Up => ev.index = ev.index.saturating_sub(1),
        KeyCode::Down => ev.index = (ev.index + 1).min(len.saturating_sub(1)),
        KeyCode::Home => ev.index = 0,
        KeyCode::End => ev.index = len.saturating_sub(1),
        KeyCode::PageUp => ev.index = ev.index.saturating_sub(10),
        KeyCode::PageDown => ev.index = (ev.index + 10).min(len.saturating_sub(1)),
        KeyCode::Char('d') if ctrl => {
            ev.events.clear();
            ev.index = 0;
        }
        KeyCode::Tab => ev.cycle_level(),
        KeyCode::Esc => {
            ev.filter.clear();
            ev.index = 0;
        }
        KeyCode::Backspace => {
            ev.filter.pop();
            ev.index = 0;
        }
        KeyCode::Char(c) if !ctrl && !mods.contains(KeyModifiers::ALT) => {
            ev.filter.push(c);
            ev.index = 0;
        }
        _ => return false,
    }
    true
}

/// Click picks an event; the wheel moves the pick.
pub fn handle_mouse(ev: &mut EventsState, m: MouseEvent) -> bool {
    let area = ev.list_inner;
    if !crate::inside(area, &m) {
        return false;
    }
    match m.kind {
        MouseEventKind::ScrollDown => handle_key(ev, KeyCode::Down, KeyModifiers::NONE),
        MouseEventKind::ScrollUp => handle_key(ev, KeyCode::Up, KeyModifiers::NONE),
        // Right-click is the copy menu's.
        MouseEventKind::Down(MouseButton::Left) => {
            let i = ev.list.offset() + (m.row - area.y) as usize;
            if i < ev.shown().len() {
                ev.index = i;
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_data_lines_only() {
        let ev =
            parse_line("data: {\"event\":\"pane.created\",\"data\":{\"pane_id\":\"w1:p2\"}}\n")
                .unwrap();
        assert_eq!(ev.event, "pane.created");
        assert_eq!(ev.data["pane_id"], "w1:p2");
        assert!(parse_line(": keep-alive\n").is_none());
        assert!(parse_line("\n").is_none());
    }

    #[test]
    fn filter_and_drain_keep_the_pick() {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut ev = EventsState {
            rx: Some(rx),
            ..Default::default()
        };
        let mk = |name: &str| HerdrEvent {
            event: name.into(),
            data: serde_json::Value::Null,
            level: EventLevel::Events,
            at: None,
        };
        tx.send(Some(mk("pane.created"))).unwrap();
        tx.send(Some(mk("tab.created"))).unwrap();
        assert!(ev.drain());
        assert_eq!(ev.events[0].event, "tab.created");
        ev.index = 1;
        tx.send(Some(mk("pane.closed"))).unwrap();
        tx.send(Some(mk("tab.closed"))).unwrap();
        ev.drain();
        assert_eq!(ev.events[ev.index].event, "pane.created");
        assert_eq!(ev.events[0].event, "tab.closed");
        handle_key(&mut ev, KeyCode::Char('p'), KeyModifiers::NONE);
        handle_key(&mut ev, KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(ev.shown().len(), 2);
        assert!(!ev.drain());
        // Filtered: only events that pass move the pick.
        ev.index = 1;
        tx.send(Some(mk("tab.created"))).unwrap();
        tx.send(Some(mk("pane.focused"))).unwrap();
        ev.drain();
        assert_eq!(ev.shown()[ev.index].event, "pane.created");

        // A reconnect replays the server's recent events: the list restarts.
        tx.send(None).unwrap();
        tx.send(Some(mk("pane.created"))).unwrap();
        assert!(ev.drain());
        assert_eq!(ev.events.len(), 1);
        assert_eq!(ev.index, 0);
    }

    /// Tab moves to the next level and starts over (no bridge here, so no
    /// stream: the list is just emptied).
    #[test]
    fn tab_cycles_the_level() {
        let mut ev = EventsState::default();
        ev.events.push(HerdrEvent::diagnostic(
            EventLevel::Reactions,
            "subscribed",
            serde_json::Value::Null,
        ));
        assert!(handle_key(&mut ev, KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(ev.level, EventLevel::Reactions);
        assert!(ev.events.is_empty());
        handle_key(&mut ev, KeyCode::Tab, KeyModifiers::NONE);
        handle_key(&mut ev, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(ev.level, EventLevel::Events);
    }
}

//! herdr's event stream (`GET /api/herdr/events`, SSE: the server's last
//! 200 events, then live ones): what collection triggers fire on, newest
//! first. The stream is read by its own task, which reconnects when the
//! bridge goes away; the main loop drains what it got with
//! [`EventsState::drain`].

use std::path::Path;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_mcp_api::HerdrEvent;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use tokio::sync::mpsc;

/// Events kept.
const KEEP: usize = 200;

#[derive(Default)]
pub struct EventsState {
    /// Newest first.
    pub events: Vec<HerdrEvent>,
    /// Shows only events whose name contains this.
    pub filter: String,
    /// The picked event among the shown ones.
    pub index: usize,
    /// From the stream task: an event, or `None` when it (re)connects and
    /// the server's recent events follow.
    rx: Option<mpsc::UnboundedReceiver<Option<HerdrEvent>>>,
    pub(crate) list: ListState,
    pub(crate) list_inner: Rect,
}

impl EventsState {
    /// Start reading the stream from the bridge on `port`, once.
    pub fn start(&mut self, port: u16, data_dir: &Path) {
        if self.rx.is_some() {
            return;
        }
        // No request timeout: it would end the stream.
        let Ok(client) = crate::http::stream_client(port, data_dir) else {
            return;
        };
        let (tx, rx) = mpsc::unbounded_channel();
        self.rx = Some(rx);
        tokio::spawn(async move {
            loop {
                if let Ok(mut resp) = client.herdr_events().await {
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

/// ↑/↓ pick an event, typing filters by name, Esc clears the filter,
/// Ctrl+D clears the list.
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
}

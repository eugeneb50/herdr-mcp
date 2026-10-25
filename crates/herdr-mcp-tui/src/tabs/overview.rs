//! Overview tab: intro + stats, the pane table, and the picked pane's text
//! (like a web deck card): re-read live, scrollable, and selectable with
//! the mouse for Ctrl+C / right-click Copy.

use std::time::Instant;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Wrap};

use crate::App;
use crate::theme::{focused_panel_block, panel_block};

/// Scrollback lines the pane view reads.
pub const SCROLLBACK: u32 = 200;

/// A text position: (line, column in chars).
type Pos = (usize, usize);

/// The picked pane's text in the lower panel: what herdr last read for it,
/// the scroll position and a mouse selection. While a selection is up the
/// text is not re-read, so the highlight stays on what was selected.
#[derive(Debug, Default)]
pub struct PaneView {
    /// Pane (in the shown session) the text belongs to.
    pub pane_id: Option<String>,
    pub lines: Vec<String>,
    pub error: Option<String>,
    /// When the text was last read.
    pub read_at: Option<Instant>,
    /// First line shown.
    pub top: usize,
    /// Keep the last line in view as the text changes (End turns it on).
    pub follow: bool,
    /// Scrollback instead of the visible screen.
    pub scrollback: bool,
    /// Where the selection started; `None`: no selection.
    pub anchor: Option<Pos>,
    /// Where it ends now (inclusive).
    pub head: Pos,
    /// A left-button drag is extending the selection.
    pub dragging: bool,
    /// Text rows the panel showed last frame.
    pub height: usize,
}

impl PaneView {
    /// Take a read of `pane_id`. Another pane starts over: top, following,
    /// no selection (the screen/scrollback choice stays).
    pub fn update(&mut self, pane_id: &str, text: Result<String, String>) {
        if self.pane_id.as_deref() != Some(pane_id) {
            *self = PaneView {
                pane_id: Some(pane_id.to_string()),
                follow: true,
                scrollback: self.scrollback,
                height: self.height,
                ..Default::default()
            };
        }
        match text {
            Ok(t) => {
                self.lines = t.trim_end().lines().map(str::to_string).collect();
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.read_at = Some(Instant::now());
        self.clamp();
    }

    /// No pane to show.
    pub fn clear(&mut self) {
        *self = PaneView {
            scrollback: self.scrollback,
            ..Default::default()
        };
    }

    /// Re-reading would move the text under the selection.
    pub fn frozen(&self) -> bool {
        self.dragging || self.selection().is_some()
    }

    fn max_top(&self) -> usize {
        self.lines.len().saturating_sub(self.height.max(1))
    }

    /// Keep `top` in range, at the bottom while following.
    pub fn clamp(&mut self) {
        if self.follow {
            self.top = self.max_top();
        }
        self.top = self.top.min(self.max_top());
    }

    /// Scroll by `delta` lines; following again once at the bottom.
    pub fn scroll(&mut self, delta: isize) {
        self.top = self.top.saturating_add_signed(delta).min(self.max_top());
        self.follow = self.top == self.max_top();
    }

    /// The text position under panel cell (`row`, `col`), clamped to the text.
    pub fn pos_at(&self, row: usize, col: usize) -> Pos {
        let line = (self.top + row).min(self.lines.len().saturating_sub(1));
        let len = self.lines.get(line).map_or(0, |l| l.chars().count());
        (line, col.min(len.saturating_sub(1)))
    }

    /// The selection as (start, end), both inclusive, start first. A click
    /// without a drag is no selection.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let anchor = self.anchor?;
        (anchor != self.head).then(|| (anchor.min(self.head), anchor.max(self.head)))
    }

    /// The char range of line `i` inside `sel`, end exclusive.
    fn span_of(line: &str, i: usize, sel: (Pos, Pos)) -> Option<(usize, usize)> {
        let ((r0, c0), (r1, c1)) = sel;
        if i < r0 || i > r1 {
            return None;
        }
        let len = line.chars().count();
        let start = if i == r0 { c0.min(len) } else { 0 };
        let end = if i == r1 { (c1 + 1).min(len) } else { len };
        Some((start, end.max(start)))
    }

    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection()?;
        let picked: Vec<String> = (sel.0.0..=sel.1.0)
            .filter_map(|i| {
                let line = self.lines.get(i)?;
                let (s, e) = Self::span_of(line, i, sel)?;
                Some(line.chars().skip(s).take(e - s).collect())
            })
            .collect();
        Some(picked.join("\n"))
    }

    /// The selection, else all of the text.
    pub fn copy_text(&self) -> String {
        self.selected_text()
            .unwrap_or_else(|| self.lines.join("\n"))
    }

    /// Line `i` as shown, the selected part reversed.
    fn render_line(&self, i: usize) -> Line<'static> {
        let line = &self.lines[i];
        match self.selection().and_then(|sel| Self::span_of(line, i, sel)) {
            None => Line::from(line.clone()),
            Some((s, e)) => {
                let part =
                    |a: usize, b: usize| line.chars().skip(a).take(b - a).collect::<String>();
                Line::from(vec![
                    Span::raw(part(0, s)),
                    Span::styled(
                        part(s, e),
                        Style::default().add_modifier(Modifier::REVERSED),
                    ),
                    Span::raw(part(e, line.chars().count())),
                ])
            }
        }
    }
}

pub fn render(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    // The table takes what its rows need (header + border), up to a third
    // of the tab; the pane text gets the rest.
    let table_rows = (app.herdr.panes.len().max(3) + 3) as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // intro + stats
            Constraint::Length(table_rows.min(area.height / 3).max(6)),
            Constraint::Min(6), // picked pane's text
        ])
        .split(area);

    render_intro(frame, chunks[0], app);
    render_pane_table(frame, chunks[1], app);
    render_pane_text(frame, chunks[2], app);
}

fn render_intro(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(" Overview ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from(format!(
            "Local control surface for herdr. MCP server + HTTP bridge on port {} · {} tools · session {} · {} panes",
            app.settings.http_port,
            app.playground.tools.len(),
            app.herdr.session_name(),
            app.herdr.panes.len(),
        )),
        Line::from(format!(
            "Tabs: {} / {} or {}…{} (Settings → keys), or click the tab strip. Here: ↑/↓ pick a pane, Tab to its text, drag to select, Ctrl+C copy.",
            crate::first_chord(&app.knobs, "keys.tab-next"),
            crate::first_chord(&app.knobs, "keys.tab-prev"),
            crate::first_chord(&app.knobs, "keys.tab-1"),
            crate::first_chord(&app.knobs, "keys.tab-5"),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_pane_table(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let pane_count = app.herdr.panes.len();
    let others = app
        .herdr
        .sessions
        .iter()
        .filter(|s| s.running && !s.current)
        .count();
    let switch = if others > 0 {
        " · s: next session"
    } else {
        ""
    };
    let title = &format!(
        " Panes — {} ({pane_count}){switch} ",
        app.herdr.session_name()
    );
    let block = if app.overview.focused_frame == 0 {
        focused_panel_block(title)
    } else {
        panel_block(title)
    };
    let inner = block.inner(area);
    app.pane_table_inner = inner;
    frame.render_widget(block, area);

    if pane_count == 0 {
        let mut lines = vec![
            Line::from("No herdr panes detected."),
            Line::from(""),
            Line::from("Pane list comes from herdr through the bridge (/api/herdr/panes)."),
        ];
        if let Some(err) = &app.herdr.last_error {
            lines.push(Line::from(format!("Last refresh error: {err}")));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Tab"),
        Cell::from("Label"),
        Cell::from("Agent"),
        Cell::from("Status"),
        Cell::from("CWD"),
    ])
    .style(
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );

    let clamped = app.overview.pane_index.min(pane_count - 1);

    let rows = app.herdr.panes.iter().enumerate().map(|(i, pane)| {
        let is_focused = pane.focused;
        let status_color = match pane.agent_status.as_str() {
            "working" => Color::Green,
            "idle" => Color::DarkGray,
            "done" => Color::Cyan,
            _ => Color::Gray,
        };
        let cwd_short = trim_cwd(pane.cwd.as_deref().unwrap_or_default());
        let mut label = pane.label.clone().unwrap_or_default();
        if is_focused {
            label = format!("● {}", label);
        }
        let row_style = if i == clamped {
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Row::new(vec![
            Cell::from(pane.tab_id.clone()).style(row_style),
            Cell::from(label).style(row_style),
            Cell::from(pane.agent.clone().unwrap_or_default()).style(row_style),
            Cell::from(pane.agent_status.clone()).style(
                Style::default()
                    .fg(status_color)
                    .add_modifier(if i == clamped {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Cell::from(cwd_short).style(row_style),
        ])
    });

    let widths = [
        Constraint::Length(14),
        Constraint::Length(20),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Min(20),
    ];

    let mut table_state = ratatui::widgets::TableState::default();
    table_state.select(Some(clamped));

    let table = Table::new(rows, widths).header(header).row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    frame.render_stateful_widget(table, inner, &mut table_state);
    app.overview.table_offset = table_state.offset();
}

fn render_pane_text(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let pane = app.herdr.panes.get(
        app.overview
            .pane_index
            .min(app.herdr.panes.len().saturating_sub(1)),
    );
    let view = &app.overview.view;
    let name = pane.map_or_else(
        || "no pane".to_string(),
        |p| {
            let who = p.label.as_deref().or(p.agent.as_deref()).unwrap_or("");
            [p.pane_id.as_str(), who, p.agent_status.as_str()]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" · ")
        },
    );
    let source = if view.scrollback {
        "scrollback"
    } else {
        "screen"
    };
    let hint = if view.selection().is_some() {
        "selected (paused) · Ctrl+C copy · Esc clear"
    } else {
        "drag to select · r: screen/scrollback"
    };
    let title = format!(" {name} · {source} · {hint} ");
    let block = if app.overview.focused_frame == 1 {
        focused_panel_block(&title)
    } else {
        panel_block(&title)
    };
    let inner = block.inner(area);
    app.pane_text_inner = inner;
    frame.render_widget(block, area);

    let view = &mut app.overview.view;
    view.height = inner.height as usize;
    view.clamp();
    let lines: Vec<Line> = if pane.is_none() {
        vec![Line::from("No pane selected.")]
    } else if let Some(err) = &view.error {
        vec![Line::from(format!("Could not read the pane: {err}"))]
    } else if view.lines.is_empty() {
        vec![Line::from(if view.read_at.is_some() {
            "(empty)"
        } else {
            "Reading…"
        })]
    } else {
        (view.top..view.lines.len())
            .take(view.height)
            .map(|i| view.render_line(i))
            .collect()
    };
    // No wrap: rows map 1:1 to text lines, which mouse selection relies on.
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Keyboard handler for the Overview tab: the pane table (frame 0) or the
/// picked pane's text (frame 1); Tab switches.
pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    let pane_count = app.herdr.panes.len();
    // Clamp if panes were removed since last frame.
    if pane_count == 0 {
        app.overview.pane_index = 0;
    } else if app.overview.pane_index >= pane_count {
        app.overview.pane_index = pane_count - 1;
    }
    let plain = !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    match code {
        KeyCode::Tab | KeyCode::BackTab => {
            app.overview.focused_frame = 1 - app.overview.focused_frame.min(1);
            return Ok(true);
        }
        KeyCode::Char('s') if plain => {
            app.herdr.cycle_session();
            app.overview.pane_index = 0;
            app.refresh_herdr().await;
            return Ok(true);
        }
        KeyCode::Char('r') if plain => {
            let view = &mut app.overview.view;
            view.scrollback = !view.scrollback;
            // Re-read now, from the bottom.
            view.anchor = None;
            view.follow = true;
            view.read_at = None;
            return Ok(true);
        }
        _ => {}
    }
    if app.overview.focused_frame == 1 {
        return Ok(handle_text_key(&mut app.overview.view, code));
    }
    match code {
        KeyCode::Up => {
            app.overview.pane_index = app.overview.pane_index.saturating_sub(1);
            Ok(true)
        }
        KeyCode::Down => {
            if pane_count > 0 {
                app.overview.pane_index = (app.overview.pane_index + 1).min(pane_count - 1);
            }
            Ok(true)
        }
        KeyCode::Home => {
            app.overview.pane_index = 0;
            Ok(true)
        }
        KeyCode::End => {
            if pane_count > 0 {
                app.overview.pane_index = pane_count - 1;
            }
            Ok(true)
        }
        KeyCode::PageUp => {
            app.overview.pane_index = app.overview.pane_index.saturating_sub(5);
            Ok(true)
        }
        KeyCode::PageDown => {
            if pane_count > 0 {
                app.overview.pane_index = (app.overview.pane_index + 5).min(pane_count - 1);
            }
            Ok(true)
        }
        KeyCode::Enter => {
            app.overview.focused_frame = 1;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Keys on the pane text: scroll, Esc drops the selection.
fn handle_text_key(view: &mut PaneView, code: KeyCode) -> bool {
    let page = view.height.max(1) as isize;
    match code {
        KeyCode::Up => view.scroll(-1),
        KeyCode::Down => view.scroll(1),
        KeyCode::PageUp => view.scroll(-page),
        KeyCode::PageDown => view.scroll(page),
        KeyCode::Home => {
            view.follow = false;
            view.top = 0;
        }
        KeyCode::End => {
            view.follow = true;
            view.clamp();
        }
        KeyCode::Esc => view.anchor = None,
        _ => return false,
    }
    true
}

/// Trim long absolute paths to show last 2-3 segments.
fn trim_cwd(cwd: &str) -> String {
    let parts: Vec<&str> = cwd.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 3 {
        return cwd.to_string();
    }
    let tail = &parts[parts.len() - 2..];
    format!("…/{}", tail.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{App, DashboardOptions, HerdrPane};

    fn test_app_with_panes(count: usize) -> App {
        let mut app = App::new(DashboardOptions {
            data_dir: std::path::PathBuf::from("/tmp"),
            http_port: 1,
        });
        app.herdr.panes = (0..count)
            .map(|i| HerdrPane {
                pane_id: format!("ws:p{i}"),
                label: Some(format!("pane-{i}")),
                agent: Some(format!("agent-{i}")),
                agent_status: "idle".into(),
                cwd: Some(format!("/tmp/pane-{i}")),
                focused: i == 0,
                workspace_id: "ws".into(),
                tab_id: "ws:t1".into(),
                ..Default::default()
            })
            .collect();
        app
    }

    #[test]
    fn trim_cwd_short() {
        assert_eq!(trim_cwd("/tmp"), "/tmp");
        assert_eq!(trim_cwd("/home/user"), "/home/user");
    }

    #[test]
    fn trim_cwd_long() {
        assert_eq!(
            trim_cwd("/home/producer32/code/herdr-mcp/crates/herdr-mcp-trim"),
            "…/crates/herdr-mcp-trim"
        );
        assert_eq!(trim_cwd("/a/b/c/d/e"), "…/d/e");
    }

    #[test]
    fn trim_cwd_exactly_3() {
        assert_eq!(trim_cwd("/a/b/c"), "/a/b/c");
    }

    #[test]
    fn overview_state_defaults() {
        let app = test_app_with_panes(0);
        assert_eq!(app.overview.pane_index, 0);
    }

    #[tokio::test]
    async fn overview_key_arrows() {
        let mut app = test_app_with_panes(4);
        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 1);
        handle_key(&mut app, KeyCode::Up, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 0);
    }

    #[tokio::test]
    async fn overview_key_home() {
        let mut app = test_app_with_panes(5);
        app.overview.pane_index = 3;
        handle_key(&mut app, KeyCode::Home, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 0);
    }

    #[tokio::test]
    async fn overview_key_end() {
        let mut app = test_app_with_panes(5);
        app.overview.pane_index = 0;
        handle_key(&mut app, KeyCode::End, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 4);
    }

    #[tokio::test]
    async fn overview_key_pageup() {
        let mut app = test_app_with_panes(10);
        app.overview.pane_index = 7;
        handle_key(&mut app, KeyCode::PageUp, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 2);
    }

    #[tokio::test]
    async fn overview_key_pagedown() {
        let mut app = test_app_with_panes(10);
        app.overview.pane_index = 2;
        handle_key(&mut app, KeyCode::PageDown, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 7);
    }

    #[tokio::test]
    async fn tab_moves_focus_between_table_and_text() {
        let mut app = test_app_with_panes(3);
        app.overview.pane_index = 1;
        handle_key(&mut app, KeyCode::Tab, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(
            (app.overview.pane_index, app.overview.focused_frame),
            (1, 1)
        );
        // Arrows now scroll the text, not the pick.
        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 1);
        handle_key(&mut app, KeyCode::BackTab, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.focused_frame, 0);
    }

    fn view(text: &str, height: usize) -> PaneView {
        let mut v = PaneView {
            height,
            ..Default::default()
        };
        v.update("w1:p1", Ok(text.to_string()));
        v
    }

    #[test]
    fn selection_spans_lines_inclusive_either_direction() {
        let mut v = view("alpha\nbravo\ncharlie", 10);
        v.anchor = Some(v.pos_at(0, 2));
        v.head = v.pos_at(2, 1);
        assert_eq!(v.selected_text().as_deref(), Some("pha\nbravo\nch"));
        // Dragged backwards: the same text.
        (v.anchor, v.head) = (Some(v.head), v.anchor.unwrap());
        assert_eq!(v.selected_text().as_deref(), Some("pha\nbravo\nch"));
        // Past a line's end clamps to its last char.
        v.anchor = Some(v.pos_at(1, 3));
        v.head = v.pos_at(1, 99);
        assert_eq!(v.selected_text().as_deref(), Some("vo"));
        assert_eq!(v.copy_text(), "vo");
    }

    #[test]
    fn a_click_is_no_selection_and_copies_everything() {
        let mut v = view("one\ntwo\n\n", 10);
        v.anchor = Some(v.pos_at(1, 1));
        v.head = v.pos_at(1, 1);
        assert!(v.selection().is_none());
        assert!(!v.frozen());
        assert_eq!(v.copy_text(), "one\ntwo");
    }

    #[test]
    fn follows_the_bottom_until_scrolled_up() {
        let text: Vec<String> = (0..10).map(|i| format!("l{i}")).collect();
        let mut v = view(&text.join("\n"), 4);
        assert_eq!(v.top, 6);
        v.scroll(-2);
        assert_eq!((v.top, v.follow), (4, false));
        // Rows map to lines from the top shown.
        assert_eq!(v.pos_at(0, 0), (4, 0));
        v.update("w1:p1", Ok(format!("{}\nl10", text.join("\n"))));
        assert_eq!(v.top, 4, "stays put while scrolled up");
        v.scroll(10);
        assert_eq!((v.top, v.follow), (7, true));
        // Another pane starts over, keeping screen/scrollback.
        v.scrollback = true;
        v.anchor = Some((0, 0));
        v.update("w1:p2", Ok("x".into()));
        assert_eq!((v.top, v.anchor, v.scrollback), (0, None, true));
    }

    #[tokio::test]
    async fn overview_unrelated_key_not_consumed() {
        let mut app = test_app_with_panes(3);
        let consumed = handle_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE)
            .await
            .unwrap();
        assert!(!consumed);
    }

    #[tokio::test]
    async fn overview_clamp_on_empty_panes() {
        let mut app = test_app_with_panes(0);
        app.overview.pane_index = 5;
        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 0);
    }

    #[tokio::test]
    async fn overview_end_on_empty_panes() {
        let mut app = test_app_with_panes(0);
        handle_key(&mut app, KeyCode::End, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(app.overview.pane_index, 0);
    }
}

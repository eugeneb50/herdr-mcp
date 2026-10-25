//! The Runs view: every run, newest first, and the picked run's trail (what
//! each request sent and got, the gate's checks, errors, the pane snapshot
//! of a failure), as in the web playground's Runs tab.

use crossterm::event::{KeyCode, KeyModifiers, MouseEvent, MouseEventKind};
use herdr_mcp_api::{RunAction, RunEvent, RunRecord, RunStatus, Uuid, trail_text};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};

use super::{View, client, inside, moved};
use crate::App;
use crate::theme::{accent_style, dim_style, muted_style, panel_block, selected_style, warn_style};

#[derive(Default)]
pub struct RunsState {
    /// Newest first.
    pub runs: Vec<RunRecord>,
    /// The picked run's log, and whose it is.
    pub events: Vec<RunEvent>,
    events_of: Option<Uuid>,
    /// The picked run among the shown ones.
    pub index: usize,
    /// `None`: every run; `Some(None)`: ad-hoc runs; `Some(Some(id))`:
    /// that collection's.
    pub filter: Option<Option<Uuid>>,
    pub failed_only: bool,
    /// Lines the trail is scrolled down by.
    pub scroll: u16,
    pub(crate) list: ListState,
    pub(crate) list_inner: Rect,
    pub(crate) trail_inner: Rect,
}

impl RunsState {
    pub fn shown(&self) -> Vec<&RunRecord> {
        self.runs
            .iter()
            .filter(|r| self.filter.is_none_or(|f| r.recipe_id == f))
            .filter(|r| {
                !self.failed_only || matches!(r.status, RunStatus::Failed | RunStatus::Interrupted)
            })
            .collect()
    }

    pub fn picked(&self) -> Option<&RunRecord> {
        self.shown().get(self.index).copied()
    }

    /// The filter's name for the list title.
    fn filter_name(&self) -> String {
        match self.filter {
            None => "all".into(),
            Some(None) => "ad-hoc".into(),
            Some(Some(id)) => self
                .runs
                .iter()
                .find(|r| r.recipe_id == Some(id))
                .and_then(|r| r.name.clone())
                .unwrap_or_else(|| "this collection".into()),
        }
    }

    /// The next collection filter: all, then each group that has runs.
    fn cycle_filter(&mut self) {
        let mut groups: Vec<Option<Uuid>> = Vec::new();
        for r in &self.runs {
            if !groups.contains(&r.recipe_id) {
                groups.push(r.recipe_id);
            }
        }
        self.filter = match self.filter {
            None => groups.first().copied(),
            Some(f) => {
                let at = groups.iter().position(|g| *g == f);
                at.and_then(|i| groups.get(i + 1)).copied()
            }
        };
        self.index = 0;
        self.scroll = 0;
    }
}

/// Show the Runs view, filtered (`None`: every run).
pub async fn open(app: &mut App, filter: Option<Option<Uuid>>) {
    let runs = &mut app.playground.runs;
    runs.filter = filter;
    runs.index = 0;
    runs.scroll = 0;
    app.playground.view = View::Runs;
    refresh(app).await;
}

/// Re-read the run list, and the picked run's log while it can still change.
pub async fn refresh(app: &mut App) {
    let Some(http) = app.http.clone() else {
        return;
    };
    let picked = app.playground.runs.picked().map(|r| r.id);
    if let Ok(list) = http.list_runs().await {
        let state = &mut app.playground.runs;
        state.runs = list;
        // Stay on the same run as new ones arrive above it.
        if let Some(i) = picked.and_then(|id| state.shown().iter().position(|r| r.id == id)) {
            state.index = i;
        }
        state.index = state.index.min(state.shown().len().saturating_sub(1));
    }
    load_events(app, &http).await;
}

async fn load_events(app: &mut App, http: &herdr_mcp_api::ApiClient) {
    let state = &app.playground.runs;
    let Some((id, finished)) = state.picked().map(|r| (r.id, r.status.is_finished())) else {
        return;
    };
    let stale = state.events_of != Some(id)
        || !finished
        || !state.events.iter().any(|e| e.kind.is_terminal());
    if !stale {
        return;
    }
    if let Ok(events) = http.run_events(id, 0).await {
        let state = &mut app.playground.runs;
        if state.events_of != Some(id) {
            state.scroll = 0;
        }
        state.events = events;
        state.events_of = Some(id);
    }
}

fn status_style(s: RunStatus) -> ratatui::style::Style {
    match s {
        RunStatus::Completed => accent_style(),
        RunStatus::Failed | RunStatus::Interrupted => warn_style(),
        RunStatus::Running | RunStatus::Paused => muted_style(),
        RunStatus::Cancelled => dim_style(),
    }
}

fn status_name(s: RunStatus) -> String {
    format!("{s:?}").to_lowercase()
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
        .split(area);
    let state = &mut app.playground.runs;
    let shown = state.shown();
    let title = format!(
        " Runs · {} of {} · {}{} ",
        shown.len(),
        state.runs.len(),
        state.filter_name(),
        if state.failed_only { " · failed" } else { "" }
    );
    let items: Vec<ListItem> = shown
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let line = Line::from(vec![
                Span::styled(
                    format!("{:<10}", status_name(r.status)),
                    status_style(r.status),
                ),
                Span::raw(r.name.clone().unwrap_or_else(|| "ad-hoc run".into())),
                Span::styled(
                    format!("  {}", r.started_at.format("%m-%d %H:%M:%S")),
                    dim_style(),
                ),
            ]);
            ListItem::new(if i == state.index {
                line.style(selected_style())
            } else {
                line
            })
        })
        .collect();
    let empty = items.is_empty();
    let block = panel_block(&title);
    let inner = block.inner(cols[0]);
    frame.render_widget(block, cols[0]);
    state.list_inner = inner;
    if empty {
        frame.render_widget(Paragraph::new(Line::styled("No runs.", dim_style())), inner);
    } else {
        state.list.select(Some(state.index));
        frame.render_stateful_widget(List::new(items), inner, &mut state.list);
    }

    let picked = state.picked().cloned();
    let (title, text) = match &picked {
        Some(run) => {
            let events = if state.events_of == Some(run.id) {
                state.events.as_slice()
            } else {
                &[]
            };
            (
                format!(" {} · by {} ", run.summary(), run.origin),
                trail_text(run, events),
            )
        }
        None => (" Trail ".into(), String::new()),
    };
    let block = panel_block(&title);
    let inner = block.inner(cols[1]);
    frame.render_widget(block, cols[1]);
    state.trail_inner = inner;
    let mut lines: Vec<Line> = text.lines().map(|l| Line::raw(l.to_string())).collect();
    if picked.is_some() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "PgUp/PgDn scroll  y copy  o open pane  r run again  x cancel",
            dim_style(),
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((state.scroll, 0)),
        inner,
    );
}

pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> bool {
    if mods.contains(KeyModifiers::CONTROL) {
        return false;
    }
    let len = app.playground.runs.shown().len();
    match code {
        KeyCode::Esc => app.playground.view = View::Collections,
        KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End => {
            if let Some(next) = moved(app.playground.runs.index, len, code) {
                app.playground.runs.index = next;
                if let Some(http) = app.http.clone() {
                    load_events(app, &http).await;
                }
            }
        }
        KeyCode::PageDown => {
            let page = app.playground.runs.trail_inner.height.max(2) - 1;
            let s = &mut app.playground.runs.scroll;
            *s = s.saturating_add(page);
        }
        KeyCode::PageUp => {
            let page = app.playground.runs.trail_inner.height.max(2) - 1;
            let s = &mut app.playground.runs.scroll;
            *s = s.saturating_sub(page);
        }
        KeyCode::Char('f') => {
            let s = &mut app.playground.runs;
            s.failed_only = !s.failed_only;
            s.index = 0;
            s.scroll = 0;
            refresh(app).await;
        }
        KeyCode::Char('c') => {
            app.playground.runs.cycle_filter();
            refresh(app).await;
        }
        KeyCode::Char('y') => {
            let state = &app.playground.runs;
            if let Some(run) = state.picked() {
                let text = trail_text(run, &state.events);
                app.set_clipboard(&text).await;
            }
        }
        KeyCode::Char('o') => open_pane(app).await,
        KeyCode::Char('r') => {
            let Some(req) = app.playground.runs.picked().map(RunRecord::rerun) else {
                return true;
            };
            if let Some(http) = client(app) {
                match http.start_run(&req).await {
                    Ok(r) => {
                        app.playground.ok(format!("started run {}", r.id));
                        app.playground.runs.index = 0;
                        refresh(app).await;
                    }
                    Err(e) => app.playground.err(format!("run failed to start: {e}")),
                }
            }
        }
        KeyCode::Char('x') => {
            let Some(id) = app
                .playground
                .runs
                .picked()
                .filter(|r| !r.status.is_finished())
                .map(|r| r.id)
            else {
                return true;
            };
            if let Some(http) = client(app) {
                match http.control_run(id, RunAction::Cancel).await {
                    Ok(()) => app.playground.ok("cancelled"),
                    Err(e) => app.playground.err(format!("cancel failed: {e}")),
                }
            }
        }
        _ => return false,
    }
    true
}

/// Focus the run's pane in herdr: agents keep their sessions, so its
/// scrollback shows what happened.
async fn open_pane(app: &mut App) {
    let state = &app.playground.runs;
    let Some(pane) = state.picked().and_then(|r| r.pane(&state.events)) else {
        app.playground.err("this run touched no pane");
        return;
    };
    let Some(http) = client(app) else {
        return;
    };
    let args = serde_json::json!({ "method": "pane.focus", "params": { "pane_id": pane } });
    match http.call_tool("herdr_api", &args).await {
        Ok(r) if !herdr_mcp_api::tool_is_error(&r) => {
            app.playground.ok(format!("focused pane {pane} in herdr"));
        }
        Ok(r) => app.playground.err(herdr_mcp_api::tool_text(&r)),
        Err(e) => app.playground.err(format!("cannot focus {pane}: {e}")),
    }
}

/// Clicks pick a run; the wheel moves the pick or scrolls the trail.
pub async fn handle_mouse(app: &mut App, m: MouseEvent) -> bool {
    let state = &mut app.playground.runs;
    let (list, trail) = (state.list_inner, state.trail_inner);
    if inside(trail, &m) {
        match m.kind {
            MouseEventKind::ScrollDown => state.scroll = state.scroll.saturating_add(3),
            MouseEventKind::ScrollUp => state.scroll = state.scroll.saturating_sub(3),
            _ => return false,
        }
        return true;
    }
    if !inside(list, &m) {
        return false;
    }
    let len = state.shown().len();
    let next = match m.kind {
        MouseEventKind::Down(_) => state.list.offset() + (m.row - list.y) as usize,
        MouseEventKind::ScrollDown => state.index + 1,
        MouseEventKind::ScrollUp => state.index.saturating_sub(1),
        _ => return false,
    };
    if next < len {
        state.index = next;
        if let Some(http) = app.http.clone() {
            load_events(app, &http).await;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(recipe: Option<Uuid>, status: RunStatus) -> RunRecord {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "recipe_id": recipe, "name": "c", "session_id": "w1",
            "steps": [], "origin": { "kind": "api" }, "gate": null, "pass": 1,
            "status": status, "current_step": null, "results": {}, "variables": {},
            "failed_step": null, "error": null,
            "started_at": "2026-10-25T00:00:00Z", "finished_at": null
        }))
        .unwrap()
    }

    /// `c` steps through all runs, each collection's, then all again; `f`
    /// keeps failed ones.
    #[test]
    fn filters_cycle_collections_and_failed() {
        let a = Uuid::new_v4();
        let mut s = RunsState {
            runs: vec![
                run(Some(a), RunStatus::Failed),
                run(None, RunStatus::Completed),
                run(Some(a), RunStatus::Completed),
            ],
            ..Default::default()
        };
        assert_eq!(s.shown().len(), 3);
        s.cycle_filter();
        assert_eq!((s.filter, s.shown().len()), (Some(Some(a)), 2));
        s.failed_only = true;
        assert_eq!(s.shown().len(), 1);
        s.failed_only = false;
        s.cycle_filter();
        assert_eq!((s.filter, s.shown().len()), (Some(None), 1));
        s.cycle_filter();
        assert_eq!(s.filter, None);
    }
}

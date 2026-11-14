//! The Runs tab: every run, newest first, and the picked run's trail (what
//! each node sent and got, each branch's checks, errors, the pane snapshot
//! of a failure), as in the web playground's Runs tab. Quick actions: run a
//! collection, run again, cancel, open the run's pane, follow parent and
//! child runs. Collections are edited in the web UI (`w`).

use crossterm::event::{KeyCode, KeyModifiers, MouseEvent, MouseEventKind};
use herdr_mcp_api::{
    ApiClient, Recipe, RunAction, RunEvent, RunRecord, RunStatus, StartRunRequest, Uuid,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::App;
use crate::theme::{
    accent_style, dim_style, focused_panel_block, muted_style, panel_block, selected_style,
    warn_style,
};
use crate::{inside, moved};

#[derive(Default)]
pub struct RunsState {
    /// Newest first.
    pub runs: Vec<RunRecord>,
    /// The picked run's log and trail (with the trail of the child run its
    /// run nodes started), and whose they are.
    pub events: Vec<RunEvent>,
    pub trail: String,
    events_of: Option<Uuid>,
    /// The picked run among the shown ones.
    pub index: usize,
    /// `None`: every run; `Some(None)`: ad-hoc runs; `Some(Some(id))`:
    /// that collection's.
    pub filter: Option<Option<Uuid>>,
    pub failed_only: bool,
    /// Lines the trail is scrolled down by.
    pub scroll: u16,
    /// The run-a-collection picker, open on this row of [`collections`].
    pub picker: Option<usize>,
    pub(crate) list: ListState,
    pub(crate) list_inner: Rect,
    pub(crate) trail_inner: Rect,
}

/// The collections a run can start, by name.
pub fn collections(recipes: &[Recipe]) -> Vec<&Recipe> {
    let mut all: Vec<&Recipe> = recipes.iter().filter(|r| !r.is_template).collect();
    all.sort_by_key(|r| r.name.to_lowercase());
    all
}

/// The bridge client, or a status message saying it isn't there.
fn client(app: &mut App) -> Option<ApiClient> {
    let http = app.http.clone();
    if http.is_none() {
        app.status_msg = "bridge not connected".into();
    }
    http
}

impl RunsState {
    pub fn shown(&self) -> Vec<&RunRecord> {
        self.runs
            .iter()
            .filter(|r| self.filter.is_none_or(|f| r.recipe_id == f))
            .filter(|r| !self.failed_only || r.status.is_failure())
            .collect()
    }

    pub fn picked(&self) -> Option<&RunRecord> {
        self.shown().get(self.index).copied()
    }

    /// Pick run `id`, clearing the filters if they hide it. False when it is
    /// not among the runs listed.
    fn pick(&mut self, id: Uuid) -> bool {
        if !self.runs.iter().any(|r| r.id == id) {
            return false;
        }
        if !self.shown().iter().any(|r| r.id == id) {
            self.filter = None;
            self.failed_only = false;
        }
        self.index = self.shown().iter().position(|r| r.id == id).unwrap_or(0);
        self.scroll = 0;
        true
    }

    /// The first run the picked one started (a run node, a
    /// `recipe_run` step).
    fn child(&self) -> Option<Uuid> {
        self.picked()?.children(&self.runs).last().map(|r| r.id)
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

/// Re-read the run list, and the picked run's log while it can still change.
pub async fn refresh(app: &mut App) {
    let Some(http) = app.http.clone() else {
        return;
    };
    let picked = app.runs.picked().map(|r| r.id);
    if let Ok(list) = http.list_runs(herdr_mcp_api::playground::RUNS_SHOWN).await {
        let state = &mut app.runs;
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
    let state = &app.runs;
    let Some((id, finished)) = state.picked().map(|r| (r.id, r.status.is_finished())) else {
        return;
    };
    let stale = state.events_of != Some(id)
        || !finished
        || !state.events.iter().any(|e| e.kind.is_terminal());
    if !stale {
        return;
    }
    if let (Ok(events), Ok(trail)) = (http.run_events(id, 0).await, http.run_trail(id).await) {
        let state = &mut app.runs;
        if state.events_of != Some(id) {
            state.scroll = 0;
        }
        state.events = events;
        state.trail = trail;
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

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
        .split(area);
    let state = &mut app.runs;
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
                Span::styled(format!("{:<10}", r.status.as_str()), status_style(r.status)),
                Span::raw(r.title().to_string()),
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
        frame.render_widget(
            Paragraph::new(Line::styled(
                "No runs. r runs a collection; w opens the web UI to build one.",
                dim_style(),
            )),
            inner,
        );
    } else {
        state.list.select(Some(state.index));
        frame.render_stateful_widget(List::new(items), inner, &mut state.list);
    }

    let picked = state.picked().cloned();
    let (title, text) = match &picked {
        Some(run) => {
            let trail = if state.events_of == Some(run.id) {
                state.trail.clone()
            } else {
                String::new()
            };
            (format!(" {} · by {} ", run.summary(), run.origin), trail)
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
            "PgUp/PgDn scroll  y copy  o open pane  p parent  d child  R run again  x cancel",
            dim_style(),
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((state.scroll, 0)),
        inner,
    );
    if let Some(at) = app.runs.picker {
        render_picker(frame, area, app, at);
    }
}

/// The run-a-collection picker, over the runs.
fn render_picker(frame: &mut Frame, area: Rect, app: &App, at: usize) {
    let all = collections(app.recipes.as_deref().unwrap_or_default());
    let width = area.width.clamp(20, 60);
    let height = (all.len() as u16 + 3).clamp(4, area.height);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    let block = focused_panel_block(" Run a collection · Enter run  Esc close ");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if all.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "No collections yet: build one in the web UI (w).",
                dim_style(),
            ))
            .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    let items: Vec<ListItem> = all
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut spans = vec![Span::raw(r.name.clone())];
            if let Some(c) = &r.category {
                spans.push(Span::styled(format!("  {c}"), dim_style()));
            }
            let line = Line::from(spans);
            ListItem::new(if i == at {
                line.style(selected_style())
            } else {
                line
            })
        })
        .collect();
    let mut list = ListState::default().with_selected(Some(at));
    frame.render_stateful_widget(List::new(items), inner, &mut list);
}

/// Keys while the picker is open: move, Enter starts the run, Esc closes.
async fn handle_picker(app: &mut App, at: usize, code: KeyCode) {
    let ids: Vec<Uuid> = collections(app.recipes.as_deref().unwrap_or_default())
        .iter()
        .map(|r| r.id)
        .collect();
    match code {
        KeyCode::Esc => app.runs.picker = None,
        KeyCode::Enter => {
            app.runs.picker = None;
            let Some(&id) = ids.get(at) else {
                return;
            };
            let req = StartRunRequest {
                recipe_id: Some(id),
                session_id: app.herdr.workspace().map(str::to_string),
                ..Default::default()
            };
            start(app, &req).await;
        }
        _ => {
            if let Some(next) = moved(at, ids.len(), code) {
                app.runs.picker = Some(next);
            }
        }
    }
}

/// Start `req` and pick the new run.
async fn start(app: &mut App, req: &StartRunRequest) {
    let Some(http) = client(app) else {
        return;
    };
    match http.start_run(req).await {
        Ok(r) => {
            app.status_msg = format!("started run {} of {}", r.id, r.title());
            app.runs.filter = None;
            app.runs.failed_only = false;
            app.runs.index = 0;
            refresh(app).await;
        }
        Err(e) => app.status_msg = format!("run failed to start: {e}"),
    }
}

pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> bool {
    if mods.contains(KeyModifiers::CONTROL) {
        return false;
    }
    if let Some(at) = app.runs.picker {
        handle_picker(app, at, code).await;
        return true;
    }
    let len = app.runs.shown().len();
    match code {
        KeyCode::Char('r') => app.runs.picker = Some(0),
        KeyCode::Char('w') => crate::open_web(app),
        KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End => {
            if let Some(next) = moved(app.runs.index, len, code) {
                app.runs.index = next;
                if let Some(http) = app.http.clone() {
                    load_events(app, &http).await;
                }
            }
        }
        KeyCode::PageDown => {
            let page = app.runs.trail_inner.height.max(2) - 1;
            let s = &mut app.runs.scroll;
            *s = s.saturating_add(page);
        }
        KeyCode::PageUp => {
            let page = app.runs.trail_inner.height.max(2) - 1;
            let s = &mut app.runs.scroll;
            *s = s.saturating_sub(page);
        }
        KeyCode::Char('f') => {
            let s = &mut app.runs;
            s.failed_only = !s.failed_only;
            s.index = 0;
            s.scroll = 0;
            refresh(app).await;
        }
        KeyCode::Char('c') => {
            app.runs.cycle_filter();
            refresh(app).await;
        }
        KeyCode::Char('y') => {
            let state = &app.runs;
            if state
                .picked()
                .is_some_and(|r| state.events_of == Some(r.id))
            {
                let text = state.trail.clone();
                app.set_clipboard(&text).await;
            }
        }
        KeyCode::Char('o') => open_pane(app).await,
        KeyCode::Char(c @ ('p' | 'd')) => {
            let state = &mut app.runs;
            let target = if c == 'p' {
                state.picked().and_then(|r| r.origin.parent())
            } else {
                state.child()
            };
            match target {
                Some(id) if state.pick(id) => {
                    if let Some(http) = app.http.clone() {
                        load_events(app, &http).await;
                    }
                }
                Some(id) => app.status_msg = format!("run {id} is not among the runs listed"),
                None if c == 'p' => app.status_msg = "no run started this one".into(),
                None => app.status_msg = "this run started no other run".into(),
            }
        }
        KeyCode::Char('R') => {
            let Some(req) = app.runs.picked().map(RunRecord::rerun) else {
                return true;
            };
            start(app, &req).await;
        }
        KeyCode::Char('x') => {
            let Some(id) = app
                .runs
                .picked()
                .filter(|r| !r.status.is_finished())
                .map(|r| r.id)
            else {
                return true;
            };
            if let Some(http) = client(app) {
                match http.control_run(id, RunAction::Cancel).await {
                    Ok(()) => app.status_msg = "cancelled".into(),
                    Err(e) => app.status_msg = format!("cancel failed: {e}"),
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
    let state = &app.runs;
    let Some(pane) = state.picked().and_then(|r| r.pane(&state.events)) else {
        app.status_msg = "this run touched no pane".into();
        return;
    };
    let Some(http) = client(app) else {
        return;
    };
    match herdr_mcp_api::playground::focus_pane(&http, &pane).await {
        Ok(m) => app.status_msg = m,
        Err(e) => app.status_msg = e,
    }
}

/// Clicks pick a run; the wheel moves the pick or scrolls the trail.
pub async fn handle_mouse(app: &mut App, m: MouseEvent) -> bool {
    let state = &mut app.runs;
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
            "flow": { "nodes": [] }, "origin": { "kind": "api" }, "pass": 1,
            "status": status, "active": [], "results": {}, "variables": {},
            "failed_node": null, "error": null,
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

    /// `d` finds the run the picked one started and `p` comes back, even
    /// through a filter that hides it.
    #[test]
    fn parent_and_child_navigation() {
        let parent = run(Some(Uuid::new_v4()), RunStatus::Completed);
        let mut child = run(None, RunStatus::Failed);
        child.origin = herdr_mcp_api::RunOrigin::Node {
            run_id: parent.id,
            node: "next".into(),
        };
        let mut s = RunsState {
            runs: vec![child.clone(), parent.clone()],
            failed_only: true,
            ..Default::default()
        };
        assert!(s.pick(parent.id));
        assert!(!s.failed_only, "the filter hid the parent");
        assert_eq!(s.child(), Some(child.id));
        assert!(s.pick(child.id));
        assert_eq!(s.picked().map(|r| r.id), Some(child.id));
        assert!(!s.pick(Uuid::new_v4()));
    }
}

//! Playground tab: collections of saved requests, as in the web playground
//! (Postman style). A collection is a saved recipe and its requests are the
//! recipe's steps: send one request (a run of just that step) or run the
//! whole collection. Extract rules save part of a result as a global or
//! collection variable for the next request. Every `{{ }}` resolves on the
//! server, in the one scope runs use. Ctrl+E switches to herdr's live event
//! stream, what collection triggers fire on.
//!
//! Three panels: the collection tree, the picked collection or request, and
//! the last response (with its result paths, Enter to extract one). Tab
//! moves between them; Enter edits a field or presses a button row.

mod events;
mod view;

pub use events::EventsState;
pub use view::render;

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_mcp_api::playground::{
    HERDR_API, HTTP_REQUEST, LLM_CALL, Method, ToolInfo, add_request, delete_step, display_value,
    event_names, folders, methods, parse_value, parse_when, raw_values, request_method,
    request_params, request_schema, request_values, result_paths, save_step, set_steps,
    sorted_vars, tool_list, var_name, when_text,
};
use herdr_mcp_api::{
    ApiClient, CreateRecipeRequest, Extract, InstantiateTemplateRequest, Recipe, RecipeStep,
    RecipeTemplate, RecipeTrigger, ResolveRequest, RunRecord, RunStatus, StartRunRequest,
    UpdateRecipeRequest, Uuid, VarScope,
};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use serde_json::Value;

use crate::{App, Tab, TextArea};

/// Result paths listed under a response.
const MAX_PATHS: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum View {
    #[default]
    Collections,
    Events,
}

/// The panel with keyboard focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Focus {
    #[default]
    Tree,
    Main,
    /// The response text (scroll, Shift+arrows select, Ctrl+C copies).
    Response,
    /// The response's result paths (Enter adds an extract rule).
    Paths,
}

/// What the main panel shows.
#[derive(Clone, PartialEq, Debug, Default)]
pub enum Pick {
    #[default]
    Nothing,
    Collection(Uuid),
    Request(Uuid, String),
}

/// One row of the collection tree.
#[derive(Clone, PartialEq, Debug)]
pub enum TreeRow {
    Globals,
    NewCollection,
    FromTemplate,
    Folder(String),
    Collection(Uuid),
    Request(Uuid, String),
}

/// The tree for `recipes`: the fixed rows, then each folder's collections
/// with their requests.
pub fn tree_rows(recipes: &[Recipe]) -> Vec<TreeRow> {
    let mut rows = vec![
        TreeRow::Globals,
        TreeRow::NewCollection,
        TreeRow::FromTemplate,
    ];
    for (folder, items) in folders(recipes) {
        if !folder.is_empty() {
            rows.push(TreeRow::Folder(folder));
        }
        for r in items {
            rows.push(TreeRow::Collection(r.id));
            rows.extend(r.steps.iter().map(|s| TreeRow::Request(r.id, s.id.clone())));
        }
    }
    rows
}

/// A collection being edited: its own fields, saved with Save. Requests
/// are saved one by one (from the request editor, or on reorder/delete).
pub struct CollectionForm {
    pub id: Uuid,
    pub name: String,
    pub folder: String,
    pub vars: Vec<(String, String)>,
    pub triggers: Vec<RecipeTrigger>,
    pub row: usize,
    /// Unsaved edits to the fields above.
    pub changed: bool,
    /// Delete was pressed once; the next press deletes.
    pub confirm_delete: bool,
    pub run: Option<RunRecord>,
}

impl CollectionForm {
    fn new(r: &Recipe) -> Self {
        Self {
            id: r.id,
            name: r.name.clone(),
            folder: r.category.clone().unwrap_or_default(),
            vars: sorted_vars(&r.variables),
            triggers: r.triggers.clone(),
            row: 0,
            changed: false,
            confirm_delete: false,
            run: None,
        }
    }

    pub fn rows(&self, requests: usize) -> Vec<CRow> {
        let mut rows = vec![CRow::Name, CRow::Folder];
        rows.extend((0..requests).map(CRow::Request));
        rows.extend([CRow::AddRequest, CRow::AddHttp, CRow::AddLlm, CRow::RunAll]);
        rows.extend((0..self.vars.len()).map(CRow::Var));
        rows.push(CRow::AddVar);
        rows.extend((0..self.triggers.len()).map(CRow::Trigger));
        rows.extend([CRow::AddTrigger, CRow::Save, CRow::Delete]);
        rows
    }
}

/// A row of the collection form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CRow {
    Name,
    Folder,
    Request(usize),
    AddRequest,
    AddHttp,
    AddLlm,
    RunAll,
    Var(usize),
    AddVar,
    Trigger(usize),
    AddTrigger,
    Save,
    Delete,
}

/// A request being edited. Send saves it first, then runs just this step.
pub struct RequestForm {
    pub rid: Uuid,
    /// The request's id as saved (it changes with a rename).
    pub saved_id: String,
    pub id: String,
    pub description: String,
    pub tool: String,
    /// The herdr method, for a `herdr_api` request.
    pub method: String,
    /// Raw form values by param name.
    pub values: HashMap<String, String>,
    pub extract: Vec<Extract>,
    pub row: usize,
    pub confirm_delete: bool,
    /// The params as a run would resolve them, and the placeholders that
    /// don't resolve outside a run.
    pub resolved: Value,
    pub unresolved: Vec<String>,
    pub response: Option<RunRecord>,
    /// The response's result paths, and the picked one.
    pub paths: Vec<(String, Value)>,
    pub path_index: usize,
}

impl RequestForm {
    pub(crate) fn new(rid: Uuid, step: &RecipeStep) -> Self {
        Self {
            rid,
            saved_id: step.id.clone(),
            id: step.id.clone(),
            description: step.description.clone().unwrap_or_default(),
            tool: step.tool.clone(),
            method: request_method(step),
            values: request_values(step),
            extract: step.extract.clone(),
            row: 0,
            confirm_delete: false,
            resolved: Value::Null,
            unresolved: Vec::new(),
            response: None,
            paths: Vec::new(),
            path_index: 0,
        }
    }

    pub fn herdr(&self) -> bool {
        self.tool == HERDR_API
    }

    /// The form's params: properties and required names.
    pub fn schema(
        &self,
        tools: &[ToolInfo],
        methods: &[Method],
    ) -> (HashMap<String, Value>, HashSet<String>) {
        request_schema(tools, methods, &self.tool, &self.method)
    }

    /// Param names in form order: required first, then by name; values
    /// the schema doesn't know (kept from the saved request) last.
    pub fn param_names(
        &self,
        props: &HashMap<String, Value>,
        required: &HashSet<String>,
    ) -> Vec<String> {
        let mut names: Vec<String> = props.keys().cloned().collect();
        names.sort_by(|a, b| {
            required
                .contains(b)
                .cmp(&required.contains(a))
                .then(a.cmp(b))
        });
        let mut extra: Vec<String> = self
            .values
            .keys()
            .filter(|k| !props.contains_key(*k))
            .cloned()
            .collect();
        extra.sort();
        names.extend(extra);
        names
    }

    pub fn rows(&self, tools: &[ToolInfo], methods: &[Method]) -> Vec<RRow> {
        let (props, required) = self.schema(tools, methods);
        let mut rows = vec![RRow::Id, RRow::Tool, RRow::Description];
        if self.herdr() {
            rows.push(RRow::Method);
        }
        rows.extend(
            self.param_names(&props, &required)
                .into_iter()
                .map(RRow::Param),
        );
        rows.extend((0..self.extract.len()).map(RRow::Extract));
        rows.extend([RRow::AddExtract, RRow::Send, RRow::Save, RRow::Delete]);
        rows
    }

    /// The params this request sends.
    fn params(&self, tools: &[ToolInfo], methods: &[Method]) -> Value {
        let (props, _) = self.schema(tools, methods);
        request_params(&props, &self.tool, &self.method, &self.values)
    }

    /// The request as saved.
    fn step(&self, tools: &[ToolInfo], methods: &[Method]) -> RecipeStep {
        RecipeStep {
            id: self.id.trim().to_string(),
            tool: self.tool.clone(),
            params: self.params(tools, methods),
            description: Some(self.description.trim().to_string()).filter(|d| !d.is_empty()),
            extract: self
                .extract
                .iter()
                .filter(|x| !x.var.trim().is_empty())
                .cloned()
                .collect(),
        }
    }

    /// The resolved value of each param, as the form shows it.
    pub fn resolved_values(&self) -> HashMap<String, String> {
        raw_values(if self.herdr() {
            &self.resolved["params"]
        } else {
            &self.resolved
        })
    }
}

/// A row of the request form.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RRow {
    Id,
    Tool,
    Description,
    Method,
    Param(String),
    Extract(usize),
    AddExtract,
    Send,
    Save,
    Delete,
}

/// What a text edit writes to when it ends.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    CollectionName,
    Folder,
    Var(usize),
    Trigger(usize),
    RequestId,
    Description,
    Param(String),
    Extract(usize),
    TemplateVar(usize),
}

/// A field being typed into.
pub struct Editing {
    pub target: Target,
    pub area: TextArea<'static>,
}

/// A picker or form drawn over the panels.
pub enum Overlay {
    Tools {
        search: String,
        index: usize,
    },
    Methods {
        search: String,
        index: usize,
    },
    Templates {
        index: usize,
    },
    /// A template's variables, filled in before it becomes a collection.
    Template {
        template: RecipeTemplate,
        values: Vec<String>,
        /// A variable, then Create, then Cancel.
        row: usize,
        error: Option<String>,
    },
}

/// A send or run in flight. It runs as its own task so the TUI keeps
/// drawing and reading keys; the main loop hands a finished one to
/// [`finish_job`].
pub(crate) enum Job {
    /// A send of request `.1` of collection `.0`.
    Send(
        Uuid,
        String,
        tokio::task::JoinHandle<Result<RunRecord, String>>,
    ),
    Run(Uuid, tokio::task::JoinHandle<Result<RunRecord, String>>),
}

impl Job {
    pub(crate) fn is_finished(&self) -> bool {
        match self {
            Self::Send(_, _, task) | Self::Run(_, task) => task.is_finished(),
        }
    }
}

/// Playground tab state.
#[derive(Default)]
pub struct PlaygroundState {
    pub view: View,
    pub focus: Focus,
    pub pick: Pick,
    /// Selected tree row.
    pub tree: usize,
    pub collection: Option<CollectionForm>,
    pub request: Option<RequestForm>,
    pub overlay: Option<Overlay>,
    pub editing: Option<Editing>,
    /// The tool catalog (`GET /api/tools`).
    pub tools: Vec<ToolInfo>,
    /// herdr's methods and event names, from its schema (fetched once).
    pub methods: Vec<Method>,
    pub event_names: Vec<String>,
    schema_loaded: bool,
    /// The response shown on the right (read-only; selectable to copy).
    pub result_area: TextArea<'static>,
    /// Outcome of the last action.
    pub msg: Option<Result<String, String>>,
    pub events: EventsState,
    // Layout from the last draw, for the mouse.
    pub(crate) tree_list: ListState,
    pub(crate) main_list: ListState,
    pub(crate) paths_list: ListState,
    pub(crate) tree_inner: Rect,
    pub(crate) main_inner: Rect,
    pub(crate) result_inner: Rect,
    pub(crate) paths_inner: Rect,
    /// The form row on each line of the main panel (headers are `None`).
    pub(crate) main_lines: Vec<Option<usize>>,
}

impl PlaygroundState {
    fn ok(&mut self, m: impl Into<String>) {
        self.msg = Some(Ok(m.into()));
    }

    fn err(&mut self, m: impl Into<String>) {
        self.msg = Some(Err(m.into()));
    }

    /// The text field being typed into, if any.
    pub fn edit_area(&self) -> Option<&TextArea<'static>> {
        self.editing.as_ref().map(|e| &e.area)
    }

    pub fn edit_area_mut(&mut self) -> Option<&mut TextArea<'static>> {
        self.editing.as_mut().map(|e| &mut e.area)
    }

    /// The editable text the focused row holds, and where it goes.
    fn row_text(&self, requests: usize) -> Option<(Target, String)> {
        if self.focus != Focus::Main {
            return None;
        }
        match &self.pick {
            Pick::Collection(_) => {
                let c = self.collection.as_ref()?;
                match *c.rows(requests).get(c.row)? {
                    CRow::Name => Some((Target::CollectionName, c.name.clone())),
                    CRow::Folder => Some((Target::Folder, c.folder.clone())),
                    CRow::Var(i) => {
                        let (k, v) = &c.vars[i];
                        Some((Target::Var(i), format!("{k} = {v}")))
                    }
                    CRow::Trigger(i) => Some((Target::Trigger(i), trigger_text(&c.triggers[i]))),
                    _ => None,
                }
            }
            Pick::Request(..) => {
                let r = self.request.as_ref()?;
                match r.rows(&self.tools, &self.methods).get(r.row)? {
                    RRow::Id => Some((Target::RequestId, r.id.clone())),
                    RRow::Description => Some((Target::Description, r.description.clone())),
                    RRow::Param(name) => Some((
                        Target::Param(name.clone()),
                        r.values.get(name).cloned().unwrap_or_default(),
                    )),
                    RRow::Extract(i) => Some((Target::Extract(*i), extract_text(&r.extract[*i]))),
                    _ => None,
                }
            }
            Pick::Nothing => None,
        }
    }

    fn begin_edit(&mut self, target: Target, text: String) {
        let mut area = TextArea::new(vec![text]);
        area.move_cursor(tui_textarea::CursorMove::End);
        self.editing = Some(Editing { target, area });
    }
}

/// A trigger as one line: the event, then its `when` filters.
pub fn trigger_text(t: &RecipeTrigger) -> String {
    format!("{} {}", t.on, when_text(&t.when))
        .trim()
        .to_string()
}

fn parse_trigger(text: &str) -> RecipeTrigger {
    let text = text.trim();
    let (on, when) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    RecipeTrigger {
        on: on.to_string(),
        when: parse_when(when),
    }
}

/// An extract rule as one line: `path -> var`.
pub fn extract_text(x: &Extract) -> String {
    format!("{} -> {}", x.path, x.var)
}

fn parse_extract(text: &str, scope: VarScope) -> Extract {
    let (path, var) = text.split_once("->").unwrap_or((text, ""));
    Extract {
        path: path.trim().to_string(),
        var: var.trim().to_string(),
        scope,
    }
}

fn parse_var(text: &str) -> (String, String) {
    match text.split_once('=') {
        Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
        None => (text.trim().to_string(), String::new()),
    }
}

// ── data ───────────────────────────────────────────────────────────────

/// Take a fresh `/api/tools` listing.
pub fn set_tools(app: &mut App, tools: &[Value]) {
    app.playground.tools = tool_list(tools);
}

fn recipe(app: &App, id: Uuid) -> Option<&Recipe> {
    app.recipes.as_ref()?.iter().find(|r| r.id == id)
}

fn request_count(app: &App) -> usize {
    match &app.playground.collection {
        Some(c) => recipe(app, c.id).map_or(0, |r| r.steps.len()),
        None => 0,
    }
}

fn client(app: &mut App) -> Option<ApiClient> {
    let http = app.http.clone();
    if http.is_none() {
        app.playground.err("bridge not connected");
    }
    http
}

/// Re-read the collections (after anything that changes them).
async fn reload_recipes(app: &mut App) {
    if let Some(http) = app.http.clone()
        && let Ok(v) = http.list_recipes().await
    {
        app.recipes = Some(v);
    }
}

/// Fetch herdr's schema once: its methods and event names.
async fn load_schema(app: &mut App) {
    if app.playground.schema_loaded {
        return;
    }
    let Some(http) = app.http.clone() else {
        return;
    };
    if let Ok(schema) = http.herdr_schema().await {
        app.playground.methods = methods(&schema);
        app.playground.event_names = event_names(&schema);
        app.playground.schema_loaded = true;
    }
}

/// Resolve the request's params as a run would.
async fn preview(app: &mut App) {
    let Some(http) = app.http.clone() else {
        return;
    };
    let pg = &app.playground;
    let Some(r) = &pg.request else {
        return;
    };
    let req = ResolveRequest {
        recipe_id: Some(r.rid),
        value: r.params(&pg.tools, &pg.methods),
        ..Default::default()
    };
    let resolved = http.resolve(&req).await;
    if let Some(r) = app.playground.request.as_mut() {
        match resolved {
            Ok(res) => {
                r.resolved = res.value;
                r.unresolved = res.unresolved;
            }
            Err(_) => {
                r.resolved = Value::Null;
                r.unresolved.clear();
            }
        }
    }
}

// ── opening ────────────────────────────────────────────────────────────

fn open_collection(app: &mut App, id: Uuid) {
    let Some(r) = recipe(app, id) else {
        return;
    };
    let form = CollectionForm::new(r);
    let pg = &mut app.playground;
    pg.collection = Some(form);
    pg.request = None;
    pg.pick = Pick::Collection(id);
    pg.focus = Focus::Main;
    pg.msg = None;
    pg.result_area = TextArea::default();
}

async fn open_request(app: &mut App, rid: Uuid, step_id: &str) {
    let Some(step) = recipe(app, rid).and_then(|r| r.steps.iter().find(|s| s.id == step_id)) else {
        return;
    };
    let form = RequestForm::new(rid, step);
    if form.herdr() {
        load_schema(app).await;
    }
    let pg = &mut app.playground;
    pg.request = Some(form);
    pg.collection = None;
    pg.pick = Pick::Request(rid, step_id.to_string());
    pg.focus = Focus::Main;
    pg.msg = None;
    pg.result_area = TextArea::default();
    preview(app).await;
}

/// Put the tree selection on `row`, if it is there.
fn select_tree(app: &mut App, row: &TreeRow) {
    let rows = tree_rows(app.recipes.as_deref().unwrap_or_default());
    if let Some(i) = rows.iter().position(|r| r == row) {
        app.playground.tree = i;
    }
}

async fn create_collection(app: &mut App, name: String, steps: Vec<RecipeStep>) {
    let Some(http) = client(app) else {
        return;
    };
    let req = CreateRecipeRequest {
        name,
        description: None,
        steps,
        variables: HashMap::new(),
        category: None,
        triggers: Vec::new(),
    };
    match http.create_recipe(&req).await {
        Ok(r) => {
            reload_recipes(app).await;
            select_tree(app, &TreeRow::Collection(r.id));
            open_collection(app, r.id);
        }
        Err(e) => app
            .playground
            .err(format!("could not create the collection: {e}")),
    }
}

/// Open what tree row `i` names.
async fn activate_tree(app: &mut App, i: usize) {
    let rows = tree_rows(app.recipes.as_deref().unwrap_or_default());
    match rows.get(i).cloned() {
        Some(TreeRow::Globals) => {
            app.tab = Tab::Variables;
            app.status_msg = "globals: every request and run sees them".into();
        }
        Some(TreeRow::NewCollection) => {
            create_collection(app, "New collection".into(), Vec::new()).await
        }
        Some(TreeRow::FromTemplate) => {
            app.playground.overlay = Some(Overlay::Templates { index: 0 });
        }
        Some(TreeRow::Collection(id)) => open_collection(app, id),
        Some(TreeRow::Request(rid, sid)) => open_request(app, rid, &sid).await,
        Some(TreeRow::Folder(_)) | None => {}
    }
}

// ── keys ───────────────────────────────────────────────────────────────

/// Returns `true` if the key was consumed (don't run global handler).
pub async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    if app.playground.editing.is_some() {
        return Ok(handle_edit(app, code, mods).await);
    }
    if app.playground.overlay.is_some() {
        handle_overlay(app, code, mods).await;
        return Ok(true);
    }
    if ctrl && code == KeyCode::Char('e') {
        let pg = &mut app.playground;
        pg.view = match pg.view {
            View::Collections => View::Events,
            View::Events => View::Collections,
        };
        if pg.view == View::Events {
            pg.events.start(app.opts.http_port, &app.opts.data_dir);
        }
        return Ok(true);
    }
    if app.playground.view == View::Events {
        return Ok(events::handle_key(&mut app.playground.events, code, mods));
    }
    match code {
        KeyCode::Tab => {
            cycle_focus(app, true);
            return Ok(true);
        }
        KeyCode::BackTab => {
            cycle_focus(app, false);
            return Ok(true);
        }
        _ => {}
    }
    if ctrl {
        match code {
            KeyCode::Char('n') => {
                create_collection(app, "New collection".into(), Vec::new()).await;
                return Ok(true);
            }
            KeyCode::Char('s') => {
                save(app).await;
                return Ok(true);
            }
            KeyCode::Char('g') => {
                go(app).await;
                return Ok(true);
            }
            _ => {}
        }
    }
    let consumed = match app.playground.focus {
        Focus::Tree => handle_tree(app, code, mods).await,
        Focus::Main => match app.playground.pick {
            Pick::Collection(_) => handle_collection(app, code, mods).await,
            Pick::Request(..) => handle_request(app, code, mods).await,
            Pick::Nothing => false,
        },
        Focus::Response => handle_response(app, code, mods),
        Focus::Paths => handle_paths(app, code),
    };
    Ok(consumed)
}

/// The panels Tab visits: the tree, the picked item, and its response.
fn focus_order(app: &App) -> Vec<Focus> {
    let pg = &app.playground;
    let mut order = vec![Focus::Tree];
    if pg.pick != Pick::Nothing {
        order.push(Focus::Main);
    }
    let response = match &pg.pick {
        Pick::Request(..) => pg.request.as_ref().is_some_and(|r| r.response.is_some()),
        Pick::Collection(_) => pg.collection.as_ref().is_some_and(|c| c.run.is_some()),
        Pick::Nothing => false,
    };
    if response {
        order.push(Focus::Response);
    }
    if pg.request.as_ref().is_some_and(|r| !r.paths.is_empty()) {
        order.push(Focus::Paths);
    }
    order
}

fn cycle_focus(app: &mut App, forward: bool) {
    let order = focus_order(app);
    let at = order
        .iter()
        .position(|f| *f == app.playground.focus)
        .unwrap_or(0);
    let n = order.len();
    let next = if forward {
        (at + 1) % n
    } else {
        (at + n - 1) % n
    };
    app.playground.focus = order[next];
}

/// `index` moved by an arrow/paging key within `0..len`; `None` for other
/// keys.
fn moved(index: usize, len: usize, code: KeyCode) -> Option<usize> {
    let last = len.saturating_sub(1);
    let next = match code {
        KeyCode::Up => index.saturating_sub(1),
        KeyCode::Down => (index + 1).min(last),
        KeyCode::Home => 0,
        KeyCode::End => last,
        KeyCode::PageUp => index.saturating_sub(10),
        KeyCode::PageDown => (index + 10).min(last),
        _ => return None,
    };
    Some(next)
}

/// A plain printable key, which starts typing into a text row.
fn typed(code: KeyCode, mods: KeyModifiers) -> Option<char> {
    match code {
        KeyCode::Char(c)
            if !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) =>
        {
            Some(c)
        }
        _ => None,
    }
}

async fn handle_tree(app: &mut App, code: KeyCode, _mods: KeyModifiers) -> bool {
    let rows = tree_rows(app.recipes.as_deref().unwrap_or_default());
    if let Some(mut next) = moved(app.playground.tree, rows.len(), code) {
        // Folder headings aren't stops: keep going the way the key went.
        let down = next >= app.playground.tree;
        while matches!(rows.get(next), Some(TreeRow::Folder(_))) {
            next = if down {
                (next + 1).min(rows.len() - 1)
            } else {
                next.saturating_sub(1)
            };
            if next == 0 || next == rows.len() - 1 {
                break;
            }
        }
        app.playground.tree = next;
        return true;
    }
    match code {
        KeyCode::Enter | KeyCode::Right => {
            activate_tree(app, app.playground.tree).await;
            true
        }
        _ => false,
    }
}

async fn handle_collection(app: &mut App, code: KeyCode, mods: KeyModifiers) -> bool {
    let requests = request_count(app);
    let Some(c) = app.playground.collection.as_ref() else {
        return false;
    };
    let (id, rows, at, confirm) = (c.id, c.rows(requests), c.row, c.confirm_delete);
    let row = rows.get(at).copied();
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    if let Some(next) = moved(at, rows.len(), code) {
        if ctrl && matches!(code, KeyCode::Up | KeyCode::Down) {
            if let Some(CRow::Request(i)) = row {
                let j = if code == KeyCode::Up {
                    i.checked_sub(1)
                } else {
                    Some(i + 1)
                };
                if let Some(j) = j.filter(|j| *j < requests) {
                    if let Some(c) = app.playground.collection.as_mut() {
                        c.row = next;
                    }
                    reorder(app, i, j).await;
                }
            }
            return true;
        }
        if let Some(c) = app.playground.collection.as_mut() {
            c.row = next;
            c.confirm_delete = false;
        }
        return true;
    }
    if code == KeyCode::Esc {
        app.playground.focus = Focus::Tree;
        return true;
    }
    if ctrl && code == KeyCode::Char('d') {
        match row {
            Some(CRow::Request(i)) => {
                let sid = recipe(app, id).map(|r| r.steps[i].id.clone());
                if let (Some(http), Some(sid)) = (client(app), sid) {
                    match delete_step(&http, id, &sid).await {
                        Ok(()) => reload_recipes(app).await,
                        Err(e) => app.playground.err(e),
                    }
                }
            }
            Some(CRow::Var(i)) => {
                if let Some(c) = app.playground.collection.as_mut() {
                    c.vars.remove(i);
                    c.changed = true;
                }
            }
            Some(CRow::Trigger(i)) => {
                if let Some(c) = app.playground.collection.as_mut() {
                    c.triggers.remove(i);
                    c.changed = true;
                }
            }
            _ => {}
        }
        return true;
    }
    if let Some(ch) = typed(code, mods) {
        if let Some((target, text)) = app.playground.row_text(requests) {
            app.playground.begin_edit(target, format!("{text}{ch}"));
            return true;
        }
        return false;
    }
    if code != KeyCode::Enter {
        return false;
    }
    let Some(row) = row else {
        return true;
    };
    match row {
        CRow::Name | CRow::Folder | CRow::Var(_) | CRow::Trigger(_) => {
            if matches!(row, CRow::Trigger(_)) {
                load_schema(app).await;
            }
            if let Some((target, text)) = app.playground.row_text(requests) {
                app.playground.begin_edit(target, text);
            }
        }
        CRow::Request(i) => {
            if let Some(sid) = recipe(app, id).map(|r| r.steps[i].id.clone()) {
                select_tree(app, &TreeRow::Request(id, sid.clone()));
                open_request(app, id, &sid).await;
            }
        }
        CRow::AddRequest => new_request(app, "list_panes", serde_json::json!({})).await,
        CRow::AddHttp => {
            new_request(
                app,
                HTTP_REQUEST,
                serde_json::json!({ "method": "GET", "url": "https://" }),
            )
            .await
        }
        CRow::AddLlm => new_request(app, LLM_CALL, serde_json::json!({ "model": "" })).await,
        CRow::RunAll => run_collection(app),
        CRow::AddVar | CRow::AddTrigger => {
            if row == CRow::AddTrigger {
                load_schema(app).await;
            }
            let Some(c) = app.playground.collection.as_mut() else {
                return true;
            };
            let (added, target) = if row == CRow::AddVar {
                c.vars.push((String::new(), String::new()));
                let i = c.vars.len() - 1;
                (CRow::Var(i), Target::Var(i))
            } else {
                c.triggers.push(RecipeTrigger {
                    on: "pane.agent_status_changed".into(),
                    when: serde_json::Map::new(),
                });
                let i = c.triggers.len() - 1;
                (CRow::Trigger(i), Target::Trigger(i))
            };
            c.changed = true;
            c.row = c
                .rows(requests)
                .iter()
                .position(|r| *r == added)
                .unwrap_or(0);
            let text = match target {
                Target::Trigger(i) => trigger_text(&c.triggers[i]),
                _ => String::new(),
            };
            app.playground.begin_edit(target, text);
        }
        CRow::Save => save_collection(app).await,
        CRow::Delete => {
            if confirm {
                delete_collection(app).await;
            } else if let Some(c) = app.playground.collection.as_mut() {
                c.confirm_delete = true;
            }
        }
    }
    true
}

/// How the form edits param `name`: its schema type and enum choices.
fn param_kind(app: &App, name: &str) -> (String, Vec<String>) {
    let pg = &app.playground;
    let Some(r) = pg.request.as_ref() else {
        return ("string".into(), Vec::new());
    };
    let (props, _) = r.schema(&pg.tools, &pg.methods);
    let prop = props.get(name);
    let variants = prop
        .and_then(|p| p.get("enum"))
        .and_then(Value::as_array)
        .map(|a| a.iter().map(display_value).collect())
        .unwrap_or_default();
    let kind = prop.map(herdr_mcp_api::schema_type).unwrap_or("string");
    (kind.to_string(), variants)
}

/// Space on a boolean param: flip it.
fn toggle_param(r: &mut RequestForm, name: &str) {
    let on = r.values.get(name).is_some_and(|v| v == "true");
    r.values
        .insert(name.to_string(), if on { "false" } else { "true" }.into());
}

/// ←/→ on an enum param: the next choice (blank = unset) that way.
fn cycle_param(r: &mut RequestForm, name: &str, variants: Vec<String>, forward: bool) {
    let mut choices = vec![String::new()];
    choices.extend(variants);
    let cur = r.values.get(name).cloned().unwrap_or_default();
    let at = choices.iter().position(|v| *v == cur).unwrap_or(0);
    let n = choices.len();
    let next = if forward {
        (at + 1) % n
    } else {
        (at + n - 1) % n
    };
    r.values.insert(name.to_string(), choices[next].clone());
}

async fn handle_request(app: &mut App, code: KeyCode, mods: KeyModifiers) -> bool {
    let pg = &app.playground;
    let Some(r) = pg.request.as_ref() else {
        return false;
    };
    let rows = r.rows(&pg.tools, &pg.methods);
    let (at, confirm) = (r.row, r.confirm_delete);
    let row = rows.get(at).cloned();
    if let Some(next) = moved(at, rows.len(), code) {
        if let Some(r) = app.playground.request.as_mut() {
            r.row = next;
            r.confirm_delete = false;
        }
        return true;
    }
    let kind = match &row {
        Some(RRow::Param(name)) => Some((name.clone(), param_kind(app, name))),
        _ => None,
    };
    let choice = kind
        .as_ref()
        .is_some_and(|(_, (k, v))| k == "boolean" || !v.is_empty());
    let Some(r) = app.playground.request.as_mut() else {
        return false;
    };
    match code {
        KeyCode::Esc => {
            app.playground.focus = Focus::Tree;
            return true;
        }
        KeyCode::Char('d') if mods.contains(KeyModifiers::CONTROL) => {
            match row {
                Some(RRow::Extract(i)) => {
                    r.extract.remove(i);
                }
                Some(RRow::Param(name)) => {
                    r.values.remove(&name);
                    preview(app).await;
                }
                _ => {}
            }
            return true;
        }
        // Space and Enter step a choice field; Space flips an extract scope.
        KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Left | KeyCode::Right if choice => {
            let Some((name, (k, variants))) = kind else {
                return true;
            };
            if k == "boolean" {
                if !matches!(code, KeyCode::Left | KeyCode::Right) {
                    toggle_param(r, &name);
                }
            } else {
                cycle_param(r, &name, variants, code != KeyCode::Left);
            }
            preview(app).await;
            return true;
        }
        KeyCode::Char(' ') if matches!(row, Some(RRow::Extract(_))) => {
            if let Some(RRow::Extract(i)) = row {
                let x = &mut r.extract[i];
                x.scope = match x.scope {
                    VarScope::Global => VarScope::Collection,
                    VarScope::Collection => VarScope::Global,
                };
            }
            return true;
        }
        _ => {}
    }
    if let Some(ch) = typed(code, mods) {
        let text_row = matches!(
            row,
            Some(RRow::Param(_) | RRow::Id | RRow::Description | RRow::Extract(_))
        );
        if text_row && let Some((target, text)) = app.playground.row_text(0) {
            app.playground.begin_edit(target, format!("{text}{ch}"));
            return true;
        }
        return false;
    }
    if code != KeyCode::Enter {
        return false;
    }
    let pg = &mut app.playground;
    match row {
        Some(RRow::Tool) => {
            pg.overlay = Some(Overlay::Tools {
                search: String::new(),
                index: 0,
            })
        }
        Some(RRow::Method) => {
            pg.overlay = Some(Overlay::Methods {
                search: String::new(),
                index: 0,
            })
        }
        Some(RRow::Param(_) | RRow::Id | RRow::Description | RRow::Extract(_)) => {
            if let Some((target, text)) = pg.row_text(0) {
                pg.begin_edit(target, text);
            }
        }
        Some(RRow::AddExtract) => {
            let Some(r) = pg.request.as_mut() else {
                return true;
            };
            r.extract.push(Extract {
                path: String::new(),
                var: String::new(),
                scope: VarScope::Global,
            });
            let i = r.extract.len() - 1;
            r.row = r
                .rows(&pg.tools, &pg.methods)
                .iter()
                .position(|x| *x == RRow::Extract(i))
                .unwrap_or(0);
            pg.begin_edit(Target::Extract(i), " -> ".into());
            if let Some(e) = pg.editing.as_mut() {
                e.area.move_cursor(tui_textarea::CursorMove::Head);
            }
        }
        Some(RRow::Send) => send(app).await,
        Some(RRow::Save) => {
            save_request(app).await;
        }
        Some(RRow::Delete) => {
            if confirm {
                delete_request(app).await;
            } else if let Some(r) = pg.request.as_mut() {
                r.confirm_delete = true;
            }
        }
        None => {}
    }
    true
}

/// The response text: arrows scroll, Shift+arrows select.
fn handle_response(app: &mut App, code: KeyCode, mods: KeyModifiers) -> bool {
    match code {
        KeyCode::Esc => {
            app.playground.focus = Focus::Main;
            true
        }
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown => {
            if let Some(input) = crate::to_textarea_input(code, mods) {
                app.playground.result_area.input(input);
            }
            true
        }
        _ => false,
    }
}

/// The result paths: Enter adds an extract rule for the picked one.
fn handle_paths(app: &mut App, code: KeyCode) -> bool {
    let pg = &mut app.playground;
    let Some(r) = pg.request.as_mut() else {
        return false;
    };
    if let Some(next) = moved(r.path_index, r.paths.len(), code) {
        r.path_index = next;
        return true;
    }
    match code {
        KeyCode::Esc => pg.focus = Focus::Main,
        KeyCode::Enter => {
            if let Some((path, _)) = r.paths.get(r.path_index).cloned() {
                let var = var_name(&path);
                r.extract.push(Extract {
                    path: path.clone(),
                    var: var.clone(),
                    scope: VarScope::Global,
                });
                pg.ok(format!("extract {path} → {var} added (Ctrl+S saves)"));
            }
        }
        _ => return false,
    }
    true
}

/// Keys while a field is being typed into: Enter or Esc ends the edit.
async fn handle_edit(app: &mut App, code: KeyCode, mods: KeyModifiers) -> bool {
    // Ctrl+C / Ctrl+V / Ctrl+X fall through to the clipboard.
    if mods.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c' | 'v' | 'x')) {
        return false;
    }
    if !matches!(code, KeyCode::Enter | KeyCode::Esc) {
        if let Some(e) = app.playground.editing.as_mut()
            && let Some(input) = crate::to_textarea_input(code, mods)
        {
            e.area.input(input);
        }
        return true;
    }
    let Some(Editing { target, area }) = app.playground.editing.take() else {
        return true;
    };
    commit(app, target, area.lines().join("\n")).await;
    true
}

/// Write an edited field back to its form.
async fn commit(app: &mut App, target: Target, text: String) {
    let pg = &mut app.playground;
    match target {
        Target::TemplateVar(i) => {
            if let Some(Overlay::Template { values, .. }) = pg.overlay.as_mut() {
                values[i] = text;
            }
        }
        Target::CollectionName | Target::Folder | Target::Var(_) | Target::Trigger(_) => {
            let Some(c) = pg.collection.as_mut() else {
                return;
            };
            match target {
                Target::CollectionName => c.name = text,
                Target::Folder => c.folder = text,
                Target::Var(i) => c.vars[i] = parse_var(&text),
                Target::Trigger(i) => c.triggers[i] = parse_trigger(&text),
                _ => unreachable!(),
            }
            c.changed = true;
        }
        Target::RequestId | Target::Description | Target::Param(_) | Target::Extract(_) => {
            let Some(r) = pg.request.as_mut() else {
                return;
            };
            match target {
                Target::RequestId => r.id = text.trim().to_string(),
                Target::Description => r.description = text,
                Target::Param(name) => {
                    if text.is_empty() {
                        r.values.remove(&name);
                    } else {
                        r.values.insert(name, text);
                    }
                    preview(app).await;
                }
                Target::Extract(i) => {
                    let scope = r.extract[i].scope;
                    r.extract[i] = parse_extract(&text, scope);
                }
                _ => unreachable!(),
            }
        }
    }
}

// ── overlays ───────────────────────────────────────────────────────────

/// Tools matching `search`, by category then name.
pub fn tool_choices<'a>(tools: &'a [ToolInfo], search: &str) -> Vec<&'a ToolInfo> {
    let needle = search.to_lowercase();
    let mut out: Vec<&ToolInfo> = tools
        .iter()
        .filter(|t| {
            needle.is_empty()
                || t.name.to_lowercase().contains(&needle)
                || t.category.to_lowercase().contains(&needle)
        })
        .collect();
    out.sort_by(|a, b| a.category.cmp(&b.category).then(a.name.cmp(&b.name)));
    out
}

/// herdr methods matching `search`, in schema order.
pub fn method_choices<'a>(methods: &'a [Method], search: &str) -> Vec<&'a str> {
    let needle = search.to_lowercase();
    methods
        .iter()
        .map(|m| m.name.as_str())
        .filter(|n| n.contains(&needle))
        .collect()
}

async fn handle_overlay(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    let Some(overlay) = app.playground.overlay.take() else {
        return;
    };
    if code == KeyCode::Esc {
        return;
    }
    let pg = &mut app.playground;
    let next = match overlay {
        Overlay::Tools { mut search, index } => {
            let len = tool_choices(&pg.tools, &search).len();
            match code {
                KeyCode::Enter => {
                    let tool = tool_choices(&pg.tools, &search)
                        .get(index)
                        .map(|t| t.name.clone());
                    if let (Some(tool), Some(r)) = (tool, pg.request.as_mut())
                        && tool != r.tool
                    {
                        r.tool = tool;
                        r.values.clear();
                        if r.herdr() {
                            load_schema(app).await;
                        }
                        preview(app).await;
                    }
                    None
                }
                KeyCode::Backspace => {
                    search.pop();
                    Some(Overlay::Tools { search, index: 0 })
                }
                _ => match (moved(index, len, code), typed(code, mods)) {
                    (Some(index), _) => Some(Overlay::Tools { search, index }),
                    (None, Some(c)) => {
                        search.push(c);
                        Some(Overlay::Tools { search, index: 0 })
                    }
                    _ => Some(Overlay::Tools { search, index }),
                },
            }
        }
        Overlay::Methods { mut search, index } => {
            let len = method_choices(&pg.methods, &search).len();
            match code {
                KeyCode::Enter => {
                    let method = method_choices(&pg.methods, &search)
                        .get(index)
                        .map(|m| m.to_string());
                    if let (Some(method), Some(r)) = (method, pg.request.as_mut())
                        && method != r.method
                    {
                        r.method = method;
                        r.values.clear();
                        preview(app).await;
                    }
                    None
                }
                KeyCode::Backspace => {
                    search.pop();
                    Some(Overlay::Methods { search, index: 0 })
                }
                _ => match (moved(index, len, code), typed(code, mods)) {
                    (Some(index), _) => Some(Overlay::Methods { search, index }),
                    (None, Some(c)) => {
                        search.push(c);
                        Some(Overlay::Methods { search, index: 0 })
                    }
                    _ => Some(Overlay::Methods { search, index }),
                },
            }
        }
        Overlay::Templates { index } => {
            let templates = app.templates.clone().unwrap_or_default();
            match (code, moved(index, templates.len(), code)) {
                (_, Some(index)) => Some(Overlay::Templates { index }),
                (KeyCode::Enter, _) => templates.get(index).map(|t| Overlay::Template {
                    values: t
                        .variables
                        .iter()
                        .map(|v| {
                            v.default_value
                                .as_ref()
                                .map(display_value)
                                .unwrap_or_default()
                        })
                        .collect(),
                    template: t.clone(),
                    row: 0,
                    error: None,
                }),
                _ => Some(Overlay::Templates { index }),
            }
        }
        Overlay::Template {
            template,
            values,
            row,
            error,
        } => {
            let vars = template.variables.len();
            match (code, moved(row, vars + 2, code)) {
                (_, Some(row)) => Some(Overlay::Template {
                    template,
                    values,
                    row,
                    error,
                }),
                (KeyCode::Enter, _) if row < vars => {
                    let text = values[row].clone();
                    pg.overlay = Some(Overlay::Template {
                        template,
                        values,
                        row,
                        error,
                    });
                    pg.begin_edit(Target::TemplateVar(row), text);
                    return;
                }
                (KeyCode::Enter, _) if row == vars => {
                    match instantiate(app, &template, &values).await {
                        Ok(()) => None,
                        Err(e) => Some(Overlay::Template {
                            template,
                            values,
                            row,
                            error: Some(e),
                        }),
                    }
                }
                (KeyCode::Enter, _) => None,
                _ => Some(Overlay::Template {
                    template,
                    values,
                    row,
                    error,
                }),
            }
        }
    };
    app.playground.overlay = next;
}

/// Fill in `template` with `values` and save it as a new collection.
async fn instantiate(
    app: &mut App,
    template: &RecipeTemplate,
    values: &[String],
) -> Result<(), String> {
    if let Some(v) = template
        .variables
        .iter()
        .zip(values)
        .find(|(v, val)| v.required && val.trim().is_empty())
    {
        return Err(format!("{} is required", v.0.name));
    }
    let http = app.http.clone().ok_or("bridge not connected")?;
    let req = InstantiateTemplateRequest {
        variables: template
            .variables
            .iter()
            .zip(values)
            .filter(|(_, val)| !val.trim().is_empty())
            .map(|(v, val)| (v.name.clone(), Value::String(val.clone())))
            .collect(),
        name: Some(template.name.clone()),
    };
    let made = http
        .instantiate_template(&template.id, &req)
        .await
        .map_err(|e| format!("could not fill in the template: {e}"))?;
    create_collection(app, made.name, made.steps).await;
    Ok(())
}

// ── actions ────────────────────────────────────────────────────────────

/// Ctrl+S: save what the main panel shows.
async fn save(app: &mut App) {
    match app.playground.pick {
        Pick::Collection(_) => save_collection(app).await,
        Pick::Request(..) => {
            save_request(app).await;
        }
        Pick::Nothing => {}
    }
}

/// Ctrl+G: send the request, or run the collection.
async fn go(app: &mut App) {
    match app.playground.pick {
        Pick::Collection(_) => run_collection(app),
        Pick::Request(..) => send(app).await,
        Pick::Nothing => {}
    }
}

async fn save_collection(app: &mut App) {
    let Some(http) = client(app) else {
        return;
    };
    let Some(c) = app.playground.collection.as_ref() else {
        return;
    };
    let id = c.id;
    let req = UpdateRecipeRequest {
        name: Some(c.name.trim().to_string()).filter(|n| !n.is_empty()),
        category: Some(c.folder.trim().to_string()),
        variables: Some(
            c.vars
                .iter()
                .filter(|(k, _)| !k.trim().is_empty())
                .map(|(k, v)| (k.trim().to_string(), parse_value(v)))
                .collect(),
        ),
        triggers: Some(
            c.triggers
                .iter()
                .filter(|t| !t.on.is_empty())
                .cloned()
                .collect(),
        ),
        ..Default::default()
    };
    match http.update_recipe(id, &req).await {
        Ok(_) => {
            reload_recipes(app).await;
            if let Some(c) = app.playground.collection.as_mut() {
                c.changed = false;
            }
            select_tree(app, &TreeRow::Collection(id));
            app.playground.ok("saved");
        }
        Err(e) => app.playground.err(format!("save failed: {e}")),
    }
}

async fn delete_collection(app: &mut App) {
    let Some(http) = client(app) else {
        return;
    };
    let Some(id) = app.playground.collection.as_ref().map(|c| c.id) else {
        return;
    };
    match http.delete_recipe(id).await {
        Ok(()) => {
            reload_recipes(app).await;
            let pg = &mut app.playground;
            pg.collection = None;
            pg.pick = Pick::Nothing;
            pg.focus = Focus::Tree;
            pg.ok("collection deleted");
        }
        Err(e) => app.playground.err(format!("delete failed: {e}")),
    }
}

/// Swap requests `i` and `j` of the open collection.
async fn reorder(app: &mut App, i: usize, j: usize) {
    let Some(http) = client(app) else {
        return;
    };
    let Some(id) = app.playground.collection.as_ref().map(|c| c.id) else {
        return;
    };
    let Some(mut steps) = recipe(app, id).map(|r| r.steps.clone()) else {
        return;
    };
    steps.swap(i, j);
    match set_steps(&http, id, steps).await {
        Ok(()) => reload_recipes(app).await,
        Err(e) => app.playground.err(e),
    }
}

/// Add a `tool` request to the open collection and open it.
async fn new_request(app: &mut App, tool: &str, params: Value) {
    let Some(http) = client(app) else {
        return;
    };
    let Some(id) = app.playground.collection.as_ref().map(|c| c.id) else {
        return;
    };
    match add_request(&http, id, tool, params).await {
        Ok(sid) => {
            reload_recipes(app).await;
            select_tree(app, &TreeRow::Request(id, sid.clone()));
            open_request(app, id, &sid).await;
        }
        Err(e) => app.playground.err(e),
    }
}

/// Save the open request; `false` (and says why) if that failed.
async fn save_request(app: &mut App) -> bool {
    let Some(http) = client(app) else {
        return false;
    };
    let pg = &app.playground;
    let Some(r) = pg.request.as_ref() else {
        return false;
    };
    let (rid, saved_id) = (r.rid, r.saved_id.clone());
    let step = r.step(&pg.tools, &pg.methods);
    let new_id = step.id.clone();
    match save_step(&http, rid, &saved_id, step).await {
        Ok(()) => {
            reload_recipes(app).await;
            if let Some(r) = app.playground.request.as_mut() {
                r.saved_id = new_id.clone();
                r.id = new_id.clone();
            }
            app.playground.pick = Pick::Request(rid, new_id.clone());
            select_tree(app, &TreeRow::Request(rid, new_id));
            app.playground.ok("saved");
            true
        }
        Err(e) => {
            app.playground.err(e);
            false
        }
    }
}

async fn delete_request(app: &mut App) {
    let Some(http) = client(app) else {
        return;
    };
    let Some((rid, sid)) = app
        .playground
        .request
        .as_ref()
        .map(|r| (r.rid, r.saved_id.clone()))
    else {
        return;
    };
    match delete_step(&http, rid, &sid).await {
        Ok(()) => {
            reload_recipes(app).await;
            select_tree(app, &TreeRow::Collection(rid));
            open_collection(app, rid);
            app.playground.ok(format!("request {sid} deleted"));
        }
        Err(e) => app.playground.err(e),
    }
}

/// `false` (and says so) if a send or run is already going.
fn idle(app: &mut App) -> bool {
    if app.job.is_some() {
        app.playground.err("still running the last request…");
        return false;
    }
    true
}

/// Start `req` and poll it to the end. Polling, not `?wait=true`, so a run
/// longer than the client's request timeout isn't reported as failed.
fn spawn_run(
    http: ApiClient,
    req: StartRunRequest,
) -> tokio::task::JoinHandle<Result<RunRecord, String>> {
    tokio::spawn(async move {
        let mut run = http.start_run(&req).await.map_err(|e| e.to_string())?;
        while !run.status.is_finished() {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            run = http.get_run(run.id).await.map_err(|e| e.to_string())?;
        }
        Ok(run)
    })
}

/// Save the request, then run just it.
async fn send(app: &mut App) {
    if !idle(app) || !save_request(app).await {
        return;
    }
    let Some(http) = client(app) else {
        return;
    };
    let Some((rid, sid)) = app
        .playground
        .request
        .as_ref()
        .map(|r| (r.rid, r.saved_id.clone()))
    else {
        return;
    };
    let req = StartRunRequest {
        recipe_id: Some(rid),
        step_ids: vec![sid.clone()],
        ..Default::default()
    };
    app.job = Some(Job::Send(rid, sid.clone(), spawn_run(http, req)));
    app.playground.ok(format!("sending {sid}…"));
}

fn run_collection(app: &mut App) {
    if !idle(app) {
        return;
    }
    let Some(http) = client(app) else {
        return;
    };
    let Some(id) = app.playground.collection.as_ref().map(|c| c.id) else {
        return;
    };
    let req = StartRunRequest {
        recipe_id: Some(id),
        ..Default::default()
    };
    app.job = Some(Job::Run(id, spawn_run(http, req)));
    app.playground.ok("running the collection…");
}

/// Show a finished [`Job`].
pub(crate) async fn finish_job(app: &mut App, job: Job) {
    let joined = |e: tokio::task::JoinError| format!("request task failed: {e}");
    let (rid, sent, result) = match job {
        Job::Send(rid, sid, task) => (rid, Some(sid), task.await.map_err(joined).and_then(|r| r)),
        Job::Run(rid, task) => (rid, None, task.await.map_err(joined).and_then(|r| r)),
    };
    // Extract rules may have written variables.
    reload_recipes(app).await;
    if let Some(http) = app.http.clone()
        && let Ok(vars) = http.list_variables().await
    {
        app.variables_state.entries = crate::parse_variables(&vars);
    }
    let fresh_vars = recipe(app, rid).map(|r| sorted_vars(&r.variables));
    let pg = &mut app.playground;
    let run = match result {
        Ok(run) => run,
        Err(e) => {
            pg.err(if sent.is_some() {
                format!("send failed: {e}")
            } else {
                format!("run failed: {e}")
            });
            return;
        }
    };
    let status = format!("{:?}", run.status).to_lowercase();
    match run.status {
        RunStatus::Completed => pg.ok(status),
        _ => pg.err(run.error.clone().unwrap_or(status)),
    }
    if let Some(sid) = sent {
        // Only into the request it was sent from, if that is still open.
        if let Some(r) = pg
            .request
            .as_mut()
            .filter(|r| r.rid == rid && r.saved_id == sid)
        {
            let text = send_text(&run, &r.saved_id, &r.extract);
            r.paths = result_paths(&send_payload(&run, &r.saved_id), MAX_PATHS);
            r.path_index = 0;
            r.response = Some(run);
            pg.result_area = TextArea::new(text);
        }
    } else if let Some(c) = pg.collection.as_mut().filter(|c| c.id == rid) {
        let order: Vec<String> = app
            .recipes
            .as_ref()
            .and_then(|rs| rs.iter().find(|r| r.id == rid))
            .map(|r| r.steps.iter().map(|s| s.id.clone()).collect())
            .unwrap_or_default();
        pg.result_area = TextArea::new(run_text(&run, &order));
        if !c.changed
            && let Some(vars) = fresh_vars
        {
            c.vars = vars;
        }
        c.run = Some(run);
    }
}

/// The tool's response to `step_id`, parsed when it is JSON.
fn send_payload(run: &RunRecord, step_id: &str) -> Value {
    run.results
        .get(step_id)
        .filter(|r| r.get("error").is_none())
        .and_then(herdr_mcp_api::tool_payload)
        .unwrap_or(Value::Null)
}

/// One step's result as text: its JSON payload pretty, else the tool's
/// text, else the error.
fn result_lines(result: &Value) -> Vec<String> {
    if let Some(e) = result.get("error").and_then(Value::as_str) {
        return vec![format!("error: {e}")];
    }
    let body = match herdr_mcp_api::tool_payload(result) {
        Some(v) => serde_json::to_string_pretty(&v).unwrap_or_default(),
        None => herdr_mcp_api::tool_text(result),
    };
    body.lines().map(str::to_string).collect()
}

/// A send's outcome: status, error, the variables its extract rules set,
/// then the tool's response.
fn send_text(run: &RunRecord, step_id: &str, rules: &[Extract]) -> Vec<String> {
    let mut out = vec![format!("Sent · {:?}", run.status).to_lowercase()];
    if let Some(e) = &run.error {
        out.push(format!("error: {e}"));
    }
    if run.status == RunStatus::Completed {
        let set: Vec<String> = rules
            .iter()
            .filter_map(|x| {
                let v = run.variables.get(&x.var)?;
                let scope = match x.scope {
                    VarScope::Global => "global",
                    VarScope::Collection => "collection",
                };
                Some(format!("  {} = {} ({scope})", x.var, display_value(v)))
            })
            .collect();
        if !set.is_empty() {
            out.push("Variables set:".into());
            out.extend(set);
        }
    }
    if let Some(result) = run.results.get(step_id) {
        out.push(String::new());
        out.extend(result_lines(result));
    }
    out
}

/// A collection run: status, then each step's result in step order.
fn run_text(run: &RunRecord, order: &[String]) -> Vec<String> {
    let mut out = vec![format!("Run · {:?}", run.status).to_lowercase()];
    if let Some(e) = &run.error {
        out.push(format!("error: {e}"));
    }
    for (n, id) in order.iter().enumerate() {
        let Some(result) = run.results.get(id) else {
            continue;
        };
        out.push(String::new());
        out.push(format!("#{} {id}", n + 1));
        out.extend(result_lines(result));
    }
    out
}

// ── clipboard ──────────────────────────────────────────────────────────

/// The focused field's text, for Copy without a selection.
pub fn focused_text(app: &App) -> Option<String> {
    let pg = &app.playground;
    if let Some(area) = pg.edit_area() {
        return Some(area.lines().join("\n"));
    }
    pg.row_text(request_count(app)).map(|(_, text)| text)
}

/// Paste into the focused text field, starting an edit if none is going.
pub fn paste(app: &mut App, text: String) {
    let requests = request_count(app);
    let pg = &mut app.playground;
    if pg.editing.is_none() {
        let Some((target, cur)) = pg.row_text(requests) else {
            app.status_msg = "no field to paste into".into();
            return;
        };
        pg.begin_edit(target, cur);
    }
    if let Some(area) = pg.edit_area_mut() {
        area.set_yank_text(text);
        area.paste();
    }
}

/// Cut without an edit going: the focused field's text, which is cleared
/// (an edit opens on the empty field, so Enter keeps the cut).
pub fn cut(app: &mut App) -> Option<String> {
    let requests = request_count(app);
    let pg = &mut app.playground;
    let (target, text) = pg.row_text(requests)?;
    pg.begin_edit(target, String::new());
    Some(text)
}

// ── mouse ──────────────────────────────────────────────────────────────

fn inside(area: Rect, m: &MouseEvent) -> bool {
    m.column >= area.x && m.column < area.right() && m.row >= area.y && m.row < area.bottom()
}

/// The list row under the mouse in a list drawn at `area` with `state`.
fn row_at(area: Rect, state: &ListState, m: &MouseEvent) -> usize {
    state.offset() + (m.row - area.y) as usize
}

/// Clicks pick tree rows, form rows and paths; the wheel moves the
/// selection. Returns `true` when the event was the playground's.
pub async fn handle_mouse(app: &mut App, m: MouseEvent) -> bool {
    if app.playground.view == View::Events {
        return events::handle_mouse(&mut app.playground.events, m);
    }
    if app.playground.overlay.is_some() || app.playground.editing.is_some() {
        return false;
    }
    let pg = &app.playground;
    let (tree, main, result, paths) = (
        pg.tree_inner,
        pg.main_inner,
        pg.result_inner,
        pg.paths_inner,
    );
    let wheel = match m.kind {
        MouseEventKind::ScrollDown => Some(KeyCode::Down),
        MouseEventKind::ScrollUp => Some(KeyCode::Up),
        _ => None,
    };
    if let Some(key) = wheel {
        let focus = [
            (tree, Focus::Tree),
            (main, Focus::Main),
            (result, Focus::Response),
            (paths, Focus::Paths),
        ]
        .into_iter()
        .find(|(area, _)| inside(*area, &m))
        .map(|(_, f)| f);
        let Some(focus) = focus else {
            return false;
        };
        let saved = app.playground.focus;
        app.playground.focus = focus;
        let _ = Box::pin(handle_key(app, key, KeyModifiers::NONE)).await;
        if focus != Focus::Response {
            app.playground.focus = saved;
        }
        return true;
    }
    if m.kind != MouseEventKind::Down(MouseButton::Left) {
        return false;
    }
    if inside(tree, &m) {
        let i = row_at(tree, &app.playground.tree_list, &m);
        let rows = tree_rows(app.recipes.as_deref().unwrap_or_default());
        if i < rows.len() && !matches!(rows[i], TreeRow::Folder(_)) {
            app.playground.tree = i;
            app.playground.focus = Focus::Tree;
            activate_tree(app, i).await;
        }
        return true;
    }
    if inside(main, &m) {
        let line = row_at(main, &app.playground.main_list, &m);
        let Some(Some(row)) = app.playground.main_lines.get(line).copied() else {
            return true;
        };
        app.playground.focus = Focus::Main;
        if let Some(c) = app.playground.collection.as_mut() {
            c.row = row;
        }
        if let Some(r) = app.playground.request.as_mut() {
            r.row = row;
        }
        return true;
    }
    if inside(result, &m) {
        app.playground.focus = Focus::Response;
        return true;
    }
    if inside(paths, &m) {
        let i = row_at(paths, &app.playground.paths_list, &m);
        app.playground.focus = Focus::Paths;
        if let Some(r) = app.playground.request.as_mut()
            && i < r.paths.len()
        {
            r.path_index = i;
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_parse_back_from_their_text() {
        let t = parse_trigger("pane.agent_status_changed agent_status=done, focused=true");
        assert_eq!(t.on, "pane.agent_status_changed");
        assert_eq!(t.when["agent_status"], "done");
        assert_eq!(t.when["focused"], true);
        assert_eq!(parse_trigger(&trigger_text(&t)), t);
        assert!(parse_trigger("tab.created").when.is_empty());

        let x = parse_extract(" panes[0].pane_id -> pane ", VarScope::Collection);
        assert_eq!(
            (x.path.as_str(), x.var.as_str()),
            ("panes[0].pane_id", "pane")
        );
        assert_eq!(x.scope, VarScope::Collection);
        assert_eq!(parse_extract(&extract_text(&x), x.scope), x);

        assert_eq!(parse_var("n = 3"), ("n".into(), "3".into()));
        assert_eq!(parse_var("url=a=b"), ("url".into(), "a=b".into()));
        assert_eq!(parse_var("x"), ("x".into(), String::new()));
    }

    #[test]
    fn tree_lists_folders_collections_and_requests() {
        let mk = |name: &str, folder: Option<&str>, steps: &[&str]| -> Recipe {
            let steps: Vec<Value> = steps
                .iter()
                .map(|s| serde_json::json!({ "id": s, "tool": "status", "params": {}, "description": null }))
                .collect();
            serde_json::from_value(serde_json::json!({
                "id": Uuid::new_v4(), "name": name, "description": null, "steps": steps,
                "variables": {}, "created_at": "2026-10-24T00:00:00Z",
                "updated_at": "2026-10-24T00:00:00Z", "is_template": false,
                "category": folder
            }))
            .unwrap()
        };
        let a = mk("a", None, &["r1", "r2"]);
        let b = mk("b", Some("smoke"), &[]);
        let rows = tree_rows(&[b.clone(), a.clone()]);
        assert_eq!(
            rows,
            [
                TreeRow::Globals,
                TreeRow::NewCollection,
                TreeRow::FromTemplate,
                TreeRow::Collection(a.id),
                TreeRow::Request(a.id, "r1".into()),
                TreeRow::Request(a.id, "r2".into()),
                TreeRow::Folder("smoke".into()),
                TreeRow::Collection(b.id),
            ]
        );
    }

    #[test]
    fn request_rows_follow_the_schema() {
        let tools = tool_list(&[serde_json::json!({
            "name": "read_pane",
            "inputSchema": { "properties": {
                "pane_id": { "type": "string" },
                "lines": { "type": "integer" },
                "strip": { "type": "boolean" }
            }, "required": ["pane_id"] }
        })]);
        let step = RecipeStep {
            id: "read".into(),
            tool: "read_pane".into(),
            params: serde_json::json!({ "lines": 5, "legacy": "x" }),
            ..Default::default()
        };
        let form = RequestForm::new(Uuid::nil(), &step);
        let rows = form.rows(&tools, &[]);
        let params: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                RRow::Param(p) => Some(p.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(params, ["pane_id", "lines", "strip", "legacy"]);
        assert!(!rows.contains(&RRow::Method));
        assert_eq!(
            form.params(&tools, &[]),
            serde_json::json!({ "lines": 5, "legacy": "x" })
        );
    }
}

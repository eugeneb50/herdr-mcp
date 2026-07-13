//! Kitchen-sink TUI dashboard for herdr-mcp.
//!
//! VS Code-style tabbed interface running directly in a terminal (and a herdr
//! pane). Supports mouse + keyboard navigation. Tabs: Overview, Playground
//! (tool runner + recipe builder), Trim, Variables, Settings.
//!
//! Rendered with `ratatui` (Frame/Block/Layout widgets) over a crossterm
//! backend, following the patterns used by the sibling `herdr` and `zerocode`
//! dashboards. The dashboard talks to the herdr-mcp HTTP bridge over `reqwest`
//! (see `http`). It also reads herdr workspace/pane context via the `herdr`
//! CLI for the sidecar header.
//!
//! Entry point: [`run`].

pub mod http;
pub mod tabs;
pub mod theme;

use std::io;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Terminal, backend::CrosstermBackend};
use serde_json::Value;
use tui_textarea::{Input as TaInput, Key as TaKey, TextArea};

use std::io::Write;

/// Append a debug line to `<data_dir>/tui-debug.log` AND echo it on stderr.
///
/// The TUI owns the terminal, so stderr lands in the spawning pane's
/// scrollback while the file gives a durable trace. This is the primary
/// runtime-diagnostics channel for clipboard / result-selection behavior
/// (which otherwise fails silently inside async futures).
fn log_debug(data_dir: &std::path::Path, msg: impl AsRef<str>) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let line = format!("[{}] {}\n", ts, msg.as_ref());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir.join("tui-debug.log"))
    {
        let _ = f.write_all(line.as_bytes());
    }
    eprint!("{line}");
}

use crate::stats;

use http::HttpClient;
use theme::{accent_style, dim_style, muted_style, title_style};

/// Available tabs, mirroring the legacy web app routes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Overview,
    Playground,
    Trim,
    Variables,
    Settings,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Overview,
        Tab::Playground,
        Tab::Trim,
        Tab::Variables,
        Tab::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Playground => "Playground",
            Tab::Trim => "Trim",
            Tab::Variables => "Variables",
            Tab::Settings => "Settings",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Tab::Overview => "1",
            Tab::Playground => "2",
            Tab::Trim => "3",
            Tab::Variables => "4",
            Tab::Settings => "5",
        }
    }
}

/// A single editable field parsed from a tool's JSON `inputSchema`.
#[derive(Clone, Debug)]
pub enum FieldKind {
    Text,
    Number,
    Boolean,
    Enum,
}

#[derive(Clone, Debug)]
pub struct ToolField {
    pub name: String,
    pub label: String,
    pub kind: FieldKind,
    pub required: bool,
    pub default: Option<String>,
    pub enum_variants: Option<Vec<String>>,
    pub description: Option<String>,
}

impl ToolField {
    /// Render the current value for display in the form.
    pub fn display_value(&self, value: &str) -> String {
        if value.is_empty() && self.default.is_none() {
            return String::new();
        }
        value.to_string()
    }
}

/// Herdr sidecar context resolved from the `herdr` CLI.
#[derive(Default, Clone)]
pub struct HerdrContext {
    pub workspace: Option<String>,
    pub pane_id: Option<String>,
    pub pane_count: usize,
}

/// Right-click context-menu actions for editing fields / panes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContextAction {
    Copy,
    Cut,
    Paste,
    PasteToPane,
}

impl ContextAction {
    fn label(self) -> &'static str {
        match self {
            ContextAction::Copy => "Copy",
            ContextAction::Cut => "Cut",
            ContextAction::Paste => "Paste",
            ContextAction::PasteToPane => "Paste to pane",
        }
    }

    const ALL: [ContextAction; 4] = [
        ContextAction::Copy,
        ContextAction::Cut,
        ContextAction::Paste,
        ContextAction::PasteToPane,
    ];
}

/// Right-click context menu overlay state.
pub struct ContextMenu {
    pub open: bool,
    pub x: u16,
    pub y: u16,
    pub index: usize,
}

/// Width/height of the context menu overlay (chars/lines).
const MENU_WIDTH: u16 = 18;
const MENU_HEIGHT: u16 = ContextAction::ALL.len() as u16 + 2; // border top + bottom

/// Compute the clamped menu rect given the terminal size.
fn menu_rect(x: u16, y: u16, cols: u16, rows: u16) -> Rect {
    let x = x.min(cols.saturating_sub(MENU_WIDTH));
    let y = y.min(rows.saturating_sub(MENU_HEIGHT));
    Rect::new(x, y, MENU_WIDTH, MENU_HEIGHT)
}

/// Options for launching the dashboard.
#[derive(Clone)]
pub struct DashboardOptions {
    pub data_dir: std::path::PathBuf,
    pub http_port: u16,
}

/// The top-level interactive application state.
pub struct App {
    pub opts: DashboardOptions,
    pub http: Option<HttpClient>,
    pub tab: Tab,
    pub herdr: HerdrContext,
    pub quitting: bool,
    pub status_msg: String,
    pub last_refresh: Instant,
    /// cached API payloads
    pub tools: Option<Value>,
    pub trim_status: Option<Value>,
    pub variables: Option<Value>,
    pub recipes: Option<Value>,
    pub trim_stats_file: Option<stats::TrimStats>,
    /// playground sub-state
    pub playground: PlaygroundState,
    /// trim sub-state
    pub trim: TrimState,
    /// variables sub-state
    pub variables_state: VariablesState,
    /// settings sub-state
    pub settings: SettingsState,
    /// whether the HTTP bridge is currently reachable
    pub bridge_connected: bool,
    /// layout hit-areas captured during draw for mouse handling
    pub tab_rects: Vec<Rect>,
    pub tool_list_inner: Rect,
    pub var_list_inner: Rect,
    pub result_inner: Rect,
    /// right-click context menu state
    pub context_menu: ContextMenu,
}

/// Playground tab state (tool runner + recipe builder).
pub struct PlaygroundState {
    pub sub_tab: PlaygroundSub,
    pub tool_index: usize,
    pub tools_list: Vec<(String, String)>, // (name, short desc)
    pub editing_field: bool,               // editing focused text/number field
    pub fields: Vec<ToolField>,            // parsed schema for current tool
    pub field_values: Vec<String>,         // current values aligned to `fields`
    pub field_focus: usize,                // focused field index
    pub fields_for_index: Option<usize>,   // tool_index the fields were parsed for
    pub edit_area: TextArea<'static>,      // live editor for the focused field
    pub result_area: TextArea<'static>,    // selectable view of the last result
    pub result_focused: bool,              // result pane has selection focus
    pub result: Option<Value>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlaygroundSub {
    Runner,
    Builder,
}

/// Trim tab state.
pub struct TrimState {
    pub diagnose: Option<Value>,
}

/// Variables tab state.
pub struct VariablesState {
    pub entries: Vec<(String, String)>, // (key, value)
    pub selected: usize,
    pub editing: bool,
    pub edit_key_area: TextArea<'static>,
    pub edit_value_area: TextArea<'static>,
    pub edit_field: EditField,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditField {
    Key,
    Value,
}

/// Settings tab state.
pub struct SettingsState {
    pub http_port: u16,
    pub data_dir: String,
    pub herdr_socket: String,
    pub selected: usize,
}

impl App {
    /// Trace a debug line (see `log_debug`) tagged with this app's data dir.
    fn log_debug(&self, msg: impl AsRef<str>) {
        log_debug(&self.opts.data_dir, msg);
    }

    fn new(opts: DashboardOptions) -> Self {
        let http = HttpClient::new(opts.http_port).ok();
        let http_port = opts.http_port;
        let data_dir = opts.data_dir.display().to_string();
        Self {
            opts,
            http,
            tab: Tab::Overview,
            herdr: HerdrContext::default(),
            quitting: false,
            status_msg: String::new(),
            last_refresh: Instant::now(),
            tools: None,
            trim_status: None,
            variables: None,
            recipes: None,
            trim_stats_file: None,
            playground: PlaygroundState {
                sub_tab: PlaygroundSub::Runner,
                tool_index: 0,
                tools_list: Vec::new(),
                editing_field: false,
                fields: Vec::new(),
                field_values: Vec::new(),
                field_focus: 0,
                fields_for_index: None,
                edit_area: TextArea::default(),
                result_area: TextArea::default(),
                result_focused: false,
                result: None,
                error: None,
            },
            trim: TrimState { diagnose: None },
            variables_state: VariablesState {
                entries: Vec::new(),
                selected: 0,
                editing: false,
                edit_key_area: TextArea::default(),
                edit_value_area: TextArea::default(),
                edit_field: EditField::Key,
            },
            settings: SettingsState {
                http_port,
                data_dir,
                herdr_socket: std::env::var("HERDR_SOCKET_PATH").unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                    format!("{home}/.config/herdr/herdr.sock")
                }),
                selected: 0,
            },
            bridge_connected: false,
            tab_rects: Vec::new(),
            tool_list_inner: Rect::default(),
            var_list_inner: Rect::default(),
            result_inner: Rect::default(),
            context_menu: ContextMenu {
                open: false,
                x: 0,
                y: 0,
                index: 0,
            },
        }
        .tap_debug()
    }

    /// Log startup + the debug-file location, then return self.
    fn tap_debug(self) -> Self {
        let path = self.opts.data_dir.join("tui-debug.log");
        log_debug(
            &self.opts.data_dir,
            format!("App started; debug log -> {}", path.display()),
        );
        self
    }

    async fn refresh(&mut self) {
        self.last_refresh = Instant::now();

        // If we're not connected, try to discover a live bridge first.
        if !self.bridge_connected {
            if let Some(port) = http::HttpClient::discover_bridge(self.opts.http_port).await {
                self.opts.http_port = port;
                if let Some(http) = &mut self.http {
                    http.set_port(port);
                }
                self.bridge_connected = true;
                self.status_msg.clear();
            } else {
                self.bridge_connected = false;
                self.status_msg = format!(
                    "bridge unreachable at :{} — start `herdr-mcp serve`",
                    self.opts.http_port
                );
            }
        }

        if self.bridge_connected
            && let Some(http) = &self.http
        {
            let mut ok = true;
            if self.playground.tools_list.is_empty()
                && let Ok(v) = http.list_tools().await
            {
                self.tools = Some(v.clone());
                self.playground.tools_list = parse_tool_list(&v);
            } else if self.playground.tools_list.is_empty() {
                ok = false;
            }
            if let Ok(v) = http.trim_status().await {
                self.trim_status = Some(v);
            }
            if let Ok(v) = http.list_variables().await {
                self.variables = Some(v.clone());
                self.variables_state.entries = parse_variables(&v);
            }
            if let Ok(v) = http.list_recipes().await {
                self.recipes = Some(v);
            }
            if !ok {
                self.bridge_connected = false;
            }
        }
        // Always read the file-based trim stats as a fallback.
        let sessions = self.opts.data_dir.join("sessions");
        if let Ok(mut entries) = tokio::fs::read_dir(&sessions).await {
            let mut first_ws: Option<String> = None;
            while let Ok(Some(e)) = entries.next_entry().await {
                let fname = e.file_name().to_string_lossy().to_string();
                if fname.ends_with(".trim_stats.json") {
                    let ws = fname.trim_end_matches(".trim_stats.json").to_string();
                    if first_ws.is_none() {
                        first_ws = Some(ws.clone());
                    }
                    if let Some(ref w) = first_ws {
                        self.trim_stats_file =
                            Some(stats::load_stats(&self.opts.data_dir, w).await);
                    }
                    break;
                }
            }
        }
    }

    /// Re-parse the schema of the currently selected tool if needed.
    pub fn sync_tool_fields(&mut self) {
        if self.playground.fields_for_index == Some(self.playground.tool_index) {
            return;
        }
        let tool = self
            .tools
            .as_ref()
            .and_then(|v| v.get("tools"))
            .and_then(|t| t.get(self.playground.tool_index))
            .cloned();
        if let Some(tool) = tool {
            let fields = parse_tool_schema(&tool);
            let values = fields
                .iter()
                .map(|f| f.default.clone().unwrap_or_default())
                .collect();
            self.playground.fields = fields;
            self.playground.field_values = values;
            self.playground.field_focus = 0;
            self.playground.editing_field = false;
            self.playground.fields_for_index = Some(self.playground.tool_index);
        }
    }

    /// Resolve current herdr workspace/pane via the CLI (best-effort).
    async fn refresh_herdr(&mut self) {
        if let Ok(raw) = herdr_cli(&["workspace", "list"]).await
            && let Ok(v) = serde_json::from_str::<Value>(&raw)
        {
            self.herdr.workspace = v
                .get("workspaces")
                .and_then(|w| w.get(0))
                .and_then(|w| w.get("id"))
                .and_then(|i| i.as_str())
                .map(str::to_string);
        }
        if let Ok(raw) = herdr_cli(&["pane", "list"]).await
            && let Ok(v) = serde_json::from_str::<Value>(&raw)
        {
            self.herdr.pane_id = v
                .pointer("/panes/0/id")
                .and_then(|i| i.as_str())
                .map(str::to_string);
            self.herdr.pane_count = v
                .get("panes")
                .and_then(|p| p.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
        }
    }

    /// The active editable `TextArea` (live editor) for the current tab/field.
    fn active_textarea_mut(&mut self) -> Option<&mut TextArea<'static>> {
        match self.tab {
            Tab::Playground if self.playground.editing_field => {
                Some(&mut self.playground.edit_area)
            }
            Tab::Variables if self.variables_state.editing => {
                if self.variables_state.edit_field == EditField::Key {
                    Some(&mut self.variables_state.edit_key_area)
                } else {
                    Some(&mut self.variables_state.edit_value_area)
                }
            }
            _ => None,
        }
    }

    /// Immutable counterpart of [`App::active_textarea_mut`].
    fn active_textarea_ref(&self) -> Option<&TextArea<'static>> {
        match self.tab {
            Tab::Playground if self.playground.editing_field => Some(&self.playground.edit_area),
            Tab::Variables if self.variables_state.editing => {
                if self.variables_state.edit_field == EditField::Key {
                    Some(&self.variables_state.edit_key_area)
                } else {
                    Some(&self.variables_state.edit_value_area)
                }
            }
            _ => None,
        }
    }

    /// The full text of the focused editable field (used when there's no active
    /// selection). Falls back to the non-editing focused playground field.
    fn focused_field_text(&self) -> Option<String> {
        match self.tab {
            Tab::Playground => {
                if self.playground.editing_field {
                    Some(self.playground.edit_area.lines().join("\n"))
                } else if !self.playground.fields.is_empty() {
                    self.playground
                        .field_values
                        .get(self.playground.field_focus)
                        .cloned()
                } else {
                    None
                }
            }
            Tab::Variables if self.variables_state.editing => {
                if self.variables_state.edit_field == EditField::Key {
                    Some(self.variables_state.edit_key_area.lines().join("\n"))
                } else {
                    Some(self.variables_state.edit_value_area.lines().join("\n"))
                }
            }
            _ => None,
        }
    }

    /// True when a Copy/Cut/Paste target is available (field, variable, or the
    /// focused Result pane).
    fn has_editable(&self) -> bool {
        self.selection_or_full_text().is_some()
    }

    /// The text to copy: the active selection if one exists, otherwise the full
    /// value of the focused target. Priority: focused Result pane → active
    /// editable field/variable → non-editing focused playground field.
    fn selection_or_full_text(&self) -> Option<String> {
        if self.tab == Tab::Playground && self.playground.result_focused {
            let ta = &self.playground.result_area;
            let full = ta.lines().join("\n");
            return Some(textarea_selection_or_full(ta, full));
        }
        if let Some(ta) = self.active_textarea_ref() {
            let full = ta.lines().join("\n");
            return Some(textarea_selection_or_full(ta, full));
        }
        self.focused_field_text()
    }

    /// True when the current copy target is the read-only Result pane (so
    /// Cut/Paste must be suppressed).
    fn copy_target_is_readonly(&self) -> bool {
        self.tab == Tab::Playground && self.playground.result_focused
    }

    /// Feed a key event into the active editable `TextArea` (if any).
    pub(crate) fn feed_active_textarea(&mut self, code: KeyCode, mods: KeyModifiers) {
        if let Some(ta) = self.active_textarea_mut()
            && let Some(input) = to_textarea_input(code, mods)
        {
            ta.input(input);
        }
    }

    /// Feed a key event into the focused Result `TextArea` (for scrolling /
    /// Shift+arrow selection). No-op unless the Result pane is focused.
    pub(crate) fn feed_result_textarea(&mut self, code: KeyCode, mods: KeyModifiers) {
        if self.tab == Tab::Playground
            && self.playground.result_focused
            && let Some(input) = to_textarea_input(code, mods)
        {
            self.playground.result_area.input(input);
        }
    }

    async fn set_clipboard(&mut self, text: &str) {
        self.log_debug(format!(
            "set_clipboard: {} chars, http={}",
            text.len(),
            self.http.is_some()
        ));
        match self.http.clone() {
            Some(http) => match http.clipboard_set(text).await {
                Ok(v) => {
                    self.status_msg = format!("copied {} chars", text.len());
                    self.log_debug(format!("set_clipboard ok: {v}"));
                }
                Err(e) => {
                    self.status_msg = format!("clipboard set failed: {e}");
                    self.log_debug(format!("set_clipboard ERR: {e}"));
                }
            },
            None => {
                self.status_msg = "bridge not connected".into();
                self.log_debug("set_clipboard: bridge not connected (http=None)");
            }
        }
    }

    async fn clipboard_copy(&mut self) {
        let text = match self.selection_or_full_text() {
            Some(t) => t,
            None => {
                self.status_msg = "nothing to copy".into();
                self.log_debug("clipboard_copy: nothing to copy (selection_or_full_text=None)");
                return;
            }
        };
        self.log_debug(format!(
            "clipboard_copy: result_focused={} text_len={}",
            self.playground.result_focused,
            text.len()
        ));
        self.set_clipboard(&text).await;
    }

    async fn clipboard_cut(&mut self) {
        if self.copy_target_is_readonly() {
            self.status_msg = "result is read-only".into();
            return;
        }
        let text = if let Some(ta) = self.active_textarea_mut() {
            if !ta.is_selecting() {
                ta.select_all();
            }
            ta.copy();
            let yank = ta.yank_text();
            ta.cut();
            yank
        } else if let Some(t) = self.focused_field_text() {
            // No live editor: clear the focused (non-editing) field.
            if self.tab == Tab::Playground
                && !self.playground.editing_field
                && let Some(v) = self
                    .playground
                    .field_values
                    .get_mut(self.playground.field_focus)
            {
                *v = String::new();
            }
            t
        } else {
            self.status_msg = "nothing to cut".into();
            return;
        };
        self.set_clipboard(&text).await;
    }

    async fn clipboard_paste(&mut self) {
        if self.copy_target_is_readonly() {
            self.status_msg = "result is read-only".into();
            return;
        }
        let text = match self.http.clone() {
            Some(http) => match http.clipboard_get().await {
                Ok(t) => {
                    self.log_debug(format!("clipboard_paste: got {} chars", t.len()));
                    t
                }
                Err(e) => {
                    self.status_msg = format!("clipboard get failed: {e}");
                    self.log_debug(format!("clipboard_paste ERR: {e}"));
                    return;
                }
            },
            None => {
                self.status_msg = "bridge not connected".into();
                self.log_debug("clipboard_paste: bridge not connected (http=None)");
                return;
            }
        };
        match self.tab {
            Tab::Playground => {
                if !self.playground.editing_field {
                    self.playground.editing_field = true;
                    let cur = self
                        .playground
                        .field_values
                        .get(self.playground.field_focus)
                        .cloned()
                        .unwrap_or_default();
                    self.playground.edit_area = TextArea::new(vec![cur]);
                }
                let ta = &mut self.playground.edit_area;
                ta.set_yank_text(text);
                ta.paste();
                if let Some(v) = self
                    .playground
                    .field_values
                    .get_mut(self.playground.field_focus)
                {
                    *v = ta.lines().join("\n");
                }
            }
            Tab::Variables if self.variables_state.editing => {
                let ta = if self.variables_state.edit_field == EditField::Key {
                    &mut self.variables_state.edit_key_area
                } else {
                    &mut self.variables_state.edit_value_area
                };
                ta.set_yank_text(text);
                ta.paste();
            }
            _ => self.status_msg = "no field to paste into".into(),
        }
    }

    async fn paste_to_pane(&mut self) {
        let text = match self.http.clone() {
            Some(http) => match http.clipboard_get().await {
                Ok(t) => t,
                Err(e) => {
                    self.status_msg = format!("clipboard get failed: {e}");
                    return;
                }
            },
            None => {
                self.status_msg = "bridge not connected".into();
                return;
            }
        };
        let pane_id = match &self.herdr.pane_id {
            Some(p) => p.clone(),
            None => {
                self.status_msg = "no focused herdr pane".into();
                return;
            }
        };
        match herdr_cli(&["pane", "send-text", &pane_id, &text]).await {
            Ok(_) => self.status_msg = format!("sent {} chars to pane {pane_id}", text.len()),
            Err(e) => self.status_msg = format!("send-text failed: {e}"),
        }
    }
}

/// Execute a right-click context-menu action and close the menu.
async fn execute_context_action(app: &mut App, action: ContextAction) {
    match action {
        ContextAction::Copy => app.clipboard_copy().await,
        ContextAction::Cut => app.clipboard_cut().await,
        ContextAction::Paste => app.clipboard_paste().await,
        ContextAction::PasteToPane => app.paste_to_pane().await,
    }
    app.context_menu.open = false;
}

/// Convert a crossterm key event into a backend-agnostic `tui_textarea::Input`.
///
/// We build `Input` manually (rather than `Input::from(crossterm_event)`) so the
/// TUI does not need to depend on tui-textarea's pinned crossterm version.
pub(crate) fn to_textarea_input(code: KeyCode, mods: KeyModifiers) -> Option<TaInput> {
    let key = match code {
        KeyCode::Char(c) => TaKey::Char(c),
        KeyCode::Enter => TaKey::Enter,
        KeyCode::Backspace => TaKey::Backspace,
        KeyCode::Left => TaKey::Left,
        KeyCode::Right => TaKey::Right,
        KeyCode::Up => TaKey::Up,
        KeyCode::Down => TaKey::Down,
        KeyCode::Tab => TaKey::Tab,
        KeyCode::Delete => TaKey::Delete,
        KeyCode::Home => TaKey::Home,
        KeyCode::End => TaKey::End,
        KeyCode::PageUp => TaKey::PageUp,
        KeyCode::PageDown => TaKey::PageDown,
        KeyCode::Esc => TaKey::Esc,
        KeyCode::F(n) => TaKey::F(n),
        _ => return None,
    };
    Some(TaInput {
        key,
        ctrl: mods.contains(KeyModifiers::CONTROL),
        alt: mods.contains(KeyModifiers::ALT),
        shift: mods.contains(KeyModifiers::SHIFT),
    })
}

/// Entry point invoked from the CLI `dashboard` subcommand.
pub async fn run(opts: DashboardOptions) -> Result<()> {
    let mut app = App::new(opts);
    app.refresh().await;
    app.refresh_herdr().await;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("creating ratatui terminal")?;

    let result = main_loop(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    result
}

/// Core event loop: draws, polls, dispatches input.
async fn main_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    let frame_ms = 120u64;
    let refresh = Duration::from_secs(3);
    let context_refresh = Duration::from_secs(15);

    let mut last_ctx = Instant::now();

    loop {
        if app.quitting {
            return Ok(());
        }

        let _ = terminal.draw(|frame| ui(frame, app));

        let poll = Duration::from_millis(frame_ms);
        while event::poll(poll)? {
            match event::read()? {
                Event::Key(k) => {
                    if k.kind != KeyEventKind::Press {
                        continue;
                    }
                    handle_key(app, k.code, k.modifiers).await?;
                }
                Event::Mouse(m) => handle_mouse(app, m).await?,
                Event::Resize(_, _) => {}
                _ => {}
            }
            if app.quitting {
                return Ok(());
            }
        }

        if app.last_refresh.elapsed() > refresh {
            app.refresh().await;
        }
        if last_ctx.elapsed() > context_refresh {
            app.refresh_herdr().await;
            last_ctx = Instant::now();
        }
    }
}

/// Extract the selected text from a `TextArea` without mutating its yank
/// buffer, falling back to the full text when no selection is active. Mirrors
/// tui-textarea's `copy` semantics but is safe to call from an `&self` context.
fn textarea_selection_or_full(ta: &TextArea, full: String) -> String {
    if ta.is_selecting()
        && let Some(((sr, sc), (er, ec))) = ta.selection_range()
    {
        if sr == er {
            return ta.lines()[sr]
                .chars()
                .skip(sc)
                .take(ec.saturating_sub(sc))
                .collect();
        }
        let mut chunk = vec![ta.lines()[sr].chars().skip(sc).collect::<String>()];
        for row in (sr + 1)..er {
            chunk.push(ta.lines()[row].clone());
        }
        chunk.push(ta.lines()[er].chars().take(ec).collect::<String>());
        return chunk.join("\n");
    }
    full
}

/// Draws the full frame: header, tab bar, tab content, footer.
fn ui(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // header + tab bar
            Constraint::Min(0),    // content
            Constraint::Length(1), // footer
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    draw_tab_bar(frame, chunks[0], app);
    draw_content(frame, chunks[1], app);
    draw_footer(frame, chunks[2], app);
    if app.context_menu.open {
        draw_context_menu(frame, app);
    }
}

/// Right-click context menu overlay (Copy / Cut / Paste / Paste to pane).
fn draw_context_menu(frame: &mut ratatui::Frame, app: &mut App) {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let rect = menu_rect(app.context_menu.x, app.context_menu.y, cols, rows);
    let block = Block::default().borders(Borders::ALL).title("Edit");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let has = app.has_editable();
    for (i, a) in ContextAction::ALL.iter().enumerate() {
        let focused = i == app.context_menu.index;
        let enabled = has || matches!(a, ContextAction::PasteToPane);
        let style = if !enabled {
            dim_style()
        } else if focused {
            accent_style().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        let label = format!(" {:<14} ", a.label());
        let line = Line::from(Span::styled(label, style));
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
        );
    }
}

/// Brand header + herdr sidecar context.
fn draw_header(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let mut spans = vec![
        Span::styled("herdr-mcp", title_style()),
        Span::styled(" — dashboard", muted_style()),
    ];
    if let Some(ws) = &app.herdr.workspace {
        spans.push(Span::styled(format!("   workspace: {ws}"), muted_style()));
    }
    if let Some(p) = &app.herdr.pane_id {
        spans.push(Span::styled(format!("   pane: {p}"), muted_style()));
    }
    spans.push(Span::styled(
        format!("   {} panes", app.herdr.pane_count),
        muted_style(),
    ));
    let header = Paragraph::new(Line::from(spans));
    frame.render_widget(header, Rect::new(area.x, area.y, area.width, 1));
}

/// VS Code-style tab strip.
fn draw_tab_bar(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let bar_y = area.y + 1;
    let mut spans = Vec::new();
    app.tab_rects.clear();
    let mut x: u16 = area.x + 1;
    for t in Tab::ALL.iter() {
        let label = format!("[{}] {}", t.key(), t.label());
        let width = label.chars().count() as u16 + 2;
        let style = if *t == app.tab {
            accent_style().add_modifier(Modifier::BOLD)
        } else {
            muted_style()
        };
        spans.push(Span::styled(format!("{label}  "), style));
        app.tab_rects.push(Rect::new(x, bar_y, width, 1));
        x += width;
    }
    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, Rect::new(area.x, bar_y, area.width, 1));
}

fn draw_content(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    match app.tab {
        Tab::Overview => tabs::overview::render(frame, area, app),
        Tab::Playground => tabs::playground::render(frame, area, app),
        Tab::Trim => tabs::trim::render(frame, area, app),
        Tab::Variables => tabs::variables::render(frame, area, app),
        Tab::Settings => tabs::settings::render(frame, area, app),
    }
}

/// Footer with status message + keybindings.
fn draw_footer(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let mut line = Line::from(vec![Span::styled(
        "Ctrl+1-5:tab  ↑/↓:nav  Ctrl+r:refresh  Ctrl+v:paste  Ctrl+c:copy  Ctrl+x:cut  right-click:menu  Ctrl+q:quit",
        dim_style(),
    )]);
    if !app.status_msg.is_empty() {
        line = Line::from(vec![
            Span::styled(
                "Ctrl+1-5:tab  ↑/↓:nav  Ctrl+r:refresh  Ctrl+v:paste  Ctrl+c:copy  Ctrl+x:cut  right-click:menu  Ctrl+q:quit",
                dim_style(),
            ),
            Span::styled(
                format!("   {}", truncate(&app.status_msg, 90)),
                accent_style(),
            ),
        ]);
    }
    frame.render_widget(Paragraph::new(line), area);
}

/// Global key handler with tab switching + per-tab dispatch.
async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<()> {
    // The right-click context menu fully owns input while open.
    if app.context_menu.open {
        handle_context_menu_key(app, code, mods).await?;
        return Ok(());
    }

    // Editing inputs are handled inside the active tab first.
    let consumed = match app.tab {
        Tab::Playground => tabs::playground::handle_key(app, code, mods).await?,
        Tab::Variables => tabs::variables::handle_key(app, code, mods).await?,
        Tab::Settings => tabs::settings::handle_key(app, code, mods).await?,
        Tab::Trim => tabs::trim::handle_key(app, code, mods).await?,
        _ => false,
    };
    if consumed {
        return Ok(());
    }

    // Global clipboard hotkeys (Ctrl+C copy / Ctrl+V paste / Ctrl+X cut).
    if mods.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('c') => {
                app.clipboard_copy().await;
                return Ok(());
            }
            KeyCode::Char('v') => {
                app.clipboard_paste().await;
                return Ok(());
            }
            KeyCode::Char('x') => {
                app.clipboard_cut().await;
                return Ok(());
            }
            _ => {}
        }
    }

    // Global keys. Every command requires a Ctrl modifier (or is a navigation
    // key such as Tab / arrows) so printable characters typed into fields are
    // never intercepted.
    if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('q') {
        app.quitting = true;
        return Ok(());
    }
    match code {
        KeyCode::Tab => {
            cycle_tab(app, 1);
        }
        KeyCode::BackTab => {
            cycle_tab(app, -1);
        }
        KeyCode::Char('r') if mods.contains(KeyModifiers::CONTROL) => {
            app.refresh().await;
            app.status_msg = "refreshed".to_string();
        }
        KeyCode::Char(c @ '1'..='5') if mods.contains(KeyModifiers::CONTROL) => {
            let idx = (c as u8 - b'1') as usize;
            if let Some(t) = Tab::ALL.get(idx).copied() {
                app.tab = t;
                app.status_msg.clear();
            }
        }
        _ => {}
    }
    Ok(())
}

fn cycle_tab(app: &mut App, step: i32) {
    let idx = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0) as i32;
    let n = Tab::ALL.len() as i32;
    let next = ((idx + step + n) % n) as usize;
    app.tab = Tab::ALL[next];
    app.status_msg.clear();
}

/// Key handling while the right-click context menu is open.
async fn handle_context_menu_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
    // Hotkeys still work while the menu is open.
    if mods.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('c') => {
                execute_context_action(app, ContextAction::Copy).await;
                return Ok(true);
            }
            KeyCode::Char('v') => {
                execute_context_action(app, ContextAction::Paste).await;
                return Ok(true);
            }
            KeyCode::Char('x') => {
                execute_context_action(app, ContextAction::Cut).await;
                return Ok(true);
            }
            _ => {}
        }
    }
    match code {
        KeyCode::Up => {
            if app.context_menu.index > 0 {
                app.context_menu.index -= 1;
            }
            Ok(true)
        }
        KeyCode::Down => {
            if app.context_menu.index + 1 < ContextAction::ALL.len() {
                app.context_menu.index += 1;
            }
            Ok(true)
        }
        KeyCode::Esc => {
            app.context_menu.open = false;
            Ok(true)
        }
        KeyCode::Enter => {
            let action = ContextAction::ALL[app.context_menu.index];
            execute_context_action(app, action).await;
            Ok(true)
        }
        _ => Ok(true),
    }
}

/// Mouse event handler: click tabs to switch, click lists to select, wheel scroll.
async fn handle_mouse(app: &mut App, m: MouseEvent) -> Result<()> {
    match m.kind {
        MouseEventKind::Down(MouseButton::Right) => {
            // Region-aware: focusing the Result pane makes Copy target it.
            let over_result = app.tab == Tab::Playground
                && m.column >= app.result_inner.x
                && m.column < app.result_inner.right()
                && m.row >= app.result_inner.y
                && m.row < app.result_inner.bottom();
            app.log_debug(format!(
                "mouse right-down at ({},{}) result_inner={:?} over_result={}",
                m.column, m.row, app.result_inner, over_result
            ));
            if over_result {
                app.playground.result_focused = true;
                app.playground.editing_field = false;
            } else if app.tab == Tab::Playground {
                app.playground.result_focused = false;
            }
            // Open the right-click context menu at the cursor.
            app.context_menu.open = true;
            app.context_menu.x = m.column;
            app.context_menu.y = m.row;
            app.context_menu.index = 0;
            return Ok(());
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if app.context_menu.open {
                if let Ok((cols, rows)) = crossterm::terminal::size() {
                    let rect = menu_rect(app.context_menu.x, app.context_menu.y, cols, rows);
                    if m.column >= rect.x
                        && m.column < rect.right()
                        && m.row >= rect.y
                        && m.row < rect.bottom()
                    {
                        let idx = (m.row.saturating_sub(rect.y + 1)) as usize;
                        if idx < ContextAction::ALL.len() {
                            let action = ContextAction::ALL[idx];
                            execute_context_action(app, action).await;
                            return Ok(());
                        }
                    }
                }
                // Click outside the menu dismisses it; continue to normal handling.
                app.context_menu.open = false;
            }
            // Tab strip.
            for (i, r) in app.tab_rects.iter().enumerate() {
                if m.row == r.y && m.column >= r.x && m.column < r.x + r.width {
                    app.tab = Tab::ALL[i];
                    app.status_msg.clear();
                    return Ok(());
                }
            }
            // Tool list (playground).
            if app.tab == Tab::Playground
                && m.column >= app.tool_list_inner.x
                && m.column < app.tool_list_inner.right()
                && m.row >= app.tool_list_inner.y
                && m.row < app.tool_list_inner.bottom()
            {
                let idx = (m.row - app.tool_list_inner.y) as usize;
                if idx < app.playground.tools_list.len() {
                    app.playground.tool_index = idx;
                    app.playground.editing_field = false;
                    app.playground.result_focused = false;
                }
                return Ok(());
            }
            // Result pane (playground): focus it for selection.
            if app.tab == Tab::Playground
                && m.column >= app.result_inner.x
                && m.column < app.result_inner.right()
                && m.row >= app.result_inner.y
                && m.row < app.result_inner.bottom()
            {
                app.playground.result_focused = true;
                app.playground.editing_field = false;
                app.log_debug(format!(
                    "mouse left-down focused Result pane; result_inner={:?}",
                    app.result_inner
                ));
                return Ok(());
            }
            // Variable list.
            if app.tab == Tab::Variables
                && !app.variables_state.editing
                && m.column >= app.var_list_inner.x
                && m.column < app.var_list_inner.right()
                && m.row >= app.var_list_inner.y
                && m.row < app.var_list_inner.bottom()
            {
                let idx = (m.row - app.var_list_inner.y) as usize;
                if idx < app.variables_state.entries.len() {
                    app.variables_state.selected = idx;
                }
                return Ok(());
            }
        }
        MouseEventKind::ScrollDown => match app.tab {
            Tab::Variables
                if !app.variables_state.editing
                    && app.variables_state.selected + 1 < app.variables_state.entries.len() =>
            {
                app.variables_state.selected += 1;
            }
            Tab::Playground
                if !app.playground.editing_field
                    && app.playground.tool_index + 1 < app.playground.tools_list.len() =>
            {
                app.playground.tool_index += 1;
            }
            _ => {}
        },
        MouseEventKind::ScrollUp => match app.tab {
            Tab::Variables if !app.variables_state.editing && app.variables_state.selected > 0 => {
                app.variables_state.selected -= 1;
            }
            Tab::Playground if !app.playground.editing_field && app.playground.tool_index > 0 => {
                app.playground.tool_index -= 1;
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

/// Parse `/api/tools` into `(name, short_desc)` pairs.
fn parse_tool_list(v: &Value) -> Vec<(String, String)> {
    let arr = match v.get("tools").and_then(|t| t.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|t| {
            let name = t.get("name").and_then(|n| n.as_str())?.to_string();
            let desc = t
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            Some((name, truncate(&desc, 60)))
        })
        .collect()
}

/// Parse a tool's JSON `inputSchema` into editable `ToolField`s.
pub fn parse_tool_schema(tool: &Value) -> Vec<ToolField> {
    let schema = match tool.get("inputSchema").and_then(|s| s.as_object()) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let props = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let required = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut fields = Vec::new();
    for (name, spec) in props {
        let typ = spec.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let enum_variants = spec.get("enum").and_then(|e| e.as_array()).map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        });
        let kind = if typ == "boolean" {
            FieldKind::Boolean
        } else if enum_variants.is_some() {
            FieldKind::Enum
        } else if typ == "number" || typ == "integer" {
            FieldKind::Number
        } else {
            FieldKind::Text
        };
        let default = spec.get("default").map(|d| match d {
            Value::String(s) => s.clone(),
            o => o.to_string(),
        });
        let description = spec
            .get("description")
            .and_then(|d| d.as_str())
            .map(str::to_string);
        let label = spec
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or(name)
            .to_string();
        fields.push(ToolField {
            name: name.clone(),
            label,
            kind,
            required: required.contains(name),
            default,
            enum_variants,
            description,
        });
    }
    fields
}

/// Parse `/api/variables` into `(key, value)` pairs.
fn parse_variables(v: &Value) -> Vec<(String, String)> {
    let arr = match v.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            let key = e.get("key").and_then(|k| k.as_str())?.to_string();
            let val = e
                .get("value")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    o => o.to_string(),
                })
                .unwrap_or_default();
            Some((key, val))
        })
        .collect()
}

/// Best-effort synchronous herdr CLI invocation shared by the sidecar refresh.
async fn herdr_cli(args: &[&str]) -> Result<String> {
    use tokio::process::Command;
    let bin = std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".to_string());
    let out = Command::new(&bin)
        .args(args)
        .output()
        .await
        .with_context(|| format!("running {bin} {}", args.join(" ")))?;
    if !out.status.success() {
        anyhow::bail!("herdr {} failed", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Truncate a string to `max` display cells (char count, not grapheme width).
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t = String::new();
    for (i, c) in s.chars().enumerate() {
        if i + 1 >= max {
            t.push('…');
            break;
        }
        t.push(c);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_with_schema() -> Value {
        serde_json::json!({
            "name": "start_agent",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": { "type": "string", "title": "Workspace", "description": "ws id" },
                    "limit": { "type": "integer", "default": 20 },
                    "enabled": { "type": "boolean", "default": true },
                    "mode": { "type": "string", "enum": ["fast", "slow"] }
                },
                "required": ["workspace_id"]
            }
        })
    }

    #[test]
    fn parse_tool_schema_kinds_and_required() {
        let fields = parse_tool_schema(&tool_with_schema());
        assert_eq!(fields.len(), 4);

        let ws = fields.iter().find(|f| f.name == "workspace_id").unwrap();
        assert!(matches!(ws.kind, FieldKind::Text));
        assert!(ws.required);
        assert_eq!(ws.label, "Workspace");
        assert_eq!(ws.description.as_deref(), Some("ws id"));

        let limit = fields.iter().find(|f| f.name == "limit").unwrap();
        assert!(matches!(limit.kind, FieldKind::Number));
        assert_eq!(limit.default.as_deref(), Some("20"));
        assert!(!limit.required);

        let enabled = fields.iter().find(|f| f.name == "enabled").unwrap();
        assert!(matches!(enabled.kind, FieldKind::Boolean));
        assert_eq!(enabled.default.as_deref(), Some("true"));

        let mode = fields.iter().find(|f| f.name == "mode").unwrap();
        assert!(matches!(mode.kind, FieldKind::Enum));
        assert_eq!(
            mode.enum_variants.as_ref().unwrap(),
            &vec!["fast".to_string(), "slow".to_string()]
        );
    }

    #[test]
    fn parse_tool_schema_empty_when_missing() {
        let none = parse_tool_schema(&serde_json::json!({ "name": "x" }));
        assert!(none.is_empty());
    }

    // ── Clipboard / result selection ─────────────────────────────────────

    fn sample_field() -> ToolField {
        ToolField {
            name: "field".into(),
            label: "field".into(),
            kind: FieldKind::Text,
            required: false,
            default: None,
            enum_variants: None,
            description: None,
        }
    }

    fn test_app() -> App {
        App::new(DashboardOptions {
            data_dir: std::path::PathBuf::from("/tmp"),
            http_port: 1,
        })
    }

    #[test]
    fn selection_or_full_text_uses_focused_field() {
        let mut app = test_app();
        app.tab = Tab::Playground;
        app.playground.fields = vec![sample_field()];
        app.playground.field_values = vec!["hello".into()];
        app.playground.field_focus = 0;
        app.playground.result_focused = false;
        assert_eq!(app.selection_or_full_text(), Some("hello".to_string()));
    }

    #[test]
    fn selection_or_full_text_uses_full_result_when_focused() {
        let mut app = test_app();
        app.tab = Tab::Playground;
        app.playground.result_focused = true;
        app.playground.result_area = TextArea::new(vec!["line1".to_string(), "line2".to_string()]);
        assert_eq!(
            app.selection_or_full_text(),
            Some("line1\nline2".to_string())
        );
    }

    #[test]
    fn selection_or_full_text_uses_result_selection() {
        let mut app = test_app();
        app.tab = Tab::Playground;
        app.playground.result_focused = true;
        let mut ta = TextArea::new(vec!["hello world".to_string()]);
        ta.start_selection();
        for _ in 0..5 {
            ta.move_cursor(tui_textarea::CursorMove::Forward);
        }
        app.playground.result_area = ta;
        assert_eq!(app.selection_or_full_text(), Some("hello".to_string()));
    }

    #[test]
    fn selection_or_full_text_none_when_nothing_focused() {
        let app = test_app();
        assert_eq!(app.selection_or_full_text(), None);
    }

    #[test]
    fn copy_target_is_readonly_only_for_result() {
        let mut app = test_app();
        app.tab = Tab::Playground;
        app.playground.result_focused = true;
        assert!(app.copy_target_is_readonly());
        app.playground.result_focused = false;
        assert!(!app.copy_target_is_readonly());
    }

    #[test]
    fn menu_rect_clamps_to_terminal() {
        let r = menu_rect(1000, 1000, 80, 24);
        assert!(r.x + r.width <= 80);
        assert!(r.y + r.height <= 24);
        assert_eq!(r.width, MENU_WIDTH);
        assert_eq!(r.height, MENU_HEIGHT);

        let r2 = menu_rect(5, 5, 80, 24);
        assert_eq!(r2.x, 5);
        assert_eq!(r2.y, 5);
    }

    #[test]
    fn to_textarea_input_maps_keys_and_mods() {
        let a = to_textarea_input(KeyCode::Char('a'), KeyModifiers::NONE).unwrap();
        assert_eq!(a.key, TaKey::Char('a'));
        assert!(!a.ctrl);

        let c = to_textarea_input(KeyCode::Char('c'), KeyModifiers::CONTROL).unwrap();
        assert!(c.ctrl);
        assert_eq!(c.key, TaKey::Char('c'));

        let up = to_textarea_input(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(up.key, TaKey::Up);

        // Unsupported keys (e.g. modifier-only) map to None.
        assert!(to_textarea_input(KeyCode::Null, KeyModifiers::NONE).is_none());
    }
}

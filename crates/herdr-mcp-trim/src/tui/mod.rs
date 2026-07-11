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
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
        KeyModifiers, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::Value;

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
}

/// Playground tab state (tool runner + recipe builder).
pub struct PlaygroundState {
    pub sub_tab: PlaygroundSub,
    pub tool_index: usize,
    pub tools_list: Vec<(String, String)>, // (name, short desc)
    pub editing_field: bool,                // editing focused text/number field
    pub fields: Vec<ToolField>,             // parsed schema for current tool
    pub field_values: Vec<String>,          // current values aligned to `fields`
    pub field_focus: usize,                 // focused field index
    pub fields_for_index: Option<usize>,    // tool_index the fields were parsed for
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
    pub edit_key: String,
    pub edit_value: String,
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
                result: None,
                error: None,
            },
            trim: TrimState { diagnose: None },
            variables_state: VariablesState {
                entries: Vec::new(),
                selected: 0,
                editing: false,
                edit_key: String::new(),
                edit_value: String::new(),
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
        }
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
                self.status_msg =
                    format!("bridge unreachable at :{} — start `herdr-mcp serve`", self.opts.http_port);
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
        app.tab_rects
            .push(Rect::new(x, bar_y, width, 1));
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
    let mut line = Line::from(vec![
        Span::styled(
            "Ctrl+1-5/Ctrl+Tab:tab  ↑/↓:navigate  Ctrl+r:refresh  Ctrl+q:quit",
            dim_style(),
        ),
    ]);
    if !app.status_msg.is_empty() {
        line = Line::from(vec![
            Span::styled(
                "Ctrl+1-5/Ctrl+Tab:tab  ↑/↓:navigate  Ctrl+r:refresh  Ctrl+q:quit",
                dim_style(),
            ),
            Span::styled(format!("   {}", truncate(&app.status_msg, 40)), accent_style()),
        ]);
    }
    frame.render_widget(Paragraph::new(line), area);
}

/// Global key handler with tab switching + per-tab dispatch.
async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<()> {
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

/// Mouse event handler: click tabs to switch, click lists to select, wheel scroll.
async fn handle_mouse(app: &mut App, m: MouseEvent) -> Result<()> {
    match m.kind {
        MouseEventKind::Down(_) => {
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
                }
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
        let enum_variants = spec
            .get("enum")
            .and_then(|e| e.as_array())
            .map(|a| {
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
        let default = spec
            .get("default")
            .map(|d| match d {
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
        assert_eq!(mode.enum_variants.as_ref().unwrap(), &vec!["fast".to_string(), "slow".to_string()]);
    }

    #[test]
    fn parse_tool_schema_empty_when_missing() {
        let none = parse_tool_schema(&serde_json::json!({ "name": "x" }));
        assert!(none.is_empty());
    }
}


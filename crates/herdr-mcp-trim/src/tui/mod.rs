//! Kitchen-sink TUI dashboard for herdr-mcp.
//!
//! VS Code-style tabbed interface running directly in a terminal (and a herdr
//! pane). Supports mouse + keyboard navigation. Tabs: Overview, Playground
//! (tool runner + recipe builder), Trim, Variables, Settings.
//!
//! The dashboard talks to the herdr-mcp HTTP bridge over `reqwest` (see `http`).
//! It also reads herdr workspace/pane context via the `herdr` CLI for the
//! sidecar header.
//!
//! Entry point: [`run`].

pub mod http;
pub mod render;
pub mod tabs;

use std::io::{self, Write as IoWrite};
use std::time::{Duration, Instant};
use std::fmt::Write as FmtWrite;

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
        KeyModifiers, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use serde_json::Value;

use crate::stats;

use http::HttpClient;
use render::{RESET, amber, bg, emerald, fg, move_to, muted, truncate};

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
    /// visible terminal size
    pub width: u16,
    pub height: u16,
    /// playground sub-state
    pub playground: PlaygroundState,
    /// trim sub-state
    pub trim: TrimState,
    /// variables sub-state
    pub variables_state: VariablesState,
    /// settings sub-state
    pub settings: SettingsState,
}

/// Playground tab state (tool runner + recipe builder).
pub struct PlaygroundState {
    pub sub_tab: PlaygroundSub,
    pub tool_index: usize,
    pub tools_list: Vec<(String, String)>, // (name, short desc)
    pub param_text: String,                  // editable JSON params
    pub param_cursor: usize,
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
            width: 100,
            height: 40,
            playground: PlaygroundState {
                sub_tab: PlaygroundSub::Runner,
                tool_index: 0,
                tools_list: Vec::new(),
                param_text: "{}".to_string(),
                param_cursor: 1,
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
            },
        }
    }

    async fn refresh(&mut self) {
        self.last_refresh = Instant::now();
        if let Some(http) = &self.http {
            if self.playground.tools_list.is_empty() {
                if let Ok(v) = http.list_tools().await {
                    self.tools = Some(v.clone());
                    self.playground.tools_list = parse_tool_list(&v);
                }
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
                        self.trim_stats_file = Some(stats::load_stats(&self.opts.data_dir, w).await);
                    }
                    break;
                }
            }
        }
    }

    /// Resolve current herdr workspace/pane via the CLI (best-effort).
    async fn refresh_herdr(&mut self) {
        if let Ok(raw) = herdr_cli(&["workspace", "list"]).await {
            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                self.herdr.workspace = v
                    .get("workspaces")
                    .and_then(|w| w.get(0))
                    .and_then(|w| w.get("id"))
                    .and_then(|i| i.as_str())
                    .map(str::to_string);
            }
        }
        if let Ok(raw) = herdr_cli(&["pane", "list"]).await {
            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
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
}

/// Entry point invoked from the CLI `dashboard` subcommand.
pub async fn run(opts: DashboardOptions) -> Result<()> {
    let mut app = App::new(opts);
    app.refresh().await;
    app.refresh_herdr().await;

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, Hide, EnableMouseCapture)?;

    let restore = RestoreTerm;
    let _ = &restore;

    if let Ok((w, h)) = crossterm::terminal::size() {
        app.width = w.max(40);
        app.height = h.max(12);
    }

    let result = main_loop(&mut out, &mut app).await;

    drop(restore);
    result
}

/// RAII guard that restores the terminal on drop.
struct RestoreTerm;

impl Drop for RestoreTerm {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = disable_raw_mode();
        let _ = execute!(out, LeaveAlternateScreen, Show, DisableMouseCapture);
    }
}

/// Core event loop: draws, polls, dispatches input.
async fn main_loop(out: &mut io::Stdout, app: &mut App) -> Result<()> {
    let frame_ms = 120u64;
    let refresh = Duration::from_secs(3);
    let context_refresh = Duration::from_secs(15);

    let mut last_ctx = Instant::now();

    loop {
        if app.quitting {
            return Ok(());
        }

        render(out, app)?;

        // Update the terminal size if it changed.
        if let Ok((w, h)) = crossterm::terminal::size() {
            app.width = w.max(40);
            app.height = h.max(12);
        }

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
                Event::Resize(w, h) => {
                    app.width = w.max(40);
                    app.height = h.max(12);
                }
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

/// Global key handler with tab switching + per-tab dispatch.
async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Result<()> {
    // Editing inputs are handled inside the active tab first.
    match app.tab {
        Tab::Playground => {
            if tabs::playground::handle_key(app, code, mods).await? {
                return Ok(());
            }
        }
        Tab::Variables => {
            if tabs::variables::handle_key(app, code, mods).await? {
                return Ok(());
            }
        }
        Tab::Settings => {
            if tabs::settings::handle_key(app, code, mods).await? {
                return Ok(());
            }
        }
        Tab::Trim => {
            if tabs::trim::handle_key(app, code, mods).await? {
                return Ok(());
            }
        }
        _ => {}
    }

    // Global keys (quit + tab switching).
    if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        app.quitting = true;
        return Ok(());
    }
    match code {
        KeyCode::Char('q') => {
            app.quitting = true;
        }
        KeyCode::Tab => {
            cycle_tab(app, 1);
        }
        KeyCode::BackTab => {
            cycle_tab(app, -1);
        }
        KeyCode::Char('r') if !mods.contains(KeyModifiers::CONTROL) => {
            app.refresh().await;
            app.status_msg = "refreshed".to_string();
        }
        KeyCode::Char(c @ '1'..='5') => {
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

/// Mouse event handler: click tabs to switch, click anywhere to activate.
async fn handle_mouse(app: &mut App, m: MouseEvent) -> Result<()> {
    match m.kind {
        MouseEventKind::Down(_) => {
            // Tab bar is row 2 (1-based). Detect clicks on tab labels.
            if m.row == 1 {
                let mut x = 4u16;
                for (i, t) in Tab::ALL.iter().enumerate() {
                    let label = format!("[{}] {}", t.key(), t.label());
                    let len = label.chars().count() as u16;
                    if m.column >= x && m.column < x + len + 2 {
                        app.tab = Tab::ALL[i];
                        app.status_msg.clear();
                        return Ok(());
                    }
                    x += len + 3;
                }
            }
        }
        MouseEventKind::ScrollDown => match app.tab {
            Tab::Variables => {
                if !app.variables_state.editing && app.variables_state.selected + 1
                    < app.variables_state.entries.len()
                {
                    app.variables_state.selected += 1;
                }
            }
            Tab::Playground => {
                if app.playground.tool_index + 1 < app.playground.tools_list.len() {
                    app.playground.tool_index += 1;
                }
            }
            _ => {}
        },
        MouseEventKind::ScrollUp => match app.tab {
            Tab::Variables => {
                if !app.variables_state.editing && app.variables_state.selected > 0 {
                    app.variables_state.selected -= 1;
                }
            }
            Tab::Playground => {
                if app.playground.tool_index > 0 {
                    app.playground.tool_index -= 1;
                }
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

/// Top-level render dispatcher.
fn render(out: &mut io::Stdout, app: &App) -> Result<()> {
    let mut f = String::with_capacity(8192);
    f.push_str("\x1b[2J");
    f.push_str(&render_header(&f_neutral(), app));
    f.push_str(&render_tab_bar(app));

    let body_row = 4u16;
    match app.tab {
        Tab::Overview => tabs::overview::render(&mut f, app),
        Tab::Playground => tabs::playground::render(&mut f, app),
        Tab::Trim => tabs::trim::render(&mut f, app),
        Tab::Variables => tabs::variables::render(&mut f, app),
        Tab::Settings => tabs::settings::render(&mut f, app),
    }

    // footer
    let foot = app.height;
    f.push_str(&move_to(1, foot));
    f.push_str(&render_footer(app));

    let _ = body_row;
    write!(out, "{f}")?;
    out.flush()?;
    Ok(())
}

/// Title bar with the brand + herdr sidecar context.
fn render_header(palette: &str, app: &App) -> String {
    let mut s = String::new();
    s.push_str(&palette);
    s.push_str(&move_to(1, 1));
    s.push_str(&emerald("herdr-mcp"));
    s.push_str(&muted(" — dashboard"));
    if let Some(ws) = &app.herdr.workspace {
        let _ = write!(&mut s, "   {}", muted("workspace:"),);
        let _ = write!(&mut s, " {}", emerald(ws));
    }
    if let Some(p) = &app.herdr.pane_id {
        let _ = write!(&mut s, "   {}", muted("pane:"),);
        let _ = write!(&mut s, " {}", emerald(p));
    }
    let _ = write!(&mut s, "   {}", muted(&format!("{} panes", app.herdr.pane_count)));
    s.push_str(RESET);
    s
}

/// VS Code-style tab strip on row 2.
fn render_tab_bar(app: &App) -> String {
    let mut s = String::new();
    s.push_str(&move_to(1, 2));
    s.push_str(&muted(" "));
    for t in Tab::ALL {
        let label = format!("[{}] {}", t.key(), t.label());
        let active = t == app.tab;
        let seg = if active {
            format!(
                "{}{}{}{}{}",
                bg(38, 38, 38),
                emerald(&label),
                RESET,
                " ",
                RESET
            )
        } else {
            muted(&label)
        };
        s.push_str(&format!("{seg}   "));
    }
    s.push_str(RESET);
    s
}

/// Footer with status message + keybindings.
fn render_footer(app: &App) -> String {
    let mut s = String::new();
    s.push_str(&fg(40, 40, 40));
    s.push_str(&muted("───────── "));
    s.push_str(&muted("1-5:tab  Tab|Ctrl+Tab:cycle  r:refresh  q/Ctrl+C:quit"));
    if !app.status_msg.is_empty() {
        s.push_str("   ");
        s.push_str(&amber(&truncate(&app.status_msg, 40)));
    }
    s.push_str(RESET);
    s
}

/// Reference neutral palette background filler.
fn f_neutral() -> String {
    String::new()
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

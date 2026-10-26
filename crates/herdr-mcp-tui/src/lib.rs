//! Kitchen-sink TUI dashboard for herdr-mcp.
//!
//! VS Code-style tabbed interface running directly in a terminal (and a herdr
//! pane). Supports mouse + keyboard navigation. Tabs: Overview, Playground
//! (collections of saved requests, and herdr's event stream), Variables,
//! Settings.
//!
//! Rendered with `ratatui` (Frame/Block/Layout widgets) over a crossterm
//! backend, following the patterns used by the sibling `herdr` and `zerocode`
//! dashboards. The dashboard talks to the herdr-mcp HTTP bridge through the
//! shared `herdr_mcp_api::ApiClient` (see `http`). It also reads herdr workspace/pane context via the `herdr`
//! CLI for the sidecar header.
//!
//! Entry point: [`run`].

pub mod http;
pub mod tabs;
pub mod theme;

use herdr_mcp_api::settings;
use std::io;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
        supports_keyboard_enhancement,
    },
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, ListState, Paragraph};
use ratatui::{Terminal, backend::CrosstermBackend};
use tui_textarea::{Input as TaInput, Key as TaKey, TextArea};

use std::io::Write;

/// Append a debug line to `<data_dir>/tui-debug.log` (file only — no stderr).
///
/// The TUI owns the terminal, so stderr output would corrupt the display.
/// All runtime diagnostics go to the log file for post-mortem analysis.
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
}

use herdr_mcp_api::{ApiClient, ConfigResponse, HerdrPane, Recipe, RecipeTemplate};
use theme::{accent_style, dim_style, muted_style, title_style};

/// Available tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Send,
    Overview,
    Playground,
    Variables,
    Settings,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Send,
        Tab::Overview,
        Tab::Playground,
        Tab::Variables,
        Tab::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Send => "Send",
            Tab::Overview => "Overview",
            Tab::Playground => "Playground",
            Tab::Variables => "Variables",
            Tab::Settings => "Settings",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Tab::Send => "1",
            Tab::Overview => "2",
            Tab::Playground => "3",
            Tab::Variables => "4",
            Tab::Settings => "5",
        }
    }
}

/// What the dashboard shows of herdr, refreshed from the bridge.
#[derive(Default, Clone)]
pub struct HerdrContext {
    /// Every named herdr session (`GET /api/herdr/sessions`).
    pub sessions: Vec<herdr_mcp_api::HerdrSession>,
    /// Session shown, by name; `None` = the bridge's own (`current`). Only
    /// that one is written to (paste-to-pane).
    pub session: Option<String>,
    /// The shown session's panes, in herdr's order.
    pub panes: Vec<HerdrPane>,
    /// Last error surfaced by `refresh_herdr`, surfaced in the footer / log so
    /// "no panes detected" is no longer a silent failure.
    pub last_error: Option<String>,
}

impl HerdrContext {
    /// herdr's focused pane in the shown session.
    pub fn focused(&self) -> Option<&HerdrPane> {
        self.panes.iter().find(|p| p.focused)
    }

    /// The focused pane's workspace.
    pub fn workspace(&self) -> Option<&str> {
        self.focused().map(|p| p.workspace_id.as_str())
    }

    /// Name of the shown session, for headers.
    pub fn session_name(&self) -> &str {
        match &self.session {
            Some(name) => name,
            None => self
                .sessions
                .iter()
                .find(|s| s.current)
                .map_or("this server's", |s| s.name.as_str()),
        }
    }

    /// Show the next running session (the bridge's own, then the others).
    pub fn cycle_session(&mut self) {
        let others: Vec<&str> = self
            .sessions
            .iter()
            .filter(|s| s.running && !s.current)
            .map(|s| s.name.as_str())
            .collect();
        let at = self
            .session
            .as_deref()
            .and_then(|name| others.iter().position(|o| *o == name));
        self.session = match at {
            None => others.first(),
            Some(i) => others.get(i + 1),
        }
        .map(|s| s.to_string());
    }

    /// `Err` with a status message when the shown session isn't the bridge's.
    pub fn writable(&self) -> Result<(), String> {
        match &self.session {
            None => Ok(()),
            Some(name) => Err(format!(
                "viewing session '{name}': press s on Overview to return to this server's"
            )),
        }
    }
}

/// What the dashboard calls a pane: its herdr label, else its id.
pub fn pane_name(p: &HerdrPane) -> &str {
    p.label.as_deref().unwrap_or(&p.pane_id)
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
    /// Tunables and keybindings in effect (`GET /api/settings`; the
    /// built-in defaults until the bridge answers).
    pub knobs: herdr_mcp_api::Settings,
    /// The tool call or recipe run in flight, if any.
    pub(crate) job: Option<tabs::playground::Job>,
    pub http: Option<ApiClient>,
    pub tab: Tab,
    pub herdr: HerdrContext,
    pub quitting: bool,
    pub status_msg: String,
    pub last_refresh: Instant,
    /// last-fetched bridge snapshots (refreshed on every tick)
    pub recipes: Option<Vec<Recipe>>,
    pub templates: Option<Vec<RecipeTemplate>>,
    /// Bridge runtime config (`GET /api/config`); fetched once per connection
    /// and replaced by the response of a clipboard Save.
    pub config: Option<ConfigResponse>,
    /// playground sub-state
    pub playground: tabs::playground::PlaygroundState,
    /// overview sub-state
    pub send: SendState,
    pub overview: OverviewState,
    /// variables sub-state
    pub variables_state: VariablesState,
    /// settings sub-state
    pub settings: SettingsState,
    /// whether the HTTP bridge is currently reachable
    pub bridge_connected: bool,
    /// layout hit-areas captured during draw for mouse handling
    pub tab_rects: Vec<Rect>,
    pub var_list_inner: Rect,
    /// Hit-area for the Overview pane table (mouse click → select pane).
    pub pane_table_inner: Rect,
    /// Hit-area for the Overview pane text (drag to select).
    pub pane_text_inner: Rect,
    /// hit-areas for clipboard preset rows in Settings (one Rect per preset)
    pub clipboard_preset_rects: Vec<Rect>,
    /// hit-areas for clipboard action buttons (0=Test, 1=Save)
    pub clipboard_btn_rects: Vec<Rect>,
    /// hit-areas for clipboard command fields (0=copy, 1=paste)
    pub clipboard_field_rects: Vec<Rect>,
    /// right-click context menu state
    pub context_menu: ContextMenu,
    /// True when state has changed and a redraw is needed. Reset after draw.
    pub dirty: bool,
}

/// Send tab state: what the user is typing and where it goes.
pub struct SendState {
    /// The picked pane's id; `None` = the first of `tabs::send::targets`.
    /// An id, not a row: the list re-sorts on every refresh.
    pub pane: Option<String>,
    pub text: TextArea<'static>,
    /// Send with `run_command` (text + Enter) instead of `send_text`.
    pub enter: bool,
    /// Result of the last send.
    pub msg: String,
}

/// Overview tab state.
pub struct OverviewState {
    /// Selected row in the pane table.
    pub pane_index: usize,
    /// Which frame has keyboard focus (0 = pane table, 1 = pane text).
    pub focused_frame: usize,
    /// First pane the table shows (it scrolls to keep the pick in view).
    pub table_offset: usize,
    /// The picked pane's text.
    pub view: tabs::overview::PaneView,
}

/// Variables tab state.
pub struct VariablesState {
    /// Group headings and variables, as listed.
    pub rows: Vec<tabs::variables::VarRow>,
    pub selected: usize,
    pub(crate) list: ListState,
    pub editing: bool,
    pub edit_key_area: TextArea<'static>,
    pub edit_value_area: TextArea<'static>,
    pub edit_field: EditField,
    /// Where the form saves, the variable's name before the edit (`None`
    /// for a new one), and the group's name for the title.
    pub edit_owner: tabs::variables::Owner,
    pub edit_old: Option<String>,
    pub edit_title: String,
    /// Which frame has keyboard focus (0 = variable list).
    pub focused_frame: usize,
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
    pub selected: usize,
    /// Clipboard backend configuration.
    pub clipboard: ClipboardSettingsState,
    /// Which frame has keyboard focus (0=runtime info, 1=clipboard config,
    /// 2=options: the settings registry).
    pub focused_frame: usize,
    /// The Options frame (keys, timeouts, limits).
    pub options: OptionsState,
}

/// The Settings tab's Options frame: one row per `herdr_mcp_api::settings::SPECS`.
#[derive(Default)]
pub struct OptionsState {
    /// Highlighted row (index into `SPECS`).
    pub selected: usize,
    /// Editing the highlighted row's value in `edit_area`.
    pub editing: bool,
    pub edit_area: TextArea<'static>,
    /// Outcome of the last save / reset (or why it was refused).
    pub msg: String,
}

/// Clipboard config sub-state for the Settings tab.
pub struct ClipboardSettingsState {
    /// Which preset row is highlighted (index into `BackendPreset::ALL`).
    pub preset_index: usize,
    /// User-editable copy command (may diverge from the preset default).
    pub copy_cmd: String,
    /// User-editable paste command.
    pub paste_cmd: String,
    /// Which clipboard input is focused: 0 = preset list, 1 = copy field,
    /// 2 = paste field.
    pub field_focus: usize,
    /// True when the user is typing into a command field (textarea editor).
    pub editing: bool,
    /// Which field is being edited (0 = copy, 1 = paste).
    pub edit_field: usize,
    /// The TextArea buffer for inline editing of command fields.
    pub edit_area: tui_textarea::TextArea<'static>,
    /// Last test outcome message (empty = no test run yet).
    pub test_msg: String,
    /// Last save outcome message (empty = no save yet).
    pub save_msg: String,
}

/// Clipboard backend presets offered in the Settings radio.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BackendPreset {
    /// The server's built-in OS clipboard (no commands).
    BuiltIn,
    Xsel,
    Xclip,
    WlCopy,
    Pbcopy,
    Custom,
}

impl BackendPreset {
    /// All presets in display order.
    pub const ALL: [BackendPreset; 6] = [
        BackendPreset::BuiltIn,
        BackendPreset::Xsel,
        BackendPreset::Xclip,
        BackendPreset::WlCopy,
        BackendPreset::Pbcopy,
        BackendPreset::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Xsel => "xsel",
            Self::Xclip => "xclip",
            Self::WlCopy => "wl-copy",
            Self::Pbcopy => "pbcopy",
            Self::Custom => "custom",
        }
    }

    /// Default (copy, paste) commands for this preset. `BuiltIn` and `Custom`
    /// return empty.
    pub fn commands(self) -> (&'static str, &'static str) {
        match self {
            Self::Xsel => ("xsel --clipboard --input", "xsel --clipboard --output"),
            Self::Xclip => (
                "xclip -selection clipboard",
                "xclip -selection clipboard -o",
            ),
            Self::WlCopy => ("wl-copy", "wl-paste"),
            Self::Pbcopy => ("pbcopy", "pbpaste"),
            Self::BuiltIn | Self::Custom => ("", ""),
        }
    }
}

impl App {
    /// Store the bridge config and load its clipboard commands into the
    /// Settings editor: the matching preset, else Custom; `BuiltIn` when the
    /// server has no commands.
    pub fn set_config(&mut self, config: ConfigResponse) {
        let (copy, paste) = config.clipboard.commands().unwrap_or_default();
        let clip = &mut self.settings.clipboard;
        clip.preset_index = BackendPreset::ALL
            .iter()
            .position(|p| p.commands() == (copy, paste))
            .unwrap_or(BackendPreset::ALL.len() - 1);
        clip.copy_cmd = copy.to_string();
        clip.paste_cmd = paste.to_string();
        self.config = Some(config);
    }

    /// Trace a debug line (see `log_debug`) tagged with this app's data dir.
    fn log_debug(&self, msg: impl AsRef<str>) {
        log_debug(&self.opts.data_dir, msg);
    }

    fn new(opts: DashboardOptions) -> Self {
        let knobs = herdr_mcp_api::Settings::default();
        let http = http::client_for(
            opts.http_port,
            knobs.duration(settings::TUI_REQUEST_MS),
            &opts.data_dir,
        )
        .ok();
        let http_port = opts.http_port;
        let data_dir = opts.data_dir.display().to_string();
        Self {
            opts,
            knobs,
            job: None,
            http,
            tab: Tab::Send,
            herdr: HerdrContext::default(),
            quitting: false,
            status_msg: String::new(),
            last_refresh: Instant::now(),
            recipes: None,
            templates: None,
            config: None,
            playground: Default::default(),
            send: SendState {
                pane: None,
                text: TextArea::default(),
                enter: true,
                msg: String::new(),
            },
            overview: OverviewState {
                pane_index: 0,
                focused_frame: 0,
                table_offset: 0,
                view: Default::default(),
            },
            variables_state: VariablesState {
                rows: Vec::new(),
                selected: 0,
                list: ListState::default(),
                editing: false,
                edit_key_area: TextArea::default(),
                edit_value_area: TextArea::default(),
                edit_field: EditField::Key,
                edit_owner: tabs::variables::Owner::Global,
                edit_old: None,
                edit_title: String::new(),
                focused_frame: 0,
            },
            settings: SettingsState {
                http_port,
                data_dir,
                selected: 0,
                clipboard: ClipboardSettingsState {
                    preset_index: 0,
                    copy_cmd: String::new(),
                    paste_cmd: String::new(),
                    field_focus: 0,
                    editing: false,
                    edit_field: 0,
                    edit_area: tui_textarea::TextArea::default(),
                    test_msg: String::new(),
                    save_msg: String::new(),
                },
                focused_frame: 2,
                options: OptionsState::default(),
            },
            bridge_connected: false,
            tab_rects: Vec::new(),
            var_list_inner: Rect::default(),
            pane_table_inner: Rect::default(),
            pane_text_inner: Rect::default(),
            clipboard_preset_rects: Vec::new(),
            clipboard_btn_rects: Vec::new(),
            clipboard_field_rects: Vec::new(),
            context_menu: ContextMenu {
                open: false,
                x: 0,
                y: 0,
                index: 0,
            },
            dirty: true,
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
        // Cap total refresh time so the UI never freezes for more than 2s.
        if tokio::time::timeout(Duration::from_secs(2), self.refresh_inner())
            .await
            .is_err()
        {
            self.status_msg = "refresh timed out".into();
        }
        self.last_refresh = Instant::now();
    }

    async fn refresh_inner(&mut self) {
        // If we're not connected, check for a live bridge first.
        if !self.bridge_connected {
            let timeout = self.knobs.duration(settings::TUI_REQUEST_MS);
            if let Some(client) =
                http::live_bridge(self.opts.http_port, timeout, &self.opts.data_dir).await
            {
                self.http = Some(client);
                self.bridge_connected = true;
                self.status_msg.clear();
            } else {
                self.bridge_connected = false;
                self.status_msg = format!(
                    "bridge unreachable at :{} — see herdr-mcp.log in the data dir",
                    self.opts.http_port
                );
            }
        }

        if self.bridge_connected
            && let Some(http) = self.http.clone()
        {
            let mut ok = true;
            // Always re-fetch tools on refresh so Ctrl+R picks up new tools.
            if let Ok(v) = http.list_tools().await {
                tabs::playground::set_tools(self, &v);
            } else {
                ok = false;
            }
            // The collections too.
            self.refresh_variables().await;
            if let Ok(v) = http.list_templates().await {
                self.templates = Some(v);
            }
            if self.tab == Tab::Playground && self.playground.view == tabs::playground::View::Runs {
                tabs::playground::runs::refresh(self).await;
            }
            if self.config.is_none()
                && let Ok(c) = http.config().await
            {
                self.set_config(c);
            }
            if let Ok(s) = http.settings().await {
                self.apply_settings(s);
            }
            if !ok {
                self.bridge_connected = false;
            }
        }
    }

    /// Take the bridge's settings: keys and intervals apply now; a new
    /// request timeout rebuilds the client.
    pub fn apply_settings(&mut self, response: herdr_mcp_api::SettingsResponse) {
        let timeout = self.knobs.duration(settings::TUI_REQUEST_MS);
        self.knobs = response.overrides();
        let next = self.knobs.duration(settings::TUI_REQUEST_MS);
        if next != timeout
            && self.http.is_some()
            && let Ok(client) = http::client_for(self.opts.http_port, next, &self.opts.data_dir)
        {
            self.http = Some(client);
        }
    }

    /// Refresh sessions and the shown session's panes. herdr's pane list is
    /// what exists; the registry (`GET /api/agents`, this server's session
    /// only) adds role and output by pane id.
    /// Show tab `t`; the Variables tab re-reads what it lists.
    async fn switch_tab(&mut self, t: Tab) {
        self.tab = t;
        self.status_msg.clear();
        if t == Tab::Variables {
            self.refresh_variables().await;
        }
    }

    async fn refresh_herdr(&mut self) {
        self.herdr.last_error = None;
        let Some(http) = self.http.clone() else {
            self.herdr.panes.clear();
            return;
        };
        match http.list_sessions().await {
            Ok(sessions) => self.herdr.sessions = sessions,
            Err(e) => self.note_herdr_error(format!("list_sessions failed: {e}")),
        }
        // A shown session that stopped or went away: back to our own.
        if let Some(name) = &self.herdr.session
            && !self
                .herdr
                .sessions
                .iter()
                .any(|s| &s.name == name && s.running)
        {
            self.herdr.session = None;
        }
        let filter = herdr_mcp_api::PaneFilter {
            session: self.herdr.session.clone(),
            ..Default::default()
        };
        let panes = match http.list_panes(&filter).await {
            Ok(panes) => panes,
            Err(e) => {
                self.herdr.panes.clear();
                self.note_herdr_error(format!("list_panes failed: {e}"));
                return;
            }
        };
        self.herdr.panes = panes;
    }

    /// The pane picked in the Overview table, if any.
    fn picked_pane(&self) -> Option<&str> {
        let panes = &self.herdr.panes;
        panes
            .get(self.overview.pane_index.min(panes.len().saturating_sub(1)))
            .map(|p| p.pane_id.as_str())
    }

    /// Whether the Overview's pane text needs a read: another pane was
    /// picked, or the interval passed and no selection holds it still.
    fn pane_read_due(&self) -> bool {
        let view = &self.overview.view;
        let picked = self.picked_pane();
        if picked != view.pane_id.as_deref() {
            return true;
        }
        picked.is_some()
            && !view.frozen()
            && view
                .read_at
                .is_none_or(|t| t.elapsed() > self.knobs.duration(settings::TUI_PANE_MS))
    }

    /// Read the picked pane (screen or scrollback) into the Overview.
    async fn read_picked_pane(&mut self) {
        let Some(pane_id) = self.picked_pane().map(str::to_string) else {
            self.overview.view.clear();
            return;
        };
        let Some(http) = self.http.clone() else {
            self.overview
                .view
                .update(&pane_id, Err("bridge not connected".into()));
            return;
        };
        let (source, lines) = if self.overview.view.scrollback {
            ("recent", Some(tabs::overview::SCROLLBACK))
        } else {
            ("visible", None)
        };
        let read = http.read_pane(&pane_id, self.herdr.session.as_deref(), source, lines);
        let text = match tokio::time::timeout(Duration::from_secs(2), read).await {
            Ok(r) => r.map_err(|e| e.to_string()),
            Err(_) => Err("read timed out".into()),
        };
        self.overview.view.update(&pane_id, text);
    }

    fn note_herdr_error(&mut self, msg: String) {
        self.log_debug(msg.clone());
        self.herdr.last_error = Some(msg);
    }

    /// The active editable `TextArea` (live editor) for the current tab/field.
    fn active_textarea_mut(&mut self) -> Option<&mut TextArea<'static>> {
        match self.tab {
            Tab::Send => Some(&mut self.send.text),
            Tab::Playground => self.playground.edit_area_mut(),
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
            Tab::Send => Some(&self.send.text),
            Tab::Playground => self.playground.edit_area(),
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
    /// selection). Falls back to the focused playground field.
    fn focused_field_text(&self) -> Option<String> {
        match self.tab {
            Tab::Playground => tabs::playground::focused_text(self),
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
        if self.tab == Tab::Overview && self.overview.focused_frame == 1 {
            return Some(self.overview.view.copy_text());
        }
        if self.tab == Tab::Playground && self.playground.focus == tabs::playground::Focus::Response
        {
            let ta = &self.playground.result_area;
            let full = ta.lines().join("\n");
            return Some(textarea_selection_or_full(ta, full));
        }
        if let Some(ta) = self.active_textarea_ref() {
            let full = ta.lines().join("\n");
            return Some(textarea_selection_or_full(ta, full));
        }
        self.focused_field_text()
            .or_else(|| self.focused_copyable_text())
    }

    /// Resolve the copy target for any tab/frame based on what the user
    /// is currently focused on. Used as the final fallback when no
    /// interactive editor or playground field is active.
    fn focused_copyable_text(&self) -> Option<String> {
        match self.tab {
            Tab::Overview => {
                let pane = self.herdr.panes.get(self.overview.pane_index)?;
                Some(pane.pane_id.clone())
            }
            Tab::Variables if !self.variables_state.editing => {
                match self
                    .variables_state
                    .rows
                    .get(self.variables_state.selected)?
                {
                    tabs::variables::VarRow::Var(_, k, v) => Some(format!("{k} = {v}")),
                    tabs::variables::VarRow::Heading(..) => None,
                }
            }
            Tab::Settings if self.settings.focused_frame == 0 => {
                let row = self.settings.selected;
                match row {
                    0 => Some(self.settings.http_port.to_string()),
                    1 => Some(self.settings.data_dir.clone()),
                    2 => self.config.as_ref().map(|c| c.herdr_socket.clone()),
                    _ => None,
                }
            }
            Tab::Settings
                if self.settings.focused_frame == 1 && self.settings.clipboard.field_focus == 1 =>
            {
                Some(self.settings.clipboard.copy_cmd.clone())
            }
            Tab::Settings
                if self.settings.focused_frame == 1 && self.settings.clipboard.field_focus == 2 =>
            {
                Some(self.settings.clipboard.paste_cmd.clone())
            }
            _ => None,
        }
    }

    /// True when the current copy target is the read-only Result pane (so
    /// Cut/Paste must be suppressed).
    fn copy_target_is_readonly(&self) -> bool {
        (self.tab == Tab::Playground && self.playground.focus == tabs::playground::Focus::Response)
            || (self.tab == Tab::Overview && self.overview.focused_frame == 1)
    }

    /// Feed a key event into the active editable `TextArea` (if any).
    pub(crate) fn feed_active_textarea(&mut self, code: KeyCode, mods: KeyModifiers) {
        if let Some(ta) = self.active_textarea_mut()
            && let Some(input) = to_textarea_input(code, mods)
        {
            ta.input(input);
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
                Ok(()) => {
                    self.status_msg = format!("copied {} chars", text.len());
                    self.log_debug("set_clipboard ok");
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
        self.log_debug(format!("clipboard_copy: text_len={}", text.len()));
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
        } else if self.tab == Tab::Playground
            && let Some(t) = tabs::playground::cut(self)
        {
            // No live editor: the focused field, opened empty.
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
            Tab::Playground => tabs::playground::paste(self, text),
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
        let Some(http) = self.http.clone() else {
            self.status_msg = "bridge not connected".into();
            return;
        };
        let text = match Some(http.clone()) {
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
        if let Err(msg) = self.herdr.writable() {
            self.status_msg = msg;
            return;
        }
        // The Send tab's pane, not herdr's focused one: right-clicking the
        // dashboard focuses the dashboard's own pane.
        let pane_id = match tabs::send::target(self) {
            Ok(p) => p.pane_id.clone(),
            Err(msg) => {
                self.status_msg = msg;
                return;
            }
        };
        let params = serde_json::json!({ "pane_id": pane_id, "text": text });
        match http.call_tool("send_text", &params).await {
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

/// A key as `tui_textarea` input; `None` for keys it has no use for.
pub(crate) fn to_textarea_input(code: KeyCode, mods: KeyModifiers) -> Option<TaInput> {
    let input = TaInput::from(KeyEvent::new(code, mods));
    (input.key != TaKey::Null).then_some(input)
}

/// Entry point invoked from the CLI `dashboard` subcommand.
pub async fn run(opts: DashboardOptions) -> Result<()> {
    let mut app = App::new(opts);
    app.refresh().await;
    app.refresh_herdr().await;

    // Release builds abort on panic (no unwinding past here), so put the
    // terminal back in the hook or the shell is left raw, on the alternate
    // screen, with mouse capture on. Then print the panic as usual.
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        report(info);
    }));
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    // Legacy encoding sends Ctrl+2 as NUL and Ctrl+3 as ESC, so those tab
    // hotkeys never arrive as digits; the kitty protocol, where the
    // terminal has it, reports them as Ctrl+'2' / Ctrl+'3'.
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("creating ratatui terminal")?;

    let result = main_loop(&mut terminal, &mut app).await;

    if enhanced {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)?;
    }
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    result
}

/// Undo [`run`]'s terminal setup, best effort (the panic path). Popping
/// keyboard flags that were never pushed is a no-op.
fn restore_terminal() {
    let mut out = io::stdout();
    let _ = execute!(out, PopKeyboardEnhancementFlags);
    let _ = disable_raw_mode();
    let _ = execute!(
        out,
        LeaveAlternateScreen,
        DisableMouseCapture,
        crossterm::cursor::Show
    );
}

/// Core event loop: draws, polls, dispatches input.
async fn main_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    let frame_ms = 300u64;
    let context_refresh = Duration::from_secs(15);

    let mut last_ctx = Instant::now();

    loop {
        if app.quitting {
            return Ok(());
        }

        // Only redraw when state has changed.
        if app.dirty {
            let _ = terminal.draw(|frame| ui(frame, app));
            app.dirty = false;
        }

        // Wait up to a frame for input, then drain only what is already
        // queued: waiting a full frame per event would starve the redraw
        // for as long as keys keep coming (a held arrow key, steady typing).
        let mut poll = Duration::from_millis(frame_ms);
        while event::poll(poll)? {
            poll = Duration::ZERO;
            match event::read()? {
                Event::Key(k) => {
                    if k.kind != KeyEventKind::Press {
                        continue;
                    }
                    handle_key(app, k.code, k.modifiers).await?;
                    app.dirty = true;
                }
                Event::Mouse(m) => {
                    handle_mouse(app, m).await?;
                    app.dirty = true;
                }
                Event::Resize(_, _) => {
                    app.dirty = true;
                }
                _ => {}
            }
            if app.quitting {
                return Ok(());
            }
        }

        if app.playground.events.drain() {
            app.dirty = true;
        }
        if app.job.as_ref().is_some_and(|j| j.is_finished())
            && let Some(job) = app.job.take()
        {
            tabs::playground::finish_job(app, job).await;
            app.dirty = true;
        }
        if app.last_refresh.elapsed() > app.knobs.duration(settings::TUI_REFRESH_MS) {
            app.refresh().await;
            app.dirty = true;
        }
        if app.tab == Tab::Overview && app.pane_read_due() {
            app.read_picked_pane().await;
            app.dirty = true;
        }
        if last_ctx.elapsed() > context_refresh {
            app.refresh_herdr().await;
            last_ctx = Instant::now();
            app.dirty = true;
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
    spans.push(Span::styled(
        format!("   session: {}", app.herdr.session_name()),
        muted_style(),
    ));
    if let Some(p) = app.herdr.focused() {
        spans.push(Span::styled(
            format!("   workspace: {}   pane: {}", p.workspace_id, p.pane_id),
            muted_style(),
        ));
    }
    spans.push(Span::styled(
        format!("   {} panes", app.herdr.panes.len()),
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
    if app.tab == Tab::Settings {
        tabs::settings::render(frame, area, app);
        return;
    }
    match app.tab {
        Tab::Send => tabs::send::render(frame, area, app),
        Tab::Overview => tabs::overview::render(frame, area, app),
        Tab::Playground => tabs::playground::render(frame, area, app),
        Tab::Variables => tabs::variables::render(frame, area, app),
        _ => {}
    }
}

/// Footer with status message + keybindings.
fn draw_footer(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let first = |key: &str| first_chord(&app.knobs, key);
    let bindings = format!(
        "{}/{}:next/prev tab  {}…{}:tab  ↑/↓/Home/End:nav  {}:refresh  ctrl+v:paste  ctrl+c:copy  ctrl+x:cut  right-click:menu  {}:quit",
        first("keys.tab-next"),
        first("keys.tab-prev"),
        first("keys.tab-1"),
        first("keys.tab-5"),
        first("keys.refresh"),
        first("keys.quit"),
    );
    let mut line = Line::from(vec![Span::styled(bindings.clone(), dim_style())]);
    if !app.status_msg.is_empty() {
        line = Line::from(vec![
            Span::styled(bindings, dim_style()),
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

    // Tab hotkeys win over the active tab, whatever has focus.
    if let Some(t) = tab_hotkey(&app.knobs, code, mods, app.tab) {
        app.switch_tab(t).await;
        return Ok(());
    }

    // Editing inputs are handled inside the active tab first.
    let consumed = match app.tab {
        Tab::Send => tabs::send::handle_key(app, code, mods).await?,
        Tab::Overview => tabs::overview::handle_key(app, code, mods).await?,
        Tab::Playground => tabs::playground::handle_key(app, code, mods).await?,
        Tab::Variables => tabs::variables::handle_key(app, code, mods).await?,
        Tab::Settings => tabs::settings::handle_key(app, code, mods).await?,
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

    // Global keys (Settings → keys.quit / keys.refresh).
    match key_action(&app.knobs, code, mods) {
        Some("quit") => app.quitting = true,
        Some("refresh") => {
            app.refresh().await;
            app.status_msg = "refreshed".to_string();
        }
        _ => {}
    }
    Ok(())
}

/// The tab a key switches to from `current`: Ctrl+Tab / Alt+→ next,
/// Ctrl+Shift+Tab / Alt+← previous (wrapping), Ctrl+1-5 or Alt+1-5
/// directly. Without the kitty protocol a terminal (VTE, for one) sends
/// Ctrl+Tab as a plain Tab, Ctrl+2 as NUL, which crossterm reads as
/// Ctrl+Space, and Ctrl+1 / Ctrl+3 as `1` / Esc; there Alt+←/→ and
/// Alt+1-5 still work, and Ctrl+Space maps back to tab 2.
fn tab_hotkey(
    knobs: &herdr_mcp_api::Settings,
    code: KeyCode,
    mods: KeyModifiers,
    current: Tab,
) -> Option<Tab> {
    let n = Tab::ALL.len();
    let at = Tab::ALL.iter().position(|t| *t == current).unwrap_or(0);
    let idx = match key_action(knobs, code, mods)? {
        "tab-next" => (at + 1) % n,
        "tab-prev" => (at + n - 1) % n,
        action => action
            .strip_prefix("tab-")?
            .parse::<usize>()
            .ok()?
            .checked_sub(1)?,
    };
    Tab::ALL.get(idx).copied()
}

/// The first chord bound to setting `key`, for hints ("-" if none).
pub(crate) fn first_chord(knobs: &herdr_mcp_api::Settings, key: &str) -> String {
    knobs
        .chords(key)
        .first()
        .map_or_else(|| "-".to_string(), ToString::to_string)
}

/// The `keys.*` action (Settings) a key press is bound to.
fn key_action(
    knobs: &herdr_mcp_api::Settings,
    code: KeyCode,
    mods: KeyModifiers,
) -> Option<&'static str> {
    knobs.action_for(&chord_of(code, mods)?)
}

/// A key press as a Settings chord. Letters carry case in `shift`; other
/// characters already encode it (`}` is shifted `]`), so a reported Shift is
/// dropped there; BackTab is Shift+Tab.
fn chord_of(code: KeyCode, mods: KeyModifiers) -> Option<herdr_mcp_api::Chord> {
    use herdr_mcp_api::settings::Key;
    let mut shift = mods.contains(KeyModifiers::SHIFT);
    let key = match code {
        KeyCode::Char(c) if c.is_alphabetic() => {
            shift |= c.is_uppercase();
            Key::Char(c.to_ascii_lowercase())
        }
        KeyCode::Char(c) => {
            shift = false;
            Key::Char(c)
        }
        KeyCode::F(n) => Key::F(n),
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => {
            shift = true;
            Key::Tab
        }
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        _ => return None,
    };
    Some(herdr_mcp_api::Chord {
        ctrl: mods.contains(KeyModifiers::CONTROL),
        alt: mods.contains(KeyModifiers::ALT),
        shift,
        key,
    })
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

/// Whether mouse event `m` is inside `area`.
pub(crate) fn inside(area: Rect, m: &MouseEvent) -> bool {
    m.column >= area.x && m.column < area.right() && m.row >= area.y && m.row < area.bottom()
}

/// Mouse event handler: click tabs to switch, click lists to select, wheel scroll.
async fn handle_mouse(app: &mut App, m: MouseEvent) -> Result<()> {
    match m.kind {
        // Ignore mouse move events — they fire on every pixel of movement
        // and cause CPU thrashing without any useful state change.
        MouseEventKind::Moved => return Ok(()),
        _ if app.tab == Tab::Playground
            && !app.context_menu.open
            && tabs::playground::handle_mouse(app, m).await =>
        {
            return Ok(());
        }
        MouseEventKind::Down(MouseButton::Right) => {
            // Region-aware: over the response, Copy takes its selection.
            if app.tab == Tab::Playground && inside(app.playground.result_inner, &m) {
                app.playground.focus = tabs::playground::Focus::Response;
            }
            // Over the Overview's pane text, Copy takes its selection.
            if app.tab == Tab::Overview {
                app.overview.focused_frame = usize::from(inside(app.pane_text_inner, &m));
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
                    app.switch_tab(Tab::ALL[i]).await;
                    return Ok(());
                }
            }
            // Variable list.
            if app.tab == Tab::Variables
                && !app.variables_state.editing
                && m.column >= app.var_list_inner.x
                && m.column < app.var_list_inner.right()
                && m.row >= app.var_list_inner.y
                && m.row < app.var_list_inner.bottom()
            {
                let idx =
                    app.variables_state.list.offset() + (m.row - app.var_list_inner.y) as usize;
                if idx < app.variables_state.rows.len() {
                    app.variables_state.selected = idx;
                }
                return Ok(());
            }
            // Overview pane table click (row 0 is the header).
            if app.tab == Tab::Overview && inside(app.pane_table_inner, &m) {
                app.overview.focused_frame = 0;
                if let Some(row) = (m.row - app.pane_table_inner.y).checked_sub(1) {
                    let idx = app.overview.table_offset + row as usize;
                    if idx < app.herdr.panes.len() {
                        app.overview.pane_index = idx;
                    }
                }
                return Ok(());
            }
            // Overview pane text: start a selection.
            if app.tab == Tab::Overview && inside(app.pane_text_inner, &m) {
                app.overview.focused_frame = 1;
                let view = &mut app.overview.view;
                let pos = view.pos_at(
                    (m.row - app.pane_text_inner.y) as usize,
                    (m.column - app.pane_text_inner.x) as usize,
                );
                view.anchor = Some(pos);
                view.head = pos;
                view.dragging = true;
                return Ok(());
            }
            // Clipboard preset rows (Settings tab).
            if app.tab == Tab::Settings && !app.settings.clipboard.editing {
                for (i, r) in app.clipboard_preset_rects.iter().enumerate() {
                    if m.row == r.y && m.column >= r.x && m.column < r.x + r.width {
                        app.settings.clipboard.preset_index = i;
                        let p = crate::BackendPreset::ALL[i];
                        app.settings.clipboard.copy_cmd = p.commands().0.to_string();
                        app.settings.clipboard.paste_cmd = p.commands().1.to_string();
                        app.settings.clipboard.field_focus = 0;
                        return Ok(());
                    }
                }
                // Clipboard Test / Save buttons.
                for (i, r) in app.clipboard_btn_rects.iter().enumerate() {
                    if m.row == r.y && m.column >= r.x && m.column < r.x + r.width {
                        if i == 0 {
                            tabs::settings::handle_key(app, KeyCode::Char('t'), KeyModifiers::NONE)
                                .await?;
                        } else if i == 1 {
                            tabs::settings::handle_key(app, KeyCode::Char('s'), KeyModifiers::NONE)
                                .await?;
                        }
                        return Ok(());
                    }
                }
                // Clipboard copy/paste command fields (click to focus + enter for edit).
                for (i, r) in app.clipboard_field_rects.iter().enumerate() {
                    if m.row == r.y && m.column >= r.x && m.column < r.x + r.width {
                        app.settings.clipboard.field_focus = i + 1; // 1=copy, 2=paste
                        app.settings.clipboard.editing = true;
                        app.settings.clipboard.edit_field = i;
                        let text = if i == 0 {
                            &app.settings.clipboard.copy_cmd
                        } else {
                            &app.settings.clipboard.paste_cmd
                        };
                        app.settings.clipboard.edit_area =
                            tui_textarea::TextArea::new(vec![text.clone()]);
                        return Ok(());
                    }
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left)
            if app.tab == Tab::Overview && app.overview.view.dragging =>
        {
            // Past the top or bottom edge scrolls, so a selection can run
            // longer than the panel.
            let area = app.pane_text_inner;
            let view = &mut app.overview.view;
            if m.row < area.y {
                view.scroll(-1);
            } else if m.row >= area.bottom() {
                view.scroll(1);
            }
            let row = m.row.clamp(area.y, area.bottom().saturating_sub(1)) - area.y;
            let col = m.column.saturating_sub(area.x);
            view.head = view.pos_at(row as usize, col as usize);
        }
        MouseEventKind::Up(MouseButton::Left) if app.overview.view.dragging => {
            app.overview.view.dragging = false;
        }
        MouseEventKind::ScrollDown
            if app.tab == Tab::Overview && inside(app.pane_text_inner, &m) =>
        {
            app.overview.view.scroll(3);
        }
        MouseEventKind::ScrollUp if app.tab == Tab::Overview && inside(app.pane_text_inner, &m) => {
            app.overview.view.scroll(-3);
        }
        MouseEventKind::ScrollDown => match app.tab {
            Tab::Variables
                if !app.variables_state.editing
                    && app.variables_state.selected + 1 < app.variables_state.rows.len() =>
            {
                app.variables_state.selected += 1;
            }
            Tab::Settings if !app.settings.clipboard.editing => {
                app.settings.selected = (app.settings.selected + 1).min(9);
            }
            Tab::Overview if app.overview.pane_index + 1 < app.herdr.panes.len() => {
                app.overview.pane_index += 1;
            }
            _ => {}
        },
        MouseEventKind::ScrollUp => match app.tab {
            Tab::Variables if !app.variables_state.editing && app.variables_state.selected > 0 => {
                app.variables_state.selected -= 1;
            }
            Tab::Settings if !app.settings.clipboard.editing && app.settings.selected > 0 => {
                app.settings.selected -= 1;
            }
            Tab::Overview if app.overview.pane_index > 0 => {
                app.overview.pane_index -= 1;
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
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

    // ── Clipboard / result selection ─────────────────────────────────────

    #[test]
    fn set_config_loads_clipboard_commands_into_editor() {
        let cfg = |copy: Option<&str>, paste: Option<&str>| ConfigResponse {
            data_dir: "d".into(),
            herdr_socket: "s".into(),
            herdr_bin: "herdr".into(),
            clipboard: herdr_mcp_api::ClipboardConfig {
                copy_command: copy.map(Into::into),
                paste_command: paste.map(Into::into),
            },
            config_path: "herdr-mcp.toml".into(),
        };
        let mut app = test_app();

        app.set_config(cfg(Some("wl-copy"), Some("wl-paste")));
        let clip = &app.settings.clipboard;
        assert_eq!(BackendPreset::ALL[clip.preset_index], BackendPreset::WlCopy);
        assert_eq!(
            (clip.copy_cmd.as_str(), clip.paste_cmd.as_str()),
            ("wl-copy", "wl-paste")
        );

        app.set_config(cfg(Some("my-copy"), Some("my-paste")));
        let clip = &app.settings.clipboard;
        assert_eq!(BackendPreset::ALL[clip.preset_index], BackendPreset::Custom);
        assert_eq!(clip.copy_cmd, "my-copy");

        // No commands: the built-in clipboard.
        app.set_config(cfg(None, None));
        let clip = &app.settings.clipboard;
        assert_eq!(
            BackendPreset::ALL[clip.preset_index],
            BackendPreset::BuiltIn
        );
        assert_eq!(clip.copy_cmd, "");
        assert!(app.config.is_some());
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
        let step = herdr_mcp_api::RecipeStep {
            id: "hello".into(),
            tool: "status".into(),
            ..Default::default()
        };
        let rid = herdr_mcp_api::Uuid::nil();
        app.playground.request = Some(tabs::playground::RequestForm::new(rid, &step));
        app.playground.pick = tabs::playground::Pick::Request(rid, "hello".into());
        app.playground.focus = tabs::playground::Focus::Main;
        // Row 0 is the request's name.
        assert_eq!(app.selection_or_full_text(), Some("hello".to_string()));
    }

    #[test]
    fn selection_or_full_text_uses_full_result_when_focused() {
        let mut app = test_app();
        app.tab = Tab::Playground;
        app.playground.focus = tabs::playground::Focus::Response;
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
        app.playground.focus = tabs::playground::Focus::Response;
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
        let mut app = test_app();
        // The Send tab (the default) always has its text box to copy from.
        assert_eq!(app.selection_or_full_text().as_deref(), Some(""));
        app.tab = Tab::Overview;
        assert_eq!(app.selection_or_full_text(), None);
    }

    #[test]
    fn copy_target_is_readonly_only_for_result() {
        let mut app = test_app();
        app.tab = Tab::Playground;
        app.playground.focus = tabs::playground::Focus::Response;
        assert!(app.copy_target_is_readonly());
        app.playground.focus = tabs::playground::Focus::Tree;
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
    fn tab_hotkeys_cover_legacy_encodings() {
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Char('1'),
                ctrl,
                Tab::Send
            ),
            Some(Tab::Send)
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Char('2'),
                ctrl,
                Tab::Send
            ),
            Some(Tab::Overview)
        );
        // Ctrl+2 without the kitty protocol: NUL, decoded as Ctrl+Space.
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Char(' '),
                ctrl,
                Tab::Send
            ),
            Some(Tab::Overview)
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Char('3'),
                KeyModifiers::ALT,
                Tab::Send
            ),
            Some(Tab::Playground)
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Char('5'),
                ctrl,
                Tab::Send
            ),
            Some(Tab::Settings)
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Char('6'),
                ctrl,
                Tab::Send
            ),
            None
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Char('2'),
                KeyModifiers::NONE,
                Tab::Send
            ),
            None
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Char(' '),
                KeyModifiers::NONE,
                Tab::Send
            ),
            None
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Esc,
                KeyModifiers::NONE,
                Tab::Send
            ),
            None
        );
    }

    #[test]
    fn chords_from_key_events_match_settings_spelling() {
        let chord = |code, mods| chord_of(code, mods).map(|c| c.to_string());
        let (ctrl, alt, shift) = (
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SHIFT,
        );
        assert_eq!(chord(KeyCode::Char('r'), ctrl).as_deref(), Some("ctrl+r"));
        // Letters: case is Shift, however the terminal reports it.
        assert_eq!(
            chord(KeyCode::Char('R'), ctrl).as_deref(),
            Some("ctrl+shift+r")
        );
        assert_eq!(
            chord(KeyCode::Char('r'), ctrl | shift).as_deref(),
            Some("ctrl+shift+r")
        );
        // Symbols carry their own shift: `}` is `}` with or without Shift.
        assert_eq!(
            chord(KeyCode::Char('}'), alt | shift).as_deref(),
            Some("alt+}")
        );
        assert_eq!(
            chord(KeyCode::BackTab, ctrl | shift).as_deref(),
            Some("ctrl+shift+tab")
        );
        assert_eq!(
            chord(KeyCode::Char(' '), ctrl).as_deref(),
            Some("ctrl+space")
        );
        assert_eq!(
            chord(KeyCode::F(6), KeyModifiers::NONE).as_deref(),
            Some("f6")
        );
        assert_eq!(chord(KeyCode::Null, ctrl), None);
    }

    #[test]
    fn remapped_tab_keys_take_effect_and_free_the_default() {
        use herdr_mcp_api::settings::Value;
        let mut knobs = herdr_mcp_api::Settings::default();
        knobs
            .set("keys.tab-next", Value::Keys(vec!["f6".into()]))
            .unwrap();
        knobs
            .set("keys.quit", Value::Keys(vec!["alt+q".into()]))
            .unwrap();
        let alt = KeyModifiers::ALT;
        assert_eq!(
            tab_hotkey(&knobs, KeyCode::F(6), KeyModifiers::NONE, Tab::Send),
            Some(Tab::Overview)
        );
        assert_eq!(tab_hotkey(&knobs, KeyCode::Right, alt, Tab::Send), None);
        assert_eq!(key_action(&knobs, KeyCode::Char('q'), alt), Some("quit"));
        assert_eq!(
            key_action(&knobs, KeyCode::Char('q'), KeyModifiers::CONTROL),
            None
        );
    }

    #[test]
    fn tab_hotkeys_cycle_and_wrap() {
        let (ctrl, alt) = (KeyModifiers::CONTROL, KeyModifiers::ALT);
        let next = |t| tab_hotkey(&herdr_mcp_api::Settings::default(), KeyCode::Tab, ctrl, t);
        assert_eq!(next(Tab::Send), Some(Tab::Overview));
        assert_eq!(next(Tab::Settings), Some(Tab::Send));
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Right,
                alt,
                Tab::Overview
            ),
            Some(Tab::Playground)
        );
        // Ctrl+Shift+Tab: kitty reports Tab + Shift; crossterm may fold it to BackTab.
        let back = ctrl | KeyModifiers::SHIFT;
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Tab,
                back,
                Tab::Send
            ),
            Some(Tab::Settings)
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::BackTab,
                back,
                Tab::Overview
            ),
            Some(Tab::Send)
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Left,
                alt,
                Tab::Send
            ),
            Some(Tab::Settings)
        );
        // Plain Tab / Shift+Tab / arrows stay with the tab (field focus, lists).
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Tab,
                KeyModifiers::NONE,
                Tab::Send
            ),
            None
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::BackTab,
                KeyModifiers::SHIFT,
                Tab::Send
            ),
            None
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Right,
                KeyModifiers::NONE,
                Tab::Send
            ),
            None
        );
        assert_eq!(
            tab_hotkey(
                &herdr_mcp_api::Settings::default(),
                KeyCode::Left,
                alt | KeyModifiers::SHIFT,
                Tab::Send
            ),
            None
        );
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

    #[test]
    fn log_debug_writes_to_file_not_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("tui-debug.log");
        log_debug(tmp.path(), "test message 12345");
        let contents = std::fs::read_to_string(&log_path).expect("log file should be created");
        assert!(
            contents.contains("test message 12345"),
            "log file should contain the message"
        );
        // Verify format: starts with `[` (timestamp prefix)
        assert!(
            contents.starts_with('['),
            "log line should start with timestamp"
        );
    }

    #[test]
    fn app_starts_with_dirty_true() {
        let app = test_app();
        assert!(
            app.dirty,
            "newly created App should be dirty to force initial draw"
        );
    }

    #[tokio::test]
    async fn handle_mouse_moved_does_not_modify_state() {
        let mut app = test_app();
        let snapshot = (app.tab, app.playground.tree, app.playground.focus);
        let evt = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column: 10,
            row: 5,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        handle_mouse(&mut app, evt).await.unwrap();
        assert_eq!(
            (app.tab, app.playground.tree, app.playground.focus),
            snapshot
        );
    }

    #[test]
    fn sessions_cycle_through_running_others() {
        let s = |name: &str, running: bool, current: bool| herdr_mcp_api::HerdrSession {
            name: name.into(),
            default: false,
            running,
            connection_error: None,
            socket_path: String::new(),
            session_dir: String::new(),
            current,
        };
        let mut ctx = HerdrContext {
            sessions: vec![
                s("default", true, true),
                s("work", true, false),
                s("old", false, false),
                s("lab", true, false),
            ],
            ..Default::default()
        };
        assert_eq!(ctx.session_name(), "default");
        assert!(ctx.writable().is_ok());
        ctx.cycle_session();
        assert_eq!(ctx.session.as_deref(), Some("work"));
        assert!(ctx.writable().is_err());
        ctx.cycle_session();
        assert_eq!(
            ctx.session.as_deref(),
            Some("lab"),
            "stopped sessions are skipped"
        );
        ctx.cycle_session();
        assert_eq!(ctx.session, None, "wraps back to the bridge's own");
    }
}

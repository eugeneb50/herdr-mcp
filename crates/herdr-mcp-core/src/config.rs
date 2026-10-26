use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    #[serde(default)]
    pub http: HttpConfig,

    #[serde(default)]
    pub herdr: HerdrConfig,

    /// Clipboard backend override for the `clipboard_set` / `clipboard_get` tools.
    #[serde(default)]
    pub clipboard: ClipboardConfig,

    /// `[settings.<section>]` overrides of the tunables in
    /// [`herdr_mcp_api::settings::SPECS`], raw: [`Config::settings`] keeps
    /// the valid ones, so a bad value falls back to its default instead of
    /// failing startup.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, BTreeMap<String, serde_json::Value>>,

    /// `[security]`: what callers may do. Read from the file only, never
    /// changed over the API.
    #[serde(default)]
    pub security: SecurityConfig,

    /// `[mcp]`: what MCP clients are shown.
    #[serde(default)]
    pub mcp: McpConfig,
}

/// `[security]`: limits on what MCP clients, recipe runs and the bridge may
/// do. Every field defaults to today's behaviour except `agent-triggers`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct SecurityConfig {
    /// Tools no caller can list or call (MCP clients, the bridge, recipe
    /// steps), e.g. `["run_command", "send_keys"]`.
    pub disabled_tools: Vec<String>,
    /// Hosts `http_request` and the LLM tools may reach: `api.example.com`
    /// or `*.example.com`. Empty: any host.
    pub allow_hosts: Vec<String>,
    /// Also refuse loopback and private-network addresses (`localhost`,
    /// `10.0.0.0/8`, …) unless the host is in `allow-hosts`. Link-local and
    /// cloud-metadata addresses are always refused unless it is.
    pub block_private: bool,
    /// The largest response body `http_request` and the LLM tools read.
    pub max_response_bytes: u64,
    /// Environment variables the LLM tools' `api_key_env` may name.
    pub key_env: Vec<String>,
    /// Whether MCP clients and recipe runs may add triggers or schedules,
    /// or change a recipe that has them. The bridge API (web UI, TUI)
    /// always may.
    pub agent_triggers: bool,
    /// Require the bridge token on `/api/*` even on a loopback bind (it is
    /// always required on any other).
    pub require_token: bool,
    /// Append every tool call (secrets redacted) to `<data-dir>/audit.jsonl`.
    pub audit_log: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            disabled_tools: Vec::new(),
            allow_hosts: Vec::new(),
            block_private: false,
            max_response_bytes: 10 * 1024 * 1024,
            key_env: ["OPENROUTER_API_KEY", "OPENAI_API_KEY", "LLM_API_KEY"]
                .map(String::from)
                .to_vec(),
            agent_triggers: false,
            require_token: false,
            audit_log: false,
        }
    }
}

impl Config {
    /// The valid `[settings.*]` overrides, plus a warning for each one
    /// dropped (back on its default).
    pub fn settings(&self) -> (herdr_mcp_api::Settings, Vec<String>) {
        herdr_mcp_api::Settings::from_file(&self.settings)
    }
}

/// Defined in `herdr-mcp-api` (it is also the `/api/config` wire type).
pub use herdr_mcp_api::ClipboardConfig;

/// The herdr binary to run: `HERDR_BIN`, else `HERDR_BIN_PATH` (herdr sets
/// it inside its panes), else `herdr` on `PATH`.
pub fn herdr_bin() -> String {
    env::var("HERDR_BIN")
        .or_else(|_| env::var("HERDR_BIN_PATH"))
        .unwrap_or_else(|_| "herdr".to_string())
}

impl Config {
    /// Make every `herdr` child process talk to the server this config
    /// resolved (inside a herdr pane: that pane's session; outside: the
    /// configured / `HERDR_SESSION` / default socket), so CLI calls and the
    /// socket client never disagree. Exported as `HERDR_SOCKET_PATH`, which
    /// herdr's own CLI honours.
    ///
    /// # Safety
    /// Mutates the process environment: call before any other thread runs
    /// (before the async runtime starts).
    pub unsafe fn export_herdr_socket(&self) {
        // SAFETY: the caller guarantees no other thread reads the environment.
        unsafe { env::set_var("HERDR_SOCKET_PATH", self.herdr_socket_path()) };
    }
}

/// `~/x` → `$HOME/x`; any other path unchanged.
fn expand_home(path: &std::path::Path) -> PathBuf {
    match (path.strip_prefix("~"), env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

/// Where state lives when nothing else says: herdr's state dir for this
/// plugin when herdr runs us as the `herdr-mcp` plugin (our working directory
/// is then the plugin checkout, replaced on update), else `./data`.
fn default_data_dir() -> PathBuf {
    plugin_state_dir(
        env::var("HERDR_PLUGIN_ID").ok().as_deref(),
        env::var_os("HERDR_PLUGIN_STATE_DIR"),
    )
    .unwrap_or_else(|| PathBuf::from("./data"))
}

/// The directory a project-local `herdr-mcp.toml` is read from: the working directory, except when herdr runs us as the
/// `herdr-mcp` plugin — the working directory is then the plugin checkout,
/// whose files are not the user's config.
fn project_dir() -> Option<PathBuf> {
    if env::var("HERDR_PLUGIN_ID").as_deref() == Ok(PLUGIN_ID) {
        return None;
    }
    env::current_dir().ok()
}

fn plugin_state_dir(
    plugin_id: Option<&str>,
    state_dir: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    (plugin_id == Some(PLUGIN_ID))
        .then_some(state_dir)
        .flatten()
        .map(PathBuf::from)
}

/// herdr-mcp's id as a herdr plugin (`herdr-plugin.toml`).
pub const PLUGIN_ID: &str = "herdr-mcp";

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            http: Default::default(),
            herdr: Default::default(),
            clipboard: Default::default(),
            settings: BTreeMap::new(),
            security: Default::default(),
            mcp: Default::default(),
        }
    }
}

/// `[mcp]`: what MCP clients are shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct McpConfig {
    /// The tools MCP clients list: `core` (panes, agents, waits, running
    /// saved recipes), `all`, a category (`Scheduler`, `Templates`, `LLM`,
    /// …) or a tool name. Each listed tool's schema costs the agent context
    /// on every turn. Unlisted tools still run in recipes and from the web
    /// UI and TUI; `[security] disabled-tools` turns a tool off everywhere.
    pub tools: Vec<String>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            tools: vec!["core".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HttpConfig {
    pub port: Option<u16>,
    pub bind_addr: Option<String>,
    pub http_only: bool,
    /// Directory holding the built Dioxus web UI (`dx bundle` output).
    /// Unset → the web UI built into the binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_dir: Option<PathBuf>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            port: Some(7676),
            bind_addr: Some("127.0.0.1".into()),
            http_only: false,
            web_dir: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HerdrConfig {
    pub socket_path: Option<PathBuf>,
}

impl Config {
    /// Load configuration with full precedence chain:
    /// Defaults < Config file < Env vars < CLI args
    pub fn load(cli_overrides: Option<CliOverrides>) -> Result<Self> {
        let mut config = Config::default();

        // 1. Load from config file (project config)
        if let Some(path) = find_config_file() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read config file: {}", path.display()))?;
            let file_config: Config = toml::from_str(&content)
                .with_context(|| format!("failed to parse config file: {}", path.display()))?;
            config = config.merge(file_config);
        }

        // 2. Apply environment variable overrides
        config = config.apply_env_overrides()?;

        // 3. Apply CLI overrides
        if let Some(cli) = cli_overrides {
            config = config.apply_cli_overrides(cli);
        }

        // Paths from TOML / env may start with `~/`; nothing else expands it.
        config.data_dir = expand_home(&config.data_dir);
        config.herdr.socket_path = config.herdr.socket_path.as_deref().map(expand_home);

        // 4. Validate
        config.validate()?;

        Ok(config)
    }

    /// Merge another config into this one (other wins on conflicts)
    fn merge(self, other: Config) -> Config {
        Config {
            data_dir: other.data_dir,
            http: other.http,
            herdr: other.herdr,
            clipboard: other.clipboard,
            settings: other.settings,
            security: other.security,
            mcp: other.mcp,
        }
    }

    /// Apply environment variable overrides
    fn apply_env_overrides(mut self) -> Result<Self> {
        // data_dir
        if let Ok(v) = env::var("HERDR_MCP_DATA_DIR") {
            self.data_dir = PathBuf::from(v);
        }

        // http
        if let Ok(v) = env::var("HERDR_MCP_HTTP_PORT") {
            self.http.port = Some(v.parse()?);
        }
        if let Ok(v) = env::var("HERDR_MCP_HTTP_ONLY") {
            self.http.http_only = v.parse()?;
        }
        if let Ok(v) = env::var("HERDR_MCP_HTTP_BIND") {
            self.http.bind_addr = Some(v);
        }
        if let Ok(v) = env::var("HERDR_MCP_WEB_DIR") {
            // Empty = unset: the web UI built into the binary.
            self.http.web_dir = Some(PathBuf::from(v)).filter(|d| !d.as_os_str().is_empty());
        }

        // herdr
        if let Ok(v) = env::var("HERDR_SOCKET_PATH") {
            self.herdr.socket_path = Some(PathBuf::from(v));
        }
        Ok(self)
    }

    /// Apply CLI overrides
    fn apply_cli_overrides(mut self, cli: CliOverrides) -> Self {
        if let Some(v) = cli.data_dir {
            self.data_dir = v;
        }
        if let Some(v) = cli.http_port {
            self.http.port = Some(v);
        }
        if let Some(v) = cli.http_only {
            self.http.http_only = v;
        }
        if let Some(v) = cli.herdr_socket {
            self.herdr.socket_path = Some(v);
        }
        self
    }

    /// Validate configuration
    fn validate(&self) -> Result<()> {
        if let Some(parent) = self.data_dir.parent()
            && !parent.exists()
        {
            anyhow::bail!("data_dir parent does not exist: {}", parent.display());
        }

        Ok(())
    }

    /// Get the resolved herdr socket path
    ///
    /// Precedence (mirrors herdr's `active_api_socket_path()` in `src/session.rs`):
    /// 1. Explicit `Config.herdr.socket_path` (from config file / env / CLI)
    /// 2. `HERDR_SESSION` env var → `~/.config/herdr/sessions/<name>/herdr.sock`
    ///    (only for named sessions; `default` is skipped)
    /// 3. Default: `~/.config/herdr/herdr.sock`
    pub fn herdr_socket_path(&self) -> PathBuf {
        self.herdr.socket_path.clone().unwrap_or_else(|| {
            // Named session (herdr session attach <name>) → sessions/<name>/herdr.sock
            if let Ok(session) = env::var("HERDR_SESSION")
                && !session.is_empty()
                && session != "default"
            {
                let home = env::var("HOME").unwrap_or_else(|_| ".".into());
                return PathBuf::from(home)
                    .join(".config/herdr/sessions")
                    .join(session)
                    .join("herdr.sock");
            }
            let home = env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".config/herdr/herdr.sock")
        })
    }
}

/// CLI overrides passed from main.rs after parsing
#[derive(Debug, Default, Clone)]
pub struct CliOverrides {
    pub data_dir: Option<PathBuf>,
    pub http_port: Option<u16>,
    pub http_only: Option<bool>,
    pub herdr_socket: Option<PathBuf>,
}

/// Returns the path to the config file that `Config::load` would read,
/// or `None` if no file was found (i.e. defaults would be used).
pub fn find_config_file() -> Option<PathBuf> {
    // 1. Explicit path from env
    if let Ok(p) = env::var("HERDR_MCP_CONFIG") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }

    // 2. Current directory
    if let Some(local) = project_dir().map(|d| d.join("herdr-mcp.toml"))
        && local.exists()
    {
        return Some(local);
    }

    // 3. XDG config dir
    if let Some(config_dir) = dirs::config_dir() {
        let xdg = config_dir.join("herdr-mcp/config.toml");
        if xdg.exists() {
            return Some(xdg);
        }
    }

    None
}

/// Resolve the config file path the loader *would* read, or — if none exists —
/// the first place it looks that a save can create: `herdr-mcp.toml` in the
/// project dir, or the XDG config file when there is none (as the plugin).
/// Used by the Settings tab's Save so it always has a concrete target.
pub fn config_file_or_default() -> PathBuf {
    find_config_file()
        .or_else(|| project_dir().map(|d| d.join("herdr-mcp.toml")))
        .or_else(|| dirs::config_dir().map(|d| d.join("herdr-mcp/config.toml")))
        .unwrap_or_else(|| PathBuf::from("herdr-mcp.toml"))
}

/// Write the `[clipboard]` table's `copy-command` / `paste-command` keys in the
/// config file at `path` (`None` removes them: the built-in clipboard),
/// preserving all other content (comments, tables, formatting). Creates the
/// file if absent.
pub fn set_clipboard_in_file(path: &std::path::Path, commands: Option<(&str, &str)>) -> Result<()> {
    edit_config_file(path, |doc| {
        if !doc.contains_table("clipboard") {
            doc.insert("clipboard", toml_edit::table());
        }
        let tbl = doc
            .get_mut("clipboard")
            .and_then(|item| item.as_table_mut())
            .context("[clipboard] is not a table")?;
        match commands {
            Some((copy, paste)) => {
                tbl.insert("copy-command", toml_edit::value(copy));
                tbl.insert("paste-command", toml_edit::value(paste));
            }
            None => {
                tbl.remove("copy-command");
                tbl.remove("paste-command");
                if tbl.is_empty() {
                    doc.remove("clipboard");
                }
            }
        }
        Ok(())
    })
}

/// Write setting `key` (`<section>.<name>`) as `[settings.<section>] name`
/// in the config file at `path`; `None` removes it (back to its default).
/// The value is not validated here: callers go through
/// [`herdr_mcp_api::Settings::set`] first.
pub fn set_setting_in_file(
    path: &std::path::Path,
    key: &str,
    value: Option<&herdr_mcp_api::settings::Value>,
) -> Result<()> {
    use herdr_mcp_api::settings::Value;
    let (section, name) = key
        .split_once('.')
        .context("a setting key is <section>.<name>")?;
    edit_config_file(path, |doc| {
        let settings = doc
            .entry("settings")
            .or_insert_with(implicit_table)
            .as_table_mut()
            .context("[settings] is not a table")?;
        let tbl = settings
            .entry(section)
            .or_insert_with(toml_edit::table)
            .as_table_mut()
            .with_context(|| format!("[settings.{section}] is not a table"))?;
        match value {
            Some(Value::Number(n)) => {
                let n = i64::try_from(*n).context("setting value too large")?;
                tbl.insert(name, toml_edit::value(n));
            }
            Some(Value::Keys(keys)) => {
                let list: toml_edit::Array = keys.iter().map(String::as_str).collect();
                tbl.insert(name, toml_edit::value(list));
            }
            None => {
                tbl.remove(name);
                if tbl.is_empty() {
                    settings.remove(section);
                }
            }
        }
        if settings.is_empty() {
            doc.remove("settings");
        }
        Ok(())
    })
}

/// Remove every `[settings.*]` override from the config file at `path`:
/// all settings back to their defaults.
pub fn reset_settings_in_file(path: &std::path::Path) -> Result<()> {
    edit_config_file(path, |doc| {
        doc.remove("settings");
        Ok(())
    })
}

/// `[settings]` holds only sub-tables, so it is never written as its own
/// (empty) header.
fn implicit_table() -> toml_edit::Item {
    let mut t = toml_edit::Table::new();
    t.set_implicit(true);
    toml_edit::Item::Table(t)
}

/// Apply `edit` to the config file at `path` (created if absent), keeping
/// everything else in it (comments, tables, formatting), and write it back
/// atomically.
fn edit_config_file(
    path: &std::path::Path,
    edit: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<()>,
) -> Result<()> {
    let content = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };

    // Parse into an editable document. A file that doesn't parse is an error,
    // never replaced: rewriting it would drop the user's other settings.
    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("{} is not valid TOML; fix it first", path.display()))?;
    edit(&mut doc)?;

    let out = doc.to_string();
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    if !parent.as_os_str().is_empty() && !parent.exists() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // Atomic, so a crash mid-write can't leave the user's hand-edited
    // config truncated.
    write_atomic(path, out.as_bytes()).with_context(|| format!("writing {}", path.display()))
}

/// Replace `path` with `data` atomically: write a sibling temp file, sync
/// it, then rename it over `path`. A crash or a concurrent save leaves the
/// old file or the new one, never a truncated mix. The temp name ends in
/// `.tmp-<pid>-<n>` (unique per call), so loaders that read `*.json` /
/// `*.toml` never see it.
pub fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp-{}-{n}", std::process::id()));
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_runs_keep_state_in_herdrs_state_dir() {
        let state = || Some(std::ffi::OsString::from("/state"));
        assert_eq!(
            plugin_state_dir(Some(PLUGIN_ID), state()),
            Some(PathBuf::from("/state"))
        );
        assert_eq!(plugin_state_dir(Some("other.plugin"), state()), None);
        assert_eq!(plugin_state_dir(None, state()), None);
        assert_eq!(plugin_state_dir(Some(PLUGIN_ID), None), None);
    }
    use std::sync::{Mutex, OnceLock};

    /// Env vars are process-global; serialize tests that touch them.
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn env_lock() -> &'static Mutex<()> {
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_default_config_validates() {
        let config = Config::default();
        config.validate().unwrap();
    }

    #[test]
    fn test_merge_precedence() {
        let a = Config {
            data_dir: PathBuf::from("/a"),
            ..Default::default()
        };
        let b = Config {
            data_dir: PathBuf::from("/b"),
            ..Default::default()
        };
        let merged = a.merge(b);
        assert_eq!(merged.data_dir, PathBuf::from("/b"));
    }

    #[test]
    fn test_resolved_herdr_socket() {
        let config = Config::default();
        let socket = config.herdr_socket_path();
        assert!(socket.ends_with(".config/herdr/herdr.sock"));
    }

    #[test]
    fn test_herdr_socket_path_named_session() {
        let _g = env_lock().lock().unwrap();
        let saved = std::env::var("HERDR_SESSION").ok();
        unsafe {
            std::env::set_var("HERDR_SESSION", "work");
        }
        let config = Config::default();
        let socket = config.herdr_socket_path();
        assert!(
            socket
                .to_string_lossy()
                .ends_with(".config/herdr/sessions/work/herdr.sock")
        );
        match saved {
            Some(v) => unsafe {
                std::env::set_var("HERDR_SESSION", v);
            },
            None => unsafe {
                std::env::remove_var("HERDR_SESSION");
            },
        }
    }

    #[test]
    fn test_herdr_socket_path_named_session_default_is_skipped() {
        let _g = env_lock().lock().unwrap();
        let saved = std::env::var("HERDR_SESSION").ok();
        unsafe {
            std::env::set_var("HERDR_SESSION", "default");
        }
        let config = Config::default();
        let socket = config.herdr_socket_path();
        assert!(
            socket
                .to_string_lossy()
                .ends_with(".config/herdr/herdr.sock")
        );
        match saved {
            Some(v) => unsafe {
                std::env::set_var("HERDR_SESSION", v);
            },
            None => unsafe {
                std::env::remove_var("HERDR_SESSION");
            },
        }
    }

    #[test]
    fn test_config_default_values() {
        let c = Config::default();
        assert_eq!(c.http.port, Some(7676));
        assert_eq!(c.http.bind_addr.as_deref(), Some("127.0.0.1"));
        assert_eq!(c.herdr.socket_path, None);
        assert!(c.settings.is_empty());
    }

    #[test]
    fn test_config_toml_roundtrip() {
        let original = Config::default();
        let toml_str = toml::to_string(&original).unwrap();
        let restored: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(original.http.port, restored.http.port);
        assert_eq!(original.settings, restored.settings);
    }

    #[test]
    fn settings_write_keeps_the_rest_and_loads_back() {
        use herdr_mcp_api::settings::{PANE_EXEC_MS, Value};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("herdr-mcp.toml");
        std::fs::write(&path, "# mine\ndata-dir = \"./d\"\n").unwrap();
        set_setting_in_file(&path, PANE_EXEC_MS, Some(&Value::Number(9_000))).unwrap();
        let keys = Value::Keys(vec!["alt+]".into(), "f6".into()]);
        set_setting_in_file(&path, "keys.tab-next", Some(&keys)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# mine\ndata-dir = \"./d\"\n"), "{text}");
        assert!(!text.contains("[settings]\n"), "{text}");
        let cfg: Config = toml::from_str(&text).unwrap();
        let (s, warnings) = cfg.settings();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(s.number(PANE_EXEC_MS), 9_000);
        assert_eq!(s.value("keys.tab-next"), Some(keys));

        set_setting_in_file(&path, PANE_EXEC_MS, None).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("timeouts"));
        reset_settings_in_file(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "# mine\ndata-dir = \"./d\"\n");
    }

    #[test]
    fn security_section_parses_over_defaults() {
        let cfg: Config = toml::from_str(
            r#"
            [security]
            disabled-tools = ["run_command"]
            block-private = true
            "#,
        )
        .unwrap();
        assert_eq!(cfg.security.disabled_tools, ["run_command"]);
        assert!(cfg.security.block_private);
        assert_eq!(cfg.security.key_env, SecurityConfig::default().key_env);
        assert!(toml::from_str::<Config>("[security]\nnope = 1\n").is_err());
    }

    #[test]
    fn test_clipboard_config_section_parses() {
        let toml_str = r#"
            data-dir = "/tmp/x"
            [clipboard]
            copy-command = "xsel --clipboard --input"
            paste-command = "xsel --clipboard --output"
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            cfg.clipboard.copy_command.as_deref(),
            Some("xsel --clipboard --input")
        );
        assert_eq!(
            cfg.clipboard.paste_command.as_deref(),
            Some("xsel --clipboard --output")
        );
    }

    #[test]
    fn test_merge_all_fields_override() {
        let a = Config::default();
        let b = Config {
            data_dir: PathBuf::from("/data"),
            http: HttpConfig {
                port: Some(9999),
                http_only: true,
                ..Default::default()
            },
            herdr: HerdrConfig {
                socket_path: Some(PathBuf::from("/sock")),
            },
            ..Default::default()
        };
        let merged = a.merge(b);
        assert_eq!(merged.data_dir, PathBuf::from("/data"));
        assert_eq!(merged.http.port, Some(9999));
        assert!(merged.http.http_only);
        assert_eq!(merged.herdr.socket_path, Some(PathBuf::from("/sock")));
    }

    #[test]
    fn test_merge_preserves_other_defaults() {
        let mut a = Config::default();
        let mut b = Config::default();
        a.data_dir = PathBuf::from("/a");
        b.http.port = Some(1234);
        let merged = a.merge(b);
        // merge takes `other` wholesale for every field (its default dir)
        assert_eq!(merged.data_dir, PathBuf::from("./data"));
        assert_eq!(merged.http.port, Some(1234));
        // untouched field keeps its default
        assert_eq!(merged.herdr.socket_path, None);
    }

    #[test]
    fn test_apply_cli_overrides_partial() {
        let cli = CliOverrides {
            http_port: Some(8080),
            ..Default::default()
        };
        let mut c = Config::default();
        c = c.apply_cli_overrides(cli);
        assert_eq!(c.http.port, Some(8080));
        assert!(!c.http.http_only);
    }

    #[test]
    fn test_expand_home() {
        let home = PathBuf::from(env::var_os("HOME").unwrap());
        assert_eq!(
            expand_home(std::path::Path::new("~/.config/herdr/herdr.sock")),
            home.join(".config/herdr/herdr.sock")
        );
        assert_eq!(
            expand_home(std::path::Path::new("./data")),
            PathBuf::from("./data")
        );
        assert_eq!(
            expand_home(std::path::Path::new("/abs/~x")),
            PathBuf::from("/abs/~x")
        );
    }

    /// `./data` is the default however the config is built, and a layer
    /// that doesn't name a data dir leaves the one below it alone.
    #[test]
    fn test_data_dir_default_survives_layers() {
        assert_eq!(Config::default().data_dir, PathBuf::from("./data"));
        let file: Config = toml::from_str("").unwrap();
        assert_eq!(file.data_dir, PathBuf::from("./data"));
        let c = Config {
            data_dir: PathBuf::from("/from/env"),
            ..Default::default()
        }
        .apply_cli_overrides(CliOverrides::default());
        assert_eq!(c.data_dir, PathBuf::from("/from/env"));
    }

    #[test]
    fn test_apply_cli_overrides_all() {
        let cli = CliOverrides {
            data_dir: Some(PathBuf::from("/cli/data")),
            http_port: Some(9000),
            http_only: Some(true),
            herdr_socket: Some(PathBuf::from("/cli/sock")),
        };
        let c = Config::default().apply_cli_overrides(cli);
        assert_eq!(c.data_dir, PathBuf::from("/cli/data"));
        assert_eq!(c.http.port, Some(9000));
        assert!(c.http.http_only);
        assert_eq!(c.herdr.socket_path, Some(PathBuf::from("/cli/sock")));
    }

    #[test]
    fn test_config_deny_unknown_fields() {
        let bad = "unknown_field = 1\nhttp.port = 8080\n";
        let result: Result<Config, _> = toml::from_str(bad);
        assert!(result.is_err(), "unknown fields must be rejected");
    }

    #[test]
    fn test_set_clipboard_preserves_other_sections() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("herdr-mcp.toml");
        // Seed with an [http] section + a comment; [clipboard] absent.
        std::fs::write(
            &path,
            "# my config\n[http]\nport = 9999\nbind-addr = \"0.0.0.0\"\nhttp-only = false\n",
        )
        .unwrap();

        set_clipboard_in_file(&path, Some(("xsel -i", "xsel"))).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        // [http] preserved.
        assert!(
            content.contains("port = 9999"),
            "http port preserved: {content}"
        );
        assert!(content.contains("0.0.0.0"), "bind-addr preserved");
        assert!(content.contains("# my config"), "comment preserved");
        // [clipboard] added.
        let cfg: Config = toml::from_str(&content).unwrap();
        assert_eq!(cfg.clipboard.copy_command.as_deref(), Some("xsel -i"));
        assert_eq!(cfg.clipboard.paste_command.as_deref(), Some("xsel"));
    }

    #[test]
    fn test_set_clipboard_overwrites_existing_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("herdr-mcp.toml");
        std::fs::write(
            &path,
            "[clipboard]\ncopy-command = \"old\"\npaste-command = \"old\"\n",
        )
        .unwrap();

        set_clipboard_in_file(&path, Some(("wl-copy", "wl-paste"))).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let cfg: Config = toml::from_str(&content).unwrap();
        assert_eq!(cfg.clipboard.copy_command.as_deref(), Some("wl-copy"));
        assert_eq!(cfg.clipboard.paste_command.as_deref(), Some("wl-paste"));
        assert!(!content.contains("\"old\""), "old values gone: {content}");
    }

    #[test]
    fn test_set_clipboard_refuses_malformed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr-mcp.toml");
        let broken = "data-dir = \"/keep\"\n[http\nport = 1\n";
        std::fs::write(&path, broken).unwrap();
        assert!(set_clipboard_in_file(&path, Some(("xsel -i", "xsel"))).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    #[test]
    fn test_set_clipboard_none_removes_the_override() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("herdr-mcp.toml");
        std::fs::write(
            &path,
            "# keep\n[http]\nport = 9999\n\n[clipboard]\ncopy-command = \"xsel -i\"\npaste-command = \"xsel\"\n",
        )
        .unwrap();

        set_clipboard_in_file(&path, None).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("clipboard"), "{content}");
        assert!(content.contains("# keep") && content.contains("port = 9999"));
    }

    #[test]
    fn test_set_clipboard_creates_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/deep/herdr-mcp.toml");
        set_clipboard_in_file(&path, Some(("xsel -i", "xsel"))).unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[clipboard]"));
        assert!(content.contains("xsel -i"));
    }
}

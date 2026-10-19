use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;

/// Persistent runtime configuration loaded from CWD `herdmcp.toml`.
/// This is applied BEFORE CLI overrides but AFTER defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PersistentConfig {
    /// HTTP bridge port (default: 7676). CLI flag --http overrides this.
    #[serde(default)]
    pub http_port: Option<u16>,

    /// Data directory for recipes, sessions, trim stats, PFC1 memory (default: ./data)
    #[serde(default)]
    pub data_dir: Option<PathBuf>,

    /// herdr daemon socket path (default: ~/.config/herdr/herdr.sock)
    #[serde(default)]
    pub herdr_socket: Option<PathBuf>,
}

impl PersistentConfig {
    /// Load persistent config from CWD `herdmcp.toml` if it exists.
    /// Only parses the `[persistent]` section.
    pub fn load_from_cwd() -> Result<Self> {
        let cwd = std::env::current_dir()?;
        let path = cwd.join("herdmcp.toml");
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read persistent config: {}", path.display()))?;
            // Parse only the [persistent] section
            let table: toml::Table = toml::from_str(&content).with_context(|| {
                format!("failed to parse persistent config: {}", path.display())
            })?;
            if let Some(persistent) = table.get("persistent") {
                let cfg: PersistentConfig = toml::from_str(&toml::to_string(persistent)?)
                    .with_context(|| {
                        format!("failed to parse [persistent] section: {}", path.display())
                    })?;
                return Ok(cfg);
            }
        }
        Ok(Self::default())
    }
}

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

/// Where state lives when nothing else says: `./data`.
fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            http: Default::default(),
            herdr: Default::default(),
            clipboard: Default::default(),
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
    /// Unset → [`HttpConfig::resolved_web_dir`] falls back to the workspace
    /// bundle path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_dir: Option<PathBuf>,
}

impl HttpConfig {
    /// Web UI directory: explicit `web_dir`, else the in-repo
    /// `dx bundle --platform web --release` output for `herdr-mcp-ui`.
    pub fn resolved_web_dir(&self) -> PathBuf {
        self.web_dir.clone().unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/dx/herdr-mcp-ui/release/web/public")
        })
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HerdrConfig {
    pub socket_path: Option<PathBuf>,
    pub reconnect_attempts: u32,
    pub reconnect_backoff_ms: u64,
}

impl Default for HerdrConfig {
    fn default() -> Self {
        Self {
            socket_path: None,
            reconnect_attempts: 3,
            reconnect_backoff_ms: 500,
        }
    }
}

impl Config {
    /// Load configuration with full precedence chain:
    /// Defaults < Config file < Persistent (CWD herdmcp.toml) < Env vars < CLI args
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

        // 2. Load persistent runtime config from CWD herdmcp.toml
        let persistent = PersistentConfig::load_from_cwd()?;
        config = config.apply_persistent(persistent);

        // 3. Apply environment variable overrides
        config = config.apply_env_overrides()?;

        // 4. Apply CLI overrides
        if let Some(cli) = cli_overrides {
            config = config.apply_cli_overrides(cli);
        }

        // Paths from TOML / env may start with `~/`; nothing else expands it.
        config.data_dir = expand_home(&config.data_dir);
        config.herdr.socket_path = config.herdr.socket_path.as_deref().map(expand_home);

        // 5. Validate
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
        }
    }

    /// Apply persistent config from CWD herdmcp.toml (wins over defaults, loses to env/CLI)
    fn apply_persistent(mut self, persistent: PersistentConfig) -> Self {
        if let Some(v) = persistent.http_port {
            self.http.port = Some(v);
        }
        if let Some(v) = persistent.data_dir {
            self.data_dir = v;
        }
        if let Some(v) = persistent.herdr_socket {
            self.herdr.socket_path = Some(v);
        }
        self
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
            self.http.web_dir = Some(PathBuf::from(v));
        }

        // clipboard
        if let Ok(v) = env::var("HERDR_MCP_CLIPBOARD_COPY") {
            self.clipboard.copy_command = Some(v);
        }
        if let Ok(v) = env::var("HERDR_MCP_CLIPBOARD_PASTE") {
            self.clipboard.paste_command = Some(v);
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
    let local = PathBuf::from("herdr-mcp.toml");
    if local.exists() {
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
/// the default location in the current working directory (`herdr-mcp.toml`).
/// Used by the Settings tab's Save so it always has a concrete target.
pub fn config_file_or_default() -> PathBuf {
    find_config_file().unwrap_or_else(|| PathBuf::from("herdr-mcp.toml"))
}

/// Write the `[clipboard]` table's `copy-command` / `paste-command` keys in the
/// config file at `path` (`None` removes them: the built-in clipboard),
/// preserving all other content (comments, tables, formatting). Creates the
/// file if absent.
pub fn set_clipboard_in_file(path: &std::path::Path, commands: Option<(&str, &str)>) -> Result<()> {
    use std::io::Write;

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

    let out = doc.to_string();
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    if !parent.as_os_str().is_empty() && !parent.exists() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("opening {} for write", path.display()))?;
    f.write_all(out.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(c.herdr.reconnect_attempts, 3);
    }

    #[test]
    fn test_config_toml_roundtrip() {
        let original = Config::default();
        let toml_str = toml::to_string(&original).unwrap();
        let restored: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(original.http.port, restored.http.port);
        assert_eq!(
            original.herdr.reconnect_attempts,
            restored.herdr.reconnect_attempts
        );
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
                ..Default::default()
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
        assert_eq!(merged.herdr.reconnect_attempts, 3);
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
    fn test_clipboard_env_vars_set_the_commands() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("HERDR_MCP_CLIPBOARD_COPY", "my-copy");
            std::env::set_var("HERDR_MCP_CLIPBOARD_PASTE", "my-paste");
        }
        let cfg = Config::default().apply_env_overrides();
        unsafe {
            std::env::remove_var("HERDR_MCP_CLIPBOARD_COPY");
            std::env::remove_var("HERDR_MCP_CLIPBOARD_PASTE");
        }
        assert_eq!(
            cfg.unwrap().clipboard.commands(),
            Some(("my-copy", "my-paste"))
        );
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

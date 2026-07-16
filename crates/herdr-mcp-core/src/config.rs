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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub data_dir: PathBuf,

    #[serde(default)]
    pub http: HttpConfig,

    #[serde(default)]
    pub mcp: McpConfig,

    #[serde(default)]
    pub herdr: HerdrConfig,

    #[serde(default)]
    pub trim: TrimConfig,

    #[serde(default)]
    pub sandbox: SandboxConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

    /// Clipboard backend override for the `clipboard_set` / `clipboard_get` tools.
    #[serde(default)]
    pub clipboard: ClipboardConfig,

    /// TUI dashboard keybinding overrides.
    #[serde(default)]
    pub keybindings: crate::keybindings::Keybindings,
}

/// Explicit clipboard backend commands for the `clipboard_set` / `clipboard_get`
/// tools. When both `copy-command` and `paste-command` are set (non-empty) they
/// override platform auto-detection (e.g. force `xsel` instead of `wl-copy` on
/// Linux). The `HERDR_MCP_CLIPBOARD_COPY` / `HERDR_MCP_CLIPBOARD_PASTE` env vars
/// still take precedence over this setting.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ClipboardConfig {
    /// Command that reads clipboard contents from stdin (copy).
    #[serde(default)]
    pub copy_command: Option<String>,

    /// Command that writes clipboard contents to stdout (paste).
    #[serde(default)]
    pub paste_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HttpConfig {
    pub port: Option<u16>,
    pub bind_addr: Option<String>,
    pub http_only: bool,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            port: Some(7676),
            bind_addr: Some("127.0.0.1".into()),
            http_only: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct McpConfig {
    pub capabilities: Vec<String>,
    pub tool_timeout_secs: u64,
    pub max_concurrent_tools: usize,
    pub default_stages: Vec<String>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            capabilities: vec!["tools".into(), "resources".into()],
            tool_timeout_secs: 300,
            max_concurrent_tools: 10,
            default_stages: vec!["caveman:full".into(), "pfc1".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HerdrConfig {
    pub socket_path: Option<PathBuf>,
    pub connect_timeout_secs: u64,
    pub reconnect_attempts: u32,
    pub reconnect_backoff_ms: u64,
}

impl Default for HerdrConfig {
    fn default() -> Self {
        Self {
            socket_path: None,
            connect_timeout_secs: 5,
            reconnect_attempts: 3,
            reconnect_backoff_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TrimConfig {
    pub default_stages: Vec<String>,
    pub pfc1: Pfc1Config,
    pub caveman: CavemanConfig,
}

impl Default for TrimConfig {
    fn default() -> Self {
        Self {
            default_stages: vec!["caveman:full".into(), "pfc1".into()],
            pfc1: Pfc1Config::default(),
            caveman: CavemanConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Pfc1Config {
    pub max_symbols: usize,
    pub min_frequency: usize,
    pub min_length: usize,
    pub min_phrase_words: usize,
    pub max_phrase_words: usize,
    pub enable_phrases: bool,
    pub seed_with_default: bool,
    pub persist_central: bool,
    pub learn_master: bool,
}

impl Default for Pfc1Config {
    fn default() -> Self {
        Self {
            max_symbols: 80,
            min_frequency: 2,
            min_length: 4,
            min_phrase_words: 2,
            max_phrase_words: 4,
            enable_phrases: true,
            seed_with_default: true,
            persist_central: false,
            learn_master: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CavemanConfig {
    pub level: String,
}

impl Default for CavemanConfig {
    fn default() -> Self {
        Self {
            level: "full".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SandboxConfig {
    pub backend: SandboxBackend,
    pub allow_network: bool,
    pub allow_fs_read: Vec<PathBuf>,
    pub allow_fs_write: Vec<PathBuf>,
    pub cpu_limit_ms: Option<u64>,
    pub memory_limit_mb: Option<u64>,
    pub enabled: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            backend: SandboxBackend::Auto,
            allow_network: false,
            allow_fs_read: vec![],
            allow_fs_write: vec![],
            cpu_limit_ms: None,
            memory_limit_mb: None,
            enabled: false, // Dev default: off
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SandboxBackend {
    #[default]
    Auto,
    Firejail,
    Bubblewrap,
    Landlock,
    Seatbelt,
    Docker,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: String,
    pub format: LogFormat,
    pub file_output: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            format: LogFormat::Text,
            file_output: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

impl std::str::FromStr for LogFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(LogFormat::Text),
            "json" => Ok(LogFormat::Json),
            _ => Err(format!("invalid log format: {}", s)),
        }
    }
}

impl std::fmt::Display for LogFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogFormat::Text => write!(f, "text"),
            LogFormat::Json => write!(f, "json"),
        }
    }
}

impl Config {
    /// Load configuration with full precedence chain:
    /// Defaults < Config file < Persistent (CWD herdmcp.toml) < Env vars < CLI args
    pub fn load(cli_overrides: Option<CliOverrides>) -> Result<Self> {
        let mut config = Config::default();

        // 1. Load from config file (project config)
        if let Some(path) = find_config_file_impl() {
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

        // 5. Validate
        config.validate()?;

        Ok(config)
    }

    /// Merge another config into this one (other wins on conflicts)
    fn merge(self, other: Config) -> Config {
        Config {
            data_dir: other.data_dir,
            http: other.http,
            mcp: other.mcp,
            herdr: other.herdr,
            trim: other.trim,
            sandbox: other.sandbox,
            logging: other.logging,
            clipboard: other.clipboard,
            keybindings: other.keybindings,
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

        // mcp
        if let Ok(v) = env::var("HERDR_MCP_TOOL_TIMEOUT") {
            self.mcp.tool_timeout_secs = v.parse()?;
        }
        if let Ok(v) = env::var("HERDR_MCP_MAX_CONCURRENT") {
            self.mcp.max_concurrent_tools = v.parse()?;
        }

        // herdr
        if let Ok(v) = env::var("HERDR_SOCKET_PATH") {
            self.herdr.socket_path = Some(PathBuf::from(v));
        }
        if let Ok(v) = env::var("HERDR_MCP_HERDR_TIMEOUT") {
            self.herdr.connect_timeout_secs = v.parse()?;
        }

        // trim
        if let Ok(v) = env::var("HERDR_MCP_PFC1_ENABLE_PHRASES") {
            self.trim.pfc1.enable_phrases = v.parse()?;
        }
        if let Ok(v) = env::var("HERDR_MCP_PFC1_MAX_SYMBOLS") {
            self.trim.pfc1.max_symbols = v.parse()?;
        }

        // logging
        if let Ok(v) = env::var("HERDR_MCP_LOG_LEVEL") {
            self.logging.level = v;
        }
        if let Ok(v) = env::var("HERDR_MCP_LOG_FORMAT") {
            self.logging.format = v.parse().map_err(|e: String| anyhow::anyhow!(e))?;
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
        if let Some(v) = cli.http_bind {
            self.http.bind_addr = Some(v);
        }
        if let Some(v) = cli.herdr_socket {
            self.herdr.socket_path = Some(v);
        }
        if let Some(v) = cli.log_level {
            self.logging.level = v;
        }
        if !cli.trim_stages.is_empty() {
            self.trim.default_stages = cli.trim_stages;
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

        if let Some(port) = self.http.port
            && port == 0
        {
            anyhow::bail!("http.port cannot be 0");
        }

        if self.mcp.tool_timeout_secs == 0 {
            anyhow::bail!("mcp.tool_timeout_secs must be > 0");
        }
        if self.mcp.max_concurrent_tools == 0 {
            anyhow::bail!("mcp.max_concurrent_tools must be > 0");
        }

        if self.trim.pfc1.min_phrase_words == 0 {
            anyhow::bail!("trim.pfc1.min_phrase_words must be > 0");
        }
        if self.trim.pfc1.max_phrase_words < self.trim.pfc1.min_phrase_words {
            anyhow::bail!("trim.pfc1.max_phrase_words must be >= min_phrase_words");
        }
        if self.trim.pfc1.max_symbols > 85 {
            anyhow::bail!("trim.pfc1.max_symbols cannot exceed 85 (Cherokee syllabary size)");
        }

        match self.logging.level.as_str() {
            "trace" | "debug" | "info" | "warn" | "error" => {}
            _ => anyhow::bail!("invalid log level: {}", self.logging.level),
        }

        match self.logging.format.to_string().as_str() {
            "text" | "json" => {}
            _ => anyhow::bail!("invalid log format: {}", self.logging.format),
        }

        Ok(())
    }

    /// Get the resolved herdr socket path
    pub fn herdr_socket_path(&self) -> PathBuf {
        self.herdr.socket_path.clone().unwrap_or_else(|| {
            let home = env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".config/herdr/herdr.sock")
        })
    }

    /// Get the effective trim stages
    pub fn trim_stages(&self) -> &[String] {
        &self.trim.default_stages
    }
}

/// CLI overrides passed from main.rs after parsing
#[derive(Debug, Default, Clone)]
pub struct CliOverrides {
    pub data_dir: Option<PathBuf>,
    pub http_port: Option<u16>,
    pub http_only: Option<bool>,
    pub http_bind: Option<String>,
    pub herdr_socket: Option<PathBuf>,
    pub log_level: Option<String>,
    pub trim_stages: Vec<String>,
}

/// Generate a default config file with comments for user reference
pub fn generate_default_config() -> String {
    let config = Config::default();
    let mut out = String::new();
    out.push_str("# herdr-mcp configuration\n");
    out.push_str("# See https://github.com/yourorg/herdr-mcp for docs\n\n");
    out.push_str(&toml::to_string_pretty(&config).unwrap());
    out.push_str(
        "\n# Clipboard backend override for the clipboard_set / clipboard_get tools.\n\
         # When both are set they override platform auto-detection (e.g. force\n\
         # xsel instead of wl-copy on Linux). HERDR_MCP_CLIPBOARD_COPY / _PASTE\n\
         # env vars still take precedence over this.\n\
         # [clipboard]\n\
         # copy-command = \"xsel -i\"\n\
         # paste-command = \"xsel\"\n",
    );
    out.push_str(
        "\n# TUI dashboard keybinding overrides. Uncomment and edit to customize.\n\
         # Format: \"ctrl-q\", \"up\", \"page_up\", \"shift-tab\", etc.\n\
         # [keybindings]\n\
         # quit = \"ctrl-q\"\n\
         # refresh = \"ctrl-r\"\n\
         # copy = \"ctrl-c\"\n\
         # paste = \"ctrl-v\"\n\
         # cut = \"ctrl-x\"\n\
         # nav-up = \"up\"\n\
         # nav-down = \"down\"\n\
         # nav-home = \"home\"\n\
         # nav-end = \"end\"\n\
         # nav-page-up = \"page_up\"\n\
         # nav-page-down = \"page_down\"\n\
         # frame-next = \"tab\"\n\
         # frame-prev = \"backtab\"\n",
    );
    out
}

/// Returns the path to the config file that `Config::load` would read,
/// or `None` if no file was found (i.e. defaults would be used).
pub fn find_config_file() -> Option<PathBuf> {
    find_config_file_impl()
}

/// Internal implementation split so `Config::load` can call it without recursion.
fn find_config_file_impl() -> Option<PathBuf> {
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

    // 4. Home dir legacy
    if let Ok(home) = env::var("HOME") {
        let legacy = PathBuf::from(home).join(".herdr-mcp.toml");
        if legacy.exists() {
            return Some(legacy);
        }
    }

    None
}

/// Resolve the config file path the loader would use (or `None` if no file
/// exists on any search path). Exposed so the TUI Settings tab can show the
/// path and Save can write to the correct file.
pub fn find_config_file_path() -> Option<PathBuf> {
    find_config_file()
}

/// Resolve the config file path the loader *would* read, or — if none exists —
/// the default location in the current working directory (`herdr-mcp.toml`).
/// Used by the Settings tab's Save so it always has a concrete target.
pub fn config_file_or_default() -> PathBuf {
    find_config_file().unwrap_or_else(|| PathBuf::from("herdr-mcp.toml"))
}

/// Upsert the `[clipboard]` table's `copy-command` / `paste-command` keys in
/// the config file at `path`, preserving all other content (comments, tables,
/// formatting). Creates the file if absent.
pub fn upsert_clipboard_in_file(path: &std::path::Path, copy: &str, paste: &str) -> Result<()> {
    use std::io::Write;

    let content = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };

    // Parse into an editable document; if parsing fails (empty or malformed),
    // start from a fresh document so Save still works on a broken file.
    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .unwrap_or_else(|_| toml_edit::DocumentMut::new());

    // Ensure the `[clipboard]` table exists.
    if !doc.contains_table("clipboard") {
        doc.insert("clipboard", toml_edit::table());
    }
    let tbl = doc
        .get_mut("clipboard")
        .and_then(|item| item.as_table_mut())
        .context("[clipboard] is not a table")?;
    tbl.insert("copy-command", toml_edit::value(copy));
    tbl.insert("paste-command", toml_edit::value(paste));

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

    #[test]
    fn test_default_config_validates() {
        let config = Config::default();
        config.validate().unwrap();
    }

    #[test]
    fn test_config_file_discovery() {
        let _ = find_config_file();
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
    fn test_config_default_values() {
        let c = Config::default();
        assert_eq!(c.http.port, Some(7676));
        assert_eq!(c.http.bind_addr.as_deref(), Some("127.0.0.1"));
        assert_eq!(c.mcp.tool_timeout_secs, 300);
        assert_eq!(c.mcp.max_concurrent_tools, 10);
        assert_eq!(c.trim.pfc1.max_symbols, 80);
        assert_eq!(c.logging.level, "info");
        assert!(!c.sandbox.enabled);
        assert_eq!(c.trim.default_stages, vec!["caveman:full", "pfc1"]);
    }

    #[test]
    fn test_config_toml_roundtrip() {
        let original = Config::default();
        let toml_str = toml::to_string(&original).unwrap();
        let restored: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(original.http.port, restored.http.port);
        assert_eq!(
            original.mcp.tool_timeout_secs,
            restored.mcp.tool_timeout_secs
        );
        assert_eq!(original.trim.default_stages, restored.trim.default_stages);
        assert_eq!(original.logging.level, restored.logging.level);
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
            mcp: McpConfig {
                tool_timeout_secs: 1,
                ..Default::default()
            },
            herdr: HerdrConfig {
                socket_path: Some(PathBuf::from("/sock")),
                ..Default::default()
            },
            logging: LoggingConfig {
                level: "debug".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = a.merge(b);
        assert_eq!(merged.data_dir, PathBuf::from("/data"));
        assert_eq!(merged.http.port, Some(9999));
        assert!(merged.http.http_only);
        assert_eq!(merged.mcp.tool_timeout_secs, 1);
        assert_eq!(merged.herdr.socket_path, Some(PathBuf::from("/sock")));
        assert_eq!(merged.logging.level, "debug");
    }

    #[test]
    fn test_merge_preserves_other_defaults() {
        let mut a = Config::default();
        let mut b = Config::default();
        a.data_dir = PathBuf::from("/a");
        b.http.port = Some(1234);
        let merged = a.merge(b);
        // merge takes `other` wholesale for every field
        assert_eq!(merged.data_dir, PathBuf::from(""));
        assert_eq!(merged.http.port, Some(1234));
        // untouched field keeps its default
        assert_eq!(merged.logging.level, "info");
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
        assert_eq!(c.logging.level, "info");
    }

    #[test]
    fn test_apply_cli_overrides_all() {
        let cli = CliOverrides {
            data_dir: Some(PathBuf::from("/cli/data")),
            http_port: Some(9000),
            http_only: Some(true),
            http_bind: Some("0.0.0.0".into()),
            herdr_socket: Some(PathBuf::from("/cli/sock")),
            log_level: Some("warn".into()),
            trim_stages: vec!["pfc1".into()],
        };
        let c = Config::default().apply_cli_overrides(cli);
        assert_eq!(c.data_dir, PathBuf::from("/cli/data"));
        assert_eq!(c.http.port, Some(9000));
        assert!(c.http.http_only);
        assert_eq!(c.http.bind_addr.as_deref(), Some("0.0.0.0"));
        assert_eq!(c.herdr.socket_path, Some(PathBuf::from("/cli/sock")));
        assert_eq!(c.logging.level, "warn");
        assert_eq!(c.trim.default_stages, vec!["pfc1"]);
    }

    #[test]
    fn test_validate_rejects_port_zero() {
        let mut c = Config::default();
        c.http.port = Some(0);
        assert!(c.validate().is_err());
        assert!(
            c.validate()
                .unwrap_err()
                .to_string()
                .contains("cannot be 0")
        );
    }

    #[test]
    fn test_validate_rejects_tool_timeout_zero() {
        let mut c = Config::default();
        c.mcp.tool_timeout_secs = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_max_concurrent_zero() {
        let mut c = Config::default();
        c.mcp.max_concurrent_tools = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_max_symbols_over_85() {
        let mut c = Config::default();
        c.trim.pfc1.max_symbols = 86;
        assert!(c.validate().is_err());
        assert!(c.validate().unwrap_err().to_string().contains("85"));
    }

    #[test]
    fn test_validate_rejects_max_symbols_85_ok() {
        let mut c = Config::default();
        c.trim.pfc1.max_symbols = 85;
        assert!(c.validate().is_ok());
    }

    #[test]
    fn test_validate_rejects_invalid_log_level() {
        let mut c = Config::default();
        c.logging.level = "verbose".into();
        assert!(c.validate().is_err());
        assert!(
            c.validate()
                .unwrap_err()
                .to_string()
                .contains("invalid log level")
        );
    }

    #[test]
    fn test_validate_accepts_all_log_levels() {
        for lvl in ["trace", "debug", "info", "warn", "error"] {
            let mut c = Config::default();
            c.logging.level = lvl.into();
            assert!(c.validate().is_ok(), "{lvl} should be valid");
        }
    }

    #[test]
    fn test_validate_rejects_max_phrase_lt_min_phrase() {
        let mut c = Config::default();
        c.trim.pfc1.min_phrase_words = 4;
        c.trim.pfc1.max_phrase_words = 2;
        assert!(c.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_min_phrase_zero() {
        let mut c = Config::default();
        c.trim.pfc1.min_phrase_words = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn test_log_format_from_str() {
        use std::str::FromStr;
        assert_eq!(LogFormat::from_str("text"), Ok(LogFormat::Text));
        assert_eq!(LogFormat::from_str("json"), Ok(LogFormat::Json));
        assert!(LogFormat::from_str("xml").is_err());
        assert_eq!(LogFormat::from_str("TEXT"), Ok(LogFormat::Text));
    }

    #[test]
    fn test_log_format_display() {
        assert_eq!(LogFormat::Text.to_string(), "text");
        assert_eq!(LogFormat::Json.to_string(), "json");
    }

    #[test]
    fn test_generate_default_config() {
        let out = generate_default_config();
        assert!(out.contains("# herdr-mcp configuration"));
        assert!(out.contains("http"));
    }

    #[test]
    fn test_trim_stages_returns_configured() {
        let c = Config::default();
        assert_eq!(c.trim_stages(), c.trim.default_stages.as_slice());
    }

    #[test]
    fn test_config_deny_unknown_fields() {
        let bad = "unknown_field = 1\nhttp.port = 8080\n";
        let result: Result<Config, _> = toml::from_str(bad);
        assert!(result.is_err(), "unknown fields must be rejected");
    }

    #[test]
    fn test_upsert_clipboard_preserves_other_sections() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("herdr-mcp.toml");
        // Seed with an [http] section + a comment; [clipboard] absent.
        std::fs::write(
            &path,
            "# my config\n[http]\nport = 9999\nbind-addr = \"0.0.0.0\"\nhttp-only = false\n",
        )
        .unwrap();

        upsert_clipboard_in_file(&path, "xsel -i", "xsel").unwrap();

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
    fn test_upsert_clipboard_overwrites_existing_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("herdr-mcp.toml");
        std::fs::write(
            &path,
            "[clipboard]\ncopy-command = \"old\"\npaste-command = \"old\"\n",
        )
        .unwrap();

        upsert_clipboard_in_file(&path, "wl-copy", "wl-paste").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let cfg: Config = toml::from_str(&content).unwrap();
        assert_eq!(cfg.clipboard.copy_command.as_deref(), Some("wl-copy"));
        assert_eq!(cfg.clipboard.paste_command.as_deref(), Some("wl-paste"));
        assert!(!content.contains("\"old\""), "old values gone: {content}");
    }

    #[test]
    fn test_upsert_clipboard_creates_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/deep/herdr-mcp.toml");
        upsert_clipboard_in_file(&path, "xsel -i", "xsel").unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[clipboard]"));
        assert!(content.contains("xsel -i"));
    }
}

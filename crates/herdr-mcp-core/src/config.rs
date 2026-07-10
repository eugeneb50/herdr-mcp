use std::env;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("herdr-mcp")
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
            port: Some(5173),
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
    /// Defaults < Config file < Env vars < CLI args
    pub fn load(cli_overrides: Option<CliOverrides>) -> Result<Self> {
        let mut config = Config::default();

        // 1. Load from config file
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

        // 4. Validate
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
        if let Some(parent) = self.data_dir.parent() {
            if !parent.exists() {
                anyhow::bail!("data_dir parent does not exist: {}", parent.display());
            }
        }

        if let Some(port) = self.http.port {
            if port == 0 {
                anyhow::bail!("http.port cannot be 0");
            }
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
        self.herdr
            .socket_path
            .clone()
            .unwrap_or_else(|| {
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

fn find_config_file() -> Option<PathBuf> {
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

/// Generate a default config file with comments for user reference
pub fn generate_default_config() -> String {
    let config = Config::default();
    let mut out = String::new();
    out.push_str("# herdr-mcp configuration\n");
    out.push_str("# See https://github.com/yourorg/herdr-mcp for docs\n\n");
    out.push_str(&toml::to_string_pretty(&config).unwrap());
    out
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
        let mut a = Config::default();
        a.data_dir = PathBuf::from("/a");
        let mut b = Config::default();
        b.data_dir = PathBuf::from("/b");
        let merged = a.merge(b);
        assert_eq!(merged.data_dir, PathBuf::from("/b"));
    }

    #[test]
    fn test_resolved_herdr_socket() {
        let config = Config::default();
        let socket = config.herdr_socket_path();
        assert!(socket.ends_with(".config/herdr/herdr.sock"));
    }
}
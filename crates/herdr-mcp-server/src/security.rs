//! `[security]` enforcement: which tools exist, where a call came from, the
//! audit log and the bridge token. The checks themselves run in
//! [`crate::server::HerdrMcpServer`]'s `call_tool`, the one door every
//! caller (MCP clients, the bridge, recipe runs) goes through.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use herdr_mcp_core::SecurityConfig;
use serde_json::{Value, json};

use crate::outbound::Guard;

/// Where a tool call came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// An MCP client (stdio): an agent.
    Mcp,
    /// `POST /api/tools/{name}`: the web UI, the TUI, curl.
    Api,
    /// A recipe step (run, schedule or trigger).
    Run,
}

impl Origin {
    /// The request `_meta` key the in-process session tags calls with.
    /// Only that session's calls are read for it: an MCP client can't
    /// claim to be the bridge.
    pub const META_KEY: &str = "herdr-mcp/origin";

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Api => "api",
            Self::Run => "run",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "api" => Some(Self::Api),
            "run" => Some(Self::Run),
            _ => None,
        }
    }
}

/// The `[security]` config in force, with what it implies.
#[derive(Debug)]
pub struct Security {
    pub config: SecurityConfig,
    pub guard: Arc<Guard>,
    /// `/api/*` needs this token (`Authorization: Bearer` or `?token=`).
    pub token: Option<String>,
    audit: Option<PathBuf>,
    audit_lock: std::sync::Mutex<()>,
}

/// The audit log rolls over to `audit.jsonl.1` past this size.
const AUDIT_MAX_BYTES: u64 = 10 * 1024 * 1024;

impl Default for Security {
    fn default() -> Self {
        Self::new(&herdr_mcp_core::Config::default())
    }
}

impl Security {
    pub fn new(config: &herdr_mcp_core::Config) -> Self {
        let sec = config.security.clone();
        let bind = config.http.bind_addr.as_deref().unwrap_or("127.0.0.1");
        let token = (config.http.port.is_some() && token_required(&sec, bind)).then(|| {
            load_or_create_token(&config.data_dir).unwrap_or_else(|e| {
                // Still refuse callers: nobody knows this one.
                tracing::error!("bridge token: {e:#}; the bridge will refuse every /api call");
                uuid::Uuid::new_v4().simple().to_string()
            })
        });
        Self {
            guard: Arc::new(Guard::new(&sec, config.http.port)),
            audit: sec.audit_log.then(|| config.data_dir.join("audit.jsonl")),
            config: sec,
            token,
            audit_lock: std::sync::Mutex::new(()),
        }
    }

    pub fn tool_enabled(&self, name: &str) -> bool {
        !self.config.disabled_tools.iter().any(|t| t == name)
    }

    /// Append one line for a finished tool call, secrets redacted.
    pub fn audit(
        &self,
        origin: Origin,
        tool: &str,
        args: Option<&serde_json::Map<String, Value>>,
        outcome: Result<bool, &str>,
    ) {
        let Some(path) = &self.audit else { return };
        let (ok, error) = match outcome {
            Ok(is_error) => (!is_error, None),
            Err(e) => (false, Some(e)),
        };
        let line = json!({
            "at": chrono::Utc::now(),
            "origin": origin.as_str(),
            "tool": tool,
            "args": args.map(|a| redact(&Value::Object(a.clone()))),
            "ok": ok,
            "error": error,
        });
        let _guard = crate::lock(&self.audit_lock);
        if std::fs::metadata(path).is_ok_and(|m| m.len() > AUDIT_MAX_BYTES) {
            let _ = std::fs::rename(path, path.with_extension("jsonl.1"));
        }
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = written {
            tracing::warn!("audit log {}: {e}", path.display());
        }
    }
}

/// Whether the bridge needs its token: asked for, or bound beyond loopback.
pub fn token_required(sec: &SecurityConfig, bind: &str) -> bool {
    sec.require_token || !is_loopback(bind)
}

/// Whether a bind address only accepts local connections.
pub fn is_loopback(bind: &str) -> bool {
    bind.eq_ignore_ascii_case("localhost")
        || bind
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The bridge token in `<data_dir>/bridge.token`, made (owner-only) when
/// missing.
pub fn load_or_create_token(data_dir: &Path) -> anyhow::Result<String> {
    let path = data_dir.join(herdr_mcp_api::TOKEN_FILE);
    if let Ok(token) = std::fs::read_to_string(&path)
        && !token.trim().is_empty()
    {
        return Ok(token.trim().to_string());
    }
    std::fs::create_dir_all(data_dir)?;
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(&path)
        .and_then(|mut f| f.write_all(token.as_bytes()))
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(token)
}

/// The web UI's address on `localhost:port`, carrying `token` in the
/// fragment (never sent to the server or written to its logs) for the page
/// to keep and send.
pub fn web_url(port: u16, token: Option<&str>) -> String {
    match token {
        Some(t) => format!("http://localhost:{port}/#token={t}"),
        None => format!("http://localhost:{port}"),
    }
}

/// Whether `given` is `token`, in time independent of where they differ.
pub fn token_matches(given: &str, token: &str) -> bool {
    given.len() == token.len()
        && given
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Whether an argument named `key` holds a secret.
fn secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    matches!(
        k.as_str(),
        "api_key" | "authorization" | "proxy-authorization" | "cookie" | "token" | "password"
    ) || ["_key", "_token", "_secret", "password"]
        .iter()
        .any(|s| k.ends_with(s))
}

/// `value` with secret-named fields replaced and long strings cut, for
/// the audit log.
pub fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let v = if secret_key(k) && !v.is_null() {
                        Value::String("[redacted]".into())
                    } else {
                        redact(v)
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        Value::String(s) if s.chars().count() > 500 => {
            Value::String(format!("{}…", s.chars().take(500).collect::<String>()))
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn redact_hides_secret_fields_at_any_depth() {
        let args = json!({
            "url": "https://x",
            "api_key": "sk-1",
            "api_key_env": "OPENROUTER_API_KEY",
            "headers": { "Authorization": "Bearer sk-2", "X-Other": "ok" },
            "steps": [{ "params": { "github_token": "ghp" } }],
        });
        assert_eq!(
            redact(&args),
            json!({
                "url": "https://x",
                "api_key": "[redacted]",
                "api_key_env": "OPENROUTER_API_KEY",
                "headers": { "Authorization": "[redacted]", "X-Other": "ok" },
                "steps": [{ "params": { "github_token": "[redacted]" } }],
            })
        );
    }

    #[test]
    fn token_is_required_off_loopback_or_when_asked() {
        let sec = SecurityConfig::default();
        assert!(!token_required(&sec, "127.0.0.1"));
        assert!(!token_required(&sec, "[::1]"));
        assert!(token_required(&sec, "0.0.0.0"));
        for local in ["127.0.0.2", "LocalHost"] {
            assert!(is_loopback(local), "{local}");
        }
        for exposed in ["::", "192.168.1.5", "box.lan"] {
            assert!(!is_loopback(exposed), "{exposed}");
        }
        let asked = SecurityConfig {
            require_token: true,
            ..sec
        };
        assert!(token_required(&asked, "localhost"));
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abd", "abc"));
        assert!(!token_matches("ab", "abc"));
    }

    #[test]
    fn token_file_is_made_once_and_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let a = load_or_create_token(dir.path()).unwrap();
        assert_eq!(a.len(), 64);
        assert_eq!(load_or_create_token(dir.path()).unwrap(), a);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(herdr_mcp_api::TOKEN_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn audit_appends_redacted_lines() {
        let dir = tempfile::tempdir().unwrap();
        let config = herdr_mcp_core::Config {
            data_dir: dir.path().to_path_buf(),
            security: SecurityConfig {
                audit_log: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let sec = Security::new(&config);
        let args = json!({ "model": "m", "api_key": "sk-1" });
        sec.audit(Origin::Run, "llm_call", args.as_object(), Ok(false));
        sec.audit(Origin::Mcp, "run_command", None, Err("disabled"));
        let log = std::fs::read_to_string(dir.path().join("audit.jsonl")).unwrap();
        let lines: Vec<Value> = log
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["origin"], "run");
        assert_eq!(lines[0]["args"]["api_key"], "[redacted]");
        assert_eq!(lines[0]["ok"], true);
        assert_eq!(lines[1]["error"], "disabled");
        assert!(!log.contains("sk-1"));
    }
}

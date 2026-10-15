//! Agent profiles (`/api/profiles`): how to drive one kind of CLI agent.

use serde::{Deserialize, Serialize};

/// How to launch, prompt, interrupt and read one CLI agent. Data, not code:
/// bundled defaults ship with the server, `data/profiles/{id}.toml` overrides
/// or adds. Exactly one of `kind` / `command` is set:
///
/// - `kind`: an agent herdr supports natively (`herdr agent start --kind`).
///   herdr detects its status and submits prompts.
/// - `command`: any other CLI, run in a fresh pane with `pane run`. herdr-mcp
///   watches its output and reports `working` / `idle` to herdr
///   (`pane report-agent`): idle once output has been quiet for `quiet_ms`
///   and, if set, the last line matches `ready_regex`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Extra arguments: after `--` for `kind`, appended to `command`.
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_quiet_ms")]
    pub quiet_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_regex: Option<String>,
    /// Keys sent after prompt text (`command` agents; herdr submits for `kind`).
    #[serde(default = "default_submit")]
    pub submit: Vec<String>,
    /// Keys that stop the current turn.
    #[serde(default = "default_interrupt")]
    pub interrupt: Vec<String>,
    /// Keys that quit the agent.
    #[serde(default = "default_stop")]
    pub stop: Vec<String>,
    /// Lines of recent output searched for a prompt's reply.
    #[serde(default = "default_reply_lines")]
    pub reply_lines: u32,
    /// Free text: install hints, what version it was tested with, caveats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

fn default_quiet_ms() -> u64 {
    2500
}
fn default_submit() -> Vec<String> {
    vec!["enter".into()]
}
fn default_interrupt() -> Vec<String> {
    vec!["esc".into()]
}
fn default_stop() -> Vec<String> {
    vec!["ctrl+c".into()]
}
fn default_reply_lines() -> u32 {
    200
}

impl AgentProfile {
    /// Driven natively by herdr (`kind`) rather than watched by herdr-mcp.
    pub fn is_native(&self) -> bool {
        self.kind.is_some()
    }

    /// Problems that make the profile unusable (empty = valid).
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let id_ok = !self.id.is_empty()
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !id_ok {
            out.push("id must be non-empty [a-z0-9_-]".into());
        }
        let blank = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
        match (blank(&self.kind), blank(&self.command)) {
            (true, true) => out.push("set `kind` (herdr agent) or `command`".into()),
            (false, false) => out.push("set only one of `kind` and `command`".into()),
            _ => {}
        }
        if self.quiet_ms == 0 {
            out.push("quiet_ms must be > 0".into());
        }
        out
    }
}

impl AgentProfile {
    /// A profile for a herdr-native kind with no profile file: all defaults.
    pub fn native(kind: &str) -> Self {
        Self {
            id: kind.to_string(),
            name: kind.to_string(),
            kind: Some(kind.to_string()),
            command: None,
            args: Vec::new(),
            quiet_ms: default_quiet_ms(),
            ready_regex: None,
            submit: default_submit(),
            interrupt: default_interrupt(),
            stop: default_stop(),
            reply_lines: default_reply_lines(),
            notes: None,
        }
    }
}

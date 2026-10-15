//! Driving CLI agents through their [`AgentProfile`] (docs/SWARMS.md §4).
//!
//! Native profiles (`kind`) are started and prompted through herdr's agent
//! surface, which detects their status. Other profiles (`command`) run in a
//! pane via `pane run`; a watcher task reads the pane and reports
//! `working` / `idle` to herdr with `pane report-agent`, so every agent
//! looks the same to herdr, its events and the registry.

use std::time::{Duration, Instant};

use herdr_mcp_api::AgentProfile;
use rmcp::ErrorData as McpError;

use crate::server::{
    agent_submit_args, agent_wait_args, as_strs, extract_pane_id, herdr_cli, to_mcp_err,
};

/// `--source` value herdr-mcp reports agent state under.
const REPORT_SOURCE: &str = "herdr-mcp";
const WATCH_EVERY: Duration = Duration::from_millis(500);
/// Consecutive failed reads after which the watcher assumes the pane is gone.
const WATCH_GIVE_UP: u32 = 5;
const START_TIMEOUT_MS: &str = "60000";

/// Quote one word for the pane's shell (`pane run` types a command line).
fn shell_quote(word: &str) -> String {
    let safe = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@+%".contains(c));
    if safe {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// The command line a `command` profile runs, with `extra` args appended.
fn command_line(profile: &AgentProfile, extra: &[String]) -> String {
    profile
        .command
        .iter()
        .chain(&profile.args)
        .chain(extra)
        .map(|w| shell_quote(w))
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_args(anchor: &str, direction: &str, cwd: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "pane",
        "split",
        anchor,
        "--direction",
        direction,
        "--no-focus",
    ]
    .map(String::from)
    .to_vec();
    if let Some(cwd) = cwd {
        args.extend(["--cwd".to_string(), cwd.to_string()]);
    }
    args
}

/// `agent start <name> --kind K --pane P --timeout ms [-- args]` (herdr 0.9).
fn start_args(kind: &str, name: &str, pane: &str, args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = [
        "agent",
        "start",
        name,
        "--kind",
        kind,
        "--pane",
        pane,
        "--timeout",
        START_TIMEOUT_MS,
    ]
    .map(String::from)
    .to_vec();
    if !args.is_empty() {
        out.push("--".into());
        out.extend(args.iter().cloned());
    }
    out
}

async fn herdr(args: &[String]) -> Result<String, McpError> {
    herdr_cli(&as_strs(args)).await
}

/// A pane to split for a new agent: the first pane of `tab`, else of
/// `workspace`, else of the whole session.
async fn anchor_pane(workspace: Option<&str>, tab: Option<&str>) -> Result<String, McpError> {
    let mut args = vec!["pane".to_string(), "list".to_string()];
    if let Some(ws) = workspace {
        args.extend(["--workspace".to_string(), ws.to_string()]);
    }
    let raw = herdr(&args).await?;
    let json: serde_json::Value = serde_json::from_str(&raw).map_err(to_mcp_err)?;
    json.pointer("/result/panes")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter(|p| tab.is_none_or(|t| p.get("tab_id").and_then(|v| v.as_str()) == Some(t)))
        .find_map(|p| {
            p.get("pane_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .ok_or_else(|| to_mcp_err("no pane to split for the new agent (is herdr running?)"))
}

/// Where a new agent goes.
pub struct Placement<'a> {
    pub workspace: Option<&'a str>,
    pub tab: Option<&'a str>,
    /// `right` (default) or `down`.
    pub direction: Option<&'a str>,
    pub cwd: Option<&'a str>,
}

/// Start an agent from `profile` (plus `extra` args) in a new pane named
/// `name`. Returns the pane id once the agent is started (native: herdr saw
/// it ready; command: launched, with its watcher running).
pub async fn spawn(
    profile: &AgentProfile,
    name: &str,
    extra: &[String],
    at: Placement<'_>,
) -> Result<String, McpError> {
    let anchor = anchor_pane(at.workspace, at.tab).await?;
    let split = herdr(&split_args(
        &anchor,
        at.direction.unwrap_or("right"),
        at.cwd,
    ))
    .await?;
    let json: serde_json::Value = serde_json::from_str(&split).map_err(to_mcp_err)?;
    let pane =
        extract_pane_id(&json).ok_or_else(|| to_mcp_err("pane split returned no pane id"))?;

    match &profile.kind {
        Some(kind) => {
            let args: Vec<String> = profile.args.iter().chain(extra).cloned().collect();
            herdr(&start_args(kind, name, &pane, &args)).await?;
        }
        None => {
            let line = command_line(profile, extra);
            herdr(&["pane".into(), "run".into(), pane.clone(), line]).await?;
            watch(pane.clone(), profile);
        }
    }
    let _ = herdr(&["pane".into(), "rename".into(), pane.clone(), name.into()]).await;
    Ok(pane)
}

/// Busy / ready, as reported to herdr for `command` agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Activity {
    Working,
    Idle,
}

impl Activity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Idle => "idle",
        }
    }
}

/// Decides `working` / `idle` from successive snapshots of a pane: output
/// changing means working; unchanged for `quiet` (and, if set, the last
/// non-empty line matching `ready`) means idle. Yields only transitions.
pub(crate) struct QuietDetector {
    quiet: Duration,
    ready: Option<regex::Regex>,
    last: Option<String>,
    changed_at: Instant,
    reported: Option<Activity>,
}

impl QuietDetector {
    pub(crate) fn new(quiet: Duration, ready: Option<regex::Regex>, now: Instant) -> Self {
        Self {
            quiet,
            ready,
            last: None,
            changed_at: now,
            reported: None,
        }
    }

    pub(crate) fn observe(&mut self, snapshot: &str, now: Instant) -> Option<Activity> {
        let state = if self.last.as_deref() != Some(snapshot) {
            self.last = Some(snapshot.to_string());
            self.changed_at = now;
            Activity::Working
        } else if now.duration_since(self.changed_at) >= self.quiet && self.looks_ready(snapshot) {
            Activity::Idle
        } else {
            return None;
        };
        (self.reported != Some(state)).then(|| {
            self.reported = Some(state);
            state
        })
    }

    fn looks_ready(&self, snapshot: &str) -> bool {
        let Some(ready) = &self.ready else {
            return true;
        };
        snapshot
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| ready.is_match(l.trim_end()))
    }
}

/// Watch a `command` agent's pane and report its activity to herdr until
/// the pane goes away. Reported under the profile id, which the registry
/// picks up as the pane's agent, so later prompts find the same profile.
fn watch(pane: String, profile: &AgentProfile) {
    let agent = profile.id.clone();
    let ready = profile
        .ready_regex
        .as_deref()
        .and_then(|r| regex::Regex::new(r).ok());
    let quiet = Duration::from_millis(profile.quiet_ms);
    tokio::spawn(async move {
        let mut detector = QuietDetector::new(quiet, ready, Instant::now());
        let mut failures = 0;
        loop {
            tokio::time::sleep(WATCH_EVERY).await;
            let read = ["pane", "read", &pane, "--source", "detection"];
            let snapshot = match herdr_cli(&read).await {
                Ok(s) => {
                    failures = 0;
                    s
                }
                Err(_) => {
                    failures += 1;
                    if failures >= WATCH_GIVE_UP {
                        tracing::debug!("agent watcher for {pane}: pane gone, stopping");
                        return;
                    }
                    continue;
                }
            };
            if let Some(activity) = detector.observe(&snapshot, Instant::now()) {
                let report = [
                    "pane",
                    "report-agent",
                    &pane,
                    "--source",
                    REPORT_SOURCE,
                    "--agent",
                    &agent,
                    "--state",
                    activity.as_str(),
                ];
                if let Err(e) = herdr_cli(&report).await {
                    tracing::debug!("agent watcher for {pane}: report failed: {}", e.message);
                }
            }
        }
    });
}

/// Type `text` into an agent and submit it, without waiting.
pub async fn submit(profile: &AgentProfile, pane: &str, text: &str) -> Result<(), McpError> {
    if profile.is_native() {
        herdr(&agent_submit_args(pane, text)).await?;
    } else {
        herdr(&["pane".into(), "send-text".into(), pane.into(), text.into()]).await?;
        send_keys(pane, &profile.submit).await?;
    }
    Ok(())
}

async fn send_keys(pane: &str, keys: &[String]) -> Result<(), McpError> {
    if keys.is_empty() {
        return Ok(());
    }
    let mut args = vec![
        "pane".to_string(),
        "send-keys".to_string(),
        pane.to_string(),
    ];
    args.extend(keys.iter().cloned());
    herdr(&args).await.map(|_| ())
}

/// Send `text`, wait for the agent to finish its turn, and return its reply.
pub async fn prompt(
    profile: &AgentProfile,
    pane: &str,
    text: &str,
    timeout_ms: u32,
) -> Result<String, McpError> {
    if profile.is_native() {
        let ms = timeout_ms.to_string();
        let mut args = agent_submit_args(pane, text);
        args.extend(["--wait".into(), "--timeout".into(), ms]);
        herdr(&args).await?;
    } else {
        submit(profile, pane, text).await?;
        // Let the watcher see it start, so the idle wait isn't satisfied by
        // the state from before the prompt.
        let _ = herdr(&agent_wait_args(pane, "working", Some(5000))).await;
        herdr(&agent_wait_args(pane, "idle", Some(timeout_ms))).await?;
    }
    let lines = profile.reply_lines.to_string();
    let read = [
        "pane",
        "read",
        pane,
        "--source",
        "recent-unwrapped",
        "--lines",
        &lines,
    ];
    Ok(extract_reply(&herdr_cli(&read).await?, text))
}

/// Stop the agent's current turn (`interrupt` keys) or quit it (`stop`).
pub async fn interrupt(profile: &AgentProfile, pane: &str, hard: bool) -> Result<(), McpError> {
    send_keys(
        pane,
        if hard {
            &profile.stop
        } else {
            &profile.interrupt
        },
    )
    .await
}

/// Lines kept when the prompt can't be found in the output.
const REPLY_FALLBACK_LINES: usize = 40;

/// The agent's reply in `output`: everything after the last line that echoes
/// the prompt's first line; if the echo can't be found, the last lines.
pub(crate) fn extract_reply(output: &str, prompt: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let needle: String = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(40)
        .collect();
    let start = (!needle.is_empty())
        .then(|| lines.iter().rposition(|l| l.contains(&needle)))
        .flatten()
        .map_or(lines.len().saturating_sub(REPLY_FALLBACK_LINES), |i| i + 1);
    lines[start..].join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_what_needs_it() {
        assert_eq!(shell_quote("zeroclaw"), "zeroclaw");
        assert_eq!(shell_quote("--model=a/b"), "--model=a/b");
        assert_eq!(shell_quote("two words"), "'two words'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
        let mut p = AgentProfile::native("x");
        p.kind = None;
        p.command = Some("zeroclaw".into());
        p.args = vec!["agent".into(), "-a".into(), "main".into()];
        assert_eq!(
            command_line(&p, &["--note".into(), "hi there".into()]),
            "zeroclaw agent -a main --note 'hi there'"
        );
    }

    #[test]
    fn herdr_09_spawn_argv() {
        assert_eq!(
            split_args("w1:p1", "right", Some("/tmp")),
            [
                "pane",
                "split",
                "w1:p1",
                "--direction",
                "right",
                "--no-focus",
                "--cwd",
                "/tmp"
            ]
        );
        assert_eq!(
            start_args("codex", "rev", "w1:p2", &[]),
            [
                "agent",
                "start",
                "rev",
                "--kind",
                "codex",
                "--pane",
                "w1:p2",
                "--timeout",
                "60000"
            ]
        );
        assert_eq!(
            start_args("claude", "a", "p", &["--model".into(), "opus".into()])[9..],
            ["--", "--model", "opus"]
        );
    }

    #[test]
    fn detector_reports_working_then_idle_once() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut d = QuietDetector::new(Duration::from_millis(1000), None, t0);
        assert_eq!(d.observe("boot", ms(0)), Some(Activity::Working));
        assert_eq!(d.observe("boot\nmore", ms(400)), None, "still working");
        assert_eq!(
            d.observe("boot\nmore", ms(900)),
            None,
            "not quiet long enough"
        );
        assert_eq!(d.observe("boot\nmore", ms(1400)), Some(Activity::Idle));
        assert_eq!(
            d.observe("boot\nmore", ms(3000)),
            None,
            "idle reported once"
        );
        assert_eq!(
            d.observe("boot\nmore\nnew", ms(3100)),
            Some(Activity::Working)
        );
    }

    #[test]
    fn detector_waits_for_ready_prompt() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(10);
        let ready = regex::Regex::new(r"[$#>] ?$").ok();
        let mut d = QuietDetector::new(Duration::from_millis(100), ready, t0);
        d.observe("compiling...", t0);
        assert_eq!(
            d.observe("compiling...", later),
            None,
            "quiet but no prompt"
        );
        d.observe("done\n$ \n\n", later);
        assert_eq!(
            d.observe("done\n$ \n\n", later + Duration::from_secs(1)),
            Some(Activity::Idle)
        );
    }

    #[test]
    fn reply_follows_the_last_prompt_echo() {
        let out = "old stuff\n> summarise the repo\nold answer\n> summarise the repo\nIt is a Rust MCP server.\n\n";
        assert_eq!(
            extract_reply(out, "summarise the repo\nbriefly"),
            "It is a Rust MCP server."
        );
        let long: String = (0..60).map(|i| format!("line{i}\n")).collect();
        let reply = extract_reply(&long, "never echoed");
        assert!(reply.starts_with("line20") && reply.ends_with("line59"));
    }
}

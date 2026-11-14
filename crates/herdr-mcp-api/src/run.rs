//! Background recipe runs (`/api/runs`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{CheckResult, Flow, Port, RecipeStep, VarScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
    /// The record says running/paused but no live run owns it (server
    /// restarted mid-run). Reported on read; agents may still be in their panes.
    Interrupted,
}

impl RunStatus {
    /// True once the run can no longer change.
    pub fn is_finished(self) -> bool {
        !matches!(self, Self::Running | Self::Paused)
    }

    /// Failed, or cut off by a restart: what "failed only" lists.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::Interrupted)
    }

    /// The status as on the wire: `running`, `failed`, …
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

/// One run of a recipe: what ran, where it got to, and what it produced.
/// Folded from its header and event log (the `runs` and `run_events` tables).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: Uuid,
    /// The saved recipe this run came from; `None` for ad-hoc runs.
    pub recipe_id: Option<Uuid>,
    pub name: Option<String>,
    /// herdr workspace the run is scoped to.
    pub session_id: String,
    /// Snapshot of the flow as it was when the run started.
    pub flow: Flow,
    pub origin: RunOrigin,
    /// Which pass this is, from 1; each loop taken adds one. Also in
    /// `variables` as `run.pass`.
    pub pass: u32,
    pub status: RunStatus,
    /// Nodes running right now.
    pub active: Vec<String>,
    /// Per-node results (or `{error, statusCode}`), keyed by node id.
    pub results: HashMap<String, Value>,
    /// Interpolation scope at the end of the run.
    pub variables: HashMap<String, Value>,
    pub failed_node: Option<String>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl RunRecord {
    /// The runs among `runs` this one started (its `run` nodes,
    /// `recipe_run` calls), in `runs`' order.
    pub fn children<'a>(&self, runs: &'a [RunRecord]) -> impl Iterator<Item = &'a RunRecord> {
        let id = self.id;
        runs.iter().filter(move |r| r.origin.parent() == Some(id))
    }

    /// The run's name, or `ad-hoc run`.
    pub fn title(&self) -> &str {
        self.name.as_deref().unwrap_or("ad-hoc run")
    }

    /// The pane to look at for this run: the one its failure snapshot read,
    /// else the last pane a step targeted or an agent a step started, else
    /// the agent that started the run.
    pub fn pane(&self, events: &[RunEvent]) -> Option<String> {
        let from_events = events.iter().rev().find_map(|e| match &e.kind {
            RunEventKind::PaneSnapshot { pane, .. } => Some(pane.clone()),
            RunEventKind::NodeStarted { pane, .. } => pane.clone(),
            RunEventKind::NodeFinished { result, .. } => crate::tool_payload(result)?
                .get("pane_id")?
                .as_str()
                .map(str::to_string),
            _ => None,
        });
        from_events.or_else(|| match &self.origin {
            RunOrigin::Agent { pane_id } => pane_id.clone(),
            _ => None,
        })
    }

    /// A request that runs this again: the same saved collection (its
    /// current flow) or the same ad-hoc flow, in the same workspace.
    pub fn rerun(&self) -> StartRunRequest {
        StartRunRequest {
            recipe_id: self.recipe_id,
            flow: self.recipe_id.is_none().then(|| self.flow.clone()),
            name: self.name.clone(),
            session_id: Some(self.session_id.clone()),
            ..Default::default()
        }
    }

    /// `failed · 2.10s · 3 passes` for a list row.
    pub fn summary(&self) -> String {
        let mut parts = vec![self.status.as_str().to_string()];
        if let Some(end) = self.finished_at {
            parts.push(seconds(end - self.started_at));
        }
        if self.pass > 1 {
            parts.push(format!("{} passes", self.pass));
        }
        parts.join(" · ")
    }
}

/// What started a run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunOrigin {
    /// The bridge API: the web UI, the TUI, the CLI, curl.
    #[default]
    Api,
    /// An agent's MCP client; `pane_id` is the herdr pane it runs in.
    Agent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
    },
    /// A `recipe_run` call by a node of run `run_id`; none when that run
    /// is unknown.
    Run {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<Uuid>,
    },
    /// Node `node` (a `run` node) of run `run_id`.
    Node {
        run_id: Uuid,
        node: String,
    },
    Schedule,
    /// A herdr event matched one of the recipe's triggers.
    Trigger {
        event: String,
    },
}

impl RunOrigin {
    /// The run that started this one: by a `run` node, or a node calling
    /// `recipe_run`.
    pub fn parent(&self) -> Option<Uuid> {
        match self {
            Self::Node { run_id, .. } => Some(*run_id),
            Self::Run { run_id } => *run_id,
            _ => None,
        }
    }
}

impl std::fmt::Display for RunOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api => f.write_str("the web UI, TUI or API"),
            Self::Agent { pane_id: Some(p) } => write!(f, "an agent in pane {p}"),
            Self::Agent { pane_id: None } => f.write_str("an agent"),
            Self::Run { run_id: Some(id) } => write!(f, "a node of run {id}"),
            Self::Run { run_id: None } => f.write_str("another run"),
            Self::Node { run_id, node } => write!(f, "node {node} of run {run_id}"),
            Self::Schedule => f.write_str("a schedule"),
            Self::Trigger { event } => write!(f, "herdr event {event}"),
        }
    }
}

/// `POST /api/runs` body: a saved recipe (`recipe_id`), an ad-hoc `flow`,
/// or ad-hoc `steps` run one after another.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartRunRequest {
    #[serde(default)]
    pub recipe_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<Flow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<RecipeStep>>,
    #[serde(default)]
    pub name: Option<String>,
    /// herdr workspace id; defaults to the recipe's workspace hint, else "w1".
    #[serde(default)]
    pub session_id: Option<String>,
    /// Added to the run's starting scope, over the recipe's own variables
    /// (a plugin invocation or trigger passes `herdr` = its context).
    #[serde(default)]
    pub variables: HashMap<String, Value>,
    /// Run only these tool nodes, one after another in flow order (sending
    /// one request of a collection); empty runs the whole flow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub node_ids: Vec<String>,
}

/// `pause` / `resume` / `cancel` for `POST /api/runs/{id}/{action}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAction {
    Pause,
    Resume,
    Cancel,
}

impl RunAction {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Resume),
            "cancel" => Some(Self::Cancel),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Cancel => "cancel",
        }
    }
}

/// One thing that happened in a run. A run's events, in `seq` order, are its
/// state: [`RunRecord`] is their fold. Stored one row each in the
/// `run_events` table; streamed by `GET /api/events`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEvent {
    pub run_id: Uuid,
    /// Position in the run's log, from 0. Also the SSE event id.
    pub seq: u64,
    pub at: DateTime<Utc>,
    #[serde(flatten)]
    pub kind: RunEventKind,
}

/// What happened. [`RunEvent`] flattens it, so no variant may have a field
/// named `run_id`, `seq`, `at` or `type` (the line would not read back).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEventKind {
    /// The run's status changed (first event: `running`).
    Status {
        status: RunStatus,
    },
    /// `params` are the node's as sent: placeholders filled in, secrets
    /// redacted. `unresolved` lists the `{{ }}` left in them; `pane` is
    /// the pane it targets; `attempt` counts retries from 1.
    NodeStarted {
        node: String,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        params: Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unresolved: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<String>,
        #[serde(default = "first", skip_serializing_if = "is_first")]
        attempt: u32,
    },
    /// The node's result (a tool's serialized `CallToolResult`, a child
    /// run's outcome) and the port it fired.
    NodeFinished {
        node: String,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        result: Value,
        #[serde(default, skip_serializing_if = "Port::is_out")]
        port: Port,
    },
    /// The attempt failed. With `retry_in_ms` another attempt follows; else
    /// the node is done: its `error` port fires, or the run fails.
    NodeFailed {
        node: String,
        error: String,
        status_code: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_in_ms: Option<u64>,
    },
    NodeCancelled {
        node: String,
    },
    /// The node will not run this pass: every path into it was ruled out.
    NodeSkipped {
        node: String,
    },
    /// A branch took `port`; `checks` is every check it tried on the way.
    Branched {
        node: String,
        port: Port,
        checks: Vec<CheckResult>,
    },
    /// `from` took an edge back to `to`: pass `pass` starts there.
    Looped {
        from: String,
        to: String,
        pass: u32,
    },
    /// Run node `node` started run `child` of `collection` (`name`).
    ChildStarted {
        node: String,
        child: Uuid,
        collection: Uuid,
        name: String,
    },
    /// The last lines of `pane` (as many as the trail's `snapshot_lines`)
    /// when the run failed (`node`: the node that failed it).
    PaneSnapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        pane: String,
        text: String,
    },
    /// The run stops with `error`: a `fail` node, `max_steps` ran out, a
    /// child that could not start.
    Error {
        error: String,
    },
    /// An [`Extract`](crate::Extract) of `node` saved `var`.
    VariableSet {
        node: String,
        var: String,
        scope: VarScope,
        value: Value,
    },
}

fn first() -> u32 {
    1
}

fn is_first(n: &u32) -> bool {
    *n == 1
}

impl RunEventKind {
    /// True for the event that ends a run.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Status { status } if status.is_finished())
    }
}

/// Where a node got to in a run, for drawing it: the last thing that
/// happened to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    Running,
    /// Finished and fired its port.
    Done(Port),
    /// Failed for good (no retry left).
    Failed,
    Skipped,
    Cancelled,
}

/// Each node's [`NodeState`] after `events`; nodes nothing happened to are
/// absent. A loop clears the states of the pass before only as its nodes
/// run again.
pub fn node_states(events: &[RunEvent]) -> HashMap<String, NodeState> {
    let mut out = HashMap::new();
    for e in events {
        let (node, state) = match &e.kind {
            RunEventKind::NodeStarted { node, .. } => (node, NodeState::Running),
            RunEventKind::NodeFinished { node, port, .. } => (node, NodeState::Done(*port)),
            RunEventKind::NodeFailed {
                node,
                retry_in_ms: None,
                ..
            } => (node, NodeState::Failed),
            RunEventKind::NodeSkipped { node } => (node, NodeState::Skipped),
            RunEventKind::NodeCancelled { node } => (node, NodeState::Cancelled),
            RunEventKind::Branched { node, port, .. } => (node, NodeState::Done(*port)),
            _ => continue,
        };
        out.insert(node.clone(), state);
    }
    out
}

/// A run as plain text for troubleshooting, one line per event with its
/// time from the start: what each node sent and got, the variables it set,
/// why each branch took its port, loops, errors and pane snapshots. What
/// `run_get trail` returns and the playgrounds copy.
pub fn trail_text(run: &RunRecord, events: &[RunEvent]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} · run {} · {}",
        run.title(),
        run.id,
        run.status.as_str()
    );
    let took = run
        .finished_at
        .map(|end| format!(" · took {}", seconds(end - run.started_at)))
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "started {} by {}{took} · pass {}",
        run.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
        run.origin,
        run.pass
    );
    let what = |node: &str| {
        run.flow
            .node(node)
            .map_or("?", |n| n.tool_name().unwrap_or(n.kind.name()))
    };
    for event in events {
        let at = format!("+{}", seconds(event.at - run.started_at));
        let line = |out: &mut String, mark: &str, text: &str| {
            let _ = writeln!(out, "{at:>9} {mark} {text}");
        };
        let more = |out: &mut String, text: &str| {
            let _ = writeln!(out, "{:>9}   {text}", "");
        };
        match &event.kind {
            RunEventKind::Status { status } => match status {
                RunStatus::Running if event.seq == 0 => {}
                s => line(&mut out, "■", s.as_str()),
            },
            RunEventKind::NodeStarted {
                node,
                params,
                unresolved,
                pane,
                attempt,
            } => {
                let sent = if params.is_null() {
                    String::new()
                } else {
                    format!(" {}", clip(&params.to_string(), 300))
                };
                let attempt = if *attempt > 1 {
                    format!(" (attempt {attempt})")
                } else {
                    String::new()
                };
                line(
                    &mut out,
                    "▶",
                    &format!("{node} · {}{attempt}{sent}", what(node)),
                );
                if let Some(pane) = pane {
                    more(&mut out, &format!("pane {pane}"));
                }
                if !unresolved.is_empty() {
                    more(&mut out, &format!("unresolved: {}", unresolved.join(", ")));
                }
            }
            RunEventKind::NodeFinished { node, result, port } => {
                let mark = if crate::tool_is_error(result) {
                    "✘"
                } else {
                    "✔"
                };
                let text = crate::tool_text(result);
                let first = text.lines().next().unwrap_or_default();
                let port = if port.is_out() {
                    String::new()
                } else {
                    format!(" → {port}")
                };
                line(
                    &mut out,
                    mark,
                    &format!("{node}{port} · {}", clip(first, 200)),
                );
            }
            RunEventKind::NodeFailed {
                node,
                error,
                status_code,
                retry_in_ms,
            } => {
                let retry = retry_in_ms
                    .map(|ms| format!(" · retrying in {ms}ms"))
                    .unwrap_or_default();
                line(
                    &mut out,
                    "✘",
                    &format!("{node} failed ({status_code}): {error}{retry}"),
                );
            }
            RunEventKind::NodeCancelled { node } => {
                line(&mut out, "■", &format!("{node} cancelled"));
            }
            RunEventKind::NodeSkipped { node } => {
                line(&mut out, "·", &format!("{node} skipped"));
            }
            RunEventKind::VariableSet {
                node,
                var,
                scope,
                value,
            } => line(
                &mut out,
                "=",
                &format!(
                    "{node} set {var} ({}) = {}",
                    match scope {
                        VarScope::Global => "global",
                        VarScope::Collection => "collection",
                    },
                    clip(&value.to_string(), 200)
                ),
            ),
            RunEventKind::Branched { node, port, checks } => {
                line(&mut out, "◆", &format!("{node} → {port}"));
                for c in checks {
                    let found = c
                        .found
                        .as_ref()
                        .map_or("nothing".to_string(), |v| clip(&v.to_string(), 120));
                    let expected = if c.value.is_null() {
                        String::new()
                    } else {
                        format!(" {}", c.value)
                    };
                    more(
                        &mut out,
                        &format!(
                            "rule {}: {} {}{expected} — found {found} {}",
                            c.rule,
                            c.path,
                            c.op.name(),
                            if c.held { "✔" } else { "✘" }
                        ),
                    );
                }
            }
            RunEventKind::Looped { from, to, pass } => {
                line(&mut out, "↺", &format!("{from} → {to} · pass {pass}"));
            }
            RunEventKind::Error { error } => line(&mut out, "✘", error),
            RunEventKind::ChildStarted {
                node, child, name, ..
            } => {
                line(&mut out, "⇢", &format!("{node} · {name} · run {child}"));
            }
            RunEventKind::PaneSnapshot { node, pane, text } => {
                let of = node
                    .as_deref()
                    .map(|s| format!(" after {s}"))
                    .unwrap_or_default();
                line(&mut out, "▤", &format!("pane {pane}{of}:"));
                for l in text.lines() {
                    more(&mut out, &format!("| {l}"));
                }
            }
        }
    }
    out
}

fn seconds(d: chrono::Duration) -> String {
    format!("{:.2}s", d.num_milliseconds() as f64 / 1000.0)
}

/// `s` cut to `max` characters, with an ellipsis when cut.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Op;
    use serde_json::json;

    #[test]
    fn failures_and_children() {
        let failed: Vec<_> = [
            RunStatus::Running,
            RunStatus::Paused,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Cancelled,
            RunStatus::Interrupted,
        ]
        .into_iter()
        .filter(|s| s.is_failure())
        .collect();
        assert_eq!(failed, [RunStatus::Failed, RunStatus::Interrupted]);

        let run = |n: u128, origin: Value| -> RunRecord {
            serde_json::from_value(json!({
                "id": Uuid::from_u128(n), "recipe_id": null, "name": null, "session_id": "w1",
                "flow": { "nodes": [] }, "origin": origin, "pass": 1,
                "status": "completed", "active": [], "results": {}, "variables": {},
                "failed_node": null, "error": null,
                "started_at": "2026-10-25T00:00:00Z", "finished_at": null
            }))
            .unwrap()
        };
        let parent = Uuid::from_u128(1);
        let runs = [
            run(1, json!({ "kind": "api" })),
            run(
                2,
                json!({ "kind": "node", "run_id": parent, "node": "next" }),
            ),
            run(3, json!({ "kind": "api" })),
            run(4, json!({ "kind": "run", "run_id": parent })),
        ];
        let kids: Vec<_> = runs[0].children(&runs).map(|r| r.id).collect();
        assert_eq!(kids, [Uuid::from_u128(2), Uuid::from_u128(4)]);
        assert_eq!(runs[2].children(&runs).count(), 0);
    }

    /// Every variant, every optional field set: a field that collides with
    /// [`RunEvent`]'s own (or a serde attribute that drops one) fails here
    /// instead of silently skipping the line on read.
    #[test]
    fn every_event_kind_round_trips() {
        let id = Uuid::from_u128(1);
        let kinds = [
            RunEventKind::Status {
                status: RunStatus::Paused,
            },
            RunEventKind::NodeStarted {
                node: "s".into(),
                params: json!({"text": "hi"}),
                unresolved: vec!["{{x}}".into()],
                pane: Some("p1".into()),
                attempt: 2,
            },
            RunEventKind::NodeFinished {
                node: "s".into(),
                result: json!({"ok": true}),
                port: Port::Error,
            },
            RunEventKind::NodeFailed {
                node: "s".into(),
                error: "boom".into(),
                status_code: 502,
                retry_in_ms: Some(10),
            },
            RunEventKind::NodeCancelled { node: "s".into() },
            RunEventKind::NodeSkipped { node: "s".into() },
            RunEventKind::Branched {
                node: "b".into(),
                port: Port::Rule(1),
                checks: vec![CheckResult {
                    rule: 1,
                    path: "s.status".into(),
                    op: Op::Eq,
                    value: json!(200),
                    found: Some(json!(200)),
                    held: true,
                }],
            },
            RunEventKind::Looped {
                from: "b".into(),
                to: "s".into(),
                pass: 2,
            },
            RunEventKind::ChildStarted {
                node: "r".into(),
                child: Uuid::from_u128(2),
                collection: id,
                name: "next".into(),
            },
            RunEventKind::PaneSnapshot {
                node: Some("s".into()),
                pane: "p1".into(),
                text: "$ ls".into(),
            },
            RunEventKind::Error {
                error: "max_steps".into(),
            },
            RunEventKind::VariableSet {
                node: "s".into(),
                var: "v".into(),
                scope: VarScope::Collection,
                value: json!(1),
            },
        ];
        for (seq, kind) in kinds.into_iter().enumerate() {
            // No wildcard: a new variant fails to compile until it is listed above.
            match &kind {
                RunEventKind::Status { .. }
                | RunEventKind::NodeStarted { .. }
                | RunEventKind::NodeFinished { .. }
                | RunEventKind::NodeFailed { .. }
                | RunEventKind::NodeCancelled { .. }
                | RunEventKind::NodeSkipped { .. }
                | RunEventKind::Branched { .. }
                | RunEventKind::Looped { .. }
                | RunEventKind::ChildStarted { .. }
                | RunEventKind::PaneSnapshot { .. }
                | RunEventKind::Error { .. }
                | RunEventKind::VariableSet { .. } => {}
            }
            let event = RunEvent {
                run_id: id,
                seq: seq as u64,
                at: DateTime::UNIX_EPOCH,
                kind,
            };
            let line = serde_json::to_string(&event).expect("serializes");
            let back: RunEvent = serde_json::from_str(&line).expect(&line);
            assert_eq!(back, event, "{line}");
        }
    }
}

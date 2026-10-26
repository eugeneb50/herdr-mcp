//! Background recipe runs (`/api/runs`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{CheckResult, Gate, GateAction, RecipeStep, VarScope};

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
/// Stored at `data/runs/{id}/run.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: Uuid,
    /// The saved recipe this run came from; `None` for ad-hoc runs.
    pub recipe_id: Option<Uuid>,
    pub name: Option<String>,
    /// herdr workspace the run is scoped to.
    pub session_id: String,
    /// Snapshot of the steps as they were when the run started.
    pub steps: Vec<RecipeStep>,
    pub origin: RunOrigin,
    /// Snapshot of the recipe's gate; `None` when it has none or the run
    /// sent only some of its steps.
    pub gate: Option<Gate>,
    /// Which pass through the steps this is, from 1; a gate restart adds one.
    /// Also in `variables` as `run.pass`.
    pub pass: u32,
    pub status: RunStatus,
    /// Step being executed (or paused before) right now.
    pub current_step: Option<String>,
    /// Per-step tool results (or `{error, statusCode}`), keyed by step id.
    pub results: HashMap<String, Value>,
    /// Interpolation scope at the end of the run.
    pub variables: HashMap<String, Value>,
    pub failed_step: Option<String>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl RunRecord {
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
            RunEventKind::StepStarted { pane, .. } => pane.clone(),
            RunEventKind::StepFinished { result, .. } => crate::tool_payload(result)?
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
    /// current requests) or the same ad-hoc steps, in the same workspace.
    pub fn rerun(&self) -> StartRunRequest {
        StartRunRequest {
            recipe_id: self.recipe_id,
            steps: self.recipe_id.is_none().then(|| self.steps.clone()),
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
    /// A step of another run (`recipe_run`).
    Run,
    Schedule,
    /// A herdr event matched one of the recipe's triggers.
    Trigger {
        event: String,
    },
}

impl std::fmt::Display for RunOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api => f.write_str("the web UI, TUI or API"),
            Self::Agent { pane_id: Some(p) } => write!(f, "an agent in pane {p}"),
            Self::Agent { pane_id: None } => f.write_str("an agent"),
            Self::Run => f.write_str("another run"),
            Self::Schedule => f.write_str("a schedule"),
            Self::Trigger { event } => write!(f, "herdr event {event}"),
        }
    }
}

/// `POST /api/runs` body: a saved recipe (`recipe_id`) or ad-hoc `steps`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartRunRequest {
    #[serde(default)]
    pub recipe_id: Option<Uuid>,
    #[serde(default)]
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
    /// Run only these steps of the recipe, in recipe order (sending one
    /// request of a collection); empty runs them all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub step_ids: Vec<String>,
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
/// state: [`RunRecord`] is their fold. Stored one JSON object per line in
/// `data/runs/{id}/events.jsonl`; streamed by `GET /api/events`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEvent {
    pub run_id: Uuid,
    /// Position in the run's log, from 0. Also the SSE event id.
    pub seq: u64,
    pub at: DateTime<Utc>,
    #[serde(flatten)]
    pub kind: RunEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEventKind {
    /// The run's status changed (first event: `running`).
    Status {
        status: RunStatus,
    },
    /// `params` are the step's as sent: placeholders filled in, secrets
    /// redacted. `unresolved` lists the `{{ }}` left in them; `pane` is
    /// the pane the step targets.
    StepStarted {
        step: String,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        params: Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unresolved: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<String>,
    },
    /// The step's tool result (a serialized `CallToolResult`).
    StepFinished {
        step: String,
        result: Value,
    },
    StepFailed {
        step: String,
        error: String,
        status_code: u16,
    },
    StepCancelled {
        step: String,
    },
    /// The gate after the last step: the rule that matched (none: the
    /// run completes) and every check it evaluated on the way.
    GateDecided {
        rule: Option<usize>,
        action: Option<GateAction>,
        checks: Vec<CheckResult>,
    },
    /// The last lines of `pane` (as many as the trail's `snapshot_lines`) when the run failed (`step`: the
    /// failed step; none for a gate failure).
    PaneSnapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<String>,
        pane: String,
        text: String,
    },
    /// The run stops with `error` and no failed step: a gate `fail` rule, or
    /// the gate's `max_steps` ran out.
    Error {
        error: String,
    },
    /// An [`Extract`](crate::Extract) of `step` saved `var`.
    VariableSet {
        step: String,
        var: String,
        scope: VarScope,
        value: Value,
    },
}

impl RunEventKind {
    /// True for the event that ends a run.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Status { status } if status.is_finished())
    }
}

/// A run as plain text for troubleshooting, one line per event with its
/// time from the start: what each step sent and got, the variables it set,
/// why the gate chose its rule, errors and pane snapshots. What `run_get
/// trail` returns and the playgrounds copy.
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
    let tool = |step: &str| {
        run.steps
            .iter()
            .find(|s| s.id == step)
            .map_or("?", |s| s.tool.as_str())
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
            RunEventKind::StepStarted {
                step,
                params,
                unresolved,
                pane,
            } => {
                let sent = if params.is_null() {
                    String::new()
                } else {
                    format!(" {}", clip(&params.to_string(), 300))
                };
                line(&mut out, "▶", &format!("{step} · {}{sent}", tool(step)));
                if let Some(pane) = pane {
                    more(&mut out, &format!("pane {pane}"));
                }
                if !unresolved.is_empty() {
                    more(&mut out, &format!("unresolved: {}", unresolved.join(", ")));
                }
            }
            RunEventKind::StepFinished { step, result } => {
                let mark = if crate::tool_is_error(result) {
                    "✘"
                } else {
                    "✔"
                };
                let text = crate::tool_text(result);
                let first = text.lines().next().unwrap_or_default();
                line(&mut out, mark, &format!("{step} · {}", clip(first, 200)));
            }
            RunEventKind::StepFailed {
                step,
                error,
                status_code,
            } => line(
                &mut out,
                "✘",
                &format!("{step} failed ({status_code}): {error}"),
            ),
            RunEventKind::StepCancelled { step } => {
                line(&mut out, "■", &format!("{step} cancelled"));
            }
            RunEventKind::VariableSet {
                step,
                var,
                scope,
                value,
            } => line(
                &mut out,
                "=",
                &format!(
                    "{step} set {var} ({}) = {}",
                    match scope {
                        VarScope::Global => "global",
                        VarScope::Collection => "collection",
                    },
                    clip(&value.to_string(), 200)
                ),
            ),
            RunEventKind::GateDecided {
                rule,
                action,
                checks,
            } => {
                let decided = match (rule, action) {
                    (Some(rule), Some(action)) => {
                        format!("rule {} → {}", rule + 1, crate::gate::action_text(action))
                    }
                    _ => "no rule matched → end".to_string(),
                };
                line(&mut out, "◆", &format!("gate: {decided}"));
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
                            c.rule + 1,
                            c.path,
                            c.op.name(),
                            if c.held { "✔" } else { "✘" }
                        ),
                    );
                }
            }
            RunEventKind::Error { error } => line(&mut out, "✘", error),
            RunEventKind::PaneSnapshot { step, pane, text } => {
                let of = step
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

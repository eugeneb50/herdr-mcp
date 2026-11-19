//! Background recipe runs: the one executor behind `/api/runs`, the
//! scheduler, recipe triggers and the plugin's recipe picker.
//!
//! A run is stored as an immutable [`RunHeader`] (the `runs` table: what
//! ran, with which starting variables) plus an append-only event log
//! (`run_events`). The log is the run's state: [`RunRecord`] is computed by
//! folding it ([`fold`]), with the same [`apply`] the executor uses live.
//! [`Runs`] holds the control channel of each run executing in this process
//! and the broadcast that carries new events to `/api/events`; a log without
//! a terminal event and without a live run reads as `interrupted`.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use herdr_mcp_api::flow::{Shape, decide};
use herdr_mcp_api::{
    Edge, Extract, Flow, Node, NodeKind, Port, RunAction, RunEvent, RunEventKind, RunOrigin,
    RunRecord, RunStatus, RunTrail, StartRunRequest, VarScope,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, watch};
use tokio::task::{JoinHandle, JoinSet};
use tracing::Instrument;
use uuid::Uuid;

use crate::server::{
    HerdrMcpServer, dispatch_tool, navigate, resolve_json_path, resolve_variables, step_text,
    unresolved_placeholders,
};

/// The immutable start of a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunHeader {
    pub id: Uuid,
    pub recipe_id: Option<Uuid>,
    pub name: Option<String>,
    pub session_id: String,
    /// Snapshot of the flow as it was when the run started.
    pub flow: Flow,
    #[serde(default)]
    pub origin: RunOrigin,
    /// Interpolation scope before the first step.
    pub seed: HashMap<String, Value>,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Run,
    Pause,
    Cancel,
}

/// Events buffered per subscriber before it lags (and re-reads the log).
const EVENT_BUFFER: usize = 1024;

/// Live runs in this process: their control channels, and the broadcast of
/// every run's new events.
#[derive(Clone)]
pub struct Runs {
    live: Arc<Mutex<HashMap<Uuid, watch::Sender<Control>>>>,
    events: broadcast::Sender<RunEvent>,
}

impl Default for Runs {
    fn default() -> Self {
        Self {
            live: Arc::default(),
            events: broadcast::channel(EVENT_BUFFER).0,
        }
    }
}

impl Runs {
    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, watch::Sender<Control>>> {
        crate::lock(&self.live)
    }

    pub fn is_live(&self, id: Uuid) -> bool {
        self.live().contains_key(&id)
    }

    /// New events of every run, from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.events.subscribe()
    }

    /// Signal a live run. Returns false if no run with that id is executing.
    /// Pause holds nodes that have not started (running ones finish);
    /// cancel also aborts the nodes in flight and the child runs.
    pub fn control(&self, id: Uuid, action: RunAction) -> bool {
        self.set(
            id,
            match action {
                RunAction::Pause => Control::Pause,
                RunAction::Resume => Control::Run,
                RunAction::Cancel => Control::Cancel,
            },
        )
    }

    fn set(&self, id: Uuid, control: Control) -> bool {
        self.live().get(&id).is_some_and(|tx| {
            // Cancel is final: a later pause/resume must not undo it.
            tx.send_if_modified(|c| {
                let changed = *c != Control::Cancel && *c != control;
                if changed {
                    *c = control;
                }
                changed
            });
            true
        })
    }

    /// Fill in [`RunStatus::Interrupted`] for an unfinished run whose
    /// executor is gone. `was_live` must be checked *before* the log was
    /// read: the executor appends its last event and only then unregisters,
    /// so checking afterwards could call a just-finished run interrupted.
    fn observe(mut run: RunRecord, was_live: bool) -> RunRecord {
        if !run.status.is_finished() && !was_live {
            run.status = RunStatus::Interrupted;
        }
        run
    }
}

/// The scope key holding [`RunRecord::pass`].
const PASS_VAR: &str = "run.pass";

/// A run's record: its header with every event applied.
pub fn fold(header: RunHeader, events: &[RunEvent]) -> RunRecord {
    let mut variables = header.seed;
    variables.insert(PASS_VAR.to_string(), 1.into());
    let mut run = RunRecord {
        id: header.id,
        recipe_id: header.recipe_id,
        name: header.name,
        session_id: header.session_id,
        flow: header.flow,
        origin: header.origin,
        pass: 1,
        status: RunStatus::Running,
        active: Vec::new(),
        results: HashMap::new(),
        variables,
        failed_node: None,
        error: None,
        started_at: header.started_at,
        finished_at: None,
    };
    for event in events {
        apply(&mut run, event);
    }
    run
}

/// Advance a record by one event.
pub fn apply(run: &mut RunRecord, event: &RunEvent) {
    let mut idle = |node: &str| run.active.retain(|n| n != node);
    match &event.kind {
        RunEventKind::Status { status } => {
            run.status = *status;
            if status.is_finished() {
                run.finished_at = Some(event.at);
                run.active.clear();
            }
        }
        RunEventKind::NodeStarted { node, .. } => {
            if !run.active.contains(node) {
                run.active.push(node.clone());
            }
        }
        RunEventKind::NodeFinished { node, result, port } => {
            idle(node);
            record_result(run, node, result);
            // The failure went out the error port: the run goes on.
            if *port == Port::Error && run.failed_node.as_ref() == Some(node) {
                run.failed_node = None;
                run.error = None;
            }
        }
        RunEventKind::NodeFailed {
            node,
            error,
            status_code,
            retry_in_ms,
        } => {
            idle(node);
            if retry_in_ms.is_none() {
                run.results.insert(
                    node.clone(),
                    serde_json::json!({ "error": error, "statusCode": status_code }),
                );
                run.failed_node = Some(node.clone());
                run.error = Some(error.clone());
            }
        }
        RunEventKind::NodeCancelled { node } => {
            idle(node);
            run.results
                .insert(node.clone(), serde_json::json!({ "error": "cancelled" }));
        }
        RunEventKind::Branched { node, port, .. } => {
            let result = serde_json::json!({ "port": port });
            run.variables.insert(node.clone(), result.clone());
            run.results.insert(node.clone(), result);
        }
        RunEventKind::Looped { pass, .. } => {
            run.pass = *pass;
            run.variables.insert(PASS_VAR.to_string(), (*pass).into());
        }
        RunEventKind::VariableSet { var, value, .. } => {
            run.variables.insert(var.clone(), value.clone());
        }
        RunEventKind::Error { error } => run.error = Some(error.clone()),
        RunEventKind::NodeSkipped { .. }
        | RunEventKind::PaneSnapshot { .. }
        | RunEventKind::ChildStarted { .. } => {}
    }
}

/// Store a node's result and expose it to later nodes: under the node id,
/// flattened (`result.*`), and — when it is an agent handle — under its pane
/// id and role so `{{pane_id.output}}` / `{{role.output}}` resolve.
fn record_result(run: &mut RunRecord, step: &str, json: &Value) {
    if json.is_null() {
        return;
    }
    run.variables
        .extend(crate::variables::extract_variables(json));
    run.variables.insert(step.to_string(), json.clone());
    // Agent tools return the handle as JSON text inside the result.
    let payload = tool_payload(json);
    if let Some(pane_id) = payload.get("pane_id").and_then(Value::as_str) {
        run.variables.insert(pane_id.to_string(), payload.clone());
        if let Some(role) = payload.get("role").and_then(Value::as_str)
            && !role.is_empty()
        {
            run.variables.insert(role.to_string(), payload.clone());
        }
    }
    run.results.insert(step.to_string(), json.clone());
}

/// The JSON a tool returned: its first text content parsed as JSON, else
/// the result as-is.
fn tool_payload(result: &Value) -> Value {
    result
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_else(|| result.clone())
}

/// Load a run's record (`None` if there is no such run).
pub async fn load(server: &HerdrMcpServer, id: Uuid) -> anyhow::Result<Option<RunRecord>> {
    let was_live = server.runs.is_live(id);
    let Some(header) = server.persistence.load_run_header(&id).await? else {
        return Ok(None);
    };
    let events = server.persistence.load_run_events(&id, 0).await?;
    Ok(Some(Runs::observe(fold(header, &events), was_live)))
}

/// The newest `limit` runs' records (all without one), newest first. Sorts
/// on the headers so only the runs returned have their logs read.
pub async fn list(server: &HerdrMcpServer, limit: Option<usize>) -> anyhow::Result<Vec<RunRecord>> {
    let newest = server.persistence.list_run_headers(limit).await?;
    let mut runs = Vec::with_capacity(newest.len());
    for header in newest {
        if let Some(run) = load(server, header.id).await? {
            runs.push(run);
        }
    }
    Ok(runs)
}

tokio::task_local! {
    /// The run whose step is being dispatched. `dispatch_tool` passes it to
    /// the tool in the call's meta ([`STEP_OF_KEY`]), so a `recipe_run` step
    /// records it as the new run's parent.
    pub(crate) static STEP_OF: Uuid;
}

/// The meta key carrying [`STEP_OF`] into the in-process tool session.
pub(crate) const STEP_OF_KEY: &str = "herdr-mcp/step-of";

/// New events as a stream. With `run`, only that run's events: first its
/// logged events from `since` (if given), then live ones, without gaps or
/// repeats (a subscriber that falls behind re-reads the log). Without `run`,
/// every run's new events (a lagging subscriber skips what it missed).
pub fn event_stream(
    server: HerdrMcpServer,
    run: Option<Uuid>,
    since: Option<u64>,
) -> impl futures_util::Stream<Item = RunEvent> + Send {
    async_stream::stream! {
        // Subscribe before reading the log so nothing falls in between.
        let mut rx = server.runs.subscribe();
        let mut next = since.unwrap_or(0);
        if let (Some(id), Some(since)) = (run, since) {
            for event in server.persistence.load_run_events(&id, since).await.unwrap_or_default() {
                next = event.seq + 1;
                yield event;
            }
        }
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Some(id) = run {
                        if event.run_id != id || event.seq < next {
                            continue;
                        }
                        next = event.seq + 1;
                    }
                    yield event;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if let Some(id) = run {
                        for event in server.persistence.load_run_events(&id, next).await.unwrap_or_default() {
                            next = event.seq + 1;
                            yield event;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

/// A started run's first record and the handle resolving to its last.
type Started = Result<(RunRecord, JoinHandle<RunRecord>), StartError>;

#[derive(Debug)]
pub enum StartError {
    RecipeNotFound,
    /// A `node_ids` entry that is not a tool node of the flow.
    UnknownNode(String),
    /// The flow cannot run: [`Flow::problems`], joined.
    Invalid(String),
    Persist(anyhow::Error),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RecipeNotFound => f.write_str("Recipe not found"),
            Self::UnknownNode(id) => write!(f, "the flow has no tool node `{id}`"),
            Self::Invalid(why) => write!(f, "the flow cannot run: {why}"),
            Self::Persist(e) => write!(f, "{e:#}"),
        }
    }
}

/// Start a run in the background. Returns its first record (already
/// persisted) and a handle resolving to the final record.
pub async fn start_run(
    server: &HerdrMcpServer,
    req: StartRunRequest,
    origin: RunOrigin,
) -> Started {
    start_with(server, req, Control::Run, origin).await
}

/// `ids`' tool nodes of `flow`, one after another from a new start (in
/// flow order, with their retry and timeout).
fn only(flow: &Flow, ids: &[String]) -> Result<Flow, StartError> {
    if let Some(id) = ids
        .iter()
        .find(|id| flow.node(id).and_then(Node::tool_name).is_none())
    {
        return Err(StartError::UnknownNode(id.clone()));
    }
    let mut out = Flow::chain(Vec::new());
    out.nodes
        .extend(flow.tools().filter(|n| ids.contains(&n.id)).cloned());
    out.edges = out
        .nodes
        .windows(2)
        .map(|w| Edge {
            from: w[0].id.clone(),
            port: Port::Out,
            to: w[1].id.clone(),
        })
        .collect();
    Ok(out)
}

pub(crate) async fn start_with(
    server: &HerdrMcpServer,
    req: StartRunRequest,
    initial: Control,
    origin: RunOrigin,
) -> Started {
    let (recipe_id, name, mut flow, trail, collection) = match req.recipe_id {
        Some(id) => {
            let recipe = server
                .persistence
                .load_recipe(&id)
                .await
                .map_err(StartError::Persist)?
                .ok_or(StartError::RecipeNotFound)?;
            (
                Some(id),
                Some(recipe.name),
                recipe.flow,
                recipe.trail,
                recipe.variables,
            )
        }
        None => (
            None,
            req.name,
            req.flow
                .or_else(|| req.steps.map(Flow::chain))
                .unwrap_or_default(),
            RunTrail::default(),
            HashMap::new(),
        ),
    };
    if !req.node_ids.is_empty() {
        flow = only(&flow, &req.node_ids)?;
    }
    let problems = flow.problems();
    if !problems.is_empty() {
        return Err(StartError::Invalid(problems.join("; ")));
    }
    let session_id = run_workspace(req.session_id, &collection, &req.variables);
    let seed = scope(server, &session_id, collection, req.variables).await;
    let header = RunHeader {
        id: Uuid::new_v4(),
        recipe_id,
        name,
        session_id,
        flow,
        origin,
        seed,
        started_at: Utc::now(),
    };
    // Register before anything is written so a reader never sees the run
    // without a live executor (which would read as interrupted).
    let (tx, rx) = watch::channel(initial);
    server.runs.live().insert(header.id, tx);
    if let Err(e) = server.persistence.save_run_header(&header).await {
        server.runs.live().remove(&header.id);
        return Err(StartError::Persist(e));
    }
    let shape = header.flow.shape();
    let mut exec = Executor {
        server: server.clone(),
        run: fold(header, &[]),
        next_seq: 0,
        nodes_run: 0,
        snapshot_lines: trail.snapshot_lines.unwrap_or_else(|| {
            let n = server
                .settings()
                .number(herdr_mcp_api::settings::SNAPSHOT_LINES);
            u32::try_from(n).unwrap_or(u32::MAX)
        }),
        last_pane: None,
        ctl: rx,
        shape,
        state: HashMap::new(),
        fired: HashSet::new(),
        ruled_out: HashSet::new(),
        ready: VecDeque::new(),
        loops: Vec::new(),
        tasks: JoinSet::new(),
        attempts: HashMap::new(),
        children: HashMap::new(),
    };
    exec.emit(RunEventKind::Status {
        status: RunStatus::Running,
    })
    .await;
    let first = exec.run.clone();
    // Every log line of the run's task carries its id. A run node's child
    // nests under its parent's span (start_child runs inside it); any other
    // run is a root, not a child of the request that started it.
    let parent = match first.origin {
        RunOrigin::Node { .. } => tracing::Span::current().id(),
        _ => None,
    };
    let span = tracing::info_span!(
        parent: parent,
        "run",
        run_id = %first.id,
        collection = first.title(),
    );
    Ok((first, tokio::spawn(exec.execute().instrument(span))))
}

/// A run's trail ([`herdr_mcp_api::trail_text`]) with, indented under it,
/// the trail of each child run its `run` nodes started, and so on down.
/// `None`: no such run.
pub async fn trail(server: &HerdrMcpServer, id: Uuid) -> anyhow::Result<Option<String>> {
    let mut out = String::new();
    let mut todo = vec![(id, 0usize)];
    while let Some((id, depth)) = todo.pop() {
        let Some(run) = load(server, id).await? else {
            // ponytail: a pruned child just leaves its branch out.
            continue;
        };
        let events = server.persistence.load_run_events(&id, 0).await?;
        let children: Vec<Uuid> = events
            .iter()
            .filter_map(|e| match e.kind {
                RunEventKind::ChildStarted { child, .. } => Some(child),
                _ => None,
            })
            .collect();
        todo.extend(children.into_iter().rev().map(|c| (c, depth + 1)));
        if !out.is_empty() {
            out.push('\n');
        }
        let indent = "    ".repeat(depth);
        for line in herdr_mcp_api::trail_text(&run, &events).lines() {
            out.push_str(&indent);
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok((!out.is_empty()).then_some(out))
}

/// [`start_with`] behind a named `Send` future: a child's executor starts
/// children too, and the recursion needs a type to stop at.
fn start_child(
    server: HerdrMcpServer,
    req: StartRunRequest,
    initial: Control,
    origin: RunOrigin,
) -> std::pin::Pin<Box<dyn Future<Output = Started> + Send>> {
    Box::pin(async move { start_with(&server, req, initial, origin).await })
}

/// The workspace a run is scoped to: the requested one, else a hint in the
/// request's or the recipe's variables, else `w1`.
pub(crate) fn run_workspace(
    requested: Option<String>,
    collection: &HashMap<String, Value>,
    request: &HashMap<String, Value>,
) -> String {
    let mut hints = collection.clone();
    hints.extend(request.iter().map(|(k, v)| (k.clone(), v.clone())));
    workspace_for(requested, &hints)
}

/// The workspace a run is scoped to: the requested one, else a hint saved in
/// the recipe's variables, else `w1`. Unresolved `{{ }}` placeholders never
/// count (they would end up in file names).
fn workspace_for(requested: Option<String>, scope: &HashMap<String, Value>) -> String {
    let usable = |s: &str| !s.trim().is_empty() && !s.contains("{{");
    requested
        .filter(|s| usable(s))
        .or_else(|| {
            [
                "session_id",
                "workspace_id",
                "template.session_id",
                "template.workspace_id",
            ]
            .iter()
            .filter_map(|k| scope.get(*k).and_then(Value::as_str))
            .find(|s| usable(s))
            .map(str::to_string)
        })
        .unwrap_or_else(|| "w1".to_string())
}

/// The scope a run starts with (and `/api/variables/resolve` resolves in),
/// lowest priority first: live agent handles by pane id / role (every
/// workspace's, then `ws`'s), `template.session_id` / `template.workspace_id`
/// = `ws`, globals, the recipe's variables, the request's. Step results and
/// extracted variables go on top as the run goes.
pub(crate) async fn scope(
    server: &HerdrMcpServer,
    ws: &str,
    collection: HashMap<String, Value>,
    request: HashMap<String, Value>,
) -> HashMap<String, Value> {
    let mut scope = server.registry.seed(None).await;
    scope.extend(server.registry.seed(Some(ws)).await);
    for key in ["template.session_id", "template.workspace_id"] {
        scope.insert(key.to_string(), Value::String(ws.to_string()));
    }
    match server.persistence.globals().await {
        Ok(globals) => scope.extend(globals),
        Err(e) => tracing::warn!("run scope: reading global variables: {e:#}"),
    }
    scope.extend(collection);
    scope.extend(request);
    scope
}

/// What an [`Extract`] reads: `path` followed into the step's result, else
/// into the JSON its tool returned as text; an empty path is that text.
fn extract_value(result: &Value, path: &str) -> Option<Value> {
    let path = path.trim();
    if path.is_empty() {
        return Some(step_text(result));
    }
    navigate(result, path).or_else(|| navigate(&herdr_mcp_api::tool_payload(result)?, path))
}

/// Delete the finished runs of `recipe` (`None`: ad-hoc runs, and runs of
/// deleted recipes) that its retention no longer keeps: beyond the newest
/// `keep_runs`, or older than `keep_days`. The recipe's own [`RunTrail`]
/// wins over the global `runs.keep-*` settings; 0 keeps everything.
pub(crate) async fn prune(server: &HerdrMcpServer, recipe: Option<Uuid>) {
    if let Err(e) = prune_runs(server, Some(recipe)).await {
        tracing::warn!("pruning runs of {recipe:?}: {e:#}");
    }
}

/// [`prune`] every recipe's runs and the ad-hoc ones (at daemon start, so
/// `keep_days` applies to collections that no longer run).
pub async fn prune_all(server: &HerdrMcpServer) {
    if let Err(e) = prune_runs(server, None).await {
        tracing::warn!("pruning runs: {e:#}");
    }
}

/// Prune one group (`only`) or all of them, reading each run header once.
async fn prune_runs(server: &HerdrMcpServer, only: Option<Option<Uuid>>) -> anyhow::Result<()> {
    let store = &server.persistence;
    let trails: HashMap<Uuid, RunTrail> = store
        .list_recipes(None)
        .await?
        .into_iter()
        .map(|r| (r.id, r.trail))
        .collect();
    // A deleted recipe's runs fall in with the ad-hoc ones.
    let group_of = |recipe: Option<Uuid>| recipe.filter(|r| trails.contains_key(r));
    let only = only.map(group_of);
    let mut groups: HashMap<Option<Uuid>, Vec<(DateTime<Utc>, Uuid)>> = HashMap::new();
    for header in store.list_run_headers(None).await? {
        let id = header.id;
        let group = group_of(header.recipe_id);
        if only.is_some_and(|o| o != group) || server.runs.is_live(id) {
            continue;
        }
        groups
            .entry(group)
            .or_default()
            .push((header.started_at, id));
    }
    let global = |key| u32::try_from(server.settings().number(key)).unwrap_or(u32::MAX);
    for (group, mut runs) in groups {
        let trail = group
            .and_then(|id| trails.get(&id).copied())
            .unwrap_or_default();
        let keep_runs = trail
            .keep_runs
            .unwrap_or_else(|| global(herdr_mcp_api::settings::KEEP_RUNS));
        let keep_days = trail
            .keep_days
            .unwrap_or_else(|| global(herdr_mcp_api::settings::KEEP_DAYS));
        runs.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        let cutoff = Utc::now() - chrono::Duration::days(i64::from(keep_days));
        for (i, (at, id)) in runs.into_iter().enumerate() {
            let over = keep_runs > 0 && i >= keep_runs as usize;
            let old = keep_days > 0 && at < cutoff;
            if over || old {
                store.delete_run(&id).await?;
            }
        }
    }
    Ok(())
}

/// Where a node stands in the current pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// Every input is in; waiting for its turn (or for a resume).
    Queued,
    Running,
    Done,
    /// Every input was ruled out.
    Skipped,
}

/// What a node's task came back with.
enum Outcome {
    Tool(Box<Result<rmcp::model::CallToolResult, (StatusCode, String)>>),
    /// A retry's delay is over: start attempt `n`.
    Retry(u32),
    Waited,
    Child(Uuid, Box<Result<RunRecord, tokio::task::JoinError>>),
}

/// A run being executed: its record so far (the fold of what it emitted)
/// and where each node of its flow stands.
struct Executor {
    server: HerdrMcpServer,
    run: RunRecord,
    next_seq: u64,
    /// Nodes started so far, every pass counted (for `max_steps`).
    nodes_run: u32,
    /// Lines of `last_pane` saved when the run fails; 0: none.
    snapshot_lines: u32,
    /// The pane the run touched last: a node's `pane_id`, or an agent a
    /// node returned.
    last_pane: Option<String>,
    ctl: watch::Receiver<Control>,
    shape: Shape,
    /// Nodes not listed are pending: some input is still open.
    state: HashMap<String, Slot>,
    /// Forward edges (indexes into the flow's edges) that fired this pass,
    /// and those ruled out (their node took another port, or was skipped).
    fired: HashSet<usize>,
    ruled_out: HashSet<usize>,
    ready: VecDeque<String>,
    /// Loop edges taken, `(from, to)`, waiting for `to`'s part of the flow
    /// to settle.
    loops: Vec<(String, String)>,
    tasks: JoinSet<(String, Outcome)>,
    /// Attempts started per tool node this pass.
    attempts: HashMap<String, u32>,
    /// Live child runs by the run node that started them.
    children: HashMap<String, Uuid>,
}

impl Executor {
    /// Record an event: apply it, append it to the log, then broadcast it.
    async fn emit(&mut self, kind: RunEventKind) {
        let event = RunEvent {
            run_id: self.run.id,
            seq: self.next_seq,
            at: Utc::now(),
            kind,
        };
        self.next_seq += 1;
        apply(&mut self.run, &event);
        if let Err(e) = self.server.persistence.append_run_event(&event).await {
            tracing::warn!("failed to append event: {e}");
        }
        // No subscribers is fine.
        let _ = self.server.runs.events.send(event);
    }

    async fn execute(mut self) -> RunRecord {
        tracing::info!(origin = %self.run.origin, "run started");
        let status = self.execute_flow().await;
        tracing::info!(
            status = status.as_str(),
            passes = self.run.pass,
            "run finished"
        );
        self.emit(RunEventKind::Status { status }).await;
        self.server.runs.live().remove(&self.run.id);
        prune(&self.server, self.run.recipe_id).await;
        self.run
    }

    /// Save the last lines of the pane the run touched last into its log
    /// (`node`: the node that failed).
    async fn snapshot(&mut self, node: Option<String>) {
        let Some(pane) = self.last_pane.clone() else {
            return;
        };
        if self.snapshot_lines == 0 {
            return;
        }
        let args = serde_json::json!({
            "pane_id": pane,
            "lines": self.snapshot_lines,
            "source": "recent",
        });
        match dispatch_tool(
            &self.server,
            "read_pane",
            args,
            crate::security::Origin::Run,
        )
        .await
        {
            Ok(result) if result.is_error != Some(true) => {
                let result = serde_json::to_value(&result).unwrap_or_default();
                let text = herdr_mcp_api::tool_text(&result);
                self.emit(RunEventKind::PaneSnapshot { node, pane, text })
                    .await;
            }
            Ok(result) => {
                let result = serde_json::to_value(&result).unwrap_or_default();
                let error = herdr_mcp_api::tool_text(&result);
                tracing::warn!("cannot snapshot pane {pane}: {error}");
            }
            Err((code, error)) => {
                tracing::warn!("cannot snapshot pane {pane}: {code} {error}");
            }
        }
    }

    /// Run the flow from its start until no path is left; the final status.
    async fn execute_flow(&mut self) -> RunStatus {
        let start = self
            .run
            .flow
            .start()
            .map(|n| n.id.clone())
            .unwrap_or_default();
        self.emit(RunEventKind::NodeFinished {
            node: start.clone(),
            result: Value::Null,
            port: Port::Out,
        })
        .await;
        self.fire(&start, Port::Out).await;
        loop {
            let control = *self.ctl.borrow_and_update();
            // Every turn, not only on a change: a change seen here is not
            // seen again by `changed()` below.
            for child in self.children.values() {
                self.server.runs.set(*child, control);
            }
            match control {
                Control::Cancel => return self.stop(RunStatus::Cancelled).await,
                Control::Pause => {
                    if self.run.status != RunStatus::Paused {
                        self.emit(RunEventKind::Status {
                            status: RunStatus::Paused,
                        })
                        .await;
                    }
                }
                Control::Run => {
                    if self.run.status == RunStatus::Paused {
                        self.emit(RunEventKind::Status {
                            status: RunStatus::Running,
                        })
                        .await;
                    }
                    self.take_loops().await;
                    while let Some(node) = self.ready.pop_front() {
                        if let Some(end) = self.begin(&node).await {
                            return self.stop(end).await;
                        }
                    }
                    if self.tasks.is_empty() && self.loops.is_empty() {
                        return RunStatus::Completed;
                    }
                }
            }
            tokio::select! {
                Some(joined) = self.tasks.join_next() => {
                    let end = match joined {
                        Ok((node, outcome)) => self.settle(node, outcome).await,
                        Err(e) => {
                            let error = format!("a node's task died: {e}");
                            self.emit(RunEventKind::Error { error }).await;
                            Some(RunStatus::Failed)
                        }
                    };
                    if let Some(end) = end {
                        return self.stop(end).await;
                    }
                }
                changed = self.ctl.changed() => {
                    // Sender gone means the run was dropped from `Runs`.
                    if changed.is_err() {
                        return self.stop(RunStatus::Cancelled).await;
                    }
                }
            }
        }
    }

    /// End the run with `status`: whatever still runs is cancelled, child
    /// runs too.
    async fn stop(&mut self, status: RunStatus) -> RunStatus {
        self.tasks.abort_all();
        for child in self.children.values() {
            self.server.runs.set(*child, Control::Cancel);
        }
        let running: Vec<String> = self.run.active.clone();
        for node in running {
            self.emit(RunEventKind::NodeCancelled { node }).await;
        }
        status
    }

    /// `node` finished and fired `port`: pass it on along that port's
    /// edges and rule out its other ports'.
    async fn fire(&mut self, node: &str, port: Port) {
        self.state.insert(node.to_string(), Slot::Done);
        let mut check = Vec::new();
        for (i, e) in self.run.flow.edges.iter().enumerate() {
            if e.from != node {
                continue;
            }
            if self.shape.back.contains(&i) {
                if e.port == port {
                    self.loops.push((e.from.clone(), e.to.clone()));
                }
                continue;
            }
            if e.port == port {
                self.fired.insert(i);
            } else {
                self.ruled_out.insert(i);
            }
            check.push(e.to.clone());
        }
        self.settle_inputs(check).await;
    }

    /// Queue each of `nodes` once all its inputs are in, or skip it (and
    /// rule out its edges, on down) when every one was ruled out.
    async fn settle_inputs(&mut self, mut nodes: Vec<String>) {
        while let Some(node) = nodes.pop() {
            if self.state.contains_key(&node) {
                continue;
            }
            let inputs = self.shape.inputs.get(&node).map_or(&[][..], Vec::as_slice);
            let open = inputs
                .iter()
                .any(|i| !self.fired.contains(i) && !self.ruled_out.contains(i));
            if open {
                continue;
            }
            if inputs.iter().any(|i| self.fired.contains(i)) {
                self.state.insert(node.clone(), Slot::Queued);
                self.ready.push_back(node);
                continue;
            }
            self.state.insert(node.clone(), Slot::Skipped);
            self.emit(RunEventKind::NodeSkipped { node: node.clone() })
                .await;
            for (i, e) in self.run.flow.edges.iter().enumerate() {
                if e.from == node && !self.shape.back.contains(&i) {
                    self.ruled_out.insert(i);
                    nodes.push(e.to.clone());
                }
            }
        }
    }

    /// Start the next pass for each loop taken whose part of the flow has
    /// settled: that part is reset and its first node queued.
    async fn take_loops(&mut self) {
        let mut waiting = Vec::new();
        for (from, to) in std::mem::take(&mut self.loops) {
            let region: HashSet<String> = self
                .shape
                .region(&self.run.flow, &to)
                .into_iter()
                .map(str::to_string)
                .collect();
            let busy = region
                .iter()
                .any(|n| self.state.get(n) == Some(&Slot::Running));
            if busy {
                waiting.push((from, to));
                continue;
            }
            for n in &region {
                self.state.remove(n);
                self.attempts.remove(n);
            }
            self.ready.retain(|n| !region.contains(n));
            for (i, e) in self.run.flow.edges.iter().enumerate() {
                if region.contains(&e.from) && region.contains(&e.to) {
                    self.fired.remove(&i);
                    self.ruled_out.remove(&i);
                }
            }
            let pass = self.run.pass + 1;
            self.emit(RunEventKind::Looped {
                from,
                to: to.clone(),
                pass,
            })
            .await;
            self.state.insert(to.clone(), Slot::Queued);
            self.ready.push_back(to);
        }
        self.loops = waiting;
    }

    /// Start a queued node. `Some` when that ends the run.
    async fn begin(&mut self, id: &str) -> Option<RunStatus> {
        if self.state.get(id) != Some(&Slot::Queued) {
            return None;
        }
        let max = self.run.flow.max_steps;
        if self.nodes_run >= max {
            let error = format!(
                "stopped after {} node runs (max_steps) on pass {}",
                self.nodes_run, self.run.pass
            );
            self.emit(RunEventKind::Error { error }).await;
            self.snapshot(None).await;
            return Some(RunStatus::Failed);
        }
        self.nodes_run += 1;
        self.state.insert(id.to_string(), Slot::Running);
        let node = self.run.flow.node(id).cloned()?;
        match node.kind {
            NodeKind::Start => self.fire(id, Port::Out).await,
            NodeKind::Tool { .. } => {
                let attempt = self.attempts.get(id).copied().unwrap_or(1);
                self.start_tool(&node, attempt).await;
            }
            NodeKind::Branch { rules } => {
                let vars = &self.run.variables;
                let (port, checks) = decide(&rules, |p| resolve_json_path(vars, p));
                self.emit(RunEventKind::Branched {
                    node: id.to_string(),
                    port,
                    checks,
                })
                .await;
                self.fire(id, port).await;
            }
            NodeKind::Wait { ms } => {
                self.started(id, Value::Null, None, 1).await;
                let node = id.to_string();
                self.tasks.spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    (node, Outcome::Waited)
                });
            }
            NodeKind::Run { collection, inputs } => {
                return self.start_child(id, collection, inputs).await;
            }
            NodeKind::Fail { message } => {
                let error = if message.is_empty() {
                    format!("node {id} failed the run")
                } else {
                    message
                };
                self.emit(RunEventKind::Error { error }).await;
                self.snapshot(Some(id.to_string())).await;
                return Some(RunStatus::Failed);
            }
        }
        None
    }

    async fn started(&mut self, node: &str, params: Value, pane: Option<String>, attempt: u32) {
        self.emit(RunEventKind::NodeStarted {
            node: node.to_string(),
            unresolved: unresolved_placeholders(&params),
            params: crate::security::redact(&params),
            pane,
            attempt,
        })
        .await;
    }

    /// Attempt `attempt` of tool node `node`: fill its params from the
    /// scope as it is now and call the tool in a task.
    async fn start_tool(&mut self, node: &Node, attempt: u32) {
        let NodeKind::Tool {
            tool,
            params,
            timeout_ms,
            ..
        } = &node.kind
        else {
            return;
        };
        self.attempts.insert(node.id.clone(), attempt);
        let mut params = params.clone();
        resolve_variables(&mut params, &self.run.variables);
        let pane = params
            .get("pane_id")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty() && !p.contains("{{"))
            .map(str::to_string);
        if pane.is_some() {
            self.last_pane.clone_from(&pane);
        }
        self.started(&node.id, params.clone(), pane, attempt).await;
        let server = self.server.clone();
        let (id, tool, timeout) = (node.id.clone(), tool.clone(), *timeout_ms);
        let span = tracing::info_span!("node", node = %id, tool = %tool, attempt);
        let call = async move {
            let call = dispatch_tool(&server, &tool, params, crate::security::Origin::Run);
            let result = match timeout {
                Some(ms) => tokio::time::timeout(Duration::from_millis(ms), call)
                    .await
                    .unwrap_or_else(|_| {
                        Err((
                            StatusCode::GATEWAY_TIMEOUT,
                            format!("timed out after {ms}ms"),
                        ))
                    }),
                None => call.await,
            };
            (id, Outcome::Tool(Box::new(result)))
        };
        self.tasks
            .spawn(STEP_OF.scope(self.run.id, call).instrument(span));
    }

    /// Run node `id`: start `collection` as a child run with `inputs`
    /// (placeholders filled from this run) and wait for it in a task.
    async fn start_child(
        &mut self,
        id: &str,
        collection: Uuid,
        inputs: BTreeMap<String, Value>,
    ) -> Option<RunStatus> {
        let variables: HashMap<String, Value> = inputs
            .into_iter()
            .map(|(k, mut v)| {
                resolve_variables(&mut v, &self.run.variables);
                (k, v)
            })
            .collect();
        self.started(id, serde_json::json!(variables), None, 1)
            .await;
        if self.ancestors().await.contains(&collection) {
            let error = format!(
                "collection {collection} is already running up this chain of run nodes; \
                 loop with an edge back instead"
            );
            return self.failed(id, error, StatusCode::CONFLICT).await;
        }
        let req = StartRunRequest {
            recipe_id: Some(collection),
            session_id: Some(self.run.session_id.clone()),
            variables,
            ..Default::default()
        };
        let origin = RunOrigin::Node {
            run_id: self.run.id,
            node: id.to_string(),
        };
        let initial = *self.ctl.borrow();
        let (child, done) = match start_child(self.server.clone(), req, initial, origin).await {
            Ok(started) => started,
            Err(e) => {
                let error = format!("cannot start collection {collection}: {e}");
                return self.failed(id, error, StatusCode::BAD_REQUEST).await;
            }
        };
        self.children.insert(id.to_string(), child.id);
        self.emit(RunEventKind::ChildStarted {
            node: id.to_string(),
            child: child.id,
            collection,
            name: child.title().to_string(),
        })
        .await;
        let node = id.to_string();
        let child_id = child.id;
        self.tasks
            .spawn(async move { (node, Outcome::Child(child_id, Box::new(done.await))) });
        None
    }

    /// The collections of this run and of every run whose run node started
    /// it, on up.
    async fn ancestors(&self) -> Vec<Uuid> {
        let mut out: Vec<Uuid> = self.run.recipe_id.into_iter().collect();
        let mut origin = self.run.origin.clone();
        while let RunOrigin::Node { run_id, .. } = origin {
            match self.server.persistence.load_run_header(&run_id).await {
                Ok(Some(parent)) => {
                    out.extend(parent.recipe_id);
                    origin = parent.origin;
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("reading parent run {run_id}: {e:#}");
                    break;
                }
            }
        }
        out
    }

    /// A node's task came back. `Some` when that ends the run.
    async fn settle(&mut self, id: String, outcome: Outcome) -> Option<RunStatus> {
        match outcome {
            Outcome::Waited => self.finished(&id, Value::Null, Port::Out).await,
            Outcome::Retry(attempt) => {
                // Queued like any node, so a pause holds the retry too.
                self.attempts.insert(id.clone(), attempt);
                self.state.insert(id.clone(), Slot::Queued);
                self.ready.push_back(id);
                None
            }
            Outcome::Tool(result) => self.tool_done(id, *result).await,
            Outcome::Child(child, joined) => self.child_done(id, child, *joined).await,
        }
    }

    /// Tool node `id`'s attempt came back.
    async fn tool_done(
        &mut self,
        id: String,
        result: Result<rmcp::model::CallToolResult, (StatusCode, String)>,
    ) -> Option<RunStatus> {
        match result {
            Err((code, error)) => {
                tracing::warn!(node = id, %code, "node failed: {error}");
                self.failed(&id, error, code).await
            }
            Ok(result) => {
                let result = serde_json::to_value(&result).unwrap_or_default();
                if herdr_mcp_api::tool_is_error(&result) {
                    // The tool ran and said it failed (bad params, herdr
                    // refusing a method): a failure like any other.
                    let text = herdr_mcp_api::tool_text(&result);
                    let tool = self
                        .run
                        .flow
                        .node(&id)
                        .and_then(Node::tool_name)
                        .unwrap_or("the tool");
                    let error = if text.is_empty() {
                        format!("{tool} reported an error")
                    } else {
                        text
                    };
                    return self
                        .failed(&id, error, StatusCode::UNPROCESSABLE_ENTITY)
                        .await;
                }
                // An agent a node started is the pane to look at next.
                if let Some(p) = tool_payload(&result).get("pane_id").and_then(Value::as_str) {
                    self.last_pane = Some(p.to_string());
                }
                let extract = match self.run.flow.node(&id).map(|n| &n.kind) {
                    Some(NodeKind::Tool { extract, .. }) => extract.clone(),
                    _ => Vec::new(),
                };
                if let Err(error) = self.extract(&id, &extract, &result).await {
                    return self
                        .failed(&id, error, StatusCode::UNPROCESSABLE_ENTITY)
                        .await;
                }
                self.finished(&id, result, Port::Out).await
            }
        }
    }

    /// Run node `id`'s child run `child` ended: its outcome is the node's
    /// result.
    async fn child_done(
        &mut self,
        id: String,
        child: Uuid,
        joined: Result<RunRecord, tokio::task::JoinError>,
    ) -> Option<RunStatus> {
        self.children.remove(&id);
        let record = match joined {
            Ok(record) => record,
            Err(e) => {
                let error = format!("child run {child} died: {e}");
                return self
                    .failed(&id, error, StatusCode::INTERNAL_SERVER_ERROR)
                    .await;
            }
        };
        let events = self
            .server
            .persistence
            .load_run_events(&child, 0)
            .await
            .unwrap_or_default();
        let variables: serde_json::Map<String, Value> = events
            .into_iter()
            .filter_map(|e| match e.kind {
                RunEventKind::VariableSet { var, value, .. } => Some((var, value)),
                _ => None,
            })
            .collect();
        let result = serde_json::json!({
            "run_id": record.id,
            "status": record.status,
            "error": record.error,
            "variables": variables,
        });
        if record.status == RunStatus::Completed {
            return self.finished(&id, result, Port::Out).await;
        }
        let why = record.error.as_deref().unwrap_or("no error recorded");
        let error = format!(
            "{} (run {}) {}: {why}",
            record.title(),
            record.id,
            record.status.as_str()
        );
        self.failed_with(&id, error, StatusCode::BAD_GATEWAY, result)
            .await
    }

    async fn finished(&mut self, id: &str, result: Value, port: Port) -> Option<RunStatus> {
        self.emit(RunEventKind::NodeFinished {
            node: id.to_string(),
            result,
            port,
        })
        .await;
        self.fire(id, port).await;
        None
    }

    /// Attempt failed: retry when the node has attempts left, else take its
    /// `error` port when it has an edge there, else fail the run.
    async fn failed(&mut self, id: &str, error: String, code: StatusCode) -> Option<RunStatus> {
        let result = serde_json::json!({ "error": error, "statusCode": code.as_u16() });
        self.failed_with(id, error, code, result).await
    }

    async fn failed_with(
        &mut self,
        id: &str,
        error: String,
        code: StatusCode,
        result: Value,
    ) -> Option<RunStatus> {
        let retry = match self.run.flow.node(id).map(|n| &n.kind) {
            Some(NodeKind::Tool { retry, .. }) => *retry,
            _ => herdr_mcp_api::Retry::default(),
        };
        let attempt = self.attempts.get(id).copied().unwrap_or(1);
        let again = attempt <= retry.times;
        self.emit(RunEventKind::NodeFailed {
            node: id.to_string(),
            error,
            status_code: code.as_u16(),
            retry_in_ms: again.then_some(retry.delay_ms),
        })
        .await;
        if again {
            let node = id.to_string();
            self.tasks.spawn(async move {
                tokio::time::sleep(Duration::from_millis(retry.delay_ms)).await;
                (node, Outcome::Retry(attempt + 1))
            });
            return None;
        }
        let handled = self
            .run
            .flow
            .edges
            .iter()
            .any(|e| e.from == id && e.port == Port::Error);
        if handled {
            return self.finished(id, result, Port::Error).await;
        }
        self.snapshot(Some(id.to_string())).await;
        Some(RunStatus::Failed)
    }

    /// Save each of node `id`'s [`Extract`]s from its `result`, in order.
    /// `Err` says which one failed and why.
    async fn extract(
        &mut self,
        id: &str,
        extract: &[Extract],
        result: &Value,
    ) -> Result<(), String> {
        for x in extract {
            let value = extract_value(result, &x.path)
                .ok_or_else(|| format!("extract {}: no `{}` in the result", x.var, x.path))?;
            self.save(x, &value)
                .await
                .map_err(|e| format!("extract {}: {e:#}", x.var))?;
            self.emit(RunEventKind::VariableSet {
                node: id.to_string(),
                var: x.var.clone(),
                scope: x.scope,
                value,
            })
            .await;
        }
        Ok(())
    }

    async fn save(&self, x: &Extract, value: &Value) -> anyhow::Result<()> {
        let store = &self.server.persistence;
        match x.scope {
            VarScope::Global => store.set_global(&x.var, value.clone()).await,
            VarScope::Collection => {
                let id = self
                    .run
                    .recipe_id
                    .context("a collection variable needs a saved recipe")?;
                let mut recipe = store
                    .load_recipe(&id)
                    .await?
                    .context("the recipe was deleted")?;
                recipe.variables.insert(x.var.clone(), value.clone());
                recipe.updated_at = Utc::now();
                store.save_recipe(&recipe).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentRegistry, Persistence};
    use herdr_mcp_api::RecipeStep;
    use std::time::Duration;

    /// Tests start runs as the API would.
    async fn start_run(server: &HerdrMcpServer, req: StartRunRequest) -> Started {
        super::start_run(server, req, RunOrigin::Api).await
    }

    async fn start_with(
        server: &HerdrMcpServer,
        req: StartRunRequest,
        initial: Control,
    ) -> Started {
        super::start_with(server, req, initial, RunOrigin::Api).await
    }

    fn test_server(dir: &std::path::Path) -> HerdrMcpServer {
        let data = dir.join("data");
        let registry = AgentRegistry::new();
        HerdrMcpServer::new(Persistence::new(data), registry)
    }

    fn step(id: &str, tool: &str) -> RecipeStep {
        RecipeStep {
            id: id.into(),
            tool: tool.into(),
            params: serde_json::json!({}),
            description: None,
            extract: Vec::new(),
        }
    }

    fn adhoc(steps: Vec<RecipeStep>) -> StartRunRequest {
        StartRunRequest {
            steps: Some(steps),
            ..Default::default()
        }
    }

    /// Poll the stored record until `pred` holds (runs advance in a task).
    async fn wait_record(
        server: &HerdrMcpServer,
        id: Uuid,
        pred: impl Fn(&RunRecord) -> bool,
    ) -> RunRecord {
        for _ in 0..200 {
            if let Ok(Some(r)) = load(server, id).await
                && pred(&r)
            {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("run {id} never reached the expected state");
    }

    fn var_set(id: &str, value: &str, extract: Vec<Extract>) -> RecipeStep {
        RecipeStep {
            params: serde_json::json!({ "session_id": "w1", "key": id, "value": value }),
            extract,
            ..step(id, "var_set")
        }
    }

    fn rule(path: &str, var: &str, scope: VarScope) -> Extract {
        Extract {
            path: path.into(),
            var: var.into(),
            scope,
        }
    }

    async fn save_recipe(
        server: &HerdrMcpServer,
        steps: Vec<RecipeStep>,
        vars: &[(&str, &str)],
    ) -> Uuid {
        let recipe = crate::variables::Recipe {
            id: Uuid::new_v4(),
            name: "collection".into(),
            description: None,
            flow: Flow::chain(steps),
            variables: vars
                .iter()
                .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
                .collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            is_template: false,
            category: None,
            triggers: Vec::new(),
            trail: Default::default(),
        };
        server.persistence.save_recipe(&recipe).await.unwrap();
        recipe.id
    }

    async fn run_recipe(server: &HerdrMcpServer, id: Uuid) -> RunRecord {
        let req = StartRunRequest {
            recipe_id: Some(id),
            ..Default::default()
        };
        start_run(server, req).await.unwrap().1.await.unwrap()
    }

    /// A step's start records its params as sent (secrets redacted), the
    /// placeholders nothing resolved, and the pane it targets; the run
    /// records who started it.
    #[tokio::test]
    async fn trail_records_sent_params_and_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let s = RecipeStep {
            params: serde_json::json!({
                "session_id": "w1", "key": "k", "value": "{{ nope }} {{ greeting }}",
                "api_key": "sk-secret", "pane_id": "w1:p9"
            }),
            ..step("send", "var_set")
        };
        let req = StartRunRequest {
            variables: HashMap::from([("greeting".into(), "hi".into())]),
            ..adhoc(vec![s])
        };
        let (_, handle) = super::start_run(
            &server,
            req,
            RunOrigin::Agent {
                pane_id: Some("w1:p2".into()),
            },
        )
        .await
        .unwrap();
        let done = handle.await.unwrap();
        assert_eq!(
            done.origin,
            RunOrigin::Agent {
                pane_id: Some("w1:p2".into())
            }
        );
        let events = server
            .persistence
            .load_run_events(&done.id, 0)
            .await
            .unwrap();
        let (params, unresolved, pane) = events
            .iter()
            .find_map(|e| match &e.kind {
                RunEventKind::NodeStarted {
                    params,
                    unresolved,
                    pane,
                    ..
                } => Some((params.clone(), unresolved.clone(), pane.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(params["value"], "{{ nope }} hi");
        assert_eq!(params["api_key"], "[redacted]");
        assert_eq!(unresolved, ["{{ nope }}"]);
        assert_eq!(pane.as_deref(), Some("w1:p9"));
        // The header keeps the origin across a reload.
        assert_eq!(
            load(&server, done.id).await.unwrap().unwrap().origin,
            done.origin
        );
    }

    /// Retention: a collection's `keep_runs` keeps its newest finished
    /// runs, `keep_days` drops older ones; with nothing set (the global
    /// default) every run is kept.
    #[tokio::test]
    async fn retention_keeps_runs_per_collection() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let store = &server.persistence;
        async fn count(server: &HerdrMcpServer, recipe: Option<Uuid>) -> usize {
            let runs = list(server, None).await.unwrap();
            runs.iter().filter(|r| r.recipe_id == recipe).count()
        }
        for _ in 0..3 {
            start_run(&server, adhoc(vec![step("a", "list_templates")]))
                .await
                .unwrap()
                .1
                .await
                .unwrap();
        }
        assert_eq!(count(&server, None).await, 3, "the default keeps every run");

        let id = save_recipe(&server, vec![step("a", "list_templates")], &[]).await;
        let mut recipe = store.load_recipe(&id).await.unwrap().unwrap();
        recipe.trail.keep_runs = Some(2);
        store.save_recipe(&recipe).await.unwrap();
        for _ in 0..3 {
            run_recipe(&server, id).await;
        }
        assert_eq!(count(&server, Some(id)).await, 2);
        assert_eq!(count(&server, None).await, 3, "other groups are left alone");

        // An old run of a collection that keeps 5 days goes.
        recipe.trail = RunTrail {
            keep_days: Some(5),
            ..Default::default()
        };
        store.save_recipe(&recipe).await.unwrap();
        let old = RunHeader {
            id: Uuid::new_v4(),
            recipe_id: Some(id),
            name: None,
            session_id: "w1".into(),
            flow: Flow::default(),
            origin: RunOrigin::Schedule,
            seed: HashMap::new(),
            started_at: Utc::now() - chrono::Duration::days(6),
        };
        store.save_run_header(&old).await.unwrap();
        assert_eq!(count(&server, Some(id)).await, 3);
        prune_all(&server).await;
        assert_eq!(count(&server, Some(id)).await, 2);
        assert!(load(&server, old.id).await.unwrap().is_none());
    }

    /// Extract rules save into globals and the recipe, and later steps of
    /// the same run see them; the recipe's variables beat globals.
    #[tokio::test]
    async fn extract_chains_through_globals_and_collection() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let store = &server.persistence;
        store
            .set_global("greeting", "from-global".into())
            .await
            .unwrap();
        let id = save_recipe(
            &server,
            vec![
                var_set(
                    "first",
                    "{{ greeting }}",
                    vec![
                        rule("value", "said", VarScope::Global),
                        rule("key", "step_key", VarScope::Collection),
                    ],
                ),
                var_set("second", "{{ said }}/{{ step_key }}", Vec::new()),
            ],
            &[("greeting", "hi")],
        )
        .await;
        let req = StartRunRequest {
            recipe_id: Some(id),
            ..Default::default()
        };
        let done = start_run(&server, req).await.unwrap().1.await.unwrap();
        assert_eq!(done.status, RunStatus::Completed, "{:?}", done.error);
        let second = herdr_mcp_api::tool_payload(&done.results["second"]).unwrap();
        assert_eq!(second["value"], "hi/first");

        assert_eq!(store.globals().await.unwrap()["said"], "hi");
        let recipe = store.load_recipe(&id).await.unwrap().unwrap();
        assert_eq!(recipe.variables["step_key"], "first");
        let sets = store
            .load_run_events(&done.id, 0)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| matches!(e.kind, RunEventKind::VariableSet { .. }))
            .count();
        assert_eq!(sets, 2);
    }

    /// `node_ids` sends single requests of a collection; unknown ids are
    /// refused before anything runs.
    #[tokio::test]
    async fn node_ids_run_only_those_steps() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let id = save_recipe(
            &server,
            vec![var_set("a", "1", Vec::new()), var_set("b", "2", Vec::new())],
            &[],
        )
        .await;
        let req = |ids: &[&str]| StartRunRequest {
            recipe_id: Some(id),
            node_ids: ids.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let done = start_run(&server, req(&["b"]))
            .await
            .unwrap()
            .1
            .await
            .unwrap();
        assert_eq!(done.status, RunStatus::Completed);
        assert_eq!(done.results.keys().collect::<Vec<_>>(), ["b"]);
        assert!(matches!(
            start_run(&server, req(&["b", "nope"])).await,
            Err(StartError::UnknownNode(s)) if s == "nope"
        ));
    }

    /// A step whose tool returns `isError` (here: params that don't
    /// deserialize) fails the run, and later steps don't run.
    #[tokio::test]
    async fn a_tool_error_result_fails_the_run() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let bad = RecipeStep {
            params: serde_json::json!({ "bogus": 1 }),
            ..step("bad", "var_set")
        };
        let run = adhoc(vec![bad, var_set("after", "x", Vec::new())]);
        let (_, handle) = start_run(&server, run).await.unwrap();
        let done = handle.await.unwrap();
        assert_eq!(done.status, RunStatus::Failed);
        assert_eq!(done.failed_node.as_deref(), Some("bad"));
        let error = done.error.unwrap();
        assert!(error.contains("deserialize"), "{error}");
        assert!(!done.results.contains_key("after"));
    }

    /// A rule whose path isn't in the result, or a collection variable with
    /// no saved recipe, fails its step with the reason.
    #[tokio::test]
    async fn failed_extracts_fail_the_step() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        for (rule, want) in [
            (
                rule("nope", "x", VarScope::Global),
                "no `nope` in the result",
            ),
            (
                rule("value", "x", VarScope::Collection),
                "needs a saved recipe",
            ),
        ] {
            let run = adhoc(vec![var_set("a", "1", vec![rule])]);
            let done = start_run(&server, run).await.unwrap().1.await.unwrap();
            assert_eq!(done.status, RunStatus::Failed);
            assert_eq!(done.failed_node.as_deref(), Some("a"));
            let error = done.error.unwrap_or_default();
            assert!(error.contains(want), "{error}");
        }
    }

    #[tokio::test]
    async fn completes_and_records_every_step() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let (first, handle) = start_run(
            &server,
            adhoc(vec![
                step("a", "list_templates"),
                step("b", "list_templates"),
            ]),
        )
        .await
        .unwrap();
        assert_eq!(first.status, RunStatus::Running);
        assert_eq!(first.session_id, "w1");

        let done = handle.await.unwrap();
        assert_eq!(done.status, RunStatus::Completed);
        assert_eq!(done.results.len(), 2);
        assert!(done.variables.contains_key("a"));
        assert!(done.finished_at.is_some());
        assert!(!server.runs.is_live(done.id));
        // The stored log folds back to exactly the executor's live record.
        let stored = load(&server, done.id).await.unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&stored).unwrap(),
            serde_json::to_value(&done).unwrap()
        );
        let kinds: Vec<_> = server
            .persistence
            .load_run_events(&done.id, 0)
            .await
            .unwrap()
            .into_iter()
            .map(|e| (e.seq, e.kind))
            .collect();
        assert!(matches!(
            kinds.as_slice(),
            [
                (
                    0,
                    RunEventKind::Status {
                        status: RunStatus::Running
                    }
                ),
                (1, RunEventKind::NodeFinished { .. }),
                (2, RunEventKind::NodeStarted { .. }),
                (3, RunEventKind::NodeFinished { .. }),
                (4, RunEventKind::NodeStarted { .. }),
                (5, RunEventKind::NodeFinished { .. }),
                (
                    6,
                    RunEventKind::Status {
                        status: RunStatus::Completed
                    }
                ),
            ]
        ));
    }

    #[tokio::test]
    async fn stops_at_the_failing_step() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let (_, handle) = start_run(
            &server,
            adhoc(vec![
                step("ok", "list_templates"),
                step("bad", "no_such_tool"),
                step("never", "list_templates"),
            ]),
        )
        .await
        .unwrap();
        let done = handle.await.unwrap();
        assert_eq!(done.status, RunStatus::Failed);
        assert_eq!(done.failed_node.as_deref(), Some("bad"));
        assert!(done.results.contains_key("ok") && !done.results.contains_key("never"));
    }

    #[tokio::test]
    async fn pause_resume_then_cancel() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());

        // Starts paused before its first step.
        let req = adhoc(vec![step("a", "list_templates")]);
        let (run, handle) = start_with(&server, req, Control::Pause).await.unwrap();
        let paused = wait_record(&server, run.id, |r| r.status == RunStatus::Paused).await;
        assert!(paused.active.is_empty(), "paused before the first node");
        assert!(paused.results.is_empty());

        assert!(server.runs.control(run.id, RunAction::Resume));
        assert_eq!(handle.await.unwrap().status, RunStatus::Completed);
        assert!(
            !server.runs.control(run.id, RunAction::Cancel),
            "finished runs are not live"
        );

        // Cancelled while paused: nothing runs, and resume can't undo it.
        let req = adhoc(vec![step("a", "list_templates")]);
        let (run, handle) = start_with(&server, req, Control::Pause).await.unwrap();
        wait_record(&server, run.id, |r| r.status == RunStatus::Paused).await;
        assert!(server.runs.control(run.id, RunAction::Cancel));
        server.runs.control(run.id, RunAction::Resume);
        let done = handle.await.unwrap();
        assert_eq!(done.status, RunStatus::Cancelled);
        assert!(done.results.is_empty());
    }

    /// Call a tool the way a recipe step or MCP client does; its JSON payload.
    async fn call(server: &HerdrMcpServer, tool: &str, args: Value) -> Result<Value, String> {
        crate::server::dispatch_tool(server, tool, args, crate::security::Origin::Api)
            .await
            .map(|r| tool_payload(&serde_json::to_value(r).unwrap()))
            .map_err(|(_, e)| e)
    }

    #[tokio::test]
    async fn recipe_tools_save_run_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let server = test_server(dir.path());
        let steps = serde_json::to_value(vec![var_set("a", "1", Vec::new())]).unwrap();
        let saved = call(
            &server,
            "recipe_save",
            serde_json::json!({ "name": "r", "steps": steps }),
        )
        .await
        .unwrap();
        let id = saved["id"].as_str().unwrap().to_string();

        let renamed = call(
            &server,
            "recipe_save",
            serde_json::json!({ "recipe_id": id, "name": "r2" }),
        )
        .await
        .unwrap();
        assert_eq!(renamed["name"], "r2");
        assert_eq!(renamed["flow"]["nodes"][1]["id"], "a");
        let list = call(&server, "recipe_list", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(
            list["recipes"][0]["nodes"],
            serde_json::json!(["start", "a"])
        );

        let run = call(
            &server,
            "recipe_run",
            serde_json::json!({ "recipe_id": id, "wait": true }),
        )
        .await
        .unwrap();
        assert_eq!(run["status"], "completed");
        let got = call(
            &server,
            "run_get",
            serde_json::json!({ "run_id": run["id"] }),
        )
        .await
        .unwrap();
        assert_eq!(got["status"], "completed");

        let err = call(
            &server,
            "recipe_save",
            serde_json::json!({ "name": "no steps" }),
        )
        .await
        .unwrap_err();
        assert!(err.contains("name and flow"), "{err}");
        let err = call(
            &server,
            "run_control",
            serde_json::json!({ "run_id": run["id"], "action": "cancel" }),
        )
        .await
        .unwrap_err();
        assert!(err.contains("not executing"), "{err}");
    }

    #[tokio::test]
    async fn saved_recipe_runs_and_missing_recipe_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let now = chrono::Utc::now();
        let recipe = herdr_mcp_api::Recipe {
            id: Uuid::new_v4(),
            name: "two".into(),
            description: None,
            flow: Flow::chain(vec![step("a", "list_templates")]),
            variables: HashMap::from([("workspace_id".into(), serde_json::json!("w7"))]),
            created_at: now,
            updated_at: now,
            is_template: false,
            category: None,
            triggers: Vec::new(),
            trail: Default::default(),
        };
        server.persistence.save_recipe(&recipe).await.unwrap();

        let req = StartRunRequest {
            recipe_id: Some(recipe.id),
            ..Default::default()
        };
        let (run, handle) = start_run(&server, req).await.unwrap();
        assert_eq!(run.recipe_id, Some(recipe.id));
        assert_eq!(run.name.as_deref(), Some("two"));
        assert_eq!(run.session_id, "w7", "workspace hint from recipe variables");
        assert_eq!(handle.await.unwrap().status, RunStatus::Completed);

        let missing = StartRunRequest {
            recipe_id: Some(Uuid::new_v4()),
            ..Default::default()
        };
        assert!(matches!(
            start_run(&server, missing).await,
            Err(StartError::RecipeNotFound)
        ));
    }

    /// The server wires its scheduler to the run engine: a schedule firing
    /// produces a recorded run of that recipe. Real time (~2s).
    #[tokio::test]
    async fn scheduled_recipe_produces_a_run() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let now = chrono::Utc::now();
        let recipe = herdr_mcp_api::Recipe {
            id: Uuid::new_v4(),
            name: "cron".into(),
            description: None,
            flow: Flow::chain(vec![step("a", "list_templates")]),
            variables: HashMap::new(),
            created_at: now,
            updated_at: now,
            is_template: false,
            category: None,
            triggers: Vec::new(),
            trail: Default::default(),
        };
        server.persistence.save_recipe(&recipe).await.unwrap();
        let schedule = crate::variables::ScheduledRecipe {
            id: Uuid::new_v4(),
            recipe_id: recipe.id,
            cron_schedule: "* * * * * *".into(),
            next_run: None,
            last_run: None,
            enabled: true,
            created_at: now,
        };
        server
            .scheduler
            .schedule_one(schedule.clone())
            .await
            .unwrap();
        server.scheduler.sync().await.unwrap();

        let mut found = None;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let runs = list(&server, None).await.unwrap();
            found = runs.into_iter().find(|r| r.recipe_id == Some(recipe.id));
            if found.is_some() {
                break;
            }
        }
        server.scheduler.remove(schedule.id).await.unwrap();
        let run = found.expect("schedule never produced a run");
        assert_eq!(run.name.as_deref(), Some("cron"));
    }

    /// Replay from `since`, then live, with no gaps or repeats; events of
    /// other runs are filtered out.
    #[tokio::test]
    async fn event_stream_replays_then_follows() {
        use futures_util::StreamExt;
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());

        let req = adhoc(vec![
            step("a", "list_templates"),
            step("b", "list_templates"),
        ]);
        let (run, handle) = start_with(&server, req, Control::Pause).await.unwrap();
        // Logged so far: running(0), start finished(1), paused(2).
        wait_record(&server, run.id, |r| r.status == RunStatus::Paused).await;
        let stream = event_stream(server.clone(), Some(run.id), Some(1));
        tokio::pin!(stream);
        assert_eq!(stream.next().await.unwrap().seq, 1, "replayed from since");

        // Noise from another run must not appear.
        let (_, other) = start_run(&server, adhoc(vec![step("x", "list_templates")]))
            .await
            .unwrap();
        other.await.unwrap();
        server.runs.control(run.id, RunAction::Resume);
        handle.await.unwrap();

        let mut seqs = vec![1];
        while let Some(event) = stream.next().await {
            assert_eq!(event.run_id, run.id);
            seqs.push(event.seq);
            if event.kind.is_terminal() {
                break;
            }
        }
        // paused(2) running(3) a-started(4) a-finished(5) b-started(6)
        // b-finished(7) completed(8)
        assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn record_without_live_run_reads_as_interrupted() {
        let mut run = RunRecord {
            id: Uuid::new_v4(),
            recipe_id: None,
            name: None,
            session_id: "w1".into(),
            flow: Flow::default(),
            origin: Default::default(),
            pass: 1,
            status: RunStatus::Running,
            active: Vec::new(),
            results: HashMap::new(),
            variables: HashMap::new(),
            failed_node: None,
            error: None,
            started_at: chrono::Utc::now(),
            finished_at: None,
        };
        assert_eq!(
            Runs::observe(run.clone(), false).status,
            RunStatus::Interrupted
        );
        assert_eq!(Runs::observe(run.clone(), true).status, RunStatus::Running);
        run.status = RunStatus::Completed;
        assert_eq!(Runs::observe(run, false).status, RunStatus::Completed);
    }

    /// An agent spawned/prompted earlier in a run is addressable by role
    /// and pane id in later steps (`{{writer.output}}`).
    #[test]
    fn agent_handles_from_steps_resolve_by_role() {
        let header = RunHeader {
            id: Uuid::new_v4(),
            recipe_id: None,
            name: None,
            session_id: "w1".into(),
            flow: Flow::default(),
            origin: Default::default(),
            seed: HashMap::new(),
            started_at: Utc::now(),
        };
        let handle = r#"{"pane_id":"w1:p9","role":"writer","output":"draft v1"}"#;
        let event = RunEvent {
            run_id: header.id,
            seq: 1,
            at: Utc::now(),
            kind: RunEventKind::NodeFinished {
                node: "spawn".into(),
                result: serde_json::json!({"content": [{"type": "text", "text": handle}]}),
                port: Port::Out,
            },
        };
        let run = fold(header, &[event]);
        let mut params =
            serde_json::json!({ "text": "review: {{writer.output}} ({{w1:p9.pane_id}})" });
        resolve_variables(&mut params, &run.variables);
        assert_eq!(params["text"], "review: draft v1 (w1:p9)");
    }

    #[test]
    fn workspace_ignores_placeholders() {
        let scope = HashMap::from([
            (
                "session_id".into(),
                serde_json::json!("{{ template.session_id }}"),
            ),
            ("template.workspace_id".into(), serde_json::json!("w3")),
        ]);
        assert_eq!(workspace_for(Some(" ".into()), &scope), "w3");
        assert_eq!(workspace_for(Some("w9".into()), &scope), "w9");
        assert_eq!(workspace_for(None, &HashMap::new()), "w1");
    }

    /// A run that a step's `recipe_run` starts names that step's run as its
    /// parent.
    #[tokio::test]
    async fn recipe_run_step_records_its_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let child = save_recipe(&server, vec![var_set("b", "y", Vec::new())], &[]).await;
        let call = RecipeStep {
            params: serde_json::json!({ "recipe_id": child.to_string(), "wait": true }),
            ..step("call", "recipe_run")
        };
        let parent = save_recipe(&server, vec![call], &[]).await;
        let req = StartRunRequest {
            recipe_id: Some(parent),
            ..Default::default()
        };
        let (first, handle) = start_run(&server, req).await.unwrap();
        let done = handle.await.unwrap();
        assert_eq!(done.status, RunStatus::Completed, "{:?}", done.error);
        let runs = list(&server, None).await.unwrap();
        let kid = runs
            .iter()
            .find(|r| r.recipe_id == Some(child))
            .expect("the step started the child");
        assert_eq!(kid.origin.parent(), Some(first.id));
    }

    // ── graph semantics ────────────────────────────────────────────────

    fn flow(value: Value) -> Flow {
        serde_json::from_value(value).unwrap()
    }

    async fn save_flow(server: &HerdrMcpServer, f: Flow) -> Uuid {
        let id = save_recipe(server, Vec::new(), &[]).await;
        let mut recipe = server.persistence.load_recipe(&id).await.unwrap().unwrap();
        recipe.flow = f;
        server.persistence.save_recipe(&recipe).await.unwrap();
        id
    }

    async fn run_flow(server: &HerdrMcpServer, f: Flow) -> (RunRecord, Vec<RunEvent>) {
        let req = StartRunRequest {
            flow: Some(f),
            ..Default::default()
        };
        let done = start_run(server, req).await.unwrap().1.await.unwrap();
        let events = server
            .persistence
            .load_run_events(&done.id, 0)
            .await
            .unwrap();
        (done, events)
    }

    fn kinds<'a>(
        events: &'a [RunEvent],
        f: impl Fn(&'a RunEventKind) -> Option<String>,
    ) -> Vec<String> {
        events.iter().filter_map(|e| f(&e.kind)).collect()
    }

    fn set(id: &str, value: &str) -> Value {
        serde_json::json!({ "id": id, "type": "tool", "tool": "var_set",
            "params": { "session_id": "w1", "key": id, "value": value } })
    }

    /// An edge back upstream loops; a branch ends the loop; `run.pass`
    /// counts the passes; the log folds back to the live record.
    #[tokio::test]
    async fn a_loop_runs_until_its_branch_lets_go() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let f = flow(serde_json::json!({
            "nodes": [
                { "id": "start", "type": "start" },
                set("count", "{{ run.pass }}"),
                { "id": "more", "type": "branch", "rules": [
                    { "checks": [{ "path": "count.value", "op": "lt", "value": 3 }] }] },
                { "id": "nap", "type": "wait", "ms": 1 }
            ],
            "edges": [
                { "from": "start", "to": "count" },
                { "from": "count", "to": "more" },
                { "from": "more", "port": "1", "to": "nap" },
                { "from": "nap", "to": "count" }
            ]
        }));
        let (done, events) = run_flow(&server, f).await;
        assert_eq!(done.status, RunStatus::Completed, "{:?}", done.error);
        assert_eq!(done.pass, 3);
        assert_eq!(done.variables["run.pass"], 3);
        let count = herdr_mcp_api::tool_payload(&done.results["count"]).unwrap();
        assert_eq!(count["value"], "3");
        let ports = kinds(&events, |k| match k {
            RunEventKind::Branched { port, .. } => Some(port.to_string()),
            _ => None,
        });
        assert_eq!(ports, ["1", "1", "else"]);
        let loops = kinds(&events, |k| match k {
            RunEventKind::Looped { from, to, pass } => Some(format!("{from}>{to}#{pass}")),
            _ => None,
        });
        assert_eq!(loops, ["nap>count#2", "nap>count#3"]);
        let trail = herdr_mcp_api::trail_text(&done, &events);
        assert!(trail.contains("more → 1"), "{trail}");
        assert!(
            trail.contains("rule 1: count.value lt 3 — found \"1\" ✔"),
            "{trail}"
        );
        assert!(trail.contains("↺ nap → count · pass 2"), "{trail}");
        let header = server
            .persistence
            .load_run_header(&done.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fold(header, &events), done);
    }

    /// Two paths from the start run at once; the node they meet at waits
    /// for both and runs once.
    #[tokio::test]
    async fn fan_out_runs_in_parallel_and_the_join_waits_for_both() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let f = flow(serde_json::json!({
            "nodes": [
                { "id": "start", "type": "start" },
                { "id": "a", "type": "wait", "ms": 150 },
                { "id": "b", "type": "wait", "ms": 150 },
                set("join", "both")
            ],
            "edges": [
                { "from": "start", "to": "a" },
                { "from": "start", "to": "b" },
                { "from": "a", "to": "join" },
                { "from": "b", "to": "join" }
            ]
        }));
        let began = std::time::Instant::now();
        let (done, events) = run_flow(&server, f).await;
        assert_eq!(done.status, RunStatus::Completed, "{:?}", done.error);
        assert!(
            began.elapsed() < Duration::from_millis(290),
            "{:?}",
            began.elapsed()
        );
        let order = kinds(&events, |k| match k {
            RunEventKind::NodeStarted { node, .. } => Some(node.clone()),
            _ => None,
        });
        assert_eq!(order.iter().filter(|n| *n == "join").count(), 1);
        assert_eq!(order.last().map(String::as_str), Some("join"));
    }

    /// The port a branch does not take is ruled out: the node behind it is
    /// skipped and a join after both paths still runs.
    #[tokio::test]
    async fn an_untaken_port_skips_its_path_and_the_join_goes_on() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let f = flow(serde_json::json!({
            "nodes": [
                { "id": "start", "type": "start" },
                { "id": "pick", "type": "branch", "rules": [{}] },
                set("yes", "1"),
                set("no", "1"),
                set("after", "1")
            ],
            "edges": [
                { "from": "start", "to": "pick" },
                { "from": "pick", "port": "1", "to": "yes" },
                { "from": "pick", "port": "else", "to": "no" },
                { "from": "yes", "to": "after" },
                { "from": "no", "to": "after" }
            ]
        }));
        let (done, events) = run_flow(&server, f).await;
        assert_eq!(done.status, RunStatus::Completed, "{:?}", done.error);
        let skipped = kinds(&events, |k| match k {
            RunEventKind::NodeSkipped { node } => Some(node.clone()),
            _ => None,
        });
        assert_eq!(skipped, ["no"]);
        assert!(done.results.contains_key("after") && !done.results.contains_key("no"));
        let states = herdr_mcp_api::run::node_states(&events);
        assert_eq!(states["no"], herdr_mcp_api::run::NodeState::Skipped);
    }

    /// A failing tool is retried, then its `error` port takes the failure
    /// and the run goes on; without that edge it fails the run.
    #[tokio::test]
    async fn retries_then_the_error_port_catches_the_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let nodes = serde_json::json!([
            { "id": "start", "type": "start" },
            { "id": "bad", "type": "tool", "tool": "no_such_tool", "retry": { "times": 2, "delay_ms": 1 } },
            set("caught", "{{ bad.error }}")
        ]);
        let f = flow(serde_json::json!({ "nodes": nodes, "edges": [
            { "from": "start", "to": "bad" },
            { "from": "bad", "port": "error", "to": "caught" }
        ]}));
        let (done, events) = run_flow(&server, f).await;
        assert_eq!(done.status, RunStatus::Completed, "{:?}", done.error);
        assert_eq!(done.failed_node, None);
        let failed = kinds(&events, |k| match k {
            RunEventKind::NodeFailed { retry_in_ms, .. } => Some(format!("{retry_in_ms:?}")),
            _ => None,
        });
        assert_eq!(failed, ["Some(1)", "Some(1)", "None"]);
        let attempts = kinds(&events, |k| match k {
            RunEventKind::NodeStarted { node, attempt, .. } if node == "bad" => {
                Some(attempt.to_string())
            }
            _ => None,
        });
        assert_eq!(attempts, ["1", "2", "3"]);
        let caught = herdr_mcp_api::tool_payload(&done.results["caught"]).unwrap();
        assert!(
            caught["value"].as_str().unwrap().contains("Unknown tool"),
            "{caught}"
        );

        let f = flow(serde_json::json!({ "nodes": nodes, "edges": [
            { "from": "start", "to": "bad" },
            { "from": "bad", "to": "caught" }
        ]}));
        let (done, _) = run_flow(&server, f).await;
        assert_eq!(done.status, RunStatus::Failed);
        assert_eq!(done.failed_node.as_deref(), Some("bad"));
    }

    /// `timeout_ms` fails an attempt that runs too long.
    #[tokio::test]
    async fn a_timeout_fails_the_attempt() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let slow = serde_json::json!({ "nodes": [
            { "id": "start", "type": "start" },
            { "id": "z", "type": "wait", "ms": 10_000 }
        ], "edges": [{ "from": "start", "to": "z" }] });
        let f = flow(serde_json::json!({
            "nodes": [
                { "id": "start", "type": "start" },
                { "id": "slow", "type": "tool", "tool": "recipe_run",
                  "params": { "flow": slow, "wait": true }, "timeout_ms": 50 }
            ],
            "edges": [{ "from": "start", "to": "slow" }]
        }));
        let (done, _) = tokio::time::timeout(Duration::from_secs(5), run_flow(&server, f))
            .await
            .expect("the timeout ends the node");
        assert_eq!(done.status, RunStatus::Failed);
        assert!(
            done.error
                .as_deref()
                .unwrap_or_default()
                .contains("timed out after 50ms"),
            "{:?}",
            done.error
        );
    }

    /// `max_steps` stops a loop that never lets go; a fail node fails the
    /// run with its message; a flow with problems does not start.
    #[tokio::test]
    async fn max_steps_fail_nodes_and_invalid_flows() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let f = flow(serde_json::json!({
            "nodes": [{ "id": "start", "type": "start" }, { "id": "spin", "type": "wait", "ms": 0 }],
            "edges": [{ "from": "start", "to": "spin" }, { "from": "spin", "to": "spin" }],
            "max_steps": 5
        }));
        let (done, _) = run_flow(&server, f).await;
        assert_eq!(done.status, RunStatus::Failed);
        assert!(
            done.error.as_deref().unwrap().contains("after 5 node runs"),
            "{:?}",
            done.error
        );
        assert_eq!(done.pass, 6, "the sixth pass is refused");

        let f = flow(serde_json::json!({
            "nodes": [{ "id": "start", "type": "start" }, { "id": "no", "type": "fail", "message": "nope" }],
            "edges": [{ "from": "start", "to": "no" }]
        }));
        let (done, _) = run_flow(&server, f).await;
        assert_eq!(done.status, RunStatus::Failed);
        assert_eq!(done.error.as_deref(), Some("nope"));

        let f = flow(serde_json::json!({ "nodes": [], "edges": [] }));
        let req = StartRunRequest {
            flow: Some(f),
            ..Default::default()
        };
        assert!(
            matches!(start_run(&server, req).await, Err(StartError::Invalid(why)) if why.contains("one start node"))
        );
    }

    /// A run node starts its collection as an isolated child (its own
    /// variables, the inputs) and waits; the result carries what the child
    /// extracted. A failing child takes the error port; a collection that
    /// runs itself is refused.
    #[tokio::test]
    async fn a_run_node_waits_for_its_isolated_child() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let child = save_recipe(
            &server,
            vec![var_set(
                "echo",
                "{{ who }}:{{ secret }}",
                vec![rule("value", "echoed", VarScope::Global)],
            )],
            &[],
        )
        .await;
        let f = flow(serde_json::json!({
            "nodes": [
                { "id": "start", "type": "start" },
                { "id": "kid", "type": "run", "collection": child, "inputs": { "who": "{{ name }}" } },
                set("after", "{{ kid.variables.echoed }}")
            ],
            "edges": [{ "from": "start", "to": "kid" }, { "from": "kid", "to": "after" }]
        }));
        let req = StartRunRequest {
            flow: Some(f),
            variables: HashMap::from([
                ("name".into(), "ann".into()),
                ("secret".into(), "parent-only".into()),
            ]),
            ..Default::default()
        };
        let done = start_run(&server, req).await.unwrap().1.await.unwrap();
        assert_eq!(done.status, RunStatus::Completed, "{:?}", done.error);
        assert_eq!(done.results["kid"]["status"], "completed");
        let after = herdr_mcp_api::tool_payload(&done.results["after"]).unwrap();
        assert_eq!(
            after["value"], "ann:{{ secret }}",
            "the child does not see the parent's scope"
        );
        let kid: Uuid = serde_json::from_value(done.results["kid"]["run_id"].clone()).unwrap();
        let kid = load(&server, kid).await.unwrap().unwrap();
        assert_eq!(
            kid.origin,
            RunOrigin::Node {
                run_id: done.id,
                node: "kid".into()
            }
        );
        let trail = trail(&server, done.id).await.unwrap().unwrap();
        assert!(trail.contains("\n    collection · run"), "{trail}");

        // The child fails; the parent's error port takes it.
        let broken = save_recipe(&server, vec![step("x", "no_such_tool")], &[]).await;
        let f = flow(serde_json::json!({
            "nodes": [
                { "id": "start", "type": "start" },
                { "id": "kid", "type": "run", "collection": broken },
                set("caught", "{{ kid.status }}")
            ],
            "edges": [{ "from": "start", "to": "kid" }, { "from": "kid", "port": "error", "to": "caught" }]
        }));
        let (done, _) = run_flow(&server, f).await;
        assert_eq!(done.status, RunStatus::Completed, "{:?}", done.error);
        let caught = herdr_mcp_api::tool_payload(&done.results["caught"]).unwrap();
        assert_eq!(caught["value"], "failed");

        // A collection whose run node starts itself.
        let me = save_recipe(&server, Vec::new(), &[]).await;
        let f = flow(serde_json::json!({
            "nodes": [{ "id": "start", "type": "start" }, { "id": "again", "type": "run", "collection": me }],
            "edges": [{ "from": "start", "to": "again" }]
        }));
        let mut recipe = server.persistence.load_recipe(&me).await.unwrap().unwrap();
        recipe.flow = f;
        server.persistence.save_recipe(&recipe).await.unwrap();
        let done = run_recipe(&server, me).await;
        assert_eq!(done.status, RunStatus::Failed);
        assert!(
            done.error
                .as_deref()
                .unwrap()
                .contains("already running up this chain"),
            "{:?}",
            done.error
        );
    }

    /// Pause and cancel reach a child run and the nodes in flight.
    #[tokio::test]
    async fn pause_and_cancel_reach_children_and_nodes_in_flight() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let slow = save_flow(
            &server,
            flow(serde_json::json!({
                "nodes": [{ "id": "start", "type": "start" }, { "id": "z", "type": "wait", "ms": 60_000 }],
                "edges": [{ "from": "start", "to": "z" }]
            })),
        )
        .await;
        let f = flow(serde_json::json!({
            "nodes": [
                { "id": "start", "type": "start" },
                { "id": "kid", "type": "run", "collection": slow },
                { "id": "nap", "type": "wait", "ms": 60_000 }
            ],
            "edges": [{ "from": "start", "to": "kid" }, { "from": "start", "to": "nap" }]
        }));
        let req = StartRunRequest {
            flow: Some(f),
            ..Default::default()
        };
        let (run, handle) = start_run(&server, req).await.unwrap();
        wait_record(&server, run.id, |r| r.active.len() == 2).await;
        let kids = list(&server, None).await.unwrap();
        let kid = kids
            .iter()
            .find(|r| r.origin.parent() == Some(run.id))
            .expect("the child run")
            .id;
        assert!(server.runs.control(run.id, RunAction::Pause));
        wait_record(&server, kid, |r| r.status == RunStatus::Paused).await;
        assert!(server.runs.control(run.id, RunAction::Cancel));
        let done = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("cancel ends the waits")
            .unwrap();
        assert_eq!(done.status, RunStatus::Cancelled);
        let kid = wait_record(&server, kid, |r| r.status.is_finished()).await;
        assert_eq!(kid.status, RunStatus::Cancelled);
        assert!(done.results["nap"]["error"] == "cancelled");
    }
}

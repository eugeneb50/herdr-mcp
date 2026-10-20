//! Background recipe runs: the one executor behind `/api/runs`, the
//! scheduler, recipe triggers and the plugin's recipe picker.
//!
//! A run is stored as an immutable [`RunHeader`] (`data/runs/{id}/run.json`:
//! what ran, with which starting variables) plus an append-only event log
//! (`events.jsonl`). The log is the run's state: [`RunRecord`] is computed by
//! folding it ([`fold`]), with the same [`apply`] the executor uses live.
//! [`Runs`] holds the control channel of each run executing in this process
//! and the broadcast that carries new events to `/api/events`; a log without
//! a terminal event and without a live run reads as `interrupted`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Context;
use chrono::{DateTime, Utc};
use herdr_mcp_api::{
    Extract, RecipeStep, RunAction, RunEvent, RunEventKind, RunRecord, RunStatus, StartRunRequest,
    VarScope,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::server::{HerdrMcpServer, dispatch_tool, navigate, resolve_variables, step_text};

/// The immutable start of a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunHeader {
    pub id: Uuid,
    pub recipe_id: Option<Uuid>,
    pub name: Option<String>,
    pub session_id: String,
    /// Snapshot of the steps as they were when the run started.
    pub steps: Vec<RecipeStep>,
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
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn is_live(&self, id: Uuid) -> bool {
        self.live().contains_key(&id)
    }

    /// New events of every run, from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.events.subscribe()
    }

    /// Signal a live run. Returns false if no run with that id is executing.
    /// Pause takes effect at the next step boundary; cancel also aborts the
    /// step in flight.
    pub fn control(&self, id: Uuid, action: RunAction) -> bool {
        let control = match action {
            RunAction::Pause => Control::Pause,
            RunAction::Resume => Control::Run,
            RunAction::Cancel => Control::Cancel,
        };
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

/// A run's record: its header with every event applied.
pub fn fold(header: RunHeader, events: &[RunEvent]) -> RunRecord {
    let mut run = RunRecord {
        id: header.id,
        recipe_id: header.recipe_id,
        name: header.name,
        session_id: header.session_id,
        steps: header.steps,
        status: RunStatus::Running,
        current_step: None,
        results: HashMap::new(),
        variables: header.seed,
        failed_step: None,
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
    match &event.kind {
        RunEventKind::Status { status } => {
            run.status = *status;
            if status.is_finished() {
                run.finished_at = Some(event.at);
            }
            if *status == RunStatus::Completed {
                run.current_step = None;
            }
        }
        RunEventKind::StepStarted { step } => run.current_step = Some(step.clone()),
        RunEventKind::StepFinished { step, result } => record_result(run, step, result),
        RunEventKind::StepFailed {
            step,
            error,
            status_code,
        } => {
            run.results.insert(
                step.clone(),
                serde_json::json!({ "error": error, "statusCode": status_code }),
            );
            run.failed_step = Some(step.clone());
            run.error = Some(error.clone());
        }
        RunEventKind::StepCancelled { step } => {
            run.results
                .insert(step.clone(), serde_json::json!({ "error": "cancelled" }));
        }
        RunEventKind::VariableSet { var, value, .. } => {
            run.variables.insert(var.clone(), value.clone());
        }
    }
}

/// Store a step's result and expose it to later steps: under the step id,
/// flattened (`result.*`), and — when it is an agent handle — under its pane
/// id and role so `{{pane_id.output}}` / `{{role.output}}` resolve.
fn record_result(run: &mut RunRecord, step: &str, json: &Value) {
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

/// Every run's record, newest first.
pub async fn list(server: &HerdrMcpServer) -> anyhow::Result<Vec<RunRecord>> {
    let mut runs = Vec::new();
    for id in server.persistence.list_run_ids().await? {
        if let Some(run) = load(server, id).await? {
            runs.push(run);
        }
    }
    runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    Ok(runs)
}

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

#[derive(Debug)]
pub enum StartError {
    RecipeNotFound,
    /// A `step_ids` entry the recipe has no step for.
    UnknownStep(String),
    Persist(anyhow::Error),
}

/// Start a run in the background. Returns its first record (already
/// persisted) and a handle resolving to the final record.
pub async fn start_run(
    server: &HerdrMcpServer,
    req: StartRunRequest,
) -> Result<(RunRecord, JoinHandle<RunRecord>), StartError> {
    start_with(server, req, Control::Run).await
}

pub(crate) async fn start_with(
    server: &HerdrMcpServer,
    req: StartRunRequest,
    initial: Control,
) -> Result<(RunRecord, JoinHandle<RunRecord>), StartError> {
    let (recipe_id, name, mut steps, collection) = match req.recipe_id {
        Some(id) => {
            let recipe = server
                .persistence
                .load_recipe(&id)
                .await
                .map_err(StartError::Persist)?
                .ok_or(StartError::RecipeNotFound)?;
            (Some(id), Some(recipe.name), recipe.steps, recipe.variables)
        }
        None => (
            None,
            req.name,
            req.steps.unwrap_or_default(),
            HashMap::new(),
        ),
    };
    if let Some(id) = req
        .step_ids
        .iter()
        .find(|id| !steps.iter().any(|s| &s.id == *id))
    {
        return Err(StartError::UnknownStep(id.clone()));
    }
    if !req.step_ids.is_empty() {
        steps.retain(|s| req.step_ids.contains(&s.id));
    }
    let session_id = run_workspace(req.session_id, &collection, &req.variables);
    let seed = scope(server, &session_id, collection, req.variables).await;
    let header = RunHeader {
        id: Uuid::new_v4(),
        recipe_id,
        name,
        session_id,
        steps,
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
    let mut exec = Executor {
        server: server.clone(),
        run: fold(header, &[]),
        next_seq: 0,
        ctl: rx,
    };
    exec.emit(RunEventKind::Status {
        status: RunStatus::Running,
    })
    .await;
    let first = exec.run.clone();
    Ok((first, tokio::spawn(exec.execute())))
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

/// A run being executed: its record so far (the fold of what it emitted).
struct Executor {
    server: HerdrMcpServer,
    run: RunRecord,
    next_seq: u64,
    ctl: watch::Receiver<Control>,
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
            tracing::warn!("run {}: failed to append event: {e}", self.run.id);
        }
        // No subscribers is fine.
        let _ = self.server.runs.events.send(event);
    }

    async fn execute(mut self) -> RunRecord {
        let status = self.execute_steps().await;
        self.emit(RunEventKind::Status { status }).await;
        self.server.runs.live().remove(&self.run.id);
        self.run
    }

    /// Run every step; returns the final status.
    async fn execute_steps(&mut self) -> RunStatus {
        let steps = self.run.steps.clone();
        for step in &steps {
            if !self.wait_unpaused().await {
                return RunStatus::Cancelled;
            }
            self.emit(RunEventKind::StepStarted {
                step: step.id.clone(),
            })
            .await;
            let mut params = step.params.clone();
            resolve_variables(&mut params, &self.run.variables);
            let outcome = tokio::select! {
                r = dispatch_tool(&self.server, &step.tool, params) => Some(r),
                () = cancelled(&mut self.ctl) => None,
            };
            let def = step;
            let step = step.id.clone();
            match outcome {
                Some(Ok(result)) => {
                    let result = serde_json::to_value(&result).unwrap_or_default();
                    self.emit(RunEventKind::StepFinished {
                        step: step.clone(),
                        result: result.clone(),
                    })
                    .await;
                    if let Err(error) = self.extract(def, &result).await {
                        self.emit(RunEventKind::StepFailed {
                            step,
                            error,
                            status_code: 422,
                        })
                        .await;
                        return RunStatus::Failed;
                    }
                }
                Some(Err((code, error))) => {
                    self.emit(RunEventKind::StepFailed {
                        step,
                        error,
                        status_code: code.as_u16(),
                    })
                    .await;
                    return RunStatus::Failed;
                }
                None => {
                    self.emit(RunEventKind::StepCancelled { step }).await;
                    return RunStatus::Cancelled;
                }
            }
        }
        RunStatus::Completed
    }

    /// Save each of `step`'s [`Extract`]s from its `result`, in order. `Err`
    /// says which one failed and why.
    async fn extract(&mut self, step: &RecipeStep, result: &Value) -> Result<(), String> {
        if !step.extract.is_empty() && herdr_mcp_api::tool_is_error(result) {
            return Err(format!(
                "nothing extracted: the tool failed: {}",
                herdr_mcp_api::tool_text(result)
            ));
        }
        for x in &step.extract {
            let value = extract_value(result, &x.path)
                .ok_or_else(|| format!("extract {}: no `{}` in the result", x.var, x.path))?;
            self.save(x, &value)
                .await
                .map_err(|e| format!("extract {}: {e:#}", x.var))?;
            self.emit(RunEventKind::VariableSet {
                step: step.id.clone(),
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

    /// Block while paused (recording pause/resume). False if cancelled.
    async fn wait_unpaused(&mut self) -> bool {
        loop {
            let control = *self.ctl.borrow_and_update();
            match control {
                Control::Run => {
                    if self.run.status == RunStatus::Paused {
                        self.emit(RunEventKind::Status {
                            status: RunStatus::Running,
                        })
                        .await;
                    }
                    return true;
                }
                Control::Cancel => return false,
                Control::Pause => {
                    if self.run.status != RunStatus::Paused {
                        self.emit(RunEventKind::Status {
                            status: RunStatus::Paused,
                        })
                        .await;
                    }
                    // Sender gone means the run was dropped from `Runs`: stop.
                    if self.ctl.changed().await.is_err() {
                        return false;
                    }
                }
            }
        }
    }
}

/// Resolves when the run is cancelled; never if its control channel closes.
async fn cancelled(ctl: &mut watch::Receiver<Control>) {
    if ctl.wait_for(|c| *c == Control::Cancel).await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentRegistry, Persistence};
    use std::time::Duration;

    fn test_server(dir: &std::path::Path) -> HerdrMcpServer {
        let data = dir.join("data");
        let registry = AgentRegistry::new(Arc::new(Persistence::new(data.clone())));
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
            steps,
            variables: vars
                .iter()
                .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
                .collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            is_template: false,
            category: None,
            triggers: Vec::new(),
        };
        server.persistence.save_recipe(&recipe).await.unwrap();
        recipe.id
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

    /// `step_ids` sends single requests of a collection; unknown ids are
    /// refused before anything runs.
    #[tokio::test]
    async fn step_ids_run_only_those_steps() {
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
            step_ids: ids.iter().map(|s| s.to_string()).collect(),
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
            Err(StartError::UnknownStep(s)) if s == "nope"
        ));
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
            assert_eq!(done.failed_step.as_deref(), Some("a"));
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
                (1, RunEventKind::StepStarted { .. }),
                (2, RunEventKind::StepFinished { .. }),
                (3, RunEventKind::StepStarted { .. }),
                (4, RunEventKind::StepFinished { .. }),
                (
                    5,
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
        assert_eq!(done.failed_step.as_deref(), Some("bad"));
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
        assert_eq!(paused.current_step, None, "paused before the first step");
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

    #[tokio::test]
    async fn saved_recipe_runs_and_missing_recipe_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());
        let now = chrono::Utc::now();
        let recipe = herdr_mcp_api::Recipe {
            id: Uuid::new_v4(),
            name: "two".into(),
            description: None,
            steps: vec![step("a", "list_templates")],
            variables: HashMap::from([("workspace_id".into(), serde_json::json!("w7"))]),
            created_at: now,
            updated_at: now,
            is_template: false,
            category: None,
            triggers: Vec::new(),
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
            steps: vec![step("a", "list_templates")],
            variables: HashMap::new(),
            created_at: now,
            updated_at: now,
            is_template: false,
            category: None,
            triggers: Vec::new(),
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
            let runs = list(&server).await.unwrap();
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
        // Logged so far: running(0), paused(1).
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
        // running(2) a-started(3) a-finished(4) b-started(5) b-finished(6) completed(7)
        assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn record_without_live_run_reads_as_interrupted() {
        let mut run = RunRecord {
            id: Uuid::new_v4(),
            recipe_id: None,
            name: None,
            session_id: "w1".into(),
            steps: vec![],
            status: RunStatus::Running,
            current_step: None,
            results: HashMap::new(),
            variables: HashMap::new(),
            failed_step: None,
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
            steps: vec![],
            seed: HashMap::new(),
            started_at: Utc::now(),
        };
        let handle = r#"{"pane_id":"w1:p9","role":"writer","output":"draft v1"}"#;
        let event = RunEvent {
            run_id: header.id,
            seq: 1,
            at: Utc::now(),
            kind: RunEventKind::StepFinished {
                step: "spawn".into(),
                result: serde_json::json!({"content": [{"type": "text", "text": handle}]}),
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
}

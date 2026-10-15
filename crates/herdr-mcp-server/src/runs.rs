//! Background recipe runs: the one executor behind `/api/runs`,
//! `POST /api/recipe` and `POST /api/recipes/{id}/run`.
//!
//! A run's state lives only in its record (`data/runs/{id}/run.json`),
//! rewritten at every step. [`Runs`] holds nothing but the control channel
//! of each run executing in this process, so pause/resume/cancel can reach
//! it; a record that claims to be running without a live channel is reported
//! as [`RunStatus::Interrupted`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use herdr_mcp_api::{RecipeStep, RunAction, RunRecord, RunStatus, StartRunRequest};
use rmcp::model::CallToolResult;
use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::server::{HerdrMcpServer, dispatch_tool, resolve_variables};
use crate::variables::{ExecutionResult, ExecutionStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Run,
    Pause,
    Cancel,
}

/// Control channels of the runs executing in this process, by run id.
#[derive(Clone, Default)]
pub struct Runs {
    live: Arc<Mutex<HashMap<Uuid, watch::Sender<Control>>>>,
}

impl Runs {
    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, watch::Sender<Control>>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn is_live(&self, id: Uuid) -> bool {
        self.live().contains_key(&id)
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

    /// Fill in [`RunStatus::Interrupted`] for a record whose run is gone.
    pub fn observe(&self, mut run: RunRecord) -> RunRecord {
        if !run.status.is_finished() && !self.is_live(run.id) {
            run.status = RunStatus::Interrupted;
        }
        run
    }
}

#[derive(Debug)]
pub enum StartError {
    RecipeNotFound,
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
    let (recipe_id, name, steps, mut scope) = match req.recipe_id {
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
    let session_id = workspace_for(req.session_id, &scope);
    seed_scope(server, &session_id, &mut scope).await;

    let run = RunRecord {
        id: Uuid::new_v4(),
        recipe_id,
        name,
        session_id,
        steps,
        status: RunStatus::Running,
        current_step: None,
        results: HashMap::new(),
        variables: scope,
        failed_step: None,
        error: None,
        started_at: chrono::Utc::now(),
        finished_at: None,
    };
    // Register before the first save so a reader never sees a "running"
    // record without a live run (which would read as interrupted).
    let (tx, rx) = watch::channel(initial);
    server.runs.live().insert(run.id, tx);
    if let Err(e) = server.persistence.save_run(&run).await {
        server.runs.live().remove(&run.id);
        return Err(StartError::Persist(e));
    }
    let handle = tokio::spawn(execute(server.clone(), run.clone(), rx));
    Ok((run, handle))
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

/// Starting interpolation scope: `{{template.session_id}}` /
/// `{{template.workspace_id}}`, the workspace's agent handles, and every
/// other live handle by pane id / role as a fallback.
async fn seed_scope(server: &HerdrMcpServer, ws: &str, scope: &mut HashMap<String, Value>) {
    for key in ["template.session_id", "template.workspace_id"] {
        scope
            .entry(key.to_string())
            .or_insert_with(|| Value::String(ws.to_string()));
    }
    scope.extend(server.registry.seed(ws).await);
    for handle in server.registry.inner_snapshot().await {
        let obj = serde_json::json!({
            "pane_id": handle.pane_id,
            "workspace_id": handle.workspace_id,
            "tab_id": handle.tab_id,
            "agent": handle.agent,
            "role": handle.role,
            "status": handle.status,
            "output": handle.output,
        });
        scope.entry(handle.pane_id.clone()).or_insert(obj.clone());
        if !handle.role.is_empty() {
            scope.entry(handle.role.clone()).or_insert(obj);
        }
    }
}

async fn execute(
    server: HerdrMcpServer,
    mut run: RunRecord,
    mut ctl: watch::Receiver<Control>,
) -> RunRecord {
    let steps = run.steps.clone();
    for step in &steps {
        run.current_step = Some(step.id.clone());
        if !wait_unpaused(&server, &mut run, &mut ctl).await {
            run.status = RunStatus::Cancelled;
            break;
        }
        let mut params = step.params.clone();
        resolve_variables(&mut params, &run.variables);
        let outcome = tokio::select! {
            r = dispatch_tool(&server, &step.tool, params) => Some(r),
            () = cancelled(&mut ctl) => None,
        };
        match outcome {
            Some(Ok(result)) => record_result(&mut run, step, &result),
            Some(Err((code, msg))) => {
                run.results.insert(
                    step.id.clone(),
                    serde_json::json!({ "error": msg, "statusCode": code.as_u16() }),
                );
                run.failed_step = Some(step.id.clone());
                run.error = Some(msg);
                run.status = RunStatus::Failed;
                break;
            }
            None => {
                run.results
                    .insert(step.id.clone(), serde_json::json!({ "error": "cancelled" }));
                run.status = RunStatus::Cancelled;
                break;
            }
        }
        save(&server, &run).await;
    }
    if run.status == RunStatus::Running {
        run.status = RunStatus::Completed;
        run.current_step = None;
    }
    run.finished_at = Some(chrono::Utc::now());
    save(&server, &run).await;
    server.runs.live().remove(&run.id);
    run
}

/// Block while paused (recording the paused state). False if cancelled.
async fn wait_unpaused(
    server: &HerdrMcpServer,
    run: &mut RunRecord,
    ctl: &mut watch::Receiver<Control>,
) -> bool {
    loop {
        let control = *ctl.borrow_and_update();
        match control {
            Control::Run => {
                if run.status == RunStatus::Paused {
                    run.status = RunStatus::Running;
                    save(server, run).await;
                }
                return true;
            }
            Control::Cancel => return false,
            Control::Pause => {
                if run.status != RunStatus::Paused {
                    run.status = RunStatus::Paused;
                    save(server, run).await;
                }
                // Sender gone means the run was dropped from `Runs`: stop.
                if ctl.changed().await.is_err() {
                    return false;
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

/// Store a step's result and expose it to later steps: under the step id,
/// flattened (`result.*`), and — when it is an agent handle — under its pane
/// id and role so `{{pane_id.output}}` / `{{role.output}}` resolve.
fn record_result(run: &mut RunRecord, step: &RecipeStep, result: &CallToolResult) {
    let json = serde_json::to_value(result).unwrap_or_default();
    run.variables
        .extend(crate::variables::extract_variables(&json));
    run.variables.insert(step.id.clone(), json.clone());
    if let Some(pane_id) = json.get("pane_id").and_then(Value::as_str) {
        run.variables.insert(pane_id.to_string(), json.clone());
        if let Some(role) = json.get("role").and_then(Value::as_str)
            && !role.is_empty()
        {
            run.variables.insert(role.to_string(), json.clone());
        }
    }
    run.results.insert(step.id.clone(), json);
}

async fn save(server: &HerdrMcpServer, run: &RunRecord) {
    if let Err(e) = server.persistence.save_run(run).await {
        tracing::warn!("run {}: failed to save record: {e}", run.id);
    }
}

/// A run in the legacy `ExecutionResult` shape (`POST /api/recipes/{id}/run`,
/// `GET /api/executions/{id}`).
pub fn to_execution(run: &RunRecord) -> ExecutionResult {
    ExecutionResult {
        id: run.id,
        recipe_id: run.recipe_id.unwrap_or_else(Uuid::nil),
        started_at: run.started_at,
        completed_at: run.finished_at,
        status: match run.status {
            RunStatus::Running | RunStatus::Paused => ExecutionStatus::Running,
            RunStatus::Completed => ExecutionStatus::Completed,
            RunStatus::Failed | RunStatus::Interrupted => ExecutionStatus::Failed,
            RunStatus::Cancelled => ExecutionStatus::Cancelled,
        },
        results: run.results.clone(),
        variables: run.variables.clone(),
        error: run.error.clone(),
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
            if let Ok(Some(r)) = server.persistence.load_run(&id).await
                && pred(&r)
            {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("run {id} never reached the expected state");
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
        let stored = server
            .persistence
            .load_run(&done.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, RunStatus::Completed);
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
        assert_eq!(to_execution(&done).status, ExecutionStatus::Failed);
    }

    #[tokio::test]
    async fn pause_resume_then_cancel() {
        let tmp = tempfile::tempdir().unwrap();
        let server = test_server(tmp.path());

        // Starts paused before its first step.
        let req = adhoc(vec![step("a", "list_templates")]);
        let (run, handle) = start_with(&server, req, Control::Pause).await.unwrap();
        let paused = wait_record(&server, run.id, |r| r.status == RunStatus::Paused).await;
        assert_eq!(paused.current_step.as_deref(), Some("a"));
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

    #[test]
    fn record_without_live_run_reads_as_interrupted() {
        let runs = Runs::default();
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
        assert_eq!(runs.observe(run.clone()).status, RunStatus::Interrupted);
        run.status = RunStatus::Completed;
        assert_eq!(runs.observe(run).status, RunStatus::Completed);
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

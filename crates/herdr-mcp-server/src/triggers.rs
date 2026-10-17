//! Recipe triggers: a herdr event matching a saved recipe's
//! [`RecipeTrigger`](herdr_mcp_api::RecipeTrigger) starts a run of it.
//!
//! Triggers live on the recipes (`data/recipes/`), read on every event, so
//! editing a recipe takes effect at the next event. A recipe never runs twice
//! at once from triggers: an event arriving while its triggered run is still
//! going is skipped (this also stops a recipe re-triggering itself, e.g. one
//! that splits a pane on `pane.created`).

use std::collections::HashMap;

use herdr_mcp_api::{HerdrEvent, Recipe, RunRecord, StartRunRequest};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::server::HerdrMcpServer;

/// Run triggered recipes for every event `events` delivers, until it closes.
pub fn spawn(server: HerdrMcpServer, mut events: broadcast::Receiver<HerdrEvent>) {
    tokio::spawn(async move {
        let mut running: HashMap<Uuid, JoinHandle<RunRecord>> = HashMap::new();
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("triggers skipped {n} herdr events (lagging)");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            };
            running.retain(|_, run| !run.is_finished());
            let recipes = match server.persistence.list_recipes(Some(false)).await {
                Ok(recipes) => recipes,
                Err(e) => {
                    tracing::warn!("triggers: cannot list recipes: {e}");
                    continue;
                }
            };
            for recipe in recipes {
                if running.contains_key(&recipe.id) {
                    continue;
                }
                let Some(req) = triggered_run(&recipe, &event) else {
                    continue;
                };
                match crate::runs::start_run(&server, req).await {
                    Ok((run, done)) => {
                        tracing::info!(
                            "{} triggered run {} of recipe {:?}",
                            event.event,
                            run.id,
                            recipe.name
                        );
                        running.insert(recipe.id, done);
                    }
                    Err(e) => tracing::warn!("triggered run of {:?}: {e:?}", recipe.name),
                }
            }
        }
    });
}

/// The run `event` starts for `recipe`, if one of its triggers matches: in
/// the event's workspace, with `herdr` = the event data plus `event`.
pub fn triggered_run(recipe: &Recipe, event: &HerdrEvent) -> Option<StartRunRequest> {
    if !recipe.triggers.iter().any(|t| t.matches(event)) {
        return None;
    }
    let mut herdr = match &event.data {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    herdr.insert("event".into(), Value::String(event.event.clone()));
    Some(StartRunRequest {
        recipe_id: Some(recipe.id),
        session_id: event.workspace_id().map(str::to_string),
        variables: HashMap::from([("herdr".to_string(), Value::Object(herdr))]),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use herdr_mcp_api::{RecipeStep, RecipeTrigger};
    use serde_json::json;

    fn recipe(triggers: Vec<RecipeTrigger>) -> Recipe {
        let now = chrono::Utc::now();
        Recipe {
            id: Uuid::new_v4(),
            name: "on-done".into(),
            description: None,
            steps: vec![RecipeStep {
                id: "read".into(),
                tool: "read_pane".into(),
                params: json!({"pane_id": "{{herdr.pane_id}}"}),
                description: None,
            }],
            variables: HashMap::new(),
            created_at: now,
            updated_at: now,
            is_template: false,
            category: None,
            triggers,
        }
    }

    fn done_event() -> HerdrEvent {
        HerdrEvent {
            event: "pane.agent_status_changed".into(),
            data: json!({"pane_id": "w2:p1", "workspace_id": "w2", "agent_status": "done"}),
        }
    }

    #[test]
    fn matching_trigger_runs_in_the_event_workspace_with_its_data() {
        let r = recipe(vec![RecipeTrigger {
            on: "pane.agent_status_changed".into(),
            when: json!({"agent_status": "done"})
                .as_object()
                .cloned()
                .unwrap(),
        }]);
        let req = triggered_run(&r, &done_event()).unwrap();
        assert_eq!(req.recipe_id, Some(r.id));
        assert_eq!(req.session_id.as_deref(), Some("w2"));
        assert_eq!(req.variables["herdr"]["pane_id"], "w2:p1");
        assert_eq!(req.variables["herdr"]["event"], "pane.agent_status_changed");
    }

    #[test]
    fn no_trigger_or_no_match_means_no_run() {
        assert!(triggered_run(&recipe(Vec::new()), &done_event()).is_none());
        let idle_only = recipe(vec![RecipeTrigger {
            on: "pane.agent_status_changed".into(),
            when: json!({"agent_status": "idle"})
                .as_object()
                .cloned()
                .unwrap(),
        }]);
        assert!(triggered_run(&idle_only, &done_event()).is_none());
    }

    /// End to end through the run engine: the step sees `{{herdr.…}}`.
    #[tokio::test]
    async fn triggered_run_resolves_herdr_variables() {
        let tmp = tempfile::tempdir().unwrap();
        let persistence = crate::Persistence::new(tmp.path().to_path_buf());
        persistence.init().await.unwrap();
        let registry = crate::AgentRegistry::new(std::sync::Arc::new(persistence.clone()));
        let server = HerdrMcpServer::new(persistence, registry);
        let mut r = recipe(vec![RecipeTrigger {
            on: "pane.agent_status_changed".into(),
            when: serde_json::Map::new(),
        }]);
        r.steps = vec![RecipeStep {
            id: "echo".into(),
            tool: "var_set".into(),
            params: json!({"key": "seen", "value": "{{herdr.pane_id}}", "session_id": "w2"}),
            description: None,
        }];
        let req = triggered_run(&r, &done_event()).unwrap();
        server.persistence.save_recipe(&r).await.unwrap();
        let (_, done) = crate::runs::start_run(&server, req).await.unwrap();
        let run = done.await.unwrap();
        assert_eq!(run.session_id, "w2");
        assert_eq!(run.variables["herdr"]["agent_status"], "done");
        let text = herdr_mcp_api::tool_text(&run.results["echo"]);
        assert!(text.contains("w2:p1"), "step saw the event's pane: {text}");
    }
}

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use uuid::Uuid;

use crate::variables::{ExecutionResult, Recipe, ScheduledRecipe};

pub use herdr_mcp_api::{PlaygroundSnapshot, RunRecord, VariableStore};

/// A persistent bag of variables shared across recipe runs within a session.
/// This is what enables chaining: the output variables of one recipe run are
/// written here and seeded into the next run that uses the same session id.
///
/// The `session_id` is the herdr workspace id (e.g. `"w1"`), so variables track
/// recipes across the panes/tabs of a single herdr workspace.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionVars {
    pub session_id: String,
    pub variables: std::collections::HashMap<String, serde_json::Value>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone)]
pub struct Persistence {
    data_dir: PathBuf,
}

impl Persistence {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    /// The configured data directory (used by the trim layer for PFC1 memory).
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub async fn init(&self) -> anyhow::Result<()> {
        fs::create_dir_all(self.data_dir.join("recipes")).await?;
        fs::create_dir_all(self.data_dir.join("executions")).await?;
        fs::create_dir_all(self.data_dir.join("variables")).await?;
        fs::create_dir_all(self.data_dir.join("schedules")).await?;
        fs::create_dir_all(self.data_dir.join("sessions")).await?;
        Ok(())
    }

    pub async fn save_recipe(&self, recipe: &Recipe) -> anyhow::Result<()> {
        let path = self
            .data_dir
            .join("recipes")
            .join(format!("{}.json", recipe.id));
        fs::create_dir_all(self.data_dir.join("recipes")).await?;
        let json = serde_json::to_string_pretty(recipe)?;
        fs::write(path, json).await?;
        Ok(())
    }

    pub async fn load_recipe(&self, id: &Uuid) -> anyhow::Result<Option<Recipe>> {
        let path = self.data_dir.join("recipes").join(format!("{}.json", id));
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(None);
        }
        let content = fs::read_to_string(path).await?;
        if content.trim().is_empty() {
            return Ok(None);
        }
        let recipe: Recipe = serde_json::from_str(&content)?;
        Ok(Some(recipe))
    }

    pub async fn list_recipes(&self, is_template: Option<bool>) -> anyhow::Result<Vec<Recipe>> {
        let recipes_dir = self.data_dir.join("recipes");
        let mut recipes = Vec::new();

        if !tokio::fs::try_exists(&recipes_dir).await.unwrap_or(false) {
            return Ok(recipes);
        }
        let mut entries = fs::read_dir(recipes_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                let content = fs::read_to_string(entry.path()).await?;
                if content.trim().is_empty() {
                    continue;
                }
                let Ok(recipe) = serde_json::from_str::<Recipe>(&content) else {
                    continue;
                };
                if let Some(template_only) = is_template
                    && recipe.is_template != template_only
                {
                    continue;
                }
                recipes.push(recipe);
            }
        }
        Ok(recipes)
    }

    pub async fn delete_recipe(&self, id: &Uuid) -> anyhow::Result<bool> {
        let path = self.data_dir.join("recipes").join(format!("{}.json", id));
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            fs::remove_file(path).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn save_variable(&self, var: &VariableStore) -> anyhow::Result<()> {
        let path = self
            .data_dir
            .join("variables")
            .join(format!("{}.json", var.key));
        fs::create_dir_all(self.data_dir.join("variables")).await?;
        let json = serde_json::to_string_pretty(var)?;
        fs::write(path, json).await?;
        Ok(())
    }

    pub async fn delete_variable(&self, key: &str) -> anyhow::Result<bool> {
        let path = self
            .data_dir
            .join("variables")
            .join(format!("{}.json", key));
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            fs::remove_file(path).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn load_variables(
        &self,
        session_id: Option<Uuid>,
        execution_id: Option<Uuid>,
    ) -> anyhow::Result<Vec<VariableStore>> {
        let vars_dir = self.data_dir.join("variables");
        let mut variables = Vec::new();
        if !tokio::fs::try_exists(&vars_dir).await.unwrap_or(false) {
            return Ok(variables);
        }
        let mut entries = fs::read_dir(vars_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                let content = fs::read_to_string(entry.path()).await?;
                if content.trim().is_empty() {
                    continue;
                }
                let Ok(var) = serde_json::from_str::<VariableStore>(&content) else {
                    continue;
                };
                if session_id.is_none_or(|sid| var.session_id == Some(sid))
                    && execution_id.is_none_or(|eid| var.execution_id == Some(eid))
                {
                    variables.push(var);
                }
            }
        }
        Ok(variables)
    }

    pub async fn save_session_vars(&self, bag: &SessionVars) -> anyhow::Result<()> {
        fs::create_dir_all(self.data_dir.join("sessions")).await?;
        let path = self
            .data_dir
            .join("sessions")
            .join(format!("{}.json", bag.session_id));
        fs::write(path, serde_json::to_string_pretty(bag)?).await?;
        Ok(())
    }

    pub async fn load_session_vars(&self, session_id: &str) -> anyhow::Result<SessionVars> {
        let path = self
            .data_dir
            .join("sessions")
            .join(format!("{}.json", session_id));
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(SessionVars {
                session_id: session_id.to_string(),
                variables: std::collections::HashMap::new(),
                updated_at: chrono::Utc::now(),
            });
        }
        let content = fs::read_to_string(path).await?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Persist an arbitrary JSON blob for a workspace under `sessions/{ws}.{name}.json`.
    /// Used to store the live agent registry (pane_id → agent handle) so it
    /// survives restarts and is shared across recipe runs in the same workspace.
    pub async fn save_session_blob(
        &self,
        ws: &str,
        name: &str,
        value: &serde_json::Value,
    ) -> anyhow::Result<()> {
        fs::create_dir_all(self.data_dir.join("sessions")).await?;
        let path = self
            .data_dir
            .join("sessions")
            .join(format!("{ws}.{name}.json"));
        fs::write(path, serde_json::to_string_pretty(value)?).await?;
        Ok(())
    }

    fn playground_path(&self, workspace_id: &str) -> PathBuf {
        let safe: String = workspace_id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let safe = if safe.is_empty() {
            "default".into()
        } else {
            safe
        };
        self.data_dir
            .join("sessions")
            .join(format!("{safe}.playground.json"))
    }

    pub async fn save_playground(&self, snapshot: &PlaygroundSnapshot) -> anyhow::Result<()> {
        fs::create_dir_all(self.data_dir.join("sessions")).await?;
        let mut snapshot = snapshot.clone();
        snapshot.updated_at = chrono::Utc::now();
        let path = self.playground_path(&snapshot.workspace_id);
        fs::write(path, serde_json::to_string_pretty(&snapshot)?).await?;
        Ok(())
    }

    pub async fn load_playground(&self, workspace_id: &str) -> anyhow::Result<PlaygroundSnapshot> {
        let ws = if workspace_id.trim().is_empty() || workspace_id.contains("{{") {
            "default".to_string()
        } else {
            workspace_id.to_string()
        };
        let path = self.playground_path(&ws);
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(PlaygroundSnapshot {
                workspace_id: ws,
                updated_at: chrono::Utc::now(),
                ..Default::default()
            });
        }
        let content = fs::read_to_string(path).await?;
        if content.trim().is_empty() {
            return Ok(PlaygroundSnapshot {
                workspace_id: ws,
                updated_at: chrono::Utc::now(),
                ..Default::default()
            });
        }
        let mut snapshot: PlaygroundSnapshot = serde_json::from_str(&content)?;
        snapshot.workspace_id = ws;
        Ok(snapshot)
    }

    /// Write `data/runs/{id}/run.json` atomically (temp file + rename): it is
    /// rewritten at every step while clients may be reading it.
    pub async fn save_run(&self, run: &RunRecord) -> anyhow::Result<()> {
        let dir = self.data_dir.join("runs").join(run.id.to_string());
        fs::create_dir_all(&dir).await?;
        let tmp = dir.join("run.json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(run)?).await?;
        fs::rename(&tmp, dir.join("run.json")).await?;
        Ok(())
    }

    pub async fn load_run(&self, id: &Uuid) -> anyhow::Result<Option<RunRecord>> {
        let path = self
            .data_dir
            .join("runs")
            .join(id.to_string())
            .join("run.json");
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(
            &fs::read_to_string(path).await?,
        )?))
    }

    /// Every readable run record, newest first.
    pub async fn list_runs(&self) -> anyhow::Result<Vec<RunRecord>> {
        let dir = self.data_dir.join("runs");
        let mut runs = Vec::new();
        if !tokio::fs::try_exists(&dir).await.unwrap_or(false) {
            return Ok(runs);
        }
        let mut entries = fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let Ok(content) = fs::read_to_string(entry.path().join("run.json")).await else {
                continue;
            };
            if let Ok(run) = serde_json::from_str::<RunRecord>(&content) {
                runs.push(run);
            }
        }
        runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        Ok(runs)
    }

    pub async fn save_execution(&self, result: &ExecutionResult) -> anyhow::Result<()> {
        let path = self
            .data_dir
            .join("executions")
            .join(format!("{}.json", result.id));
        fs::create_dir_all(self.data_dir.join("executions")).await?;
        let json = serde_json::to_string_pretty(result)?;
        fs::write(path, json).await?;
        Ok(())
    }

    pub async fn load_execution(&self, id: &Uuid) -> anyhow::Result<Option<ExecutionResult>> {
        let path = self
            .data_dir
            .join("executions")
            .join(format!("{}.json", id));
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(None);
        }
        let content = fs::read_to_string(path).await?;
        if content.trim().is_empty() {
            return Ok(None);
        }
        let result: ExecutionResult = serde_json::from_str(&content)?;
        Ok(Some(result))
    }

    pub async fn save_schedule(&self, schedule: &ScheduledRecipe) -> anyhow::Result<()> {
        let path = self
            .data_dir
            .join("schedules")
            .join(format!("{}.json", schedule.id));
        fs::create_dir_all(self.data_dir.join("schedules")).await?;
        fs::write(path, serde_json::to_string_pretty(schedule)?).await?;
        Ok(())
    }

    pub async fn load_schedule(&self, id: &Uuid) -> anyhow::Result<Option<ScheduledRecipe>> {
        let path = self.data_dir.join("schedules").join(format!("{}.json", id));
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(None);
        }
        let content = fs::read_to_string(path).await?;
        let schedule: ScheduledRecipe = serde_json::from_str(&content)?;
        Ok(Some(schedule))
    }

    pub async fn delete_schedule(&self, id: &Uuid) -> anyhow::Result<bool> {
        let path = self.data_dir.join("schedules").join(format!("{}.json", id));
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            fs::remove_file(path).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn list_schedules(&self) -> anyhow::Result<Vec<ScheduledRecipe>> {
        let schedules_dir = self.data_dir.join("schedules");
        let mut schedules = Vec::new();
        if !tokio::fs::try_exists(&schedules_dir).await.unwrap_or(false) {
            return Ok(schedules);
        }
        let mut entries = fs::read_dir(schedules_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                let content = fs::read_to_string(entry.path()).await?;
                let schedule: ScheduledRecipe = serde_json::from_str(&content)?;
                schedules.push(schedule);
            }
        }
        Ok(schedules)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variables::{ExecutionStatus, RecipeStep};
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn sample_recipe() -> Recipe {
        Recipe {
            id: Uuid::new_v4(),
            name: "test recipe".into(),
            description: Some("desc".into()),
            steps: vec![RecipeStep {
                id: "s1".into(),
                tool: "status".into(),
                params: serde_json::json!({}),
                description: None,
            }],
            variables: HashMap::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            is_template: false,
            category: None,
        }
    }

    #[tokio::test]
    async fn test_init_creates_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        p.init().await.unwrap();
        assert!(tmp.path().join("recipes").exists());
        assert!(tmp.path().join("executions").exists());
        assert!(tmp.path().join("variables").exists());
        assert!(tmp.path().join("schedules").exists());
        assert!(tmp.path().join("sessions").exists());
    }

    #[tokio::test]
    async fn test_save_load_recipe_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let r = sample_recipe();
        p.save_recipe(&r).await.unwrap();
        let loaded = p.load_recipe(&r.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, r.id);
        assert_eq!(loaded.name, "test recipe");
    }

    #[tokio::test]
    async fn test_list_recipes_returns_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let r = sample_recipe();
        p.save_recipe(&r).await.unwrap();
        p.save_recipe(&sample_recipe()).await.unwrap();
        let list = p.list_recipes(None).await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_list_recipes_template_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let mut t = sample_recipe();
        t.is_template = true;
        p.save_recipe(&t).await.unwrap();
        p.save_recipe(&sample_recipe()).await.unwrap();
        let templates = p.list_recipes(Some(true)).await.unwrap();
        assert_eq!(templates.len(), 1);
        assert!(templates[0].is_template);
    }

    #[tokio::test]
    async fn test_delete_recipe() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let r = sample_recipe();
        p.save_recipe(&r).await.unwrap();
        assert!(p.delete_recipe(&r.id).await.unwrap());
        assert!(p.load_recipe(&r.id).await.unwrap().is_none());
        assert!(!p.delete_recipe(&r.id).await.unwrap());
    }

    #[tokio::test]
    async fn test_save_load_session_vars() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let mut bag = SessionVars {
            session_id: "w1".into(),
            variables: HashMap::new(),
            updated_at: chrono::Utc::now(),
        };
        bag.variables.insert("k".into(), serde_json::json!("v"));
        p.save_session_vars(&bag).await.unwrap();
        let loaded = p.load_session_vars("w1").await.unwrap();
        assert_eq!(loaded.session_id, "w1");
        assert_eq!(loaded.variables.get("k"), Some(&serde_json::json!("v")));
    }

    #[tokio::test]
    async fn test_load_session_vars_default_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let loaded = p.load_session_vars("missing").await.unwrap();
        assert_eq!(loaded.session_id, "missing");
        assert!(loaded.variables.is_empty());
    }

    #[tokio::test]
    async fn test_save_load_execution() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let result = ExecutionResult {
            id: Uuid::new_v4(),
            recipe_id: Uuid::new_v4(),
            started_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            status: ExecutionStatus::Completed,
            results: HashMap::new(),
            variables: HashMap::new(),
            error: None,
        };
        p.save_execution(&result).await.unwrap();
        let loaded = p.load_execution(&result.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, result.id);
        assert_eq!(loaded.status, ExecutionStatus::Completed);
    }

    #[tokio::test]
    async fn test_save_load_schedule() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let sched = ScheduledRecipe {
            id: Uuid::new_v4(),
            recipe_id: Uuid::new_v4(),
            cron_schedule: "0 * * * *".into(),
            next_run: None,
            last_run: None,
            enabled: true,
            created_at: chrono::Utc::now(),
        };
        p.save_schedule(&sched).await.unwrap();
        let loaded = p.load_schedule(&sched.id).await.unwrap().unwrap();
        assert_eq!(loaded.cron_schedule, "0 * * * *");
    }

    #[tokio::test]
    async fn test_delete_schedule() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let sched = ScheduledRecipe {
            id: Uuid::new_v4(),
            recipe_id: Uuid::new_v4(),
            cron_schedule: "0 * * * *".into(),
            next_run: None,
            last_run: None,
            enabled: true,
            created_at: chrono::Utc::now(),
        };
        p.save_schedule(&sched).await.unwrap();
        assert!(p.delete_schedule(&sched.id).await.unwrap());
        assert!(p.load_schedule(&sched.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_save_session_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        p.save_session_blob("w1", "agents", &serde_json::json!({"a": 1}))
            .await
            .unwrap();
        let path = tmp.path().join("sessions").join("w1.agents.json");
        assert!(tokio::fs::try_exists(&path).await.unwrap());
    }

    #[tokio::test]
    async fn test_save_load_variable() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let var = VariableStore {
            id: Uuid::new_v4(),
            session_id: Some(Uuid::new_v4()),
            execution_id: Some(Uuid::new_v4()),
            key: "mykey".into(),
            value: serde_json::json!("hello"),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        p.save_variable(&var).await.unwrap();
        let loaded = p.load_variables(None, None).await.unwrap();
        assert!(loaded.iter().any(|v| v.key == "mykey"));
    }

    #[tokio::test]
    async fn test_recipe_id_isolation() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        assert!(p.load_recipe(&Uuid::new_v4()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_variable() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let var = VariableStore {
            id: Uuid::new_v4(),
            session_id: Some(Uuid::new_v4()),
            execution_id: Some(Uuid::new_v4()),
            key: "deleteme".into(),
            value: serde_json::json!("test"),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        p.save_variable(&var).await.unwrap();
        assert!(p.delete_variable("deleteme").await.unwrap());
        assert!(!p.delete_variable("deleteme").await.unwrap());
        assert!(p.load_variables(None, None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_load_variables_skips_empty_and_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        tokio::fs::create_dir_all(tmp.path().join("variables"))
            .await
            .unwrap();
        tokio::fs::write(tmp.path().join("variables").join("empty.json"), "")
            .await
            .unwrap();
        tokio::fs::write(
            tmp.path().join("variables").join("corrupt.json"),
            "{not json",
        )
        .await
        .unwrap();
        let loaded = p.load_variables(None, None).await.unwrap();
        assert!(loaded.is_empty());
    }

    #[tokio::test]
    async fn test_list_recipes_missing_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().join("nonexistent"));
        assert!(p.list_recipes(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_save_load_playground_state_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let mut field_values = HashMap::new();
        field_values.insert("workspace_id".into(), "w1".into());
        let snapshot = PlaygroundSnapshot {
            workspace_id: "w1".into(),
            tool_name: Some("list_panes".into()),
            field_values,
            builder_recipe_name: "my recipe".into(),
            builder_steps: vec![RecipeStep {
                id: "s1".into(),
                tool: "list_panes".into(),
                params: serde_json::json!({}),
                description: None,
            }],
            updated_at: chrono::Utc::now(),
        };
        p.save_playground(&snapshot).await.unwrap();
        let loaded = p.load_playground("w1").await.unwrap();
        assert_eq!(loaded.workspace_id, "w1");
        assert_eq!(loaded.tool_name.as_deref(), Some("list_panes"));
        assert_eq!(
            loaded.field_values.get("workspace_id"),
            Some(&"w1".to_string())
        );
        assert_eq!(loaded.builder_recipe_name, "my recipe");
        assert_eq!(loaded.builder_steps.len(), 1);
    }

    #[tokio::test]
    async fn test_load_playground_state_default_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let loaded = p.load_playground("missing").await.unwrap();
        assert_eq!(loaded.workspace_id, "missing");
        assert!(loaded.tool_name.is_none());
        assert!(loaded.field_values.is_empty());
    }

    #[tokio::test]
    async fn test_playground_state_isolated_per_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let mut a = PlaygroundSnapshot {
            workspace_id: "w1".into(),
            updated_at: chrono::Utc::now(),
            ..Default::default()
        };
        a.tool_name = Some("list_panes".into());
        p.save_playground(&a).await.unwrap();
        let b = p.load_playground("w2").await.unwrap();
        assert!(b.tool_name.is_none());
    }

    #[tokio::test]
    async fn test_list_schedules_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let schedules = p.list_schedules().await.unwrap();
        assert!(schedules.is_empty());
    }

    #[tokio::test]
    async fn test_list_schedules_after_save() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let sched = ScheduledRecipe {
            id: Uuid::new_v4(),
            recipe_id: Uuid::new_v4(),
            cron_schedule: "0 * * * *".into(),
            next_run: None,
            last_run: None,
            enabled: true,
            created_at: chrono::Utc::now(),
        };
        p.save_schedule(&sched).await.unwrap();
        let schedules = p.list_schedules().await.unwrap();
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0].cron_schedule, "0 * * * *");
    }
}

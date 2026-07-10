use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use tokio::fs;

use crate::variables::{Recipe, ExecutionResult, ScheduledRecipe};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableStore {
    pub id: Uuid,
    pub session_id: Option<Uuid>,
    pub execution_id: Option<Uuid>,
    pub key: String,
    pub value: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

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
        let path = self.data_dir.join("recipes").join(format!("{}.json", recipe.id));
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
        let recipe: Recipe = serde_json::from_str(&content)?;
        Ok(Some(recipe))
    }

    pub async fn list_recipes(&self, is_template: Option<bool>) -> anyhow::Result<Vec<Recipe>> {
        let recipes_dir = self.data_dir.join("recipes");
        let mut recipes = Vec::new();
        
        let mut entries = fs::read_dir(recipes_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                let content = fs::read_to_string(entry.path()).await?;
                let recipe: Recipe = serde_json::from_str(&content)?;
                if let Some(template_only) = is_template
                    && recipe.is_template != template_only {
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
        let path = self.data_dir.join("variables").join(format!("{}.json", var.key));
        let json = serde_json::to_string_pretty(var)?;
        fs::write(path, json).await?;
        Ok(())
    }

    pub async fn load_variables(&self, session_id: Option<Uuid>, execution_id: Option<Uuid>) -> anyhow::Result<Vec<VariableStore>> {
        let vars_dir = self.data_dir.join("variables");
        let mut variables = Vec::new();
        if !tokio::fs::try_exists(&vars_dir).await.unwrap_or(false) {
            return Ok(variables);
        }
        let mut entries = fs::read_dir(vars_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                let content = fs::read_to_string(entry.path()).await?;
                let var: VariableStore = serde_json::from_str(&content)?;
                if session_id.is_none_or(|sid| var.session_id == Some(sid))
                    && execution_id.is_none_or(|eid| var.execution_id == Some(eid)) {
                    variables.push(var);
                }
            }
        }
        Ok(variables)
    }

    pub async fn save_session_vars(&self, bag: &SessionVars) -> anyhow::Result<()> {
        fs::create_dir_all(self.data_dir.join("sessions")).await?;
        let path = self.data_dir.join("sessions").join(format!("{}.json", bag.session_id));
        fs::write(path, serde_json::to_string_pretty(bag)?).await?;
        Ok(())
    }

    pub async fn load_session_vars(&self, session_id: &str) -> anyhow::Result<SessionVars> {
        let path = self.data_dir.join("sessions").join(format!("{}.json", session_id));
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

    pub async fn save_execution(&self, result: &ExecutionResult) -> anyhow::Result<()> {
        let path = self.data_dir.join("executions").join(format!("{}.json", result.id));
        let json = serde_json::to_string_pretty(result)?;
        fs::write(path, json).await?;
        Ok(())
    }

    pub async fn load_execution(&self, id: &Uuid) -> anyhow::Result<Option<ExecutionResult>> {
        let path = self.data_dir.join("executions").join(format!("{}.json", id));
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(None);
        }
        let content = fs::read_to_string(path).await?;
        let result: ExecutionResult = serde_json::from_str(&content)?;
        Ok(Some(result))
    }

    pub async fn save_schedule(&self, schedule: &ScheduledRecipe) -> anyhow::Result<()> {
        let path = self.data_dir.join("schedules").join(format!("{}.json", schedule.id));
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
}
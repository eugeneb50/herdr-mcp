use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use uuid::Uuid;

use crate::variables::{Recipe, ScheduledRecipe};

pub use crate::runs::RunHeader;
pub use herdr_mcp_api::{RunEvent, VariableStore};

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

/// [`herdr_mcp_core::write_atomic`] off the async runtime.
pub(crate) async fn write_atomic(
    path: impl AsRef<std::path::Path>,
    data: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    let (path, data) = (path.as_ref().to_path_buf(), data.as_ref().to_vec());
    tokio::task::spawn_blocking(move || herdr_mcp_core::write_atomic(&path, &data))
        .await
        .map_err(std::io::Error::other)?
}

/// Validate a caller-supplied name (variable key, workspace id, blob name)
/// that becomes part of a file name under `data/`. It must be one plain path
/// component: non-empty, not `.`/`..`, no separators or NUL. Rejected rather
/// than rewritten so two distinct names can never map to the same file.
pub fn file_component(name: &str) -> anyhow::Result<&str> {
    let bad = name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']);
    if bad {
        anyhow::bail!("invalid name {name:?}: must be a single path component");
    }
    Ok(name)
}

#[derive(Clone)]
pub struct Persistence {
    data_dir: PathBuf,
}

impl Persistence {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    /// The configured data directory.
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// Make the data dir and its folders, readable by its owner only: it
    /// holds recipes, variables and run results, which may carry keys.
    pub async fn init(&self) -> anyhow::Result<()> {
        fs::create_dir_all(self.data_dir.join("recipes")).await?;
        fs::create_dir_all(self.data_dir.join("variables")).await?;
        fs::create_dir_all(self.data_dir.join("schedules")).await?;
        fs::create_dir_all(self.data_dir.join("sessions")).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.data_dir, std::fs::Permissions::from_mode(0o700)).await?;
        }
        Ok(())
    }

    pub async fn save_recipe(&self, recipe: &Recipe) -> anyhow::Result<()> {
        let path = self
            .data_dir
            .join("recipes")
            .join(format!("{}.json", recipe.id));
        fs::create_dir_all(self.data_dir.join("recipes")).await?;
        let json = serde_json::to_string_pretty(recipe)?;
        write_atomic(path, json).await?;
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
            .join(format!("{}.json", file_component(&var.key)?));
        fs::create_dir_all(self.data_dir.join("variables")).await?;
        let json = serde_json::to_string_pretty(var)?;
        write_atomic(path, json).await?;
        Ok(())
    }

    /// Set global variable `key`, keeping its id and creation time when it
    /// exists.
    pub async fn set_global(&self, key: &str, value: serde_json::Value) -> anyhow::Result<()> {
        let now = chrono::Utc::now();
        let existing = self
            .load_variables(None, None)
            .await?
            .into_iter()
            .find(|v| v.key == key);
        let var = match existing {
            Some(v) => VariableStore {
                value,
                updated_at: now,
                ..v
            },
            None => VariableStore {
                id: Uuid::new_v4(),
                session_id: None,
                execution_id: None,
                key: key.to_string(),
                value,
                created_at: now,
                updated_at: now,
            },
        };
        self.save_variable(&var).await
    }

    /// Every global variable's value by key.
    pub async fn globals(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, serde_json::Value>> {
        Ok(self
            .load_variables(None, None)
            .await?
            .into_iter()
            .map(|v| (v.key, v.value))
            .collect())
    }

    pub async fn delete_variable(&self, key: &str) -> anyhow::Result<bool> {
        let path = self
            .data_dir
            .join("variables")
            .join(format!("{}.json", file_component(key)?));
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
            .join(format!("{}.json", file_component(&bag.session_id)?));
        write_atomic(path, serde_json::to_string_pretty(bag)?).await?;
        Ok(())
    }

    pub async fn load_session_vars(&self, session_id: &str) -> anyhow::Result<SessionVars> {
        let path = self
            .data_dir
            .join("sessions")
            .join(format!("{}.json", file_component(session_id)?));
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
        let path = self.data_dir.join("sessions").join(format!(
            "{}.{}.json",
            file_component(ws)?,
            file_component(name)?
        ));
        write_atomic(path, serde_json::to_string_pretty(value)?).await?;
        Ok(())
    }

    fn run_dir(&self, id: &Uuid) -> PathBuf {
        self.data_dir.join("runs").join(id.to_string())
    }

    /// Write a run's header (`data/runs/{id}/run.json`). Written once, at start.
    pub async fn save_run_header(&self, header: &RunHeader) -> anyhow::Result<()> {
        let dir = self.run_dir(&header.id);
        fs::create_dir_all(&dir).await?;
        write_atomic(dir.join("run.json"), serde_json::to_string_pretty(header)?).await?;
        Ok(())
    }

    pub async fn load_run_header(&self, id: &Uuid) -> anyhow::Result<Option<RunHeader>> {
        let path = self.run_dir(id).join("run.json");
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(
            &fs::read_to_string(path).await?,
        )?))
    }

    /// Append one event to `data/runs/{id}/events.jsonl`.
    pub async fn append_run_event(&self, event: &RunEvent) -> anyhow::Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut line = serde_json::to_string(event)?;
        line.push('\n');
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.run_dir(&event.run_id).join("events.jsonl"))
            .await?;
        f.write_all(line.as_bytes()).await?;
        // tokio writes on a background thread: without the flush the event
        // may not be on disk (or may land after the next one) when we return,
        // and a reader could see a finished run without its final event.
        f.flush().await?;
        Ok(())
    }

    /// A run's events with `seq >= since`, in order. A line that doesn't
    /// parse (a write cut short by a crash) is skipped.
    pub async fn load_run_events(&self, id: &Uuid, since: u64) -> anyhow::Result<Vec<RunEvent>> {
        let path = self.run_dir(id).join("events.jsonl");
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(Vec::new());
        }
        Ok(fs::read_to_string(path)
            .await?
            .lines()
            .filter_map(|l| serde_json::from_str::<RunEvent>(l).ok())
            .filter(|e| e.seq >= since)
            .collect())
    }

    /// Ids of every run with a header on disk.
    pub async fn list_run_ids(&self) -> anyhow::Result<Vec<Uuid>> {
        let dir = self.data_dir.join("runs");
        let mut ids = Vec::new();
        if !tokio::fs::try_exists(&dir).await.unwrap_or(false) {
            return Ok(ids);
        }
        let mut entries = fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|n| Uuid::parse_str(n).ok())
                && tokio::fs::try_exists(entry.path().join("run.json"))
                    .await
                    .unwrap_or(false)
            {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    pub async fn save_schedule(&self, schedule: &ScheduledRecipe) -> anyhow::Result<()> {
        let path = self
            .data_dir
            .join("schedules")
            .join(format!("{}.json", schedule.id));
        fs::create_dir_all(self.data_dir.join("schedules")).await?;
        write_atomic(path, serde_json::to_string_pretty(schedule)?).await?;
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

    // Racing saves of one file (two runs updating the same session vars).
    // Plain `fs::write` truncates in place and leaves a torn mix here.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_atomic_writes_leave_one_whole_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.json");
        let payloads: Vec<String> = (0..16u8)
            .map(|n| {
                char::from(b'a' + n)
                    .to_string()
                    .repeat((16 - usize::from(n)) << 16)
            })
            .collect();
        let writes = payloads.iter().map(|p| {
            let (path, p) = (path.clone(), p.clone());
            tokio::spawn(async move { write_atomic(&path, p).await })
        });
        for r in futures_util::future::join_all(writes).await {
            r.unwrap().unwrap();
        }
        let got = std::fs::read_to_string(&path).unwrap();
        assert!(payloads.contains(&got), "torn file: {} bytes", got.len());
        let names: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(names.len(), 1, "temp files left behind");
    }
    use crate::variables::RecipeStep;
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
                extract: Vec::new(),
            }],
            variables: HashMap::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            is_template: false,
            category: None,
            triggers: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_init_creates_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        p.init().await.unwrap();
        assert!(tmp.path().join("recipes").exists());
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

    #[test]
    fn file_component_rejects_traversal() {
        for bad in ["", ".", "..", "../x", "a/b", "a\\b", "x\0y"] {
            assert!(file_component(bad).is_err(), "{bad:?} should be rejected");
        }
        for ok in ["greeting", "w1", "panes.snapshot", "..hidden", "a b"] {
            assert_eq!(file_component(ok).unwrap(), ok);
        }
    }

    #[tokio::test]
    async fn test_traversal_names_never_touch_outside_data_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let p = Persistence::new(data.clone());
        p.init().await.unwrap();
        let now = chrono::Utc::now();
        let var = VariableStore {
            id: Uuid::new_v4(),
            session_id: None,
            execution_id: None,
            key: "../../escaped".into(),
            value: serde_json::json!("x"),
            created_at: now,
            updated_at: now,
        };
        assert!(p.save_variable(&var).await.is_err());
        assert!(!tmp.path().join("escaped.json").exists());

        let victim = tmp.path().join("victim.json");
        std::fs::write(&victim, "{}").unwrap();
        assert!(p.delete_variable("../../victim").await.is_err());
        assert!(victim.exists());

        assert!(p.load_session_vars("../../victim").await.is_err());
        let bag = SessionVars {
            session_id: "../escaped".into(),
            ..Default::default()
        };
        assert!(p.save_session_vars(&bag).await.is_err());
        assert!(
            p.save_session_blob("../x", "agents", &serde_json::json!({}))
                .await
                .is_err()
        );
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

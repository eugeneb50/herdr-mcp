//! The store: one SQLite database, `<data-dir>/herdr-mcp.db`. Each row
//! holds one value's JSON (a recipe, variable, session's variables, run
//! header, run event or schedule); `PRAGMA user_version` is the schema
//! version. WAL mode lets the TUI's bridge and an `--mcp-only` server share
//! one data dir.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::variables::{Recipe, ScheduledRecipe};

pub use crate::runs::RunHeader;
pub use herdr_mcp_api::{RunEvent, VariableStore};

/// The database file in the data dir.
pub const DB_FILE: &str = "herdr-mcp.db";

/// What `user_version` says this code writes.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE recipes (
    id TEXT PRIMARY KEY,
    is_template INTEGER NOT NULL,
    json TEXT NOT NULL
);
CREATE TABLE variables (key TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE session_vars (session_id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE runs (id TEXT PRIMARY KEY, started_at TEXT NOT NULL, json TEXT NOT NULL);
CREATE INDEX runs_by_start ON runs (started_at);
CREATE TABLE run_events (
    run_id TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (run_id, seq)
);
CREATE TABLE schedules (id TEXT PRIMARY KEY, json TEXT NOT NULL);
";

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

#[derive(Clone)]
pub struct Persistence {
    data_dir: PathBuf,
    /// Opened on first use.
    db: Arc<Mutex<Option<Connection>>>,
}

/// Make the data dir (owner-only: it holds recipes, variables and run
/// results, which may carry keys) and open its database at the current
/// schema.
fn open(data_dir: &Path) -> anyhow::Result<Connection> {
    std::fs::create_dir_all(data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = data_dir.join(DB_FILE);
    let conn = Connection::open(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    match version {
        0 => {
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        SCHEMA_VERSION => {}
        v => anyhow::bail!(
            "{}: schema version {v}, this herdr-mcp knows {SCHEMA_VERSION}",
            path.display()
        ),
    }
    Ok(conn)
}

/// `json` as a `T`; `None` (with a warning naming `what`) when it doesn't
/// parse, so one bad row doesn't hide the rest.
fn parse<T: DeserializeOwned>(what: &str, json: &str) -> Option<T> {
    serde_json::from_str(json)
        .inspect_err(|e| tracing::warn!("skipping {what}: {e}"))
        .ok()
}

fn rows<T: DeserializeOwned>(
    conn: &Connection,
    what: &str,
    sql: &str,
    args: impl rusqlite::Params,
) -> anyhow::Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    let jsons = stmt
        .query_map(args, |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(jsons.iter().filter_map(|j| parse(what, j)).collect())
}

fn one<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> anyhow::Result<Option<T>> {
    let json: Option<String> = conn.query_row(sql, args, |r| r.get(0)).optional()?;
    Ok(match json {
        Some(j) => Some(serde_json::from_str(&j)?),
        None => None,
    })
}

impl Persistence {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            db: Arc::default(),
        }
    }

    /// The configured data directory.
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// Run `f` on the database, off the async runtime.
    async fn with<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let (db, dir) = (self.db.clone(), self.data_dir.clone());
        tokio::task::spawn_blocking(move || {
            let mut slot = crate::lock(&db);
            if slot.is_none() {
                *slot = Some(open(&dir)?);
            }
            f(slot.as_ref().expect("opened above"))
        })
        .await?
    }

    /// Open the store now (rather than on first use), so a bad data dir
    /// fails at startup.
    pub async fn init(&self) -> anyhow::Result<()> {
        self.with(|_| Ok(())).await
    }

    pub async fn save_recipe(&self, recipe: &Recipe) -> anyhow::Result<()> {
        let (id, template, json) = (
            recipe.id.to_string(),
            recipe.is_template,
            serde_json::to_string(recipe)?,
        );
        self.with(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO recipes (id, is_template, json) VALUES (?1, ?2, ?3)",
                params![id, template, json],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn load_recipe(&self, id: &Uuid) -> anyhow::Result<Option<Recipe>> {
        let id = id.to_string();
        self.with(move |c| one(c, "SELECT json FROM recipes WHERE id = ?1", [id]))
            .await
    }

    /// Every recipe, or only templates / only collections.
    pub async fn list_recipes(&self, is_template: Option<bool>) -> anyhow::Result<Vec<Recipe>> {
        self.with(move |c| match is_template {
            Some(t) => rows(
                c,
                "recipe",
                "SELECT json FROM recipes WHERE is_template = ?1",
                [t],
            ),
            None => rows(c, "recipe", "SELECT json FROM recipes", []),
        })
        .await
    }

    pub async fn delete_recipe(&self, id: &Uuid) -> anyhow::Result<bool> {
        let id = id.to_string();
        self.with(move |c| Ok(c.execute("DELETE FROM recipes WHERE id = ?1", [id])? > 0))
            .await
    }

    pub async fn save_variable(&self, var: &VariableStore) -> anyhow::Result<()> {
        if var.key.is_empty() {
            anyhow::bail!("a variable needs a key");
        }
        let (key, json) = (var.key.clone(), serde_json::to_string(var)?);
        self.with(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO variables (key, json) VALUES (?1, ?2)",
                params![key, json],
            )?;
            Ok(())
        })
        .await
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
        let key = key.to_string();
        self.with(move |c| Ok(c.execute("DELETE FROM variables WHERE key = ?1", [key])? > 0))
            .await
    }

    pub async fn load_variables(
        &self,
        session_id: Option<Uuid>,
        execution_id: Option<Uuid>,
    ) -> anyhow::Result<Vec<VariableStore>> {
        let all: Vec<VariableStore> = self
            .with(|c| rows(c, "variable", "SELECT json FROM variables", []))
            .await?;
        Ok(all
            .into_iter()
            .filter(|v| {
                session_id.is_none_or(|sid| v.session_id == Some(sid))
                    && execution_id.is_none_or(|eid| v.execution_id == Some(eid))
            })
            .collect())
    }

    pub async fn save_session_vars(&self, bag: &SessionVars) -> anyhow::Result<()> {
        let (id, json) = (bag.session_id.clone(), serde_json::to_string(bag)?);
        self.with(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO session_vars (session_id, json) VALUES (?1, ?2)",
                params![id, json],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn load_session_vars(&self, session_id: &str) -> anyhow::Result<SessionVars> {
        let id = session_id.to_string();
        let found: Option<SessionVars> = self
            .with(move |c| {
                one(
                    c,
                    "SELECT json FROM session_vars WHERE session_id = ?1",
                    [id],
                )
            })
            .await?;
        Ok(found.unwrap_or_else(|| SessionVars {
            session_id: session_id.to_string(),
            variables: std::collections::HashMap::new(),
            updated_at: chrono::Utc::now(),
        }))
    }

    /// Write a run's header. Written once, at start.
    pub async fn save_run_header(&self, header: &RunHeader) -> anyhow::Result<()> {
        let (id, at, json) = (
            header.id.to_string(),
            sortable(header.started_at),
            serde_json::to_string(header)?,
        );
        self.with(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO runs (id, started_at, json) VALUES (?1, ?2, ?3)",
                params![id, at, json],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn load_run_header(&self, id: &Uuid) -> anyhow::Result<Option<RunHeader>> {
        let id = id.to_string();
        self.with(move |c| one(c, "SELECT json FROM runs WHERE id = ?1", [id]))
            .await
    }

    /// Run headers, newest first; at most `limit`.
    pub async fn list_run_headers(&self, limit: Option<usize>) -> anyhow::Result<Vec<RunHeader>> {
        // SQLite reads a negative LIMIT as none.
        let limit = limit.map_or(-1, |n| i64::try_from(n).unwrap_or(i64::MAX));
        self.with(move |c| {
            rows(
                c,
                "run",
                "SELECT json FROM runs ORDER BY started_at DESC LIMIT ?1",
                [limit],
            )
        })
        .await
    }

    /// Append one event to its run's log.
    pub async fn append_run_event(&self, event: &RunEvent) -> anyhow::Result<()> {
        let (id, seq, json) = (
            event.run_id.to_string(),
            i64::try_from(event.seq)?,
            serde_json::to_string(event)?,
        );
        self.with(move |c| {
            c.execute(
                "INSERT INTO run_events (run_id, seq, json) VALUES (?1, ?2, ?3)",
                params![id, seq, json],
            )?;
            Ok(())
        })
        .await
    }

    /// A run's events with `seq >= since`, in order.
    pub async fn load_run_events(&self, id: &Uuid, since: u64) -> anyhow::Result<Vec<RunEvent>> {
        let (id, since) = (id.to_string(), i64::try_from(since).unwrap_or(i64::MAX));
        self.with(move |c| {
            rows(
                c,
                "run event",
                "SELECT json FROM run_events WHERE run_id = ?1 AND seq >= ?2 ORDER BY seq",
                params![id, since],
            )
        })
        .await
    }

    /// Delete a run's header and log.
    pub async fn delete_run(&self, id: &Uuid) -> anyhow::Result<()> {
        let id = id.to_string();
        self.with(move |c| {
            c.execute("DELETE FROM runs WHERE id = ?1", [id])?;
            Ok(())
        })
        .await
    }

    pub async fn save_schedule(&self, schedule: &ScheduledRecipe) -> anyhow::Result<()> {
        let (id, json) = (schedule.id.to_string(), serde_json::to_string(schedule)?);
        self.with(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO schedules (id, json) VALUES (?1, ?2)",
                params![id, json],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn load_schedule(&self, id: &Uuid) -> anyhow::Result<Option<ScheduledRecipe>> {
        let id = id.to_string();
        self.with(move |c| one(c, "SELECT json FROM schedules WHERE id = ?1", [id]))
            .await
    }

    pub async fn delete_schedule(&self, id: &Uuid) -> anyhow::Result<bool> {
        let id = id.to_string();
        self.with(move |c| Ok(c.execute("DELETE FROM schedules WHERE id = ?1", [id])? > 0))
            .await
    }

    pub async fn list_schedules(&self) -> anyhow::Result<Vec<ScheduledRecipe>> {
        self.with(|c| rows(c, "schedule", "SELECT json FROM schedules", []))
            .await
    }
}

/// `at` as text that sorts in time order (fixed width, UTC).
fn sortable(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variables::RecipeStep;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;

    // Racing saves of one file (two writers of one agent profile).
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
            gate: None,
            trail: Default::default(),
        }
    }

    fn sample_schedule() -> ScheduledRecipe {
        ScheduledRecipe {
            id: Uuid::new_v4(),
            recipe_id: Uuid::new_v4(),
            cron_schedule: "0 * * * *".into(),
            next_run: None,
            last_run: None,
            enabled: true,
            created_at: chrono::Utc::now(),
        }
    }

    fn store() -> (tempfile::TempDir, Persistence) {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().join("data"));
        (tmp, p)
    }

    #[tokio::test]
    async fn init_makes_an_owner_only_dir_with_the_current_schema() {
        let (tmp, p) = store();
        p.init().await.unwrap();
        let db = tmp.path().join("data").join(DB_FILE);
        let conn = Connection::open(&db).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(tmp.path().join("data"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        // A newer schema is refused, not overwritten.
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(conn);
        let again = Persistence::new(tmp.path().join("data"));
        let err = again.init().await.unwrap_err().to_string();
        assert!(err.contains("schema version 2"), "{err}");
    }

    #[tokio::test]
    async fn recipes_round_trip_filter_and_delete() {
        let (_tmp, p) = store();
        let r = sample_recipe();
        p.save_recipe(&r).await.unwrap();
        let mut t = sample_recipe();
        t.is_template = true;
        p.save_recipe(&t).await.unwrap();
        assert_eq!(
            p.load_recipe(&r.id).await.unwrap().unwrap().name,
            "test recipe"
        );
        assert_eq!(p.list_recipes(None).await.unwrap().len(), 2);
        let templates = p.list_recipes(Some(true)).await.unwrap();
        assert_eq!(templates.len(), 1);
        assert!(templates[0].is_template);
        assert!(p.delete_recipe(&r.id).await.unwrap());
        assert!(p.load_recipe(&r.id).await.unwrap().is_none());
        assert!(!p.delete_recipe(&r.id).await.unwrap());
    }

    #[tokio::test]
    async fn a_row_that_does_not_parse_is_skipped() {
        let (tmp, p) = store();
        p.save_recipe(&sample_recipe()).await.unwrap();
        let conn = Connection::open(tmp.path().join("data").join(DB_FILE)).unwrap();
        conn.execute(
            "INSERT INTO recipes (id, is_template, json) VALUES ('bad', 0, '{not json')",
            [],
        )
        .unwrap();
        assert_eq!(p.list_recipes(None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn session_vars_round_trip_or_start_empty() {
        let (_tmp, p) = store();
        let mut bag = SessionVars {
            session_id: "w1".into(),
            variables: HashMap::new(),
            updated_at: chrono::Utc::now(),
        };
        bag.variables.insert("k".into(), serde_json::json!("v"));
        p.save_session_vars(&bag).await.unwrap();
        let loaded = p.load_session_vars("w1").await.unwrap();
        assert_eq!(loaded.variables.get("k"), Some(&serde_json::json!("v")));
        let missing = p.load_session_vars("missing").await.unwrap();
        assert_eq!(missing.session_id, "missing");
        assert!(missing.variables.is_empty());
    }

    #[tokio::test]
    async fn variables_save_set_and_delete() {
        let (_tmp, p) = store();
        let now = chrono::Utc::now();
        // Keys are plain strings now: a slash is just a character.
        let var = VariableStore {
            id: Uuid::new_v4(),
            session_id: None,
            execution_id: None,
            key: "a/b".into(),
            value: serde_json::json!("hello"),
            created_at: now,
            updated_at: now,
        };
        p.save_variable(&var).await.unwrap();
        p.set_global("a/b", serde_json::json!("again"))
            .await
            .unwrap();
        let loaded = p.load_variables(None, None).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, var.id, "set_global keeps the id");
        assert_eq!(p.globals().await.unwrap()["a/b"], "again");
        assert!(p.delete_variable("a/b").await.unwrap());
        assert!(!p.delete_variable("a/b").await.unwrap());
        let empty = VariableStore {
            key: String::new(),
            ..var
        };
        assert!(p.save_variable(&empty).await.is_err());
    }

    #[tokio::test]
    async fn schedules_round_trip_list_and_delete() {
        let (_tmp, p) = store();
        assert!(p.list_schedules().await.unwrap().is_empty());
        let s = sample_schedule();
        p.save_schedule(&s).await.unwrap();
        assert_eq!(
            p.load_schedule(&s.id).await.unwrap().unwrap().cron_schedule,
            "0 * * * *"
        );
        assert_eq!(p.list_schedules().await.unwrap().len(), 1);
        assert!(p.delete_schedule(&s.id).await.unwrap());
        assert!(p.load_schedule(&s.id).await.unwrap().is_none());
    }
}

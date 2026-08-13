use chrono::{DateTime, Utc};
use cron::Schedule;
use dashmap::DashMap;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

use crate::persistence::Persistence;
use crate::variables::ExecutionResult;
use crate::variables::ExecutionStatus;
use crate::variables::ScheduledRecipe;

pub type ExecutorFn =
    Arc<dyn Fn(Uuid) -> tokio::sync::oneshot::Receiver<ExecutionResult> + Send + Sync>;

#[derive(Clone)]
pub struct Scheduler {
    inner: Arc<SchedulerInner>,
}

struct SchedulerInner {
    persistence: Arc<Persistence>,
    /// Receives a recipe ID, must call to execute it and return a receiver for the result.
    executor: tokio::sync::RwLock<Option<ExecutorFn>>,
    schedules: DashMap<Uuid, ActiveSchedule>,
    handles: tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Track in-flight executions to prevent concurrent runs of the same recipe.
    in_flight: DashMap<Uuid, ()>,
}

#[derive(Clone)]
struct ActiveSchedule {
    recipe_id: Uuid,
    cron: cron::Schedule,
    next_run: DateTime<Utc>,
    enabled: bool,
}

impl Scheduler {
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            inner: Arc::new(SchedulerInner {
                persistence,
                executor: tokio::sync::RwLock::new(None),
                schedules: DashMap::new(),
                handles: tokio::sync::Mutex::new(Vec::new()),
                in_flight: DashMap::new(),
            }),
        }
    }

    /// Load persisted schedules from disk and re-spawn their runners.
    /// This ensures schedules survive server restarts.
    pub async fn load_existing(&self) -> anyhow::Result<()> {
        let schedules = self.inner.persistence.list_schedules().await?;
        for schedule in schedules {
            if schedule.enabled {
                let cron = Schedule::from_str(&schedule.cron_schedule)
                    .map_err(|e| anyhow::anyhow!("invalid cron in persisted schedule: {e}"))?;
                let next_run = cron
                    .upcoming(Utc)
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("cron never fires"))?;
                let active = ActiveSchedule {
                    recipe_id: schedule.recipe_id,
                    cron,
                    next_run,
                    enabled: schedule.enabled,
                };
                self.inner
                    .schedules
                    .insert(schedule.id, active.clone());
                let handle = self.spawn_runner(schedule.id, active);
                let mut handles = self.inner.handles.lock().await;
                handles.retain(|h| !h.is_finished());
                handles.push(handle);
            }
        }
        Ok(())
    }

    pub async fn schedule_one(&self, s: ScheduledRecipe) -> anyhow::Result<()> {
        let cron = Schedule::from_str(&s.cron_schedule)
            .map_err(|e| anyhow::anyhow!("invalid cron: {e}"))?;
        let next_run = cron
            .upcoming(Utc)
            .next()
            .ok_or_else(|| anyhow::anyhow!("cron never fires"))?;
        let active = ActiveSchedule {
            recipe_id: s.recipe_id,
            cron,
            next_run,
            enabled: s.enabled,
        };
        self.inner.schedules.insert(s.id, active.clone());
        self.inner.persistence.save_schedule(&s).await?;
        let handle = self.spawn_runner(s.id, active);
        let mut handles = self.inner.handles.lock().await;
        handles.retain(|h| !h.is_finished());
        handles.push(handle);
        Ok(())
    }

    fn spawn_runner(&self, id: Uuid, mut active: ActiveSchedule) -> tokio::task::JoinHandle<()> {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            loop {
                if !active.enabled {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    continue;
                }
                let now = Utc::now();
                if active.next_run > now {
                    let dur = (active.next_run - now)
                        .to_std()
                        .unwrap_or(std::time::Duration::from_secs(60));
                    tokio::time::sleep(dur).await;
                }

                // Check if this recipe is already running — skip to next scheduled time
                if inner.in_flight.contains_key(&active.recipe_id) {
                    tracing::debug!(
                        recipe_id = %active.recipe_id,
                        "scheduled recipe already running, skipping this tick"
                    );
                    // Compute next run and continue without executing
                    active.next_run = match active.cron.upcoming(Utc).next() {
                        Some(t) => t,
                        None => break,
                    };
                    inner.schedules.insert(id, active.clone());
                    continue;
                }

                // Trigger the executor if available
                let exec = inner.executor.read().await.clone();
                if let Some(exec_fn) = exec {
                    // Mark as in-flight before executing
                    inner.in_flight.insert(active.recipe_id, ());

                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let exec_fn = Arc::clone(&exec_fn);
                    let recipe_id = active.recipe_id;
                    let persistence = Arc::clone(&inner.persistence);
                    let sched_id = id;
                    tokio::spawn(async move {
                        let exec_rx = exec_fn(recipe_id);
                        let result = exec_rx.await.unwrap_or_else(|_| ExecutionResult {
                            id: Uuid::new_v4(),
                            recipe_id,
                            started_at: Utc::now(),
                            completed_at: Some(Utc::now()),
                            status: ExecutionStatus::Failed,
                            results: Default::default(),
                            variables: Default::default(),
                            error: Some("executor dropped channel".into()),
                        });
                        let _ = persistence.save_execution(&result).await;
                        // update schedule last_run
                        if let Ok(Some(mut sched)) = persistence.load_schedule(&sched_id).await {
                            sched.last_run = Some(Utc::now());
                            let _ = persistence.save_schedule(&sched).await;
                        }
                        let _ = tx.send(());
                    });
                    let _ = rx.await;

                    // Remove from in-flight after execution completes
                    inner.in_flight.remove(&recipe_id);
                } else {
                    tracing::warn!("no executor available for scheduled recipe");
                }

                // compute next run
                active.next_run = match active.cron.upcoming(Utc).next() {
                    Some(t) => t,
                    None => break,
                };
                inner.schedules.insert(id, active.clone());
            }
        })
    }

    pub async fn remove(&self, id: Uuid) -> anyhow::Result<bool> {
        let removed = self.inner.persistence.delete_schedule(&id).await?;
        self.inner.schedules.remove(&id);
        let mut handles = self.inner.handles.lock().await;
        handles.retain(|h| !h.is_finished());
        Ok(removed)
    }

    /// Update the enabled flag of an existing schedule. Persists the change so
    /// it survives restarts, and updates the live runner via the in-memory map.
    pub async fn set_enabled(&self, id: Uuid, enabled: bool) -> anyhow::Result<bool> {
        if let Some(mut sched) = self.inner.persistence.load_schedule(&id).await? {
            sched.enabled = enabled;
            self.inner.persistence.save_schedule(&sched).await?;
            if let Some(mut active) = self.inner.schedules.get_mut(&id) {
                active.enabled = enabled;
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn list(&self) -> Vec<ScheduledRecipe> {
        self.inner
            .schedules
            .iter()
            .map(|entry| {
                let id = *entry.key();
                let active = entry.value().clone();
                ScheduledRecipe {
                    id,
                    recipe_id: active.recipe_id,
                    cron_schedule: active.cron.to_string(),
                    next_run: Some(active.next_run),
                    last_run: None,
                    enabled: active.enabled,
                    created_at: Utc::now(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn scheduled() -> ScheduledRecipe {
        ScheduledRecipe {
            id: Uuid::new_v4(),
            recipe_id: Uuid::new_v4(),
            cron_schedule: "0 0 0 1 1 *".into(), // far future (2027-01-01), won't fire during test
            next_run: None,
            last_run: None,
            enabled: true,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_list_empty_initially() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let sched = Scheduler::new(std::sync::Arc::new(p));
        assert!(sched.list().await.is_empty());
    }

    #[tokio::test]
    async fn test_schedule_one_and_list() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let sched = Scheduler::new(std::sync::Arc::new(p));
        let s = scheduled();
        sched.schedule_one(s.clone()).await.unwrap();
        let list = sched.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, s.id);
    }

    #[tokio::test]
    async fn test_schedule_one_invalid_cron_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let sched = Scheduler::new(std::sync::Arc::new(p));
        let mut s = scheduled();
        s.cron_schedule = "not a cron".into();
        assert!(sched.schedule_one(s).await.is_err());
    }

    #[tokio::test]
    async fn test_remove_schedule() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let sched = Scheduler::new(std::sync::Arc::new(p));
        let s = scheduled();
        sched.schedule_one(s.clone()).await.unwrap();
        assert!(sched.remove(s.id).await.unwrap());
        assert!(sched.list().await.is_empty());
    }

    #[tokio::test]
    async fn test_set_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let sched = Scheduler::new(std::sync::Arc::new(p));
        let s = scheduled();
        sched.schedule_one(s.clone()).await.unwrap();
        assert!(sched.set_enabled(s.id, false).await.unwrap());
        let loaded = sched
            .inner
            .persistence
            .load_schedule(&s.id)
            .await
            .unwrap()
            .unwrap();
        assert!(!loaded.enabled);
    }

    #[tokio::test]
    async fn test_set_enabled_missing_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let sched = Scheduler::new(std::sync::Arc::new(p));
        assert!(!sched.set_enabled(Uuid::new_v4(), false).await.unwrap());
    }

    #[tokio::test]
    async fn test_load_existing_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let sched = Scheduler::new(std::sync::Arc::new(p));
        // Should not error when there are no schedules
        sched.load_existing().await.unwrap();
        assert!(sched.list().await.is_empty());
    }

    #[tokio::test]
    async fn test_load_existing_loads_persisted() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        // First save a schedule
        let sched1 = Scheduler::new(std::sync::Arc::new(p.clone()));
        let s = scheduled();
        sched1.schedule_one(s.clone()).await.unwrap();

        // Create a new scheduler and load existing
        let sched2 = Scheduler::new(std::sync::Arc::new(p));
        sched2.load_existing().await.unwrap();
        let list = sched2.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, s.id);
    }
}

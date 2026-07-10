use std::sync::Arc;
use chrono::{DateTime, Utc};
use cron::Schedule;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

use crate::persistence::Persistence;
use crate::variables::ScheduledRecipe;
use crate::variables::ExecutionResult;
use crate::variables::ExecutionStatus;

pub type ExecutorFn = Arc<dyn Fn(Uuid) -> tokio::sync::oneshot::Receiver<ExecutionResult> + Send + Sync>;

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
            }),
        }
    }

    pub async fn set_executor(&self, exec: ExecutorFn) {
        *self.inner.executor.write().await = Some(exec);
    }

    pub async fn load_from_disk(&self) -> anyhow::Result<()> {
        let schedules = self.inner.persistence.list_schedules().await?;
        for s in schedules {
            if let Err(e) = self.schedule_one(s.clone()).await {
                tracing::warn!("failed to schedule {}: {e}", s.id);
            }
        }
        Ok(())
    }

    pub async fn schedule_one(&self, s: ScheduledRecipe) -> anyhow::Result<()> {
        let cron = Schedule::from_str(&s.cron_schedule)
            .map_err(|e| anyhow::anyhow!("invalid cron: {e}"))?;
        let next_run = cron.upcoming(Utc).next()
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
                    let dur = (active.next_run - now).to_std()
                        .unwrap_or(std::time::Duration::from_secs(60));
                    tokio::time::sleep(dur).await;
                }

                // Trigger the executor if available
                let exec = inner.executor.read().await.clone();
                if let Some(exec_fn) = exec {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let exec_fn = Arc::clone(&exec_fn);
                    let recipe_id = active.recipe_id;
                    let persistence = Arc::clone(&inner.persistence);
                    let sched_id = id;
                    tokio::spawn(async move {
                        let exec_rx = exec_fn(recipe_id);
                        let result = exec_rx.await
                            .unwrap_or_else(|_| ExecutionResult {
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
        self.inner.schedules
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
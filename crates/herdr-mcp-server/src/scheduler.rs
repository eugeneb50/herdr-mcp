//! Cron schedules for saved recipes.
//!
//! A schedule's only state is its persisted record (`data/schedules/`). Each
//! schedule has one runner task that re-reads that record every tick, so
//! deleting or disabling a schedule takes effect at its next tick without
//! any in-memory copy to keep in sync. Firing hands the recipe id to the
//! executor (the run engine), which records the run itself.

use chrono::{DateTime, Utc};
use cron::Schedule;
use dashmap::DashMap;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use uuid::Uuid;

use crate::persistence::Persistence;
use crate::variables::ScheduledRecipe;

/// How often [`Scheduler::run`] picks up schedules saved since the last look.
const SYNC_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// Runs a recipe by id to completion.
pub type ExecutorFn = Arc<dyn Fn(Uuid) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

#[derive(Clone)]
pub struct Scheduler {
    inner: Arc<SchedulerInner>,
}

struct SchedulerInner {
    persistence: Arc<Persistence>,
    executor: OnceLock<ExecutorFn>,
    /// Runner task per schedule id (task handles only, no schedule state).
    runners: DashMap<Uuid, tokio::task::JoinHandle<()>>,
    /// Recipes currently executing, so two schedules never overlap one recipe.
    in_flight: DashMap<Uuid, ()>,
}

fn parse_cron(expr: &str) -> anyhow::Result<Schedule> {
    let cron = Schedule::from_str(expr).map_err(|e| anyhow::anyhow!("invalid cron: {e}"))?;
    if cron.upcoming(Utc).next().is_none() {
        anyhow::bail!("cron never fires");
    }
    Ok(cron)
}

impl Scheduler {
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            inner: Arc::new(SchedulerInner {
                persistence,
                executor: OnceLock::new(),
                runners: DashMap::new(),
                in_flight: DashMap::new(),
            }),
        }
    }

    /// Set the function schedules fire into. Only the first call takes effect.
    pub fn set_executor(&self, executor: ExecutorFn) {
        let _ = self.inner.executor.set(executor);
    }

    /// Fire schedules from this process: start a runner for every saved
    /// schedule now and every [`SYNC_EVERY`] after, so schedules saved by any
    /// process sharing the data dir (another MCP client, the bridge) start
    /// here. Only the data dir's daemon calls this; it never returns.
    pub async fn run(&self) {
        loop {
            if let Err(e) = self.sync().await {
                tracing::warn!("scheduler: listing schedules: {e}");
            }
            tokio::time::sleep(SYNC_EVERY).await;
        }
    }

    /// Start a runner for each enabled schedule that has none running. An
    /// enabled schedule whose cron is invalid or never fires again is
    /// disabled (so it is reported once, not on every pass).
    pub async fn sync(&self) -> anyhow::Result<()> {
        for mut schedule in self.inner.persistence.list_schedules().await? {
            let running = self
                .inner
                .runners
                .get(&schedule.id)
                .is_some_and(|h| !h.is_finished());
            if running || !schedule.enabled {
                continue;
            }
            if let Err(e) = parse_cron(&schedule.cron_schedule) {
                tracing::warn!("schedule {}: {e}; disabling it", schedule.id);
                schedule.enabled = false;
                self.inner.persistence.save_schedule(&schedule).await?;
                continue;
            }
            self.spawn_runner(schedule.id);
        }
        Ok(())
    }

    /// Validate and save a schedule; the daemon's [`Scheduler::run`] starts
    /// firing it within [`SYNC_EVERY`].
    pub async fn schedule_one(&self, s: ScheduledRecipe) -> anyhow::Result<()> {
        parse_cron(&s.cron_schedule)?;
        self.inner.persistence.save_schedule(&s).await
    }

    fn spawn_runner(&self, id: Uuid) {
        let inner = Arc::clone(&self.inner);
        let handle = tokio::spawn(async move { inner.run_schedule(id).await });
        if let Some(old) = self.inner.runners.insert(id, handle) {
            old.abort();
        }
    }

    pub async fn remove(&self, id: Uuid) -> anyhow::Result<bool> {
        let removed = self.inner.persistence.delete_schedule(&id).await?;
        if let Some((_, handle)) = self.inner.runners.remove(&id) {
            handle.abort();
        }
        Ok(removed)
    }

    /// Persist the enabled flag; the runner reads it at its next tick.
    pub async fn set_enabled(&self, id: Uuid, enabled: bool) -> anyhow::Result<bool> {
        let Some(mut sched) = self.inner.persistence.load_schedule(&id).await? else {
            return Ok(false);
        };
        sched.enabled = enabled;
        self.inner.persistence.save_schedule(&sched).await?;
        Ok(true)
    }

    /// Persisted schedules, with `next_run` computed from the cron when enabled.
    pub async fn list(&self) -> Vec<ScheduledRecipe> {
        let mut list = self
            .inner
            .persistence
            .list_schedules()
            .await
            .unwrap_or_default();
        for s in &mut list {
            s.next_run = if s.enabled {
                parse_cron(&s.cron_schedule)
                    .ok()
                    .and_then(|c| c.upcoming(Utc).next())
            } else {
                None
            };
        }
        list
    }
}

impl SchedulerInner {
    async fn load(&self, id: Uuid) -> Option<ScheduledRecipe> {
        self.persistence.load_schedule(&id).await.ok().flatten()
    }

    /// Fire schedule `id` at each cron tick until it is deleted, disabled or
    /// its cron runs out; [`Scheduler::sync`] restarts (or disables) it.
    async fn run_schedule(&self, id: Uuid) {
        loop {
            let Some(sched) = self.load(id).await.filter(|s| s.enabled) else {
                return;
            };
            let Some(next) = parse_cron(&sched.cron_schedule)
                .ok()
                .and_then(|c| c.upcoming(Utc).next())
            else {
                return;
            };
            sleep_until(next).await;

            // Re-read: it may have been deleted or disabled while we slept.
            let Some(sched) = self.load(id).await.filter(|s| s.enabled) else {
                return;
            };
            let Some(executor) = self.executor.get() else {
                tracing::warn!("schedule {id}: no executor set; skipping tick");
                continue;
            };
            if self.in_flight.insert(sched.recipe_id, ()).is_some() {
                tracing::debug!(
                    "schedule {id}: recipe {} still running; skipping tick",
                    sched.recipe_id
                );
                continue;
            }
            {
                // Cleared on drop, so an abort mid-run (remove) can't leave
                // the recipe marked in flight forever.
                let _in_flight = InFlight(&self.in_flight, sched.recipe_id);
                executor(sched.recipe_id).await;
            }

            if let Some(mut latest) = self.load(id).await {
                latest.last_run = Some(Utc::now());
                if let Err(e) = self.persistence.save_schedule(&latest).await {
                    tracing::warn!("schedule {id}: failed to record last_run: {e}");
                }
            }
        }
    }
}

struct InFlight<'a>(&'a DashMap<Uuid, ()>, Uuid);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.remove(&self.1);
    }
}

async fn sleep_until(at: DateTime<Utc>) {
    if let Ok(wait) = (at - Utc::now()).to_std() {
        tokio::time::sleep(wait).await;
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

    /// A per-second schedule fires into the executor; disabling stops it,
    /// and removing ends the runner. Uses real time (~4s).
    #[tokio::test]
    async fn fires_until_disabled_and_stops_when_removed() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let tmp = tempfile::tempdir().unwrap();
        let p = Arc::new(Persistence::new(tmp.path().to_path_buf()));
        let sched = Scheduler::new(Arc::clone(&p));
        let fired = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&fired);
        sched.set_executor(Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }));
        let mut s = scheduled();
        s.cron_schedule = "* * * * * *".into();
        sched.schedule_one(s.clone()).await.unwrap();
        assert!(
            sched.inner.runners.is_empty(),
            "saving alone starts nothing"
        );
        sched.sync().await.unwrap();
        sched.sync().await.unwrap();
        assert_eq!(sched.inner.runners.len(), 1, "sync is idempotent");

        tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
        assert!(fired.load(Ordering::SeqCst) >= 1, "never fired");
        let stored = p.load_schedule(&s.id).await.unwrap().unwrap();
        assert!(stored.last_run.is_some());

        sched.set_enabled(s.id, false).await.unwrap();
        assert_eq!(sched.list().await[0].next_run, None);
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let after_disable = fired.load(Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        assert_eq!(
            fired.load(Ordering::SeqCst),
            after_disable,
            "fired while disabled"
        );

        assert!(sched.remove(s.id).await.unwrap());
        assert!(sched.inner.runners.is_empty());
        assert!(sched.list().await.is_empty());
    }

    /// A schedule whose cron never fires again is disabled once, not
    /// re-warned and restarted on every pass.
    #[tokio::test]
    async fn sync_disables_a_cron_that_never_fires() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Arc::new(Persistence::new(tmp.path().to_path_buf()));
        let sched = Scheduler::new(Arc::clone(&p));
        let mut s = scheduled();
        s.cron_schedule = "0 0 0 1 1 * 2020".into();
        p.save_schedule(&s).await.unwrap();

        sched.sync().await.unwrap();
        assert!(sched.inner.runners.is_empty());
        let stored = p.load_schedule(&s.id).await.unwrap().unwrap();
        assert!(!stored.enabled);
    }

    #[tokio::test]
    async fn test_sync_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let sched = Scheduler::new(std::sync::Arc::new(p));
        // Should not error when there are no schedules
        sched.sync().await.unwrap();
        assert!(sched.list().await.is_empty());
    }

    /// A schedule saved through one process starts in the daemon's.
    #[tokio::test]
    async fn sync_starts_schedules_saved_elsewhere() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persistence::new(tmp.path().to_path_buf());
        let saver = Scheduler::new(std::sync::Arc::new(p.clone()));
        let s = scheduled();
        saver.schedule_one(s.clone()).await.unwrap();

        let daemon = Scheduler::new(std::sync::Arc::new(p));
        daemon.sync().await.unwrap();
        assert!(daemon.inner.runners.contains_key(&s.id));
        assert!(saver.inner.runners.is_empty());
    }
}

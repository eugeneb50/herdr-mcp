use super::*;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
use futures_util::{FutureExt, future::BoxFuture};
use herdr_mcp_api::{RunAction, RunOrigin, RunStatus, StartRunRequest};
use herdr_queue::{Fenced, Lease, Outcome};
use std::{collections::VecDeque, panic::AssertUnwindSafe};
use tokio::time::Instant;
use tracing::Instrument;

trait Executor: Send + Sync {
    fn start<'a>(
        &'a self,
        item: &'a Item,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Started, String>>;
}

struct Started {
    run_id: String,
    outcome: BoxFuture<'static, RunOutcome>,
}

enum RunOutcome {
    Done(Value),
    Failed(String),
    Cancelled,
}

struct RunExecutor(HerdrMcpServer);

struct CancelRun(crate::runs::Runs, uuid::Uuid);

impl Drop for CancelRun {
    fn drop(&mut self) {
        self.0.control(self.1, RunAction::Cancel);
    }
}

impl Executor for RunExecutor {
    fn start<'a>(
        &'a self,
        item: &'a Item,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Started, String>> {
        Box::pin(async move {
            let mut input = item.payload.clone();
            input.extend([
                ("id".into(), Value::String(item.id.clone())),
                ("requester".into(), serde_json::json!(item.requester)),
                ("attempt".into(), serde_json::json!(item.attempts)),
                ("waiters".into(), serde_json::json!(item.waiters)),
            ]);
            let (run, mut done) = crate::runs::start_run(
                &self.0,
                StartRunRequest {
                    recipe_id: Some(
                        item.queue
                            .parse::<uuid::Uuid>()
                            .map_err(|err| err.to_string())?,
                    ),
                    variables: HashMap::from([("item".into(), Value::Object(input))]),
                    ..Default::default()
                },
                RunOrigin::Queue {
                    item: item.id.clone(),
                },
            )
            .await
            .map_err(|err| err.to_string())?;
            let guard = CancelRun(self.0.runs.clone(), run.id);
            Ok(Started {
                run_id: run.id.to_string(),
                outcome: Box::pin(async move {
                    let result = tokio::select! {
                        result = &mut done => result,
                        () = cancel.cancelled() => {
                            guard.0.control(guard.1, RunAction::Cancel);
                            done.await
                        }
                    };
                    drop(guard);
                    match result {
                        Ok(run) if run.status == RunStatus::Completed => {
                            RunOutcome::Done(serde_json::json!(run.results))
                        }
                        Ok(run) if run.status == RunStatus::Cancelled => RunOutcome::Cancelled,
                        Ok(run) => RunOutcome::Failed(
                            run.error.unwrap_or_else(|| run.status.as_str().to_owned()),
                        ),
                        Err(error) => RunOutcome::Failed(error.to_string()),
                    }
                }),
            })
        })
    }
}

impl QueueService {
    pub fn start(&self, server: &HerdrMcpServer) {
        self.start_with(server, Arc::new(RunExecutor(server.clone())));
        // Idle until `mail.enabled` is on.
        self.start_mail(server, Arc::new(super::mail::CliMailHerdr));
    }

    fn start_with(&self, server: &HerdrMcpServer, executor: Arc<dyn Executor>) {
        let service = self.clone();
        let server = server.clone();
        self.0.tracker.spawn(async move {
            loop {
                if service.0.root.is_cancelled() {
                    break;
                }
                if let Err(error) = service.discover(&server, &executor).await {
                    tracing::error!("discovering queue collections: {error:#}");
                }
                let now = service.now();
                match service.db(&server, move |db| Ok(db.reap(now)?)).await {
                    Ok(items) => {
                        for item in items {
                            if let Some(active) =
                                lock(&service.0.active).get(&(item.id, item.claims))
                            {
                                active.cancel.cancel();
                            }
                            service.wake(&item.queue);
                        }
                    }
                    Err(error) => tracing::error!("reaping queue leases: {error:#}"),
                }
                tokio::select! {
                    () = service.0.root.cancelled() => break,
                    () = tokio::time::sleep(duration(&server, QUEUE_REAP_EVERY_MS)) => {}
                }
            }
        });
    }

    async fn discover(&self, server: &HerdrMcpServer, executor: &Arc<dyn Executor>) -> Result<()> {
        for recipe in server.persistence.list_recipes(None).await? {
            if recipe.queue.is_none() || self.0.root.is_cancelled() {
                continue;
            }
            let queue = recipe.id.to_string();
            let mut wakes = lock(&self.0.wakes);
            if wakes.contains_key(&queue) {
                continue;
            }
            let (tx, rx) = mpsc::channel(1);
            wakes.insert(queue.clone(), tx);
            let service = self.clone();
            let server = server.clone();
            let executor = executor.clone();
            self.0.tracker.spawn(async move {
                let mut first = Some(rx);
                service
                    .supervise(&server, &queue, || {
                        let mut rx = first.take().unwrap_or_else(|| {
                            let (tx, rx) = mpsc::channel(1);
                            lock(&service.0.wakes).insert(queue.clone(), tx);
                            rx
                        });
                        let server = server.clone();
                        let service = service.clone();
                        let executor = executor.clone();
                        let queue = queue.clone();
                        async move {
                            service
                                .actor(&server, recipe.id, &queue, &mut rx, &executor)
                                .await
                        }
                    })
                    .await;
            });
        }
        Ok(())
    }

    async fn supervise<F: std::future::Future<Output = Result<()>>>(
        &self,
        server: &HerdrMcpServer,
        queue: &str,
        mut actor: impl FnMut() -> F,
    ) {
        let mut crashes = VecDeque::new();
        loop {
            let exit = AssertUnwindSafe(actor()).catch_unwind().await;
            if self.0.root.is_cancelled() {
                break;
            }
            let now = Instant::now();
            while crashes
                .front()
                .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(600))
            {
                crashes.pop_front();
            }
            let cause = match exit {
                Ok(Ok(())) => "actor exited".to_owned(),
                Ok(Err(error)) => format!("{error:#}"),
                Err(_) => "actor panicked".to_owned(),
            };
            if u64::try_from(crashes.len()).unwrap_or(u64::MAX)
                >= number(server, QUEUE_SUPERVISOR_MAX_RESTARTS)
            {
                tracing::error!(queue, "queue restart limit reached: {cause}");
                break;
            }
            let delay = (1_u64 << crashes.len().min(5)).min(30);
            crashes.push_back(now);
            tracing::warn!(queue, "restarting queue actor: {cause}");
            tokio::select! {
                () = self.0.root.cancelled() => break,
                () = tokio::time::sleep(Duration::from_secs(delay)) => {}
            }
        }
    }

    async fn actor(
        &self,
        server: &HerdrMcpServer,
        id: uuid::Uuid,
        queue: &str,
        wake: &mut mpsc::Receiver<()>,
        executor: &Arc<dyn Executor>,
    ) -> Result<()> {
        while !self.0.root.is_cancelled() {
            if let Some(config) = server
                .persistence
                .load_recipe(&id)
                .await?
                .and_then(|r| r.queue)
            {
                validate_config(&config)?;
                let concurrency = config
                    .concurrency
                    .unwrap_or_else(|| count(server, QUEUE_DEFAULT_CONCURRENCY));
                let lease_ms = config
                    .lease_ms
                    .unwrap_or_else(|| number(server, QUEUE_DEFAULT_LEASE_MS));
                loop {
                    let running = lock(&self.0.active)
                        .values()
                        .filter(|active| active.queue == queue)
                        .count();
                    if running >= concurrency as usize {
                        break;
                    }
                    let runtime = self.0.clone();
                    let cancel = runtime.root.child_token();
                    let name = queue.to_owned();
                    let service = self.clone();
                    let claimed = self
                        .db(server, move |db| {
                            if cancel.is_cancelled() {
                                return Ok(None);
                            }
                            let draining: Vec<_> = lock(&runtime.active)
                                .iter()
                                .filter(|(_, active)| active.queue == name)
                                .map(|((id, _), _)| id.clone())
                                .collect();
                            let excluded: Vec<_> = draining.iter().map(String::as_str).collect();
                            let Some((item, lease)) =
                                db.claim_excluding(&name, lease_ms, service.now(), &excluded)?
                            else {
                                return Ok(None);
                            };
                            lock(&runtime.active).insert(
                                (lease.id.clone(), lease.claim),
                                ActiveClaim {
                                    queue: name,
                                    cancel: cancel.clone(),
                                    settling: false,
                                },
                            );
                            Ok(Some((item, lease, cancel)))
                        })
                        .await?;
                    let Some((item, lease, cancel)) = claimed else {
                        break;
                    };
                    let service = self.clone();
                    let server = server.clone();
                    let executor = executor.clone();
                    let span = tracing::info_span!(
                        "queue_item",
                        queue,
                        item = item.id,
                        attempt = item.attempts,
                        claim = lease.claim,
                        run = tracing::field::Empty
                    );
                    self.0.tracker.spawn(
                        async move {
                            let key = (lease.id.clone(), lease.claim);
                            if let Err(error) = service
                                .execute(
                                    &server,
                                    executor.as_ref(),
                                    &item,
                                    lease,
                                    &cancel,
                                    lease_ms,
                                )
                                .await
                            {
                                cancel.cancel();
                                tracing::error!("queue item worker: {error:#}");
                            }
                            lock(&service.0.active).remove(&key);
                            service.wake(&item.queue);
                        }
                        .instrument(span),
                    );
                }
            }
            tokio::select! {
                () = self.0.root.cancelled() => break,
                _ = wake.recv() => {}
                () = tokio::time::sleep(duration(server, QUEUE_TICK_MS)) => {}
            }
        }
        Ok(())
    }

    async fn execute(
        &self,
        server: &HerdrMcpServer,
        executor: &dyn Executor,
        item: &Item,
        lease: Lease,
        cancel: &CancellationToken,
        lease_ms: u64,
    ) -> Result<()> {
        let run = async {
            if cancel.is_cancelled() {
                return Ok(Some(RunOutcome::Cancelled));
            }
            let started = match executor.start(item, cancel.clone()).await {
                Ok(started) => started,
                Err(error) => return Ok::<_, anyhow::Error>(Some(RunOutcome::Failed(error))),
            };
            let now = self.now();
            let run_id = started.run_id;
            tracing::Span::current().record("run", &run_id);
            let token = lease.clone();
            if matches!(
                self.db(server, move |db| Ok(db.set_run(&token, &run_id, now)?))
                    .await?,
                Fenced::Stale
            ) {
                cancel.cancel();
                return Ok(None);
            }

            Ok(Some(started.outcome.await))
        };
        let renew = async {
            let heartbeat = Duration::from_millis((lease_ms / 3).max(1));
            loop {
                tokio::time::sleep(heartbeat).await;
                let token = lease.clone();
                let service = self.clone();
                if matches!(
                    self.db(server, move |db| Ok(db.heartbeat(
                        &token,
                        lease_ms,
                        service.now()
                    )?))
                    .await?,
                    Fenced::Stale
                ) {
                    return Ok::<_, anyhow::Error>(());
                }
            }
        };
        let work = async {
            tokio::pin!(run, renew);
            let grace = duration(server, QUEUE_SHUTDOWN_WAIT_MS);
            tokio::select! {
                biased;
                result = &mut run => result,
                () = cancel.cancelled() => {
                    tokio::time::timeout(grace, &mut run)
                        .await
                        .unwrap_or(Ok(Some(RunOutcome::Cancelled)))
                }
                result = &mut renew => {
                    result?;
                    cancel.cancel();
                    tokio::time::timeout(grace, &mut run).await.ok().transpose()?;
                    Ok(None)
                }
            }
        };
        let outcome = match AssertUnwindSafe(work).catch_unwind().await {
            Ok(outcome) => outcome?,
            Err(_) => RunOutcome::Failed("panicked".into()).into(),
        };
        let Some(outcome) = outcome else {
            return Ok(());
        };
        if let Some(active) = lock(&self.0.active).get_mut(&(lease.id.clone(), lease.claim)) {
            active.settling = true;
        }
        let interrupted = cancel.is_cancelled();
        let now = self.now();
        self.db(server, move |db| {
            match outcome {
                RunOutcome::Done(value) => {
                    db.finish(&lease, Outcome::Done(value), now)?;
                }
                RunOutcome::Failed(error) => {
                    db.finish(&lease, Outcome::Failed(error), now)?;
                }
                RunOutcome::Cancelled if interrupted => {
                    db.release(&lease, now)?;
                }
                RunOutcome::Cancelled => {
                    db.finish(&lease, Outcome::Cancelled, now)?;
                }
            }
            Ok(())
        })
        .await
    }

    pub async fn shutdown(&self, server: &HerdrMcpServer) {
        self.0.root.cancel();
        self.0.tracker.close();
        let grace = duration(server, QUEUE_SHUTDOWN_WAIT_MS);
        if tokio::time::timeout(grace, self.0.tracker.wait())
            .await
            .is_ok()
        {
            return;
        }

        let runtime = self.0.clone();
        let now = self.now();
        let release = self.0.tracker.spawn_blocking(move || {
            // A blocked store already prevents immediate recovery. Let these
            // leases expire instead of queueing another task behind its mutex.
            let Ok(mut db) = runtime.db.try_lock() else {
                anyhow::bail!("queue database busy during shutdown");
            };
            if let Some(db) = db.as_mut() {
                let leases: Vec<_> = lock(&runtime.active)
                    .iter()
                    .filter(|(_, worker)| !worker.settling)
                    .map(|((id, claim), _)| Lease {
                        id: id.clone(),
                        claim: *claim,
                    })
                    .collect();
                for lease in leases {
                    db.release(&lease, now)?;
                }
            }
            Ok::<_, anyhow::Error>(())
        });
        match tokio::time::timeout(grace.max(Duration::from_millis(1)), release).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => tracing::warn!("releasing queue leases at shutdown: {error:#}"),
            Ok(Err(error)) => tracing::warn!("queue shutdown release task: {error}"),
            Err(_) => tracing::warn!("queue shutdown release timed out"),
        }
    }
}

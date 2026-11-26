use super::*;
use herdr_mcp_api::{CreateRecipeRequest, Flow, RecipeStep};
use serde_json::json;
use tokio::sync::{Semaphore, mpsc::UnboundedReceiver};

struct Fake {
    starts: mpsc::UnboundedSender<String>,
    gate: Arc<Semaphore>,
    attach: Arc<Semaphore>,
    cancelled: Arc<Mutex<Vec<String>>>,
}

impl Executor for Fake {
    fn start<'a>(
        &'a self,
        item: &'a Item,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Started, String>> {
        Box::pin(async move {
            let id = item.id.clone();
            self.starts.send(id.clone()).unwrap();
            let mode = item
                .payload
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("ok")
                .to_owned();
            if mode == "start_err" {
                return Err("start failed".into());
            }
            if mode == "attach" {
                self.attach.acquire().await.unwrap().forget();
            }
            let cancelled = self.cancelled.clone();
            let gate = self.gate.clone();
            Ok(Started {
                run_id: format!("fake-{id}"),
                outcome: Box::pin(async move {
                    match mode.as_str() {
                        "panic" => panic!("fake executor"),
                        "fail" => RunOutcome::Failed("fake failed".into()),
                        "external_cancel" => RunOutcome::Cancelled,
                        "hang" => {
                            cancel.cancelled().await;
                            lock(&cancelled).push(id);
                            RunOutcome::Cancelled
                        }
                        "finish_during_shutdown" | "fail_during_shutdown" => {
                            cancel.cancelled().await;
                            tokio::time::sleep(Duration::from_millis(25)).await;
                            if mode == "finish_during_shutdown" {
                                RunOutcome::Done(json!({"saved": true}))
                            } else {
                                RunOutcome::Failed("run failed while draining".into())
                            }
                        }
                        "ignore" => {
                            gate.acquire().await.unwrap().forget();
                            RunOutcome::Done(Value::Null)
                        }
                        "hold" => {
                            tokio::select! {
                                () = cancel.cancelled() => {
                                    lock(&cancelled).push(id);
                                    RunOutcome::Cancelled
                                }
                                permit = gate.acquire() => {
                                    permit.unwrap().forget();
                                    RunOutcome::Done(Value::Null)
                                }
                            }
                        }
                        _ => RunOutcome::Done(Value::Null),
                    }
                }),
            })
        })
    }
}

async fn fixture(concurrency: u32) -> (tempfile::TempDir, HerdrMcpServer, Recipe) {
    let dir = tempfile::tempdir().unwrap();
    let persistence = crate::Persistence::new(dir.path().to_owned());
    persistence.init().await.unwrap();
    let server = HerdrMcpServer::new(persistence, crate::AgentRegistry::new());
    setting(&server, QUEUE_REAP_EVERY_MS, 100);
    setting(&server, QUEUE_TICK_MS, 100);
    setting(&server, QUEUE_SHUTDOWN_WAIT_MS, 250);
    let recipe = server
        .create_recipe(CreateRecipeRequest {
            name: "build".into(),
            description: None,
            flow: Flow::chain(vec![RecipeStep {
                id: "probe".into(),
                tool: "herdr_api".into(),
                params: json!({"method": "pane.list", "params": {}}),
                ..Default::default()
            }]),
            variables: HashMap::new(),
            category: None,
            triggers: vec![],
            trail: Default::default(),
            queue: Some(QueueConfig {
                concurrency: Some(concurrency),
                max_depth: Some(1000),
                max_attempts: Some(1),
                lease_ms: Some(1000),
                ..Default::default()
            }),
        })
        .await
        .unwrap();
    (dir, server, recipe)
}

fn setting(server: &HerdrMcpServer, key: &str, value: u64) {
    crate::write(&server.settings)
        .set(key, herdr_mcp_api::settings::Value::Number(value))
        .unwrap();
}

fn fake(server: &HerdrMcpServer) -> (Arc<Fake>, UnboundedReceiver<String>) {
    let (starts, rx) = mpsc::unbounded_channel();
    let executor = Arc::new(Fake {
        starts,
        gate: Arc::new(Semaphore::new(0)),
        attach: Arc::new(Semaphore::new(0)),
        cancelled: Arc::default(),
    });
    server.queues.start_with(server, executor.clone());
    (executor, rx)
}

async fn push(server: &HerdrMcpServer, mode: &str) -> Item {
    let result = server
        .queues
        .push(
            server,
            "build",
            Map::from_iter([("mode".into(), json!(mode))]),
            None,
            Some("tester".into()),
        )
        .await
        .unwrap();
    let Pushed::New(item) = result else {
        panic!("expected new item")
    };
    item
}

async fn next(rx: &mut UnboundedReceiver<String>) -> String {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

async fn terminal(server: &HerdrMcpServer, id: &str, status: ItemStatus) -> Item {
    let item = server.queues.wait(server, id, Some(10_000)).await.unwrap();
    assert_eq!(item.status, status);
    replay(server, id).await;
    item
}

async fn replay(server: &HerdrMcpServer, id: &str) {
    use ItemStatus::*;
    let herdr_mcp_api::recipe::QueueItemResponse { item, events } =
        server.queues.item(server, id).await.unwrap();
    let mut status = Queued;
    let mut attempts = 0;
    let mut claims = 0;
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.seq, index as i64 + 1);
        if matches!(
            event.kind.as_str(),
            "run_started" | "failed" | "stalled" | "released" | "done"
        ) {
            let data = event.data.as_ref().unwrap();
            assert_eq!(data["attempt"], attempts);
            assert_eq!(data["claim"], claims);
        }
        match event.kind.as_str() {
            "enqueued" | "deduped" | "coalesced" => {}
            "claimed" => {
                assert_eq!(status, Queued);
                attempts += 1;
                claims += 1;
                let data = event.data.as_ref().unwrap();
                assert_eq!(data["attempt"], attempts);
                assert_eq!(data["claim"], claims);
                status = Running;
            }
            "run_started" | "failed" => assert_eq!(status, Running),
            "stalled" => {
                assert_eq!(status, Running);
                status = if attempts >= item.max_attempts {
                    Dead
                } else {
                    Queued
                };
            }
            "requeued" => {
                assert_eq!(status, Running);
                status = Queued;
            }
            "released" => {
                assert_eq!(status, Running);
                attempts -= 1;
                status = Queued;
            }
            "done" => {
                assert_eq!(status, Running);
                status = Done;
            }
            "dead" => {
                assert_eq!(status, Running);
                status = Dead;
            }
            "cancelled" => {
                assert!(matches!(status, Running | Queued));
                status = Cancelled;
            }
            "retried" => {
                assert!(matches!(status, Dead | Cancelled));
                status = Queued;
                attempts = 0;
            }
            kind => panic!("unknown event {kind}"),
        }
    }
    assert_eq!(
        (item.status, item.attempts, item.claims),
        (status, attempts, claims)
    );
}

#[tokio::test(start_paused = true)]
async fn fifo_and_parallel_slots() {
    for concurrency in [1, 2] {
        let (_dir, server, _) = fixture(concurrency).await;
        let a = push(&server, "hold").await;
        let b = push(&server, "hold").await;
        let (executor, mut starts) = fake(&server);
        let first = next(&mut starts).await;
        if concurrency == 1 {
            assert_eq!(first, a.id);
            assert_eq!(
                server
                    .queues
                    .item(&server, &b.id)
                    .await
                    .unwrap()
                    .item
                    .status,
                ItemStatus::Queued
            );
            executor.gate.add_permits(1);
            terminal(&server, &a.id, ItemStatus::Done).await;
            assert_eq!(next(&mut starts).await, b.id);
            executor.gate.add_permits(1);
        } else {
            let second = next(&mut starts).await;
            assert_ne!(first, second);
            for id in [&a.id, &b.id] {
                assert_eq!(
                    server.queues.item(&server, id).await.unwrap().item.status,
                    ItemStatus::Running
                );
            }
            executor.gate.add_permits(2);
        }
        terminal(&server, &a.id, ItemStatus::Done).await;
        terminal(&server, &b.id, ItemStatus::Done).await;
        server.queues.shutdown(&server).await;
    }
}

#[tokio::test(start_paused = true)]
async fn executor_panic_and_start_error_do_not_stop_actor() {
    let (_dir, server, _) = fixture(1).await;
    let panic = push(&server, "panic").await;
    let start = push(&server, "start_err").await;
    let failure = push(&server, "fail").await;
    let second = push(&server, "ok").await;
    let (_executor, _starts) = fake(&server);
    for (item, error) in [
        (panic, "panicked"),
        (start, "start failed"),
        (failure, "fake failed"),
    ] {
        assert_eq!(
            terminal(&server, &item.id, ItemStatus::Dead).await.result,
            Some(json!(error))
        );
    }
    terminal(&server, &second.id, ItemStatus::Done).await;
    server.queues.shutdown(&server).await;
}

#[tokio::test(start_paused = true)]
async fn actor_panic_restarts_up_to_cap() {
    let (_dir, server, _) = fixture(1).await;
    setting(&server, QUEUE_SUPERVISOR_MAX_RESTARTS, 2);
    let mut attempts = Vec::new();
    server
        .queues
        .supervise(&server, "build", || {
            attempts.push(Instant::now());
            async {
                panic!("fake actor");
            }
        })
        .await;
    assert_eq!(attempts.len(), 3);
    assert_eq!(attempts[1] - attempts[0], Duration::from_secs(1));
    assert_eq!(attempts[2] - attempts[1], Duration::from_secs(2));
}

#[tokio::test(start_paused = true)]
async fn dead_owner_is_reaped_and_fenced() {
    let (dir, server, recipe) = fixture(1).await;
    let mut recipe = recipe;
    recipe.queue.as_mut().unwrap().max_attempts = Some(2);
    server.persistence.save_recipe(&recipe).await.unwrap();
    let item = push(&server, "ok").await;
    let mut other = Queue::open(dir.path().join("queue.db")).unwrap();
    let (_, lease) = other
        .claim(&recipe.id.to_string(), 100, server.queues.now())
        .unwrap()
        .unwrap();
    let (_executor, _starts) = fake(&server);
    terminal(&server, &item.id, ItemStatus::Done).await;
    let before = other.item(&item.id).unwrap();
    assert_eq!(
        other
            .finish(&lease, Outcome::Done(Value::Null), server.queues.now())
            .unwrap(),
        Fenced::Stale
    );
    assert_eq!(other.item(&item.id).unwrap(), before);
    assert!(before.unwrap().1.iter().any(|e| e.kind == "stalled"));
    server.queues.shutdown(&server).await;
}

#[tokio::test(start_paused = true)]
async fn stale_attachment_and_mid_run_cancel() {
    let (dir, server, _) = fixture(1).await;
    let a = push(&server, "attach").await;
    let b = push(&server, "hang").await;
    let (executor, mut starts) = fake(&server);
    assert_eq!(next(&mut starts).await, a.id);
    Queue::open(dir.path().join("queue.db"))
        .unwrap()
        .cancel(&a.id, server.queues.now())
        .unwrap();
    executor.attach.add_permits(1);
    assert_eq!(next(&mut starts).await, b.id);
    let herdr_mcp_api::recipe::QueueItemResponse { item: _, events } =
        server.queues.item(&server, &a.id).await.unwrap();
    assert_eq!(
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        ["enqueued", "claimed", "cancelled"]
    );
    loop {
        if server
            .queues
            .item(&server, &b.id)
            .await
            .unwrap()
            .item
            .run_id
            .is_some()
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    server.queues.cancel(&server, b.id.clone()).await.unwrap();
    terminal(&server, &a.id, ItemStatus::Cancelled).await;
    terminal(&server, &b.id, ItemStatus::Cancelled).await;
    server.queues.shutdown(&server).await;
    assert_eq!(*lock(&executor.cancelled), [b.id]);
}

#[tokio::test(start_paused = true)]
async fn shutdown_releases_cooperative_and_uncooperative_leases() {
    let (_dir, server, _) = fixture(2).await;
    let held = push(&server, "hold").await;
    let ignored = push(&server, "ignore").await;
    let (executor, mut starts) = fake(&server);
    next(&mut starts).await;
    next(&mut starts).await;
    for id in [&held.id, &ignored.id] {
        while server
            .queues
            .item(&server, id)
            .await
            .unwrap()
            .item
            .run_id
            .is_none()
        {
            tokio::task::yield_now().await;
        }
    }
    let now = Instant::now();
    server.queues.shutdown(&server).await;
    assert!(now.elapsed() >= Duration::from_millis(250));
    for id in [&held.id, &ignored.id] {
        let item = server.queues.item(&server, id).await.unwrap().item;
        assert_eq!((item.status, item.attempts), (ItemStatus::Queued, 0));
        replay(&server, id).await;
    }
    let before = server.queues.item(&server, &ignored.id).await.unwrap();
    executor.gate.add_permits(1);
    server.queues.0.tracker.wait().await;
    assert!(server.queues.0.tracker.is_empty());
    assert_eq!(
        server.queues.item(&server, &ignored.id).await.unwrap(),
        before
    );
    assert!(
        server
            .queues
            .push(&server, "build", Map::new(), None, None)
            .await
            .unwrap_err()
            .is::<ShuttingDown>()
    );
}

#[tokio::test(start_paused = true)]
async fn external_push_is_seen_on_tick_after_live_update() {
    let (dir, server, recipe) = fixture(1).await;
    setting(&server, QUEUE_TICK_MS, 2000);
    let (executor, mut starts) = fake(&server);
    let held = push(&server, "hold").await;
    assert_eq!(next(&mut starts).await, held.id);
    executor.gate.add_permits(1);
    terminal(&server, &held.id, ItemStatus::Done).await;
    setting(&server, QUEUE_TICK_MS, 100);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut other = Queue::open(dir.path().join("queue.db")).unwrap();
    let Pushed::New(item) = other
        .push(
            Push {
                queue: recipe.id.to_string(),
                payload: Map::new(),
                dedupe_key: None,
                coalesce_key: None,
                requester: None,
                max_attempts: 1,
                max_depth: None,
            },
            server.queues.now(),
        )
        .unwrap()
    else {
        panic!("expected new item")
    };
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(500), starts.recv())
            .await
            .unwrap(),
        Some(item.id.clone())
    );
    terminal(&server, &item.id, ItemStatus::Done).await;
    server.queues.shutdown(&server).await;
}

#[tokio::test(start_paused = true)]
async fn burst_of_thousand_drains() {
    let (_dir, server, _) = fixture(1).await;
    let held = push(&server, "hold").await;
    let (executor, mut starts) = fake(&server);
    assert_eq!(next(&mut starts).await, held.id);
    let mut ids = vec![held.id];
    for _ in 1..1000 {
        ids.push(push(&server, "ok").await.id);
    }
    executor.gate.add_permits(1);
    for id in ids {
        terminal(&server, &id, ItemStatus::Done).await;
    }
    server.queues.shutdown(&server).await;
}

#[tokio::test]
async fn production_executor_runs_recipe_with_item_scope() {
    let (_dir, server, recipe) = fixture(1).await;
    let herdr = crate::fake_herdr::FakeHerdr::start().await;
    let client = Arc::new(crate::HerdrClient::new(herdr.socket(), 3, 10));
    let server = server.with_herdr_client(client);
    let item = push(&server, "ok").await;
    server.queues.start(&server);
    let finished = terminal(&server, &item.id, ItemStatus::Done).await;
    let run = crate::runs::load(&server, finished.run_id.unwrap().parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.recipe_id, Some(recipe.id));
    assert_eq!(run.status, RunStatus::Completed);
    assert_eq!(
        run.origin,
        RunOrigin::Queue {
            item: item.id.clone()
        }
    );
    assert_eq!(
        run.variables["item"],
        json!({
            "id": item.id, "requester": "tester", "attempt": 1, "waiters": [], "mode": "ok"
        })
    );
    assert!(run.results.contains_key("probe"));
    server.queues.shutdown(&server).await;
}

#[tokio::test(start_paused = true)]
async fn shutdown_preserves_completed_outcomes() {
    let (_dir, server, _) = fixture(2).await;
    let completed = push(&server, "finish_during_shutdown").await;
    let failed = push(&server, "fail_during_shutdown").await;
    let (_executor, mut starts) = fake(&server);
    next(&mut starts).await;
    next(&mut starts).await;
    for id in [&completed.id, &failed.id] {
        while server
            .queues
            .item(&server, id)
            .await
            .unwrap()
            .item
            .run_id
            .is_none()
        {
            tokio::task::yield_now().await;
        }
    }

    server.queues.shutdown(&server).await;

    for (id, status, result) in [
        (completed.id, ItemStatus::Done, json!({"saved": true})),
        (
            failed.id,
            ItemStatus::Dead,
            json!("run failed while draining"),
        ),
    ] {
        let herdr_mcp_api::recipe::QueueItemResponse { item, events: _ } =
            server.queues.item(&server, &id).await.unwrap();
        assert_eq!(
            (item.status, item.attempts, item.result),
            (status, 1, Some(result))
        );
        replay(&server, &id).await;
    }
}

#[tokio::test(start_paused = true)]
async fn listing_defaults_to_one_hundred_and_honors_explicit_limit() {
    let (_dir, server, _) = fixture(1).await;
    for _ in 0..101 {
        push(&server, "ok").await;
    }

    let default = server.queues.list(&server, None, None, None).await.unwrap();
    let explicit = server
        .queues
        .list(&server, Some("build".into()), None, Some(101))
        .await
        .unwrap();
    assert_eq!(default.len(), 100);
    assert_eq!(explicit.len(), 101);
    assert_eq!(default, explicit[..100]);
}

#[tokio::test(start_paused = true)]
async fn run_cancel_keeps_the_attempt_and_stops_the_item() {
    let (_dir, server, _) = fixture(1).await;
    let item = push(&server, "external_cancel").await;
    let (_executor, mut starts) = fake(&server);
    assert_eq!(next(&mut starts).await, item.id);
    let finished = tokio::select! {
        finished = terminal(&server, &item.id, ItemStatus::Cancelled) => finished,
        repeated = next(&mut starts) => panic!("cancelled work began again: {repeated}"),
    };
    assert_eq!((finished.attempts, finished.claims), (1, 1));
    assert!(starts.try_recv().is_err());
    server.queues.shutdown(&server).await;
}

#[tokio::test(start_paused = true)]
async fn cancelled_executor_that_ignores_the_token_frees_its_slot() {
    let (_dir, server, _) = fixture(1).await;
    let ignored = push(&server, "ignore").await;
    let next_item = push(&server, "ok").await;
    let (_executor, mut starts) = fake(&server);
    assert_eq!(next(&mut starts).await, ignored.id);
    while server
        .queues
        .item(&server, &ignored.id)
        .await
        .unwrap()
        .item
        .run_id
        .is_none()
    {
        tokio::task::yield_now().await;
    }
    let start = Instant::now();
    server
        .queues
        .cancel(&server, ignored.id.clone())
        .await
        .unwrap();
    assert_eq!(next(&mut starts).await, next_item.id);
    assert!(start.elapsed() >= Duration::from_millis(250));
    terminal(&server, &ignored.id, ItemStatus::Cancelled).await;
    terminal(&server, &next_item.id, ItemStatus::Done).await;
    server.queues.shutdown(&server).await;
}

#[tokio::test(start_paused = true)]
async fn startup_renews_the_lease_until_the_run_attaches() {
    let (_dir, server, _) = fixture(1).await;
    let item = push(&server, "attach").await;
    let (executor, mut starts) = fake(&server);
    assert_eq!(next(&mut starts).await, item.id);
    tokio::time::sleep(Duration::from_millis(1600)).await;
    let herdr_mcp_api::recipe::QueueItemResponse {
        item: starting,
        events: history,
    } = server.queues.item(&server, &item.id).await.unwrap();
    assert_eq!(
        (starting.status, starting.attempts, starting.claims),
        (ItemStatus::Running, 1, 1)
    );
    assert!(starting.run_id.is_none());
    assert!(!history.iter().any(|e| e.kind == "stalled"));
    executor.attach.add_permits(1);
    terminal(&server, &item.id, ItemStatus::Done).await;
    server.queues.shutdown(&server).await;
}

#[tokio::test(start_paused = true)]
async fn recipe_queue_overrides_are_checked_on_create_and_update() {
    let (_dir, server, recipe) = fixture(1).await;
    let request = CreateRecipeRequest {
        name: "invalid".into(),
        description: None,
        flow: Flow::default(),
        variables: HashMap::new(),
        category: None,
        triggers: vec![],
        trail: Default::default(),
        queue: Some(QueueConfig {
            concurrency: Some(0),
            ..Default::default()
        }),
    };
    assert!(
        server
            .create_recipe(request)
            .await
            .unwrap_err()
            .is::<RequestError>()
    );
    for queue in [
        QueueConfig {
            lease_ms: Some(0),
            ..Default::default()
        },
        QueueConfig {
            max_attempts: Some(0),
            ..Default::default()
        },
        QueueConfig {
            max_depth: Some(u32::MAX),
            ..Default::default()
        },
    ] {
        assert!(
            server
                .update_recipe(
                    recipe.id,
                    herdr_mcp_api::UpdateRecipeRequest {
                        queue: Some(queue),
                        ..Default::default()
                    }
                )
                .await
                .unwrap_err()
                .is::<RequestError>()
        );
    }
    assert_eq!(
        server
            .persistence
            .load_recipe(&recipe.id)
            .await
            .unwrap()
            .unwrap()
            .queue,
        recipe.queue
    );
}

#[tokio::test(start_paused = true)]
async fn wait_defaults_to_a_deadline_and_checks_on_queue_tick() {
    let (_dir, server, _) = fixture(1).await;
    let item = push(&server, "ok").await;
    setting(&server, QUEUE_TICK_MS, 1000);
    let until = Instant::now() + Duration::from_millis(300);
    let (waited, ()) = tokio::join!(server.queues.wait(&server, &item.id, Some(500)), async {
        tokio::time::sleep_until(until).await;
        server
            .queues
            .cancel(&server, item.id.clone())
            .await
            .unwrap();
    },);
    assert!(waited.unwrap_err().is::<WaitTimeout>());
    let another = push(&server, "ok").await;
    let result = tokio::time::timeout(
        Duration::from_secs(61),
        server.queues.wait(&server, &another.id, None),
    )
    .await
    .unwrap();
    assert!(result.unwrap_err().is::<WaitTimeout>());
}

#[tokio::test(start_paused = true)]
async fn retry_from_another_connection_waits_until_old_claim_drains() {
    let (dir, server, _) = fixture(2).await;
    let item = push(&server, "ignore").await;
    let (_executor, mut starts) = fake(&server);
    assert_eq!(next(&mut starts).await, item.id);
    while server
        .queues
        .item(&server, &item.id)
        .await
        .unwrap()
        .item
        .run_id
        .is_none()
    {
        tokio::task::yield_now().await;
    }
    let mut external = Queue::open(dir.path().join("queue.db")).unwrap();
    external.cancel(&item.id, server.queues.now()).unwrap();
    external.retry(&item.id, server.queues.now()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), starts.recv())
            .await
            .is_err()
    );
    assert_eq!(next(&mut starts).await, item.id);
    assert_eq!(
        server
            .queues
            .item(&server, &item.id)
            .await
            .unwrap()
            .item
            .claims,
        2
    );
    replay(&server, &item.id).await;
    server.queues.shutdown(&server).await;
}

#[tokio::test(start_paused = true)]
async fn explicit_list_limit_is_capped() {
    let (_dir, server, mut recipe) = fixture(1).await;
    recipe.queue.as_mut().unwrap().max_depth = Some(1001);
    server.persistence.save_recipe(&recipe).await.unwrap();
    for _ in 0..1001 {
        push(&server, "ok").await;
    }
    assert_eq!(
        server
            .queues
            .list(&server, None, None, Some(u32::MAX))
            .await
            .unwrap()
            .len(),
        1000
    );
}

#[tokio::test]
async fn shutdown_does_not_wait_behind_a_busy_store() {
    let (_dir, server, _) = fixture(1).await;
    setting(&server, QUEUE_SHUTDOWN_WAIT_MS, 10);
    let runtime = server.queues.0.clone();
    let (locked, ready) = tokio::sync::oneshot::channel();
    let (unlock, release) = std::sync::mpsc::channel();
    let blocker = server.queues.0.tracker.spawn_blocking(move || {
        let _guard = lock(&runtime.db);
        locked.send(()).unwrap();
        release.recv().unwrap();
    });
    ready.await.unwrap();

    let shutdown =
        tokio::time::timeout(Duration::from_millis(500), server.queues.shutdown(&server)).await;
    unlock.send(()).unwrap();
    blocker.await.unwrap();
    shutdown.unwrap();
}

#[tokio::test(start_paused = true)]
async fn shutdown_fallback_leaves_a_resolved_outcome_to_commit() {
    let (_dir, server, recipe) = fixture(1).await;
    setting(&server, QUEUE_SHUTDOWN_WAIT_MS, 0);
    let item = push(&server, "ok").await;
    let queue = recipe.id.to_string();
    let now = server.queues.now();
    let (_, lease) = server
        .queues
        .db(&server, move |db| Ok(db.claim(&queue, 1000, now)?.unwrap()))
        .await
        .unwrap();
    lock(&server.queues.0.active).insert(
        (lease.id.clone(), lease.claim),
        ActiveClaim {
            queue: item.queue,
            cancel: server.queues.0.root.child_token(),
            settling: true,
        },
    );
    let (commit, ready) = tokio::sync::oneshot::channel();
    let service = server.queues.clone();
    let worker_server = server.clone();
    let worker = server.queues.0.tracker.spawn(async move {
        ready.await.unwrap();
        service
            .db(&worker_server, move |db| {
                Ok(db.finish(&lease, Outcome::Done(json!(42)), now)?)
            })
            .await
            .unwrap()
    });
    server.queues.shutdown(&server).await;
    commit.send(()).unwrap();
    assert!(matches!(worker.await.unwrap(), Fenced::Applied(_)));
    assert_eq!(
        server
            .queues
            .item(&server, &item.id)
            .await
            .unwrap()
            .item
            .result,
        Some(json!(42))
    );
    replay(&server, &item.id).await;
}

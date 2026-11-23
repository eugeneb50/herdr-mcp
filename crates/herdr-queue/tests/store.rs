#![allow(
    clippy::panic,
    reason = "test helpers fail the test on an unexpected variant"
)]

use herdr_queue::{Fenced, Item, ItemStatus::*, Lease, Outcome, Push, Pushed, Queue, QueueError};
use serde_json::{Value, json};
use tempfile::TempDir;

fn db() -> (TempDir, Queue) {
    let dir = tempfile::tempdir().expect("create temporary queue directory");
    let queue = Queue::open(dir.path().join("queue.db")).expect("open temporary queue");
    (dir, queue)
}

fn request() -> Push {
    Push {
        queue: "build".into(),
        payload: Default::default(),
        dedupe_key: None,
        coalesce_key: None,
        requester: None,
        max_attempts: 3,
        max_depth: None,
    }
}

fn new_item(q: &mut Queue, p: Push) -> Item {
    let Pushed::New(item) = q.push(p, 1).expect("push fixture item") else {
        panic!("expected new item")
    };
    item
}

fn lease(item: &Item) -> Lease {
    Lease {
        id: item.id.clone(),
        claim: item.claims,
    }
}

fn applied<T: std::fmt::Debug>(value: Fenced<T>) -> T {
    let Fenced::Applied(value) = value else {
        panic!("expected applied lease")
    };
    value
}

fn claim(q: &mut Queue) -> Item {
    q.claim("build", 10, 2)
        .expect("claim fixture item")
        .expect("fixture item is queued")
        .0
}

#[test]
fn fifo() {
    let (_dir, mut q) = db();
    for seq in 1..=3 {
        assert_eq!(new_item(&mut q, request()).seq, seq);
    }
    for seq in 1..=3 {
        assert_eq!(claim(&mut q).seq, seq);
    }
}

#[test]
fn dedupe_returns_unfinished_and_finished_key_is_reusable() {
    let (_dir, mut q) = db();
    let p = || Push {
        dedupe_key: Some("key".into()),
        ..request()
    };
    let first = new_item(&mut q, p());
    for status in [Queued, Running] {
        let Pushed::Deduped(existing) = q.push(p(), 3).unwrap() else {
            panic!("expected dedupe")
        };
        assert_eq!((existing.id, existing.status), (first.id.clone(), status));
        if status == Queued {
            claim(&mut q);
        }
    }
    applied(
        q.finish(
            &Lease {
                id: first.id.clone(),
                claim: 1,
            },
            Outcome::Done(Value::Null),
            4,
        )
        .unwrap(),
    );
    assert_ne!(new_item(&mut q, p()).id, first.id);
}

#[test]
fn coalesce_replaces_queued_payload_and_inserts_while_running() {
    let (_dir, mut q) = db();
    let p = |who: &str, n| Push {
        payload: [("n".into(), json!(n))].into_iter().collect(),
        coalesce_key: Some("key".into()),
        requester: Some(who.into()),
        ..request()
    };
    let first = new_item(&mut q, p("alice", 1));
    let Pushed::Coalesced(item) = q.push(p("bob", 2), 2).unwrap() else {
        panic!("expected coalesce")
    };
    assert_eq!(item.id, first.id);
    assert_eq!(item.payload["n"], 2);
    assert_eq!(item.waiters, ["alice", "bob"]);
    assert_eq!(claim(&mut q).payload["n"], 2);
    assert_ne!(new_item(&mut q, p("carol", 3)).id, first.id);
    assert_eq!(q.item(&first.id).unwrap().unwrap().0.payload["n"], 2);
}

#[test]
fn max_depth_refuses_with_actual_depth() {
    let (_dir, mut q) = db();
    new_item(&mut q, request());
    claim(&mut q);
    assert!(matches!(
        q.push(
            Push {
                max_depth: Some(1),
                ..request()
            },
            3
        ),
        Err(QueueError::Full { depth: 1 })
    ));
}

#[test]
fn empty_claim_returns_none() {
    let (_dir, mut q) = db();
    assert!(q.claim("build", 10, 2).unwrap().is_none());
}

#[test]
fn concurrent_connections_claim_fifty_items_once_each() {
    let (dir, mut q) = db();
    let expected: std::collections::BTreeSet<_> =
        (0..50).map(|_| new_item(&mut q, request()).id).collect();
    let path = dir.path().join("queue.db");
    let barrier = std::sync::Barrier::new(2);
    let claimed = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    let mut conn = Queue::open(&path).unwrap();
                    barrier.wait();
                    let mut ids = Vec::new();
                    while let Some(item) = conn.claim("build", 10, 2).unwrap() {
                        ids.push(item.0.id);
                    }
                    ids
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(claimed.len(), 50);
    assert_eq!(
        claimed
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
}

#[test]
fn expired_lease_requeues_and_counts_attempt() {
    let (_dir, mut q) = db();
    let item = new_item(&mut q, request());
    claim(&mut q);
    assert!(q.reap(11).unwrap().is_empty());
    let reaped = q.reap(12).unwrap();
    assert_eq!((reaped[0].status, reaped[0].attempts), (Queued, 1));
    assert_eq!(reaped[0].id, item.id);
    assert_eq!(q.claim("build", 10, 13).unwrap().unwrap().0.attempts, 2);
}

#[test]
fn one_failure_exhausts_one_attempt() {
    let (_dir, mut q) = db();
    let item = new_item(
        &mut q,
        Push {
            max_attempts: 1,
            ..request()
        },
    );
    let running = claim(&mut q);
    assert_eq!(running.id, item.id);
    let dead = applied(
        q.finish(&lease(&running), Outcome::Failed("boom".into()), 3)
            .unwrap(),
    );
    assert_eq!(dead.status, Dead);
    assert_eq!(dead.result, Some(json!("boom")));
}

#[test]
fn three_attempts_allow_exactly_three_claims() {
    let (_dir, mut q) = db();
    let item = new_item(&mut q, request());
    for (attempt, expected) in [(1, Queued), (2, Queued), (3, Dead)] {
        let running = claim(&mut q);
        assert_eq!(
            (running.id.as_str(), running.attempts),
            (item.id.as_str(), attempt)
        );
        assert_eq!(
            applied(
                q.finish(&lease(&running), Outcome::Failed("boom".into()), 3)
                    .unwrap()
            )
            .status,
            expected
        );
    }
    assert!(q.claim("build", 10, 4).unwrap().is_none());
}

#[test]
fn release_decrements_attempts() {
    let (_dir, mut q) = db();
    let item = new_item(
        &mut q,
        Push {
            max_attempts: 1,
            ..request()
        },
    );
    let running = claim(&mut q);
    applied(q.release(&lease(&running), 3).unwrap());
    let (released, _) = q.item(&item.id).unwrap().unwrap();
    assert_eq!(
        (released.status, released.attempts, released.lease_until),
        (Queued, 0, None)
    );
    assert_eq!(q.claim("build", 10, 4).unwrap().unwrap().0.attempts, 1);
}

#[test]
fn cancel_and_retry_reset_items() {
    let (_dir, mut q) = db();
    for running in [false, true] {
        let item = new_item(&mut q, request());
        if running {
            claim(&mut q);
        }
        assert_eq!(q.cancel(&item.id, 3).unwrap().status, Cancelled);
        let retried = q.retry(&item.id, 4).unwrap();
        assert_eq!(
            (retried.status, retried.attempts, retried.finished_at),
            (Queued, 0, None)
        );
        let running = claim(&mut q);
        applied(
            q.finish(&lease(&running), Outcome::Done(json!(true)), 5)
                .unwrap(),
        );
    }
}

#[test]
fn heartbeat_and_finish_are_stale_after_cancel() {
    let (_dir, mut q) = db();
    let item = new_item(&mut q, request());
    let running = claim(&mut q);
    let lease = lease(&running);
    applied(q.heartbeat(&lease, 20, 3).unwrap());
    assert_eq!(q.item(&item.id).unwrap().unwrap().0.lease_until, Some(23));
    q.cancel(&item.id, 4).unwrap();
    let cancelled = q.item(&item.id).unwrap();
    assert_eq!(q.heartbeat(&lease, 20, 5).unwrap(), Fenced::Stale);
    assert_eq!(
        q.finish(&lease, Outcome::Done(Value::Null), 5).unwrap(),
        Fenced::Stale
    );
    assert_eq!(q.item(&item.id).unwrap(), cancelled);
}

#[test]
fn heartbeat_rejects_invalid_duration_before_fencing() {
    let (_dir, mut q) = db();
    let item = new_item(&mut q, request());
    let (_, lease) = q.claim("build", 10, 2).unwrap().unwrap();
    assert!(matches!(
        q.heartbeat(&lease, 0, 3),
        Err(QueueError::InvalidLease)
    ));
    assert_eq!(q.item(&item.id).unwrap().unwrap().0.lease_until, Some(12));

    q.cancel(&item.id, 4).unwrap();
    assert!(matches!(
        q.heartbeat(&lease, 0, 5),
        Err(QueueError::InvalidLease)
    ));
}

#[test]
fn every_transition_appends_an_ordered_event() {
    let (_dir, mut q) = db();
    let p = || Push {
        coalesce_key: Some("key".into()),
        ..request()
    };
    let item = new_item(
        &mut q,
        Push {
            dedupe_key: Some("d".into()),
            ..p()
        },
    );
    q.push(
        Push {
            dedupe_key: Some("d".into()),
            ..p()
        },
        2,
    )
    .unwrap();
    q.push(p(), 3).unwrap();
    let running = claim(&mut q);
    applied(q.set_run(&lease(&running), "run-1", 4).unwrap());
    applied(q.heartbeat(&lease(&running), 10, 4).unwrap());
    applied(q.release(&lease(&running), 5).unwrap());
    let released = q.item(&item.id).unwrap().unwrap().0;
    assert_eq!(
        (released.run_id, released.started_at, released.lease_until),
        (None, None, None)
    );
    claim(&mut q);
    q.reap(12).unwrap();
    let running = claim(&mut q);
    applied(q.set_run(&lease(&running), "run-2", 12).unwrap());
    let queued = applied(
        q.finish(&lease(&running), Outcome::Failed("first".into()), 13)
            .unwrap(),
    );
    assert_eq!(
        (queued.run_id, queued.started_at, queued.lease_until),
        (None, None, None)
    );
    claim(&mut q);
    q.reap(12).unwrap();
    q.retry(&item.id, 14).unwrap();
    q.cancel(&item.id, 15).unwrap();
    q.retry(&item.id, 16).unwrap();
    let running = claim(&mut q);
    applied(
        q.finish(&lease(&running), Outcome::Done(json!({"ok": true})), 17)
            .unwrap(),
    );
    let (done, events) = q.item(&item.id).unwrap().unwrap();
    assert_eq!(done.result, Some(json!({"ok": true})));
    let run_started_times: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "run_started")
        .map(|event| event.at)
        .collect();
    assert_eq!(run_started_times, [4, 12]);
    let expected = [
        ("enqueued", None),
        ("deduped", None),
        ("coalesced", None),
        ("claimed", Some(json!({"attempt": 1, "claim": 1}))),
        (
            "run_started",
            Some(json!({"attempt": 1, "claim": 1, "run_id": "run-1"})),
        ),
        ("released", Some(json!({"attempt": 1, "claim": 1}))),
        ("claimed", Some(json!({"attempt": 1, "claim": 2}))),
        ("stalled", Some(json!({"attempt": 1, "claim": 2}))),
        ("claimed", Some(json!({"attempt": 2, "claim": 3}))),
        (
            "run_started",
            Some(json!({"attempt": 2, "claim": 3, "run_id": "run-2"})),
        ),
        (
            "failed",
            Some(json!({"attempt": 2, "claim": 3, "error": "first"})),
        ),
        ("requeued", None),
        ("claimed", Some(json!({"attempt": 3, "claim": 4}))),
        ("stalled", Some(json!({"attempt": 3, "claim": 4}))),
        ("retried", None),
        ("cancelled", None),
        ("retried", None),
        ("claimed", Some(json!({"attempt": 1, "claim": 5}))),
        ("done", Some(json!({"attempt": 1, "claim": 5}))),
    ];
    assert_eq!(
        events
            .iter()
            .map(|event| (event.kind.as_str(), event.data.clone()))
            .collect::<Vec<_>>(),
        expected
    );
    assert!(events.iter().zip(1..).all(|(e, seq)| e.seq == seq));
}

#[test]
fn prune_keeps_unfinished_and_cascades_events() {
    let (dir, mut q) = db();
    let done = new_item(&mut q, request());
    let claimed = claim(&mut q);
    applied(
        q.finish(&lease(&claimed), Outcome::Done(Value::Null), 3)
            .unwrap(),
    );
    let running = new_item(&mut q, request());
    claim(&mut q);
    let queued = new_item(&mut q, request());
    let recent = new_item(&mut q, request());
    q.cancel(&recent.id, 4).unwrap();
    assert_eq!(q.prune(4).unwrap(), 1);
    assert!(q.item(&done.id).unwrap().is_none());
    for id in [running.id, queued.id, recent.id] {
        assert!(q.item(&id).unwrap().is_some());
    }
    let conn = rusqlite::Connection::open(dir.path().join("queue.db")).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM queue_events WHERE item_id = ?",
            [&done.id],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn newer_schema_is_refused_without_overwriting_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queue.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", 3).unwrap();
    assert!(matches!(
        Queue::open(&path),
        Err(QueueError::UnsupportedSchema { version: 3 })
    ));
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn reaped_lease_cannot_mutate_reclaimed_item() {
    let (_dir, mut q) = db();
    let item = new_item(&mut q, request());
    let (_, old) = q.claim("build", 10, 2).unwrap().unwrap();
    applied(q.set_run(&old, "old-run", 3).unwrap());
    let reaped = q.reap(12).unwrap();
    assert_eq!(
        (
            &reaped[0].run_id,
            reaped[0].lease_until,
            reaped[0].started_at
        ),
        (&None, None, None)
    );
    let (_, current) = q.claim("build", 10, 13).unwrap().unwrap();
    assert_eq!(current.claim, 2);
    let snapshot = q.item(&item.id).unwrap();
    assert_eq!(
        q.finish(&old, Outcome::Done(Value::Null), 14).unwrap(),
        Fenced::Stale
    );
    assert_eq!(q.heartbeat(&old, 50, 14).unwrap(), Fenced::Stale);
    assert_eq!(q.set_run(&old, "stale-run", 14).unwrap(), Fenced::Stale);
    assert_eq!(q.release(&old, 14).unwrap(), Fenced::Stale);
    assert_eq!(q.item(&item.id).unwrap(), snapshot);
}

#[test]
fn coalesce_prefers_queued_item_over_older_running_match() {
    let (_dir, mut q) = db();
    let p = |n| Push {
        coalesce_key: Some("key".into()),
        requester: Some(format!("requester-{n}")),
        payload: [("n".into(), json!(n))].into_iter().collect(),
        ..request()
    };
    let running = new_item(&mut q, p(1));
    claim(&mut q);
    let waiting = new_item(&mut q, p(2));
    for n in [3, 4] {
        let Pushed::Coalesced(item) = q.push(p(n), 3).unwrap() else {
            panic!("expected coalesce")
        };
        assert_eq!(item.id, waiting.id);
    }
    let queued = q.list(Some("build"), Some(Queued), 10).unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].id, waiting.id);
    assert_eq!(queued[0].payload["n"], 4);
    assert_eq!(
        queued[0].waiters,
        ["requester-2", "requester-3", "requester-4"]
    );
    assert_eq!(q.item(&running.id).unwrap().unwrap().0.payload["n"], 1);
}

#[test]
fn concurrent_first_opens_initialize_one_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queue.db");
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    Queue::open(&path).map(|_| ())
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
    });
    let conn = rusqlite::Connection::open(path).unwrap();
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
}

#[test]
fn release_and_retry_never_reuse_a_fencing_token() {
    for retry in [false, true] {
        let (_dir, mut q) = db();
        let item = new_item(&mut q, request());
        let (first, old) = q.claim("build", 10, 2).unwrap().unwrap();
        if retry {
            q.cancel(&item.id, 3).unwrap();
            q.retry(&item.id, 4).unwrap();
        } else {
            applied(q.release(&old, 3).unwrap());
        }
        let (second, current) = q.claim("build", 10, 5).unwrap().unwrap();
        assert_eq!(first.attempts, second.attempts);
        let snapshot = q.item(&item.id).unwrap();
        assert_eq!(
            q.finish(&old, Outcome::Done(Value::Null), 6).unwrap(),
            Fenced::Stale
        );
        assert_eq!(q.heartbeat(&old, 10, 6).unwrap(), Fenced::Stale);
        assert_eq!(q.set_run(&old, "old", 6).unwrap(), Fenced::Stale);
        assert_eq!(q.release(&old, 6).unwrap(), Fenced::Stale);
        assert_eq!(q.item(&item.id).unwrap(), snapshot);
        applied(q.finish(&current, Outcome::Done(Value::Null), 7).unwrap());
    }
}

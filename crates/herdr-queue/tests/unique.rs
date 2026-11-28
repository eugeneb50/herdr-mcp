#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "fixture setup and queue operations must succeed"
)]
//! `push_unique`: a key is reserved across every queue and status, and a
//! delivery accepts one reply.

use herdr_queue::{Outcome, Push, Pushed, Queue, QueueError};
use serde_json::{Value, json};
use std::{sync::Barrier, thread};
use tempfile::tempdir;

fn item(queue: &str, key: &str, payload: Value) -> Push {
    Push {
        queue: queue.into(),
        payload: payload.as_object().unwrap().clone(),
        dedupe_key: Some(key.into()),
        coalesce_key: None,
        requester: None,
        max_attempts: 2,
        max_depth: None,
    }
}

fn new_id(pushed: Pushed) -> String {
    match pushed {
        Pushed::New(item) => item.id,
        other => panic!("expected a new item, got {other:?}"),
    }
}

#[test]
fn a_key_taken_in_another_queue_is_refused() {
    let mut q = Queue::open(":memory:").unwrap();
    new_id(
        q.push_unique(item("mail-a", "kalabo", json!({})), 1)
            .unwrap(),
    );
    let err = q
        .push_unique(item("mail-b", "kalabo", json!({})), 2)
        .unwrap_err();
    assert!(matches!(err, QueueError::KeyTaken { .. }), "{err:?}");
}

#[test]
fn a_key_of_a_finished_item_stays_reserved() {
    let mut q = Queue::open(":memory:").unwrap();
    new_id(
        q.push_unique(item("mail-a", "kalabo", json!({})), 1)
            .unwrap(),
    );
    let (_, lease) = q.claim("mail-a", 100, 2).unwrap().unwrap();
    q.finish(&lease, Outcome::Done(Value::Null), 3).unwrap();
    let err = q
        .push_unique(item("mail-a", "kalabo", json!({})), 4)
        .unwrap_err();
    assert!(matches!(err, QueueError::KeyTaken { .. }), "{err:?}");
    // A refused push leaves no trace on the item that owns the key.
    let (owner, _) = q.find_key("kalabo").unwrap().pop().unwrap();
    let (_, events) = q.item(&owner.id).unwrap().unwrap();
    assert!(events.iter().all(|e| e.kind != "deduped"), "{events:?}");
}

#[test]
fn a_second_reply_to_one_delivery_is_refused() {
    let mut q = Queue::open(":memory:").unwrap();
    let reply = |key: &str| item("mail-lead", key, json!({"re": "task1", "delivery": "d1"}));
    new_id(q.push_unique(reply("rotisu"), 1).unwrap());
    let err = q.push_unique(reply("tirumo"), 2).unwrap_err();
    match err {
        QueueError::AlreadyAnswered { reply } => assert_eq!(reply, "rotisu"),
        other => panic!("expected AlreadyAnswered, got {other:?}"),
    }
    // Another delivery of the same task, and a note without a delivery, are not replies to d1.
    new_id(
        q.push_unique(
            item(
                "mail-lead",
                "sofamu",
                json!({"re": "task1", "delivery": "d2"}),
            ),
            3,
        )
        .unwrap(),
    );
    new_id(
        q.push_unique(item("mail-lead", "hilama", json!({"re": "task1"})), 4)
            .unwrap(),
    );
}

#[test]
fn the_answered_rule_holds_after_the_reply_is_read() {
    let mut q = Queue::open(":memory:").unwrap();
    let reply = |key: &str| item("mail-lead", key, json!({"re": "task1", "delivery": "d1"}));
    new_id(q.push_unique(reply("rotisu"), 1).unwrap());
    let (_, lease) = q.claim("mail-lead", 100, 2).unwrap().unwrap();
    q.finish(&lease, Outcome::Done(Value::Null), 3).unwrap();
    let err = q.push_unique(reply("tirumo"), 4).unwrap_err();
    assert!(matches!(err, QueueError::AlreadyAnswered { .. }), "{err:?}");
}

#[test]
fn racing_replies_to_one_delivery_commit_once() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("q.db");
    Queue::open(&path).unwrap();
    let barrier = Barrier::new(2);
    let results: Vec<_> = thread::scope(|s| {
        let handles: Vec<_> = ["rotisu", "tirumo"]
            .into_iter()
            .map(|key| {
                let (path, barrier) = (&path, &barrier);
                s.spawn(move || {
                    let mut q = Queue::open(path).unwrap();
                    barrier.wait();
                    q.push_unique(
                        item("mail-lead", key, json!({"re": "task1", "delivery": "d1"})),
                        1,
                    )
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "{results:?}"
    );
    assert!(
        results
            .iter()
            .any(|r| matches!(r, Err(QueueError::AlreadyAnswered { .. }))),
        "{results:?}"
    );
}

#[test]
fn find_key_returns_the_item_and_its_events_in_any_queue() {
    let mut q = Queue::open(":memory:").unwrap();
    new_id(
        q.push_unique(item("mail-b", "kalabo", json!({"n": 1})), 1)
            .unwrap(),
    );
    let hits = q.find_key("kalabo").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.queue, "mail-b");
    assert_eq!(hits[0].1[0].kind, "enqueued");
    assert!(q.find_key("missing").unwrap().is_empty());
}

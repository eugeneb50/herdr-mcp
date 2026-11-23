#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "fixture setup and queue operations must succeed"
)]

use herdr_queue::{AuditError, Fenced, ItemStatus, Outcome, Push, Pushed, Queue, run_audit};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{process::Command, sync::Barrier, thread};
use tempfile::tempdir;

fn push(queue: &mut Queue, mailbox: &str, payload: Value, at: i64) -> String {
    let Pushed::New(item) = queue
        .push(
            Push {
                queue: mailbox.into(),
                payload: payload.as_object().unwrap().clone(),
                dedupe_key: None,
                coalesce_key: None,
                requester: None,
                max_attempts: 3,
                max_depth: None,
            },
            at,
        )
        .unwrap()
    else {
        panic!("expected new item")
    };
    item.id
}

#[test]
fn migrates_v1_without_losing_items_or_events() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("queue.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(include_str!("schema-v1.sql")).unwrap();
    db.execute_batch(
        "
        PRAGMA user_version = 1;
        INSERT INTO queue_items (id, queue, seq, status, max_attempts, payload, enqueued_at)
          VALUES ('old', 'mail-a', 1, 'queued', 3, '{\"re\":\"original\",\"delivery\":\"d\"}', 1);
        INSERT INTO queue_events (item_id, seq, at, kind) VALUES ('old', 1, 1, 'enqueued');
    ",
    )
    .unwrap();
    drop(db);
    assert!(run_audit(&path, None, None).is_ok());
    let mut queue = Queue::open(&path).unwrap();
    let (item, events) = queue.item("old").unwrap().unwrap();
    assert_eq!(item.delivery, None);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "enqueued");
    assert_eq!(
        queue.find_reply("original", "d").unwrap().unwrap().id,
        "old"
    );
    let (_, lease) = queue.claim("mail-a", 10, 2).unwrap().unwrap();
    assert_eq!(
        queue.set_delivery(&lease, "new", json!({}), 3).unwrap(),
        Fenced::Applied(())
    );
    assert!(
        queue
            .claim_consumer("mail-a", "worker", 10, 3)
            .unwrap()
            .is_some()
    );
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn delivery_records_metadata_and_fences_previous_claim() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("queue.db");
    let mut queue = Queue::open(&path).unwrap();
    let id = push(&mut queue, "mail-a", json!({}), 1);
    let (_, old) = queue.claim("mail-a", 10, 2).unwrap().unwrap();
    queue.release(&old, 3).unwrap();
    let (_, current) = queue.claim("mail-a", 10, 4).unwrap().unwrap();
    let details = json!({"pane":"w1:p7","state_change_seq":42,"claim":-1,"delivery":"wrong"});
    assert_eq!(
        queue.set_delivery(&current, "d1", details, 5).unwrap(),
        Fenced::Applied(())
    );
    let before = queue.item(&id).unwrap().unwrap();
    assert!(matches!(
        queue.set_delivery(&current, "invalid", Value::Null, 6),
        Err(herdr_queue::QueueError::InvalidDeliveryData)
    ));
    assert_eq!(
        queue.set_delivery(&old, "stale", json!({}), 6).unwrap(),
        Fenced::Stale
    );
    assert_eq!(queue.item(&id).unwrap().unwrap(), before);
    assert_eq!(before.0.delivery.as_deref(), Some("d1"));
    let event = before.1.last().unwrap();
    assert_eq!(event.kind, "delivered");
    assert_eq!(
        event.data,
        Some(
            json!({"pane":"w1:p7","state_change_seq":42,"attempt":1,"claim":current.claim,"delivery":"d1"})
        )
    );
    assert!(run_audit(&path, None, None).is_ok());
}

#[test]
fn terminal_dead_leaves_attempts_and_writes_one_event() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("queue.db");
    let mut queue = Queue::open(&path).unwrap();
    let id = push(&mut queue, "mail-a", json!({}), 1);
    let (_, lease) = queue.claim("mail-a", 10, 2).unwrap().unwrap();
    let Fenced::Applied(item) = queue
        .finish(&lease, Outcome::Dead("unanswered".into()), 3)
        .unwrap()
    else {
        panic!("expected live lease")
    };
    assert_eq!(item.status, ItemStatus::Dead);
    assert_eq!(item.attempts, 1);
    assert_eq!(item.result, Some(json!("unanswered")));
    assert!(queue.claim("mail-a", 10, 4).unwrap().is_none());
    let events = queue.item(&id).unwrap().unwrap().1;
    assert_eq!(events.len(), 3);
    assert_eq!(events[2].kind, "dead");
    assert_eq!(
        events[2].data,
        Some(json!({"attempt":1,"claim":lease.claim,"error":"unanswered"}))
    );
    assert!(run_audit(&path, None, None).is_ok());
}

#[test]
fn lookup_returns_oldest_exact_match_across_mailboxes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("queue.db");
    let mut queue = Queue::open(&path).unwrap();
    push(
        &mut queue,
        "mail-a",
        json!({"re":"message","delivery":"other"}),
        1,
    );
    push(
        &mut queue,
        "mail-b",
        json!({"re":"other","delivery":"d"}),
        2,
    );
    let first = push(
        &mut queue,
        "mail-b",
        json!({"re":"message","delivery":"d"}),
        3,
    );
    push(
        &mut queue,
        "mail-a",
        json!({"re":"message","delivery":"d"}),
        3,
    );
    assert_eq!(queue.find_reply("message", "d").unwrap().unwrap().id, first);
    assert!(queue.find_reply("missing", "d").unwrap().is_none());
}

#[test]
fn consumer_race_renewal_expiry_and_release_fence_old_owners() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("queue.db");
    let mut queue = Queue::open(&path).unwrap();
    let id = push(&mut queue, "mail-a", json!({}), 1);
    let barrier = Barrier::new(2);
    let claims = thread::scope(|scope| {
        let claim = |owner| {
            barrier.wait();
            let mut queue = Queue::open(&path).unwrap();
            queue.claim_consumer("mail-a", owner, 10, 2).unwrap()
        };
        let a = scope.spawn(move || claim("a"));
        let b = scope.spawn(move || claim("b"));
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(claims.iter().flatten().count(), 1);
    let first = claims.into_iter().flatten().next().unwrap();
    let renewed = queue
        .claim_consumer("mail-a", &first.owner, 10, 3)
        .unwrap()
        .unwrap();
    assert_eq!(renewed, first);
    let other = if first.owner == "a" { "b" } else { "a" };
    assert!(
        queue
            .claim_consumer("mail-a", other, 10, 12)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        queue.claim_owned(&first, 10, 13).unwrap(),
        Fenced::Stale
    ));
    let new = queue
        .claim_consumer("mail-a", other, 10, 13)
        .unwrap()
        .unwrap();
    assert_eq!(new.token, first.token + 1);
    assert!(matches!(
        queue.claim_owned(&first, 10, 14).unwrap(),
        Fenced::Stale
    ));
    assert_eq!(queue.release_consumer(&first).unwrap(), Fenced::Stale);
    let Fenced::Applied(Some((item, _))) = queue.claim_owned(&new, 10, 14).unwrap() else {
        panic!("expected owned claim")
    };
    assert_eq!(item.id, id);
    assert!(matches!(
        queue.claim_owned(&new, 10, 15).unwrap(),
        Fenced::Applied(None)
    ));
    assert_eq!(queue.release_consumer(&new).unwrap(), Fenced::Applied(()));
    assert_eq!(queue.release_consumer(&new).unwrap(), Fenced::Stale);
    let reacquired = queue
        .claim_consumer("mail-a", other, 10, 15)
        .unwrap()
        .unwrap();
    assert_eq!(reacquired.token, new.token + 1);
    let after_gap = queue
        .claim_consumer("mail-a", other, 10, 25)
        .unwrap()
        .unwrap();
    assert_eq!(after_gap.token, reacquired.token + 1);
    assert!(matches!(
        queue.claim_owned(&reacquired, 10, 25).unwrap(),
        Fenced::Stale
    ));
}

fn audit_rejects_corruption(sql: &str, delivered: bool) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("queue.db");
    let mut queue = Queue::open(&path).unwrap();
    push(&mut queue, "mail-a", json!({}), 1);
    if delivered {
        let (_, lease) = queue.claim("mail-a", 10, 2).unwrap().unwrap();
        queue.set_delivery(&lease, "real", json!({}), 3).unwrap();
    }
    Connection::open(&path).unwrap().execute_batch(sql).unwrap();
    assert!(matches!(
        run_audit(&path, None, None),
        Err(AuditError::Violations(_))
    ));
}

#[test]
fn audit_flags_delivery_on_a_queued_item() {
    audit_rejects_corruption(
        "INSERT INTO queue_events SELECT id, 2, 2, 'delivered', '{\"delivery\":\"real\"}' FROM queue_items; UPDATE queue_items SET delivery='real'",
        false,
    );
}

#[test]
fn audit_flags_dead_row_without_terminal_event() {
    audit_rejects_corruption(
        "DELETE FROM queue_events; UPDATE queue_items SET status='dead', attempts=max_attempts",
        false,
    );
}

#[test]
fn audit_flags_delivery_that_disagrees_with_last_event() {
    audit_rejects_corruption("UPDATE queue_items SET delivery='different'", true);
}

#[test]
fn cli_lists_consumers_and_delivery_history_as_json() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("queue.db");
    let mut queue = Queue::open(&path).unwrap();
    let id = push(&mut queue, "mail-a", json!({}), 1);
    let owner = queue
        .claim_consumer("mail-a", "me", 10, 2)
        .unwrap()
        .unwrap();
    let Fenced::Applied(Some((_, lease))) = queue.claim_owned(&owner, 10, 3).unwrap() else {
        panic!("expected owned claim")
    };
    queue.set_delivery(&lease, "d", json!({}), 4).unwrap();
    let invoke = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_herdr-queue"))
            .arg("--db")
            .arg(&path)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    assert_eq!(
        invoke(&["consumers"]),
        json!([{"queue":"mail-a","owner":"me","token":owner.token,"until":12}])
    );
    assert_eq!(invoke(&["list"])[0]["delivery"], "d");
    assert_eq!(invoke(&["events", "--id", &id])[2]["data"]["delivery"], "d");
}

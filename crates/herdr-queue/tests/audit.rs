#![allow(
    clippy::unwrap_used,
    reason = "test fixtures build temp DBs; a failure there fails the test"
)]

use herdr_queue::{Item, ItemStatus, QueueEvent, run_audit};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use tempfile::TempDir;

fn schema_path() -> PathBuf {
    std::env::var("CARGO_MANIFEST_DIR")
        .map(|d| PathBuf::from(d).join("src/schema.sql"))
        .unwrap_or_else(|_| PathBuf::from("src/schema.sql"))
}

fn ss(s: ItemStatus) -> &'static str {
    match s {
        ItemStatus::Queued => "queued",
        ItemStatus::Running => "running",
        ItemStatus::Done => "done",
        ItemStatus::Dead => "dead",
        ItemStatus::Cancelled => "cancelled",
    }
}

fn item(
    iid: &str,
    status: ItemStatus,
    attempts: u32,
    max_attempts: u32,
    lease_until: Option<i64>,
) -> Item {
    Item {
        id: iid.into(),
        queue: "build".into(),
        seq: 1,
        status,
        attempts,
        max_attempts,
        claims: 0,
        dedupe_key: None,
        coalesce_key: None,
        payload: Map::new(),
        requester: None,
        waiters: vec![],
        run_id: None,
        delivery: None,
        lease_until,
        result: None,
        enqueued_at: 1000,
        started_at: None,
        finished_at: None,
    }
}

fn ev(iid: &str, seq: i64, kind: &str, data: Option<Value>) -> QueueEvent {
    QueueEvent {
        item_id: iid.into(),
        seq,
        at: seq,
        kind: kind.to_string(),
        data,
    }
}

fn run(items: Vec<Item>, events: Vec<QueueEvent>, want: i32, driver: Option<&PathBuf>) {
    let schema = std::fs::read_to_string(schema_path()).expect("read schema");
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("queue.db");
    let conn = rusqlite::Connection::open(&path).expect("open temp db");
    conn.execute_batch(&schema).expect("create schema");
    for it in &items {
        let result_str = it.result.as_ref().map(|v| v.to_string());
        conn.execute(
            "INSERT INTO queue_items(id,queue,seq,status,attempts,max_attempts,dedupe_key,coalesce_key,payload,requester,waiters,run_id,lease_until,result,enqueued_at,started_at,finished_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            rusqlite::params![it.id, it.queue, it.seq, ss(it.status), it.attempts, it.max_attempts, it.dedupe_key, it.coalesce_key, serde_json::to_string(&it.payload).unwrap(), it.requester, serde_json::to_string(&it.waiters).unwrap(), it.run_id, it.lease_until, result_str, it.enqueued_at, it.started_at, it.finished_at],
        ).unwrap();
    }
    for ev in &events {
        let data_str = ev.data.as_ref().map(|v| v.to_string());
        conn.execute(
            "INSERT INTO queue_events(item_id,seq,at,kind,data) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![ev.item_id, ev.seq, ev.at, ev.kind, data_str],
        )
        .unwrap();
    }
    drop(conn);
    let rc = run_audit(&path, None, driver.map(|p| p.as_path()));
    assert_eq!(rc.is_ok(), want == 0, "expected rc={want}, got {rc:?}");
}

fn full_item(
    iid: &str,
    status: ItemStatus,
    attempts: u32,
    max_attempts: u32,
    lease_until: Option<i64>,
) -> Item {
    Item {
        id: iid.into(),
        queue: "build".into(),
        seq: 1,
        status,
        attempts,
        max_attempts,
        claims: 0,
        dedupe_key: None,
        coalesce_key: None,
        payload: Map::new(),
        requester: None,
        waiters: vec![],
        run_id: None,
        delivery: None,
        lease_until,
        result: None,
        enqueued_at: 1000,
        started_at: None,
        finished_at: None,
    }
}

#[test]
fn valid_history_exercises_every_event_kind() {
    run(
        vec![item("v1", ItemStatus::Done, 3, 3, None)],
        vec![
            ev("v1", 1, "enqueued", None),
            ev("v1", 2, "claimed", Some(json!({"attempt": 1, "claim": 1}))),
            ev(
                "v1",
                3,
                "run_started",
                Some(json!({"attempt": 1, "claim": 1, "run_id": "r1"})),
            ),
            ev(
                "v1",
                4,
                "failed",
                Some(json!({"attempt": 1, "claim": 1, "error": "boom"})),
            ),
            ev("v1", 5, "requeued", None),
            ev("v1", 6, "claimed", Some(json!({"attempt": 2, "claim": 2}))),
            ev("v1", 7, "stalled", Some(json!({"attempt": 2, "claim": 2}))),
            ev("v1", 8, "claimed", Some(json!({"attempt": 3, "claim": 3}))),
            ev("v1", 9, "done", Some(json!({"attempt": 3, "claim": 3}))),
        ],
        0,
        None,
    );
}

#[test]
fn done_without_claimed_is_violation() {
    run(
        vec![item("b1", ItemStatus::Done, 0, 3, None)],
        vec![ev("b1", 1, "enqueued", None), ev("b1", 2, "done", None)],
        1,
        None,
    );
}

#[test]
fn attempt_skipped_is_violation() {
    run(
        vec![item("b2", ItemStatus::Running, 3, 3, None)],
        vec![
            ev("b2", 1, "enqueued", None),
            ev("b2", 2, "claimed", Some(json!({"attempt": 1, "claim": 1}))),
            ev("b2", 3, "claimed", Some(json!({"attempt": 3, "claim": 3}))),
        ],
        1,
        None,
    );
}

#[test]
fn running_without_lease_until_is_violation() {
    run(
        vec![item("b3", ItemStatus::Running, 1, 3, None)],
        vec![
            ev("b3", 1, "enqueued", None),
            ev("b3", 2, "claimed", Some(json!({"attempt": 1, "claim": 1}))),
        ],
        1,
        None,
    );
}

#[test]
fn duplicate_coalesce_key_straight_from_enqueued_is_violation() {
    let mut c1 = full_item("c1", ItemStatus::Queued, 0, 3, None);
    c1.seq = 1;
    c1.coalesce_key = Some("k".into());
    let mut c2 = full_item("c2", ItemStatus::Queued, 0, 3, None);
    c2.seq = 2;
    c2.coalesce_key = Some("k".into());
    run(
        vec![c1, c2],
        vec![ev("c1", 1, "enqueued", None), ev("c2", 1, "enqueued", None)],
        1,
        None,
    );
}

#[test]
fn coalesce_key_ok_when_one_cycled_back() {
    let mut c1 = full_item("c1", ItemStatus::Queued, 0, 3, None);
    c1.seq = 1;
    c1.coalesce_key = Some("k".into());
    let mut c2 = full_item("c2", ItemStatus::Queued, 1, 3, None);
    c2.seq = 2;
    c2.coalesce_key = Some("k".into());
    run(
        vec![c1, c2],
        vec![
            ev("c1", 1, "enqueued", None),
            ev("c2", 1, "enqueued", None),
            ev("c2", 2, "claimed", Some(json!({"attempt": 1}))),
            ev("c2", 3, "requeued", None),
        ],
        0,
        None,
    );
}

#[test]
fn repeated_claim_is_violation() {
    run(
        vec![full_item("m1", ItemStatus::Running, 2, 3, Some(5000))],
        vec![
            ev("m1", 1, "enqueued", None),
            ev("m1", 2, "claimed", Some(json!({"attempt": 1, "claim": 1}))),
            ev("m1", 3, "claimed", Some(json!({"attempt": 2, "claim": 1}))),
        ],
        1,
        None,
    );
}

#[test]
fn increasing_claim_is_ok() {
    run(
        vec![full_item("m2", ItemStatus::Running, 2, 3, Some(5000))],
        vec![
            ev("m2", 1, "enqueued", None),
            ev("m2", 2, "claimed", Some(json!({"attempt": 1, "claim": 1}))),
            ev("m2", 3, "claimed", Some(json!({"attempt": 2, "claim": 2}))),
        ],
        0,
        None,
    );
}

#[test]
fn bad_data_is_violation_not_crash() {
    run(
        vec![full_item("d1", ItemStatus::Running, 1, 3, None)],
        vec![
            ev("d1", 1, "enqueued", None),
            ev("d1", 2, "claimed", Some(Value::String("garbage".into()))),
        ],
        1,
        None,
    );
}

#[test]
fn overlapping_executions_detected_with_claim_key() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("driver.jsonl");
    std::fs::write(&path, [
        json!({"t": 1, "worker": "w1", "op": "push", "item": "o1", "attempt": 1, "applied": true, "lease": 1}),
        json!({"t": 2, "worker": "w1", "op": "start", "item": "o1", "attempt": 1, "applied": true, "lease": 1}),
        json!({"t": 3, "worker": "w2", "op": "start", "item": "o1", "attempt": 2, "applied": true, "lease": 2}),
        json!({"t": 4, "worker": "w1", "op": "end", "item": "o1", "attempt": 1, "applied": true, "lease": 1}),
        json!({"t": 5, "worker": "w2", "op": "end", "item": "o1", "attempt": 2, "applied": true, "lease": 2}),
    ].iter().map(|v| v.to_string()).collect::<Vec<_>>().join("\n")).expect("write driver");
    run(
        vec![full_item("o1", ItemStatus::Done, 2, 3, None)],
        vec![
            ev("o1", 1, "enqueued", None),
            ev("o1", 2, "claimed", Some(json!({"attempt": 1, "claim": 1}))),
            ev("o1", 3, "claimed", Some(json!({"attempt": 2, "claim": 2}))),
            ev("o1", 4, "done", Some(json!({"attempt": 2, "claim": 2}))),
        ],
        1,
        Some(&path),
    );
}

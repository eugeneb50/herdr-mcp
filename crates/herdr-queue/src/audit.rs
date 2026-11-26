//! Offline audit oracle for herdr-queue histories.
//! Also verifies delivery history and terminal failures introduced in schema v2.

use rusqlite::{Connection, OpenFlags};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet},
    fmt, fs, io,
    path::Path,
};

#[derive(Debug)]
pub enum AuditError {
    Io(io::Error),
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Violations(usize),
}
impl fmt::Display for AuditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Sqlite(e) => write!(f, "sqlite error: {e}"),
            Self::Json(e) => write!(f, "json error: {e}"),
            Self::Violations(n) => write!(f, "audit failed with {n} violations"),
        }
    }
}
impl std::error::Error for AuditError {}

/// Columns used to reconcile the row with its event history.
struct Row {
    id: String,
    queue: String,
    status: String,
    attempts: i64,
    max_attempts: i64,
    dedupe_key: Option<String>,
    coalesce_key: Option<String>,
    run_id: Option<String>,
    lease_until: Option<i64>,
    delivery: Option<String>,
}
struct Event {
    seq: i64,
    kind: String,
    data: Option<String>,
}
fn bad(v: &mut Vec<String>, id: &str, seq: i64, st: Option<&str>, kind: &str, what: &str) {
    let line = format!(
        "item={id} seq={seq} state={} event={kind}: {what}",
        st.unwrap_or("None")
    );
    v.push(line);
}
fn rowbad(v: &mut Vec<String>, row: &Row, what: &str) {
    bad(v, &row.id, -1, Some(row.status.as_str()), "<row>", what);
}
fn dbad(v: &mut Vec<String>, id: &str, kind: &str, what: &str) {
    bad(v, id, -1, Some("<driver>"), kind, what);
}
/// Keys in queue DBs are plain ASCII; Python's repr uses single quotes.
fn py_repr(s: &str) -> String {
    format!("'{s}'")
}
fn num(v: Option<i64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "None".into())
}
/// Python's `parse_data`: falsy -> `{}`, invalid JSON or non-object -> `None`.
fn parse_data(raw: Option<&str>) -> Option<Map<String, Value>> {
    let raw = raw.unwrap_or("");
    if raw.is_empty() {
        return Some(Map::new());
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}
/// Mirrors Python's TRANS: (expected states, next state); `None` expected = special.
fn trans(kind: &str) -> (Option<&'static [&'static str]>, Option<&'static str>) {
    match kind {
        "enqueued" => (None, Some("queued")),
        "delivered" => (Some(&["running"]), Some("running")),
        "run_started" => (Some(&["running"]), Some("running")),
        "done" => (Some(&["running"]), Some("done")),
        "failed" => (Some(&["running"]), Some("running")),
        "requeued" => (Some(&["running"]), Some("queued")),
        "dead" => (Some(&["running"]), Some("dead")),
        "stalled" => (Some(&["running"]), None),
        "released" => (Some(&["running"]), Some("queued")),
        "cancelled" => (Some(&["queued", "running"]), Some("cancelled")),
        "retried" => (Some(&["dead", "cancelled"]), Some("queued")),
        _ => (None, None),
    }
}
/// Mirrors Python's MSG (the `claimed` entry is never reached in the oracle).
fn msg(kind: &str) -> &'static str {
    match kind {
        "claimed" => "expected queued, got %s",
        "cancelled" => "expected queued/running, got %s",
        "retried" => "expected dead/cancelled, got %s",
        _ => "expected running, got %s",
    }
}
fn fold(row: &Row, events: &[Event], v: &mut Vec<String>) -> (Option<&'static str>, i64, bool) {
    let mut status: Option<&'static str> = None;
    let mut attempts: i64 = 0;
    let ma = row.max_attempts;
    let mut last_claim: Option<i64> = None;
    let mut cycled = false;
    let mut last_delivery = None;
    let mut explicit_dead = false;
    for ev in events {
        let (kind, seq) = (ev.kind.as_str(), ev.seq);
        let data = match parse_data(ev.data.as_deref()) {
            Some(d) => d,
            None => {
                bad(v, &row.id, seq, status, kind, "data is not a JSON object");
                Map::new()
            }
        };
        let attempt = data.get("attempt").and_then(Value::as_i64);
        let claim = data.get("claim").and_then(Value::as_i64);
        if kind == "deduped" || kind == "coalesced" {
            continue;
        }
        let undeliverable =
            kind == "dead" && data.get("undeliverable").and_then(Value::as_bool) == Some(true);
        let (exp, nxt) = if undeliverable {
            (Some(&["queued"][..]), Some("dead"))
        } else {
            trans(kind)
        };
        match exp {
            None => {
                let known = matches!(
                    kind,
                    "claimed" | "stalled" | "released" | "retried" | "enqueued"
                );
                if !known {
                    bad(v, &row.id, seq, status, kind, "unknown event kind");
                    continue;
                }
                if kind == "enqueued" {
                    if status.is_some() {
                        bad(v, &row.id, seq, status, kind, "already enqueued");
                        continue;
                    }
                    status = Some("queued");
                    continue;
                }
            }
            Some(exp) => {
                if !status.is_some_and(|s| exp.contains(&s)) {
                    let template = if undeliverable {
                        "expected queued, got %s"
                    } else {
                        msg(kind)
                    };
                    let what = template.replacen("%s", status.unwrap_or("None"), 1);
                    bad(v, &row.id, seq, status, kind, &what);
                    continue;
                }
            }
        }
        match kind {
            "claimed" => {
                if let Some(c) = claim {
                    if let Some(lc) = last_claim
                        && c <= lc
                    {
                        let what = format!("claim {c} not > previous {lc}");
                        bad(v, &row.id, seq, status, kind, &what);
                    }
                    last_claim = Some(c);
                }
                if let Some(a) = attempt
                    && attempts + 1 != a
                {
                    let what = format!("attempt {a} != previous {attempts} + 1");
                    bad(v, &row.id, seq, status, kind, &what);
                }
                if let Some(a) = attempt {
                    attempts = a;
                }
                status = Some("running");
            }
            "requeued" | "released" | "retried" => {
                status = Some("queued");
                cycled = true;
                if kind == "released" {
                    attempts = attempt
                        .map(|a| a - 1)
                        .unwrap_or_else(|| attempts.saturating_sub(1));
                } else if kind == "retried" {
                    attempts = 0;
                }
            }
            "delivered" => {
                if let Some(delivery) = data.get("delivery").and_then(Value::as_str) {
                    last_delivery = Some(delivery.to_owned());
                } else {
                    bad(v, &row.id, seq, status, kind, "missing delivery ID");
                }
            }
            "dead" => {
                let terminal = if undeliverable {
                    attempt.is_none() && claim.is_none()
                } else {
                    attempt == Some(attempts) && claim.is_some() && claim == last_claim
                };
                explicit_dead = terminal && data.get("error").and_then(Value::as_str).is_some();
                if undeliverable && !explicit_dead {
                    bad(v, &row.id, seq, status, kind, "invalid undeliverable event");
                }
                status = nxt;
            }
            "stalled" => {
                explicit_dead = false;
                status = if attempts >= ma {
                    Some("dead")
                } else {
                    Some("queued")
                };
                if status == Some("queued") {
                    cycled = true;
                }
            }
            _ => status = nxt,
        }
        if attempts > ma {
            let what = format!("attempts {attempts} > max_attempts {ma}");
            bad(v, &row.id, seq, status, kind, &what);
        }
    }
    if row.delivery != last_delivery {
        rowbad(v, row, "delivery does not match the last delivered event");
    }
    if row.status == "dead" {
        if status.is_none() {
            rowbad(v, row, "dead row has no terminal event");
        }
        if row.attempts != row.max_attempts && !(status == Some("dead") && explicit_dead) {
            let what = format!(
                "dead at attempts {} != max_attempts {}",
                row.attempts, row.max_attempts
            );
            rowbad(v, row, &what);
        }
    }
    (status, attempts, cycled)
}

/// Applied driver executions per item, in first-seen item order.
type Interval = (Option<i64>, Option<i64>, Option<i64>);
fn check_driver(path: &Path, rows: &[Row], v: &mut Vec<String>) -> Result<(), AuditError> {
    let mut pushed: HashSet<String> = HashSet::new();
    let mut executions: HashMap<String, Vec<Interval>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for line in fs::read_to_string(path).map_err(AuditError::Io)?.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let rec: Value = serde_json::from_str(line).map_err(AuditError::Json)?;
        let op = rec.get("op").and_then(Value::as_str).unwrap_or("");
        let item = rec
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match op {
            "push" => {
                pushed.insert(item);
            }
            "start" | "end" if rec.get("applied").and_then(Value::as_bool).unwrap_or(false) => {
                let key = rec
                    .get("lease")
                    .or_else(|| rec.get("attempt"))
                    .and_then(Value::as_i64);
                let t = rec.get("t").and_then(Value::as_i64);
                let lst = executions.entry(item.clone()).or_default();
                if !order.contains(&item) {
                    order.push(item.clone());
                }
                if op == "start" {
                    lst.push((t, None, key));
                } else if let Some(e) = lst.iter_mut().find(|e| e.1.is_none() && e.2 == key) {
                    e.1 = t;
                } else {
                    lst.push((None, t, key));
                }
            }
            _ => {}
        }
    }
    let db_ids: HashSet<String> = rows.iter().map(|r| r.id.clone()).collect();
    let mut missing: Vec<&String> = pushed.difference(&db_ids).collect();
    missing.sort();
    for iid in missing {
        dbad(v, iid, "push", "pushed by driver but missing from DB");
    }
    let mut extra: Vec<&String> = db_ids.difference(&pushed).collect();
    extra.sort();
    for iid in extra {
        dbad(v, iid, "push", "in DB but not pushed by driver");
    }
    for item_id in &order {
        let Some(ex_list) = executions.get(item_id) else {
            continue;
        };
        let mut intervals: Vec<(i64, i64, Option<i64>)> = ex_list
            .iter()
            .filter_map(|(s, e, k)| match (s, e) {
                (Some(s), Some(e)) => Some((*s, *e, *k)),
                _ => None,
            })
            .collect();
        intervals.sort();
        for i in 0..intervals.len() {
            for j in (i + 1)..intervals.len() {
                let (a_start, a_end, a_key) = intervals[i];
                let (b_start, b_end, b_key) = intervals[j];
                if a_start < b_end && b_start < a_end {
                    let what = format!(
                        "overlapping applied executions key {} and {} [{},{}] and [{},{}]",
                        num(a_key),
                        num(b_key),
                        a_start,
                        a_end,
                        b_start,
                        b_end
                    );
                    dbad(v, item_id, "start/end", &what);
                }
            }
        }
    }
    Ok(())
}

pub fn run_audit(
    db_path: &Path,
    max_violations: Option<usize>,
    driver_path: Option<&Path>,
) -> Result<(), AuditError> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(AuditError::Sqlite)?;
    let has_delivery = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('queue_items') WHERE name = 'delivery')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(AuditError::Sqlite)?;
    let delivery = if has_delivery {
        "delivery"
    } else {
        "NULL AS delivery"
    };
    let mut rows: Vec<Row> = Vec::new();
    {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT id, queue, status, attempts, max_attempts, dedupe_key, coalesce_key,
                 run_id, lease_until, {delivery} FROM queue_items"
            ))
            .map_err(AuditError::Sqlite)?;
        let mapped = stmt
            .query_map([], |r| {
                Ok(Row {
                    id: r.get("id")?,
                    queue: r.get("queue")?,
                    status: r.get("status")?,
                    attempts: r.get("attempts")?,
                    max_attempts: r.get("max_attempts")?,
                    dedupe_key: r.get("dedupe_key")?,
                    coalesce_key: r.get("coalesce_key")?,
                    run_id: r.get("run_id")?,
                    lease_until: r.get("lease_until")?,
                    delivery: r.get("delivery")?,
                })
            })
            .map_err(AuditError::Sqlite)?;
        for row in mapped {
            rows.push(row.map_err(AuditError::Sqlite)?);
        }
    }
    let mut events: HashMap<String, Vec<Event>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT item_id, seq, kind, data FROM queue_events ORDER BY item_id, seq")
            .map_err(AuditError::Sqlite)?;
        let mapped = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>("item_id")?,
                    Event {
                        seq: r.get("seq")?,
                        kind: r.get("kind")?,
                        data: r.get("data")?,
                    },
                ))
            })
            .map_err(AuditError::Sqlite)?;
        for entry in mapped {
            let (iid, ev) = entry.map_err(AuditError::Sqlite)?;
            events.entry(iid).or_default().push(ev);
        }
    }

    let mut v: Vec<String> = Vec::new();
    let mut cycled: HashMap<String, bool> = HashMap::new();
    for row in &rows {
        let evs = events.get(&row.id).map(Vec::as_slice).unwrap_or(&[]);
        let (fstatus, fattempts, cb) = fold(row, evs, &mut v);
        cycled.insert(row.id.clone(), cb);
        if let Some(fs) = fstatus {
            if fs != row.status {
                let what = format!("folded status {fs} != row status {}", row.status);
                rowbad(&mut v, row, &what);
            }
            if fattempts != row.attempts {
                let what = format!(
                    "folded attempts {fattempts} != row attempts {}",
                    row.attempts
                );
                rowbad(&mut v, row, &what);
            }
        }
        if row.status == "running" && row.lease_until.unwrap_or(0) == 0 {
            rowbad(&mut v, row, "running row has no lease_until");
        }
        if row.status == "queued" && row.run_id.is_some() {
            rowbad(&mut v, row, "queued row has run_id");
        }
    }

    let mut dedupe_seen: HashMap<(String, String), String> = HashMap::new();
    let mut coalesce: Vec<(String, Vec<(String, bool)>)> = Vec::new();
    for row in &rows {
        if row.status == "queued" || row.status == "running" {
            if let Some(key) = &row.dedupe_key {
                let k = (row.queue.clone(), key.clone());
                if let Some(existing) = dedupe_seen.get(&k) {
                    let what = format!(
                        "duplicate unfinished dedupe_key {} in {} (also {existing})",
                        py_repr(key),
                        row.queue
                    );
                    rowbad(&mut v, row, &what);
                } else {
                    dedupe_seen.insert(k, row.id.clone());
                }
            }
            if let Some(key) = &row.coalesce_key
                && row.status == "queued"
            {
                let entry = if let Some(idx) = coalesce.iter().position(|(k, _)| k == key) {
                    &mut coalesce[idx].1
                } else {
                    coalesce.push((key.clone(), Vec::new()));
                    &mut coalesce.last_mut().expect("just pushed").1
                };
                entry.push((row.id.clone(), cycled[&row.id]));
            }
        }
    }
    for (ck, group) in &coalesce {
        if group.len() > 1 && group.iter().all(|(_, cb)| !*cb) {
            let mut sorted = group.clone();
            sorted.sort();
            for (iid, _) in &sorted[1..] {
                let what = format!(
                    "duplicate queued coalesce_key {} (also {})",
                    py_repr(ck),
                    sorted[0].0
                );
                bad(&mut v, iid, -1, Some("queued"), "<row>", &what);
            }
        }
    }

    let n_events: usize = events.values().map(Vec::len).sum();
    if let Some(driver_path) = driver_path {
        check_driver(driver_path, &rows, &mut v)?;
    }

    let n = v.len();
    let limit = max_violations.unwrap_or(n).min(n);
    for line in &v[..limit] {
        println!("{line}");
    }
    println!(
        "audit: {} items, {} events, {} violations",
        rows.len(),
        n_events,
        n
    );
    if n == 0 {
        Ok(())
    } else {
        Err(AuditError::Violations(n))
    }
}

use crate::{
    Fenced, Item, ItemStatus, Lease, Outcome, Push, Pushed, QueueDepth, QueueError, QueueEvent,
};
use rusqlite::{
    Connection, OptionalExtension, Params, Row, Transaction, TransactionBehavior, params,
    types::Type,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    fs::DirBuilder,
    path::Path,
    time::{Duration, Instant},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, QueueError>;

const SCHEMA_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Queue {
    conn: Connection,
}

impl Queue {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            let mut builder = DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.recursive(true).create(parent)?;
        }
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match tx.pragma_query_value(None, "user_version", |row| row.get(0))? {
            0 => {
                tx.execute_batch(include_str!("schema.sql"))?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            SCHEMA_VERSION => {}
            version => return Err(QueueError::UnsupportedSchema { version }),
        }
        tx.commit()?;
        let started = Instant::now();
        loop {
            match conn.pragma_update(None, "journal_mode", "WAL") {
                Ok(()) => break,
                // SQLite can skip the busy handler when changing journal modes.
                Err(error)
                    if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy)
                        && started.elapsed() < BUSY_TIMEOUT =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(Self { conn })
    }

    pub fn push(&mut self, push: Push, now_ms: i64) -> Result<Pushed> {
        validate_name(&push.queue)?;
        if push.max_attempts == 0 {
            return Err(QueueError::InvalidMaxAttempts);
        }
        self.write(|tx| {
            if let Some(key) = &push.dedupe_key
                && let Some(item) = unfinished_by_key(tx, &push.queue, key)?
            {
                event(tx, &item.id, now_ms, "deduped", None)?;
                return Ok(Pushed::Deduped(item));
            }
            if let Some(key) = &push.coalesce_key
                && let Some(mut item) = tx.query_row(
                    "SELECT * FROM queue_items WHERE queue = ? AND coalesce_key = ?
                     AND status = 'queued' ORDER BY seq LIMIT 1",
                    params![push.queue, key],
                    read_item,
                ).optional()?
            {
                if let Some(requester) = push.requester
                    && !item.waiters.contains(&requester)
                {
                    item.waiters.push(requester);
                }
                item.payload = push.payload;
                tx.execute(
                    "UPDATE queue_items SET payload = ?, waiters = ? WHERE id = ?",
                    params![json!(item.payload).to_string(), json!(item.waiters).to_string(), item.id],
                )?;
                event(tx, &item.id, now_ms, "coalesced", None)?;
                return Ok(Pushed::Coalesced(item));
            }
            if let Some(max_depth) = push.max_depth {
                let depth = tx.query_row(
                    "SELECT COUNT(*) FROM queue_items WHERE queue = ?
                     AND status IN ('queued', 'running')",
                    [&push.queue],
                    |row| row.get(0),
                )?;
                if depth >= i64::from(max_depth) {
                    return Err(QueueError::Full { depth });
                }
            }
            let id = Uuid::new_v4().to_string();
            let waiters: Vec<_> = push.requester.iter().filter(|_| push.coalesce_key.is_some()).collect();
            tx.execute(
                "INSERT INTO queue_items
                 (id, queue, seq, status, max_attempts, dedupe_key, coalesce_key,
                  payload, requester, waiters, enqueued_at)
                 VALUES (?1, ?2, (SELECT COALESCE(MAX(seq), 0) + 1 FROM queue_items WHERE queue = ?2),
                         'queued', ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![id, push.queue, push.max_attempts, push.dedupe_key, push.coalesce_key,
                    json!(push.payload).to_string(), push.requester, json!(waiters).to_string(), now_ms],
            )?;
            event(tx, &id, now_ms, "enqueued", None)?;
            Ok(Pushed::New(get_item(tx, &id)?))
        })
    }

    pub fn claim(
        &mut self,
        queue: &str,
        lease_ms: u64,
        now_ms: i64,
    ) -> Result<Option<(Item, Lease)>> {
        validate_name(queue)?;
        let until = lease_until(lease_ms, now_ms).ok_or(QueueError::InvalidLease)?;
        self.write(|tx| {
            let item = tx
                .query_row(
                    "UPDATE queue_items SET status = 'running', attempts = attempts + 1, claims = claims + 1,
                 lease_until = ?2, started_at = ?3
                 WHERE id = (SELECT id FROM queue_items WHERE queue = ?1
                             AND status = 'queued' ORDER BY seq LIMIT 1)
                 RETURNING *",
                    params![queue, until, now_ms],
                    read_item,
                )
                .optional()?;
            let Some(item) = item else { return Ok(None) };
            event(
                tx,
                &item.id,
                now_ms,
                "claimed",
                Some(&json!({"attempt": item.attempts, "claim": item.claims})),
            )?;
            let lease = Lease {
                id: item.id.clone(),
                claim: item.claims,
            };
            Ok(Some((item, lease)))
        })
    }

    pub fn set_run(&mut self, lease: &Lease, run_id: &str, now_ms: i64) -> Result<Fenced<()>> {
        self.write(|tx| {
            let attempt = tx
                .query_row(
                    "UPDATE queue_items SET run_id = ?3
                 WHERE id = ?1 AND status = 'running' AND claims = ?2
                 RETURNING attempts",
                    params![lease.id, lease.claim, run_id],
                    |row| row.get::<_, u32>(0),
                )
                .optional()?;
            let Some(attempt) = attempt else {
                return Ok(Fenced::Stale);
            };
            event(
                tx,
                &lease.id,
                now_ms,
                "run_started",
                Some(&json!({"attempt": attempt, "claim": lease.claim, "run_id": run_id})),
            )?;
            Ok(Fenced::Applied(()))
        })
    }

    pub fn heartbeat(&mut self, lease: &Lease, lease_ms: u64, now_ms: i64) -> Result<Fenced<()>> {
        let until = lease_until(lease_ms, now_ms).ok_or(QueueError::InvalidLease)?;
        self.write(|tx| {
            let changed = tx.execute(
                "UPDATE queue_items SET lease_until = ?3
                 WHERE id = ?1 AND status = 'running' AND claims = ?2",
                params![lease.id, lease.claim, until],
            )?;
            if changed == 0 {
                return Ok(Fenced::Stale);
            }
            Ok(Fenced::Applied(()))
        })
    }

    pub fn finish(&mut self, lease: &Lease, outcome: Outcome, now_ms: i64) -> Result<Fenced<Item>> {
        self.write(|tx| {
            let running = tx
                .query_row(
                    "SELECT attempts, max_attempts FROM queue_items
                 WHERE id = ?1 AND status = 'running' AND claims = ?2",
                    params![lease.id, lease.claim],
                    |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)),
                )
                .optional()?;
            let Some((attempt, max_attempts)) = running else {
                return Ok(Fenced::Stale);
            };
            let (status, kind, result, data) = match outcome {
                Outcome::Done(value) => (
                    ItemStatus::Done,
                    "done",
                    value,
                    Some(json!({"attempt": attempt, "claim": lease.claim})),
                ),
                Outcome::Failed(error) => {
                    event(
                        tx,
                        &lease.id,
                        now_ms,
                        "failed",
                        Some(&json!({"attempt": attempt, "claim": lease.claim, "error": error})),
                    )?;
                    let (status, kind) = if attempt < max_attempts {
                        (ItemStatus::Queued, "requeued")
                    } else {
                        (ItemStatus::Dead, "dead")
                    };
                    (status, kind, Value::String(error), None)
                }
            };
            end_run(
                tx,
                &lease.id,
                lease.claim,
                status,
                Some(&result),
                now_ms,
                false,
            )?;
            event(tx, &lease.id, now_ms, kind, data.as_ref())?;
            Ok(Fenced::Applied(get_item(tx, &lease.id)?))
        })
    }

    pub fn release(&mut self, lease: &Lease, now_ms: i64) -> Result<Fenced<()>> {
        self.write(|tx| {
            let Some(attempt) = end_run(
                tx,
                &lease.id,
                lease.claim,
                ItemStatus::Queued,
                None,
                now_ms,
                true,
            )?
            else {
                return Ok(Fenced::Stale);
            };
            event(
                tx,
                &lease.id,
                now_ms,
                "released",
                Some(&json!({"attempt": attempt, "claim": lease.claim})),
            )?;
            Ok(Fenced::Applied(()))
        })
    }

    pub fn reap(&mut self, now_ms: i64) -> Result<Vec<Item>> {
        self.reap_matching(None, now_ms)
    }

    /// Reaps expired leases belonging to one queue.
    pub fn reap_queue(&mut self, queue: &str, now_ms: i64) -> Result<Vec<Item>> {
        validate_name(queue)?;
        self.reap_matching(Some(queue), now_ms)
    }

    fn reap_matching(&mut self, queue: Option<&str>, now_ms: i64) -> Result<Vec<Item>> {
        self.write(|tx| {
            let expired = query_items(
                tx,
                "SELECT * FROM queue_items WHERE status = 'running' AND lease_until <= ?1
                 AND (?2 IS NULL OR queue = ?2) ORDER BY queue, seq",
                params![now_ms, queue],
            )?;
            expired
                .into_iter()
                .map(|item| {
                    let exhausted = item.attempts >= item.max_attempts;
                    let status = if exhausted {
                        ItemStatus::Dead
                    } else {
                        ItemStatus::Queued
                    };
                    let error = json!("lease expired");
                    end_run(
                        tx,
                        &item.id,
                        item.claims,
                        status,
                        exhausted.then_some(&error),
                        now_ms,
                        false,
                    )?;
                    event(
                        tx,
                        &item.id,
                        now_ms,
                        "stalled",
                        Some(&json!({"attempt": item.attempts, "claim": item.claims})),
                    )?;
                    get_item(tx, &item.id)
                })
                .collect()
        })
    }

    pub fn cancel(&mut self, id: &str, now_ms: i64) -> Result<Item> {
        self.write(|tx| {
            if tx.execute(
                "UPDATE queue_items SET status = 'cancelled', finished_at = ?2, lease_until = NULL
                 WHERE id = ?1 AND status IN ('queued', 'running')",
                params![id, now_ms],
            )? != 0
            {
                event(tx, id, now_ms, "cancelled", None)?;
            }
            get_item(tx, id)
        })
    }

    pub fn retry(&mut self, id: &str, now_ms: i64) -> Result<Item> {
        self.write(|tx| {
            let item = get_item(tx, id)?;
            if !matches!(item.status, ItemStatus::Dead | ItemStatus::Cancelled) {
                return Err(QueueError::InvalidStatus { id: item.id, status: item.status });
            }
            if let Some(key) = &item.dedupe_key
                && let Some(existing) = unfinished_by_key(tx, &item.queue, key)?
            {
                return Err(QueueError::DedupeConflict { id: existing.id });
            }
            tx.execute(
                "UPDATE queue_items SET status = 'queued', attempts = 0, run_id = NULL,
                 lease_until = NULL, started_at = NULL, result = NULL, finished_at = NULL WHERE id = ?",
                [id],
            )?;
            event(tx, id, now_ms, "retried", None)?;
            get_item(tx, id)
        })
    }

    pub fn list(
        &mut self,
        queue: Option<&str>,
        status: Option<ItemStatus>,
        limit: u32,
    ) -> Result<Vec<Item>> {
        if let Some(queue) = queue {
            validate_name(queue)?;
        }
        query_items(
            &self.conn,
            "SELECT * FROM queue_items WHERE (?1 IS NULL OR queue = ?1)
             AND (?2 IS NULL OR status = ?2) ORDER BY queue, seq LIMIT ?3",
            params![queue, status.map(ItemStatus::as_str), limit],
        )
    }

    pub fn item(&mut self, id: &str) -> Result<Option<(Item, Vec<QueueEvent>)>> {
        let tx = self.conn.transaction()?;
        let item = tx
            .query_row("SELECT * FROM queue_items WHERE id = ?", [id], read_item)
            .optional()?;
        let Some(item) = item else { return Ok(None) };
        let events = {
            let mut stmt = tx.prepare(
                "SELECT item_id, seq, at, kind, data FROM queue_events WHERE item_id = ? ORDER BY seq",
            )?;
            stmt.query_map([id], |row| {
                Ok(QueueEvent {
                    item_id: row.get(0)?,
                    seq: row.get(1)?,
                    at: row.get(2)?,
                    kind: row.get(3)?,
                    data: decode_optional(row, "data")?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        tx.commit()?;
        Ok(Some((item, events)))
    }

    pub fn depths(&mut self) -> Result<Vec<QueueDepth>> {
        let mut stmt = self.conn.prepare(
            "SELECT queue, SUM(status = 'queued'), SUM(status = 'running'), SUM(status = 'dead')
             FROM queue_items GROUP BY queue ORDER BY queue",
        )?;
        let depths = stmt
            .query_map([], |row| {
                Ok(QueueDepth {
                    queue: row.get(0)?,
                    queued: row.get(1)?,
                    running: row.get(2)?,
                    dead: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(depths)
    }

    pub fn prune(&mut self, finished_before_ms: i64) -> Result<usize> {
        self.write(|tx| Ok(tx.execute(
            "DELETE FROM queue_items WHERE status IN ('done', 'dead', 'cancelled') AND finished_at < ?",
            [finished_before_ms],
        )?))
    }

    fn write<T>(&mut self, action: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = action(&tx)?;
        tx.commit()?;
        Ok(value)
    }
}

fn end_run(
    tx: &Transaction<'_>,
    id: &str,
    claim: i64,
    status: ItemStatus,
    result: Option<&Value>,
    now_ms: i64,
    refund_attempt: bool,
) -> Result<Option<u32>> {
    Ok(tx
        .query_row(
            "UPDATE queue_items SET status = ?3, lease_until = NULL,
         run_id = CASE WHEN ?3 = 'queued' THEN NULL ELSE run_id END,
         started_at = CASE WHEN ?3 = 'queued' THEN NULL ELSE started_at END,
         finished_at = CASE WHEN ?3 = 'queued' THEN NULL ELSE ?5 END,
         result = COALESCE(?4, result), attempts = attempts - ?6
         WHERE id = ?1 AND status = 'running' AND claims = ?2
         RETURNING attempts + ?6",
            params![
                id,
                claim,
                status.as_str(),
                result.map(Value::to_string),
                now_ms,
                refund_attempt
            ],
            |row| row.get(0),
        )
        .optional()?)
}

fn unfinished_by_key(tx: &Transaction<'_>, queue: &str, key: &str) -> Result<Option<Item>> {
    Ok(tx
        .query_row(
            "SELECT * FROM queue_items WHERE queue = ? AND dedupe_key = ?
         AND status IN ('queued', 'running')",
            params![queue, key],
            read_item,
        )
        .optional()?)
}

fn event(
    tx: &Transaction<'_>,
    id: &str,
    now_ms: i64,
    kind: &str,
    data: Option<&Value>,
) -> Result<()> {
    tx.execute(
        "INSERT INTO queue_events (item_id, seq, at, kind, data)
         VALUES (?1, (SELECT COALESCE(MAX(seq), 0) + 1 FROM queue_events WHERE item_id = ?1),
                 ?2, ?3, ?4)",
        params![id, now_ms, kind, data.map(Value::to_string)],
    )?;
    Ok(())
}

fn get_item(conn: &Connection, id: &str) -> Result<Item> {
    conn.query_row("SELECT * FROM queue_items WHERE id = ?", [id], read_item)
        .optional()?
        .ok_or_else(|| QueueError::NotFound(id.into()))
}

fn query_items(conn: &Connection, sql: &str, params: impl Params) -> Result<Vec<Item>> {
    let mut stmt = conn.prepare(sql)?;
    Ok(stmt
        .query_map(params, read_item)?
        .collect::<rusqlite::Result<_>>()?)
}

fn read_item(row: &Row<'_>) -> rusqlite::Result<Item> {
    let status: String = row.get("status")?;
    Ok(Item {
        id: row.get("id")?,
        queue: row.get("queue")?,
        seq: row.get("seq")?,
        status: status
            .parse()
            .map_err(|error| conversion_error(row, "status", error))?,
        attempts: row.get("attempts")?,
        claims: row.get("claims")?,
        max_attempts: row.get("max_attempts")?,
        dedupe_key: row.get("dedupe_key")?,
        coalesce_key: row.get("coalesce_key")?,
        payload: decode(row, "payload", &row.get::<_, String>("payload")?)?,
        requester: row.get("requester")?,
        waiters: decode(row, "waiters", &row.get::<_, String>("waiters")?)?,
        run_id: row.get("run_id")?,
        lease_until: row.get("lease_until")?,
        result: decode_optional(row, "result")?,
        enqueued_at: row.get("enqueued_at")?,
        started_at: row.get("started_at")?,
        finished_at: row.get("finished_at")?,
    })
}

fn decode<T: DeserializeOwned>(row: &Row<'_>, column: &str, value: &str) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|error| conversion_error(row, column, error))
}

fn decode_optional<T: DeserializeOwned>(
    row: &Row<'_>,
    column: &str,
) -> rusqlite::Result<Option<T>> {
    row.get::<_, Option<String>>(column)?
        .as_deref()
        .map(|value| decode(row, column, value))
        .transpose()
}

fn conversion_error(
    row: &Row<'_>,
    column: &str,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    match row.as_ref().column_index(column) {
        Ok(index) => rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error)),
        Err(error) => error,
    }
}

fn lease_until(lease_ms: u64, now_ms: i64) -> Option<i64> {
    i64::try_from(lease_ms)
        .ok()
        .filter(|ms| *ms > 0)
        .and_then(|ms| now_ms.checked_add(ms))
}

fn validate_name(queue: &str) -> Result<()> {
    let alphanumeric = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    if queue.len() <= 64
        && queue.as_bytes().first().is_some_and(alphanumeric)
        && queue
            .bytes()
            .all(|byte| alphanumeric(&byte) || matches!(byte, b'_' | b'-'))
        || Uuid::parse_str(queue).is_ok()
    {
        Ok(())
    } else {
        Err(QueueError::InvalidQueueName(queue.into()))
    }
}

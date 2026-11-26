use crate::{
    Consumer, ConsumerRow, Fenced, Item, ItemStatus, Lease, Outcome, Push, Pushed, QueueDepth,
    QueueError, QueueEvent,
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

const SCHEMA_VERSION: i64 = 2;
// Keep released rows so a later acquisition can never reuse a fencing token.
const RELEASED_CONSUMER: i64 = i64::MIN;
const FIND_REPLY: &str = "SELECT * FROM queue_items
             WHERE json_extract(payload,'$.re') = ?1 AND json_extract(payload,'$.delivery') = ?2
             ORDER BY enqueued_at, rowid LIMIT 1";
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
        let version = tx.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        match version {
            0 => tx.execute_batch(include_str!("schema.sql"))?,
            1 | SCHEMA_VERSION => {}
            version => return Err(QueueError::UnsupportedSchema { version }),
        }
        if version < SCHEMA_VERSION {
            tx.execute_batch(include_str!("schema-v2.sql"))?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
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
        self.write(|tx| push_item(tx, push, now_ms))
    }

    pub fn claim(
        &mut self,
        queue: &str,
        lease_ms: u64,
        now_ms: i64,
    ) -> Result<Option<(Item, Lease)>> {
        self.claim_excluding(queue, lease_ms, now_ms, &[])
    }

    /// Claim the oldest queued item whose previous worker is no longer draining.
    pub fn claim_excluding(
        &mut self,
        queue: &str,
        lease_ms: u64,
        now_ms: i64,
        excluded: &[&str],
    ) -> Result<Option<(Item, Lease)>> {
        validate_name(queue)?;
        let until = lease_until(lease_ms, now_ms).ok_or(QueueError::InvalidLease)?;
        self.write(|tx| claim_item(tx, queue, until, now_ms, excluded))
    }

    pub fn claim_consumer(
        &mut self,
        queue: &str,
        owner: &str,
        lease_ms: u64,
        now_ms: i64,
    ) -> Result<Option<Consumer>> {
        validate_name(queue)?;
        let until = lease_until(lease_ms, now_ms).ok_or(QueueError::InvalidLease)?;
        self.write(|tx| {
            Ok(tx
                .query_row(
                    "INSERT INTO queue_consumers(queue, owner, token, until) VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(queue) DO UPDATE SET owner = excluded.owner, until = excluded.until,
                 token = queue_consumers.token + (queue_consumers.until <= ?4)
                 WHERE queue_consumers.until <= ?4 OR queue_consumers.owner = excluded.owner
                 RETURNING queue, owner, token",
                    params![queue, owner, until, now_ms],
                    read_consumer,
                )
                .optional()?)
        })
    }

    pub fn claim_owned(
        &mut self,
        consumer: &Consumer,
        lease_ms: u64,
        now_ms: i64,
    ) -> Result<Fenced<Option<(Item, Lease)>>> {
        let until = lease_until(lease_ms, now_ms).ok_or(QueueError::InvalidLease)?;
        self.write(|tx| {
            let owns = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_consumers
                  WHERE queue = ?1 AND owner = ?2 AND token = ?3 AND until > ?4)",
                params![consumer.queue, consumer.owner, consumer.token, now_ms],
                |row| row.get::<_, bool>(0),
            )?;
            if !owns {
                return Ok(Fenced::Stale);
            }
            Ok(Fenced::Applied(claim_item(
                tx,
                &consumer.queue,
                until,
                now_ms,
                &[],
            )?))
        })
    }

    /// Oldest eligible queued item by mailbox sequence.
    pub fn queued_before(&self, queue: &str, cutoff_ms: i64) -> Result<Option<Item>> {
        validate_name(queue)?;
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM queue_items WHERE queue = ?1 AND status = 'queued'
               AND enqueued_at < ?2 ORDER BY seq LIMIT 1",
                params![queue, cutoff_ms],
                read_item,
            )
            .optional()?)
    }

    /// Settles an unclaimed, aged item and enqueues its notice under the mailbox
    /// consumer fence. Either both writes commit or neither does.
    pub fn expire_queued_with_notice(
        &mut self,
        consumer: &Consumer,
        id: &str,
        cutoff_ms: i64,
        reason: &str,
        notice: Push,
        now_ms: i64,
    ) -> Result<Fenced<()>> {
        self.write(|tx| {
            let updated = tx.execute(
                "UPDATE queue_items SET status = 'dead', finished_at = ?6, result = ?7
                  WHERE id = ?1 AND queue = ?2 AND status = 'queued' AND enqueued_at < ?3
                    AND EXISTS (SELECT 1 FROM queue_consumers
                     WHERE queue = ?2 AND owner = ?4 AND token = ?5 AND until > ?6)",
                params![
                    id,
                    consumer.queue,
                    cutoff_ms,
                    consumer.owner,
                    consumer.token,
                    now_ms,
                    json!(reason).to_string(),
                ],
            )?;
            if updated == 0 {
                return Ok(Fenced::Stale);
            }
            event(
                tx,
                id,
                now_ms,
                "dead",
                Some(&json!({"error": reason, "undeliverable": true})),
            )?;
            push_item(tx, notice, now_ms)?;
            Ok(Fenced::Applied(()))
        })
    }

    pub fn release_consumer(&mut self, consumer: &Consumer) -> Result<Fenced<()>> {
        self.write(|tx| {
            let changed = tx.execute(
                "UPDATE queue_consumers SET until = ?4
                 WHERE queue = ?1 AND owner = ?2 AND token = ?3 AND until <> ?4",
                params![
                    consumer.queue,
                    consumer.owner,
                    consumer.token,
                    RELEASED_CONSUMER
                ],
            )?;
            if changed == 0 {
                return Ok(Fenced::Stale);
            }
            Ok(Fenced::Applied(()))
        })
    }

    pub fn consumers(&self) -> Result<Vec<ConsumerRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT queue, owner, token, until FROM queue_consumers ORDER BY queue")?;
        Ok(stmt
            .query_map([], |row| {
                Ok(ConsumerRow {
                    consumer: read_consumer(row)?,
                    until: row.get("until")?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_delivery(
        &mut self,
        lease: &Lease,
        delivery: &str,
        data: Value,
        now_ms: i64,
    ) -> Result<Fenced<()>> {
        self.write(|tx| {
            let attempt = tx
                .query_row(
                    "UPDATE queue_items SET delivery = ?3
                 WHERE id = ?1 AND status = 'running' AND claims = ?2
                 RETURNING attempts",
                    params![lease.id, lease.claim, delivery],
                    |row| row.get::<_, u32>(0),
                )
                .optional()?;
            let Some(attempt) = attempt else {
                return Ok(Fenced::Stale);
            };
            let Value::Object(mut data) = data else {
                return Err(QueueError::InvalidDeliveryData);
            };
            data.insert("attempt".into(), json!(attempt));
            data.insert("claim".into(), json!(lease.claim));
            data.insert("delivery".into(), json!(delivery));
            event(
                tx,
                &lease.id,
                now_ms,
                "delivered",
                Some(&Value::Object(data)),
            )?;
            Ok(Fenced::Applied(()))
        })
    }

    /// Records the observed end of a delivery's turn and reserves at most one
    /// reminder. Call only for a ready pane with evidence that the turn moved.
    /// The reservation is durable before the caller sends terminal input.
    pub fn reserve_reply_reminder(
        &mut self,
        lease: &Lease,
        re: &str,
        delivery: &str,
        remind_after_ms: i64,
        now_ms: i64,
    ) -> Result<Fenced<bool>> {
        self.write(|tx| {
            let owns_delivery = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_items
                  WHERE id = ?1 AND claims = ?2 AND delivery = ?3 AND status = 'running')",
                params![lease.id, lease.claim, delivery],
                |row| row.get::<_, bool>(0),
            )?;
            if !owns_delivery {
                return Ok(Fenced::Stale);
            }
            let reply_exists = tx
                .query_row(FIND_REPLY, params![re, delivery], |_| Ok(()))
                .optional()?
                .is_some();
            if reply_exists {
                return Ok(Fenced::Applied(false));
            }
            let (ended_at, reserved): (Option<i64>, bool) = tx.query_row(
                "SELECT MIN(at) FILTER (WHERE kind = 'turn_ended'),
                        COUNT(*) FILTER (WHERE kind = 'reminder_reserved') > 0
                   FROM queue_events
                  WHERE item_id = ?1 AND kind IN ('turn_ended', 'reminder_reserved')
                    AND json_extract(data, '$.claim') = ?2
                    AND json_extract(data, '$.delivery') = ?3",
                params![lease.id, lease.claim, delivery],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if reserved {
                return Ok(Fenced::Applied(false));
            }
            let data = json!({"claim": lease.claim, "delivery": delivery});
            let ended_at = match ended_at {
                Some(at) => at,
                None => {
                    event(tx, &lease.id, now_ms, "turn_ended", Some(&data))?;
                    now_ms
                }
            };
            let due = now_ms.saturating_sub(ended_at) >= remind_after_ms;
            if due {
                event(tx, &lease.id, now_ms, "reminder_reserved", Some(&data))?;
            }
            Ok(Fenced::Applied(due))
        })
    }

    pub fn find_reply(&self, re: &str, delivery: &str) -> Result<Option<Item>> {
        Ok(self
            .conn
            .query_row(FIND_REPLY, params![re, delivery], read_item)
            .optional()?)
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
        self.write(|tx| finish_item(tx, lease, outcome, now_ms))
    }

    /// Marks the current claim dead and enqueues its notice in one transaction.
    /// A stale claim has no effect. An enqueue error rolls back the dead transition.
    pub fn finish_dead_with_notice(
        &mut self,
        lease: &Lease,
        reason: String,
        notice: Push,
        now_ms: i64,
    ) -> Result<Fenced<Item>> {
        self.write(|tx| {
            let finished = finish_item(tx, lease, Outcome::Dead(reason), now_ms)?;
            // A reply rechecked by finish_item wins: no deadline notice.
            if matches!(&finished, Fenced::Applied(item) if item.status == ItemStatus::Done) {
                return Ok(finished);
            }
            if matches!(finished, Fenced::Applied(_)) {
                push_item(tx, notice, now_ms)?;
            }
            Ok(finished)
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
                // An item with a recorded delivery was typed to an agent: its
                // consumer (the postman) adopts it again after a restart and
                // settles it by reply, turn end or deadline. Requeueing it
                // would type it twice and orphan the agent's reply.
                "SELECT * FROM queue_items WHERE status = 'running' AND lease_until <= ?1
                 AND delivery IS NULL AND (?2 IS NULL OR queue = ?2) ORDER BY queue, seq",
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

    /// Reads a bounded, filtered mailbox page and its histories in one snapshot.
    pub fn mail_items_with_events(
        &mut self,
        queue: Option<&str>,
        status: Option<ItemStatus>,
        limit: u32,
    ) -> Result<Vec<(Item, Vec<QueueEvent>)>> {
        if let Some(queue) = queue {
            validate_name(queue)?;
        }
        let tx = self.conn.transaction()?;
        let status = status.map(ItemStatus::as_str);
        let items = match queue {
            Some(queue) => query_items(
                &tx,
                MAIL_ITEMS_BY_QUEUE,
                params![MAILBOX_GLOB, queue, status, limit],
            )?,
            None => query_items(
                &tx,
                "SELECT * FROM queue_items WHERE queue GLOB ?1
                 AND (?2 IS NULL OR status = ?2) ORDER BY queue, seq LIMIT ?3",
                params![MAILBOX_GLOB, status, limit],
            )?,
        };
        let mut statement = tx.prepare(ITEM_EVENTS)?;
        items
            .into_iter()
            .map(|item| {
                let events = statement
                    .query_map([&item.id], read_event)?
                    .collect::<rusqlite::Result<_>>()?;
                Ok((item, events))
            })
            .collect()
    }

    /// Projects a mailbox statistics snapshot into the caller's response type.
    pub fn mail_stats<T>(
        &self,
        since: Option<i64>,
        map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>> {
        let mut statement = self.conn.prepare(include_str!("mail-stats.sql"))?;
        Ok(statement
            .query_map(
                rusqlite::named_params! {":since": since, ":mailbox_glob": MAILBOX_GLOB},
                map,
            )?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Running items that a consumer has recorded a delivery for.
    pub fn list_running_delivered(&mut self) -> Result<Vec<Item>> {
        query_items(
            &self.conn,
            "SELECT * FROM queue_items WHERE status = 'running' AND delivery IS NOT NULL
             ORDER BY queue, seq",
            [],
        )
    }

    pub fn get(&self, id: &str) -> Result<Item> {
        get_item(&self.conn, id)
    }

    pub fn item(&mut self, id: &str) -> Result<Option<(Item, Vec<QueueEvent>)>> {
        self.item_matching(id, None)
    }

    /// Reads an exact ID only if the item belongs to a mailbox.
    pub fn mail_item(&mut self, id: &str) -> Result<Option<(Item, Vec<QueueEvent>)>> {
        self.item_matching(id, Some(MAILBOX_GLOB))
    }

    fn item_matching(
        &mut self,
        id: &str,
        queue_pattern: Option<&str>,
    ) -> Result<Option<(Item, Vec<QueueEvent>)>> {
        let tx = self.conn.transaction()?;
        let item = tx
            .query_row(
                "SELECT * FROM queue_items WHERE id = ?1 AND (?2 IS NULL OR queue GLOB ?2)",
                params![id, queue_pattern],
                read_item,
            )
            .optional()?;
        let Some(item) = item else { return Ok(None) };
        let events = {
            let mut stmt = tx.prepare(ITEM_EVENTS)?;
            stmt.query_map([id], read_event)?
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

fn push_item(tx: &Transaction<'_>, push: Push, now_ms: i64) -> Result<Pushed> {
    validate_name(&push.queue)?;
    if push.max_attempts == 0 {
        return Err(QueueError::InvalidMaxAttempts);
    }
    if let Some(key) = &push.dedupe_key
        && let Some(item) = unfinished_by_key(tx, &push.queue, key)?
    {
        event(tx, &item.id, now_ms, "deduped", None)?;
        return Ok(Pushed::Deduped(item));
    }
    if let Some(key) = &push.coalesce_key
        && let Some(mut item) = tx
            .query_row(
                "SELECT * FROM queue_items WHERE queue = ? AND coalesce_key = ?
                 AND status = 'queued' ORDER BY seq LIMIT 1",
                params![push.queue, key],
                read_item,
            )
            .optional()?
    {
        if let Some(requester) = push.requester
            && !item.waiters.contains(&requester)
        {
            item.waiters.push(requester);
        }
        item.payload = push.payload;
        tx.execute(
            "UPDATE queue_items SET payload = ?, waiters = ? WHERE id = ?",
            params![
                json!(item.payload).to_string(),
                json!(item.waiters).to_string(),
                item.id
            ],
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
    let waiters: Vec<_> = push
        .requester
        .iter()
        .filter(|_| push.coalesce_key.is_some())
        .collect();
    tx.execute(
        "INSERT INTO queue_items
             (id, queue, seq, status, max_attempts, dedupe_key, coalesce_key,
              payload, requester, waiters, enqueued_at)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(seq), 0) + 1 FROM queue_items WHERE queue = ?2),
                     'queued', ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            push.queue,
            push.max_attempts,
            push.dedupe_key,
            push.coalesce_key,
            json!(push.payload).to_string(),
            push.requester,
            json!(waiters).to_string(),
            now_ms
        ],
    )?;
    event(tx, &id, now_ms, "enqueued", None)?;
    Ok(Pushed::New(get_item(tx, &id)?))
}

fn finish_item(
    tx: &Transaction<'_>,
    lease: &Lease,
    outcome: Outcome,
    now_ms: i64,
) -> Result<Fenced<Item>> {
    let running = tx
        .query_row(
            "SELECT * FROM queue_items
             WHERE id = ?1 AND status = 'running' AND claims = ?2",
            params![lease.id, lease.claim],
            read_item,
        )
        .optional()?;
    let Some(item) = running else {
        return Ok(Fenced::Stale);
    };
    let attempt = item.attempts;
    let max_attempts = item.max_attempts;
    // The caller may have looked up the reply before another process committed
    // it. Recheck inside this write before recording a failure or death.
    let outcome = if matches!(&outcome, Outcome::Failed(_) | Outcome::Dead(_))
        && item.payload.get("status").and_then(Value::as_str) == Some("task")
    {
        let reply = match (
            item.payload.get("id").and_then(Value::as_str),
            item.delivery.as_deref(),
        ) {
            (Some(re), Some(delivery)) => tx
                .query_row(FIND_REPLY, params![re, delivery], read_item)
                .optional()?,
            _ => None,
        };
        match reply {
            Some(reply) => Outcome::Done(json!({
                "reply": reply.id,
                "delivery": item.delivery.clone(),
            })),
            None => outcome,
        }
    } else {
        outcome
    };
    let (status, kind, result, data) = match outcome {
        Outcome::Done(value) => (
            ItemStatus::Done,
            "done",
            Some(value),
            Some(json!({"attempt": attempt, "claim": lease.claim})),
        ),
        Outcome::Cancelled => (ItemStatus::Cancelled, "cancelled", None, None),
        Outcome::Dead(error) => {
            let data = json!({"attempt": attempt, "claim": lease.claim, "error": error});
            (
                ItemStatus::Dead,
                "dead",
                Some(Value::String(error)),
                Some(data),
            )
        }
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
            (status, kind, Some(Value::String(error)), None)
        }
    };
    end_run(
        tx,
        &lease.id,
        lease.claim,
        status,
        result.as_ref(),
        now_ms,
        false,
    )?;
    event(tx, &lease.id, now_ms, kind, data.as_ref())?;
    Ok(Fenced::Applied(get_item(tx, &lease.id)?))
}

fn read_consumer(row: &Row<'_>) -> rusqlite::Result<Consumer> {
    Ok(Consumer {
        queue: row.get("queue")?,
        owner: row.get("owner")?,
        token: row.get("token")?,
    })
}

fn claim_item(
    tx: &Transaction<'_>,
    queue: &str,
    until: i64,
    now_ms: i64,
    excluded: &[&str],
) -> Result<Option<(Item, Lease)>> {
    let item = tx
        .query_row(
            "UPDATE queue_items SET status = 'running', attempts = attempts + 1, claims = claims + 1,
         lease_until = ?2, started_at = ?3
         WHERE id = (SELECT id FROM queue_items WHERE queue = ?1
                     AND status = 'queued' AND id NOT IN (SELECT value FROM json_each(?4))
                     ORDER BY seq LIMIT 1)
         RETURNING *",
            params![queue, until, now_ms, json!(excluded).to_string()],
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

// Queue name validation covers the remaining suffix after its first byte.
const MAILBOX_GLOB: &str = "mail-[a-z0-9]*";
const MAIL_ITEMS_BY_QUEUE: &str = "SELECT * FROM queue_items WHERE queue = ?2 AND queue GLOB ?1
     AND (?3 IS NULL OR status = ?3) ORDER BY seq LIMIT ?4";

const ITEM_EVENTS: &str =
    "SELECT item_id, seq, at, kind, data FROM queue_events WHERE item_id = ? ORDER BY seq";

fn read_event(row: &Row<'_>) -> rusqlite::Result<QueueEvent> {
    Ok(QueueEvent {
        item_id: row.get(0)?,
        seq: row.get(1)?,
        at: row.get(2)?,
        kind: row.get(3)?,
        data: decode_optional(row, "data")?,
    })
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
        delivery: row.get("delivery")?,
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

/// Whether `queue` is a name a push accepts: `[a-z0-9][a-z0-9_-]*`, at most
/// 64 bytes, or a UUID.
pub fn valid_queue_name(queue: &str) -> bool {
    let alphanumeric = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    queue.len() <= 64
        && queue.as_bytes().first().is_some_and(alphanumeric)
        && queue
            .bytes()
            .all(|byte| alphanumeric(&byte) || matches!(byte, b'_' | b'-'))
        || Uuid::parse_str(queue).is_ok()
}

fn validate_name(queue: &str) -> Result<()> {
    if valid_queue_name(queue) {
        Ok(())
    } else {
        Err(QueueError::InvalidQueueName(queue.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_exact_recipient_seeks_the_queue_index() {
        let queue = Queue::open(":memory:").unwrap();
        let plan: String = queue
            .conn
            .query_row(
                &format!("EXPLAIN QUERY PLAN {MAIL_ITEMS_BY_QUEUE}"),
                params![MAILBOX_GLOB, "mail-worker", "done", 10],
                |row| row.get(3),
            )
            .unwrap();
        assert!(
            plan.contains("SEARCH") && plan.contains("queue=?"),
            "{plan}"
        );
    }

    #[test]
    fn reply_lookup_uses_expression_index() {
        let queue = Queue::open(":memory:").unwrap();
        let plan: String = queue
            .conn
            .query_row(
                &format!("EXPLAIN QUERY PLAN {FIND_REPLY}"),
                ["message", "delivery"],
                |row| row.get(3),
            )
            .unwrap();
        assert!(plan.contains("USING INDEX queue_items_reply"), "{plan}");
    }
}

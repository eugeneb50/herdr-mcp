mod mail;
mod worker;

use crate::{HerdrMcpServer, Recipe, lock};
use anyhow::{Context, Result};
use herdr_mcp_api::{
    mail::{MailItemResponse, MailLabelStats, MailListQuery, MailStatsQuery},
    recipe::QueueConfig,
    settings::*,
};
use herdr_queue::{Item, ItemStatus, Push, Pushed, Queue, QueueDepth, QueueError, QueueEvent};
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, mpsc};
#[cfg(test)]
use tokio::time::Instant;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(crate) type MailItemHistory = MailItemResponse<Item, QueueEvent>;

type Active = HashMap<(String, i64), ActiveClaim>;

struct ActiveClaim {
    queue: String,
    cancel: CancellationToken,
    settling: bool,
}

#[derive(Clone, Default)]
pub struct QueueService(Arc<Runtime>);

#[derive(Default)]
struct Runtime {
    db: Mutex<Option<Queue>>,
    root: CancellationToken,
    tracker: TaskTracker,
    wakes: Mutex<HashMap<String, mpsc::Sender<()>>>,
    mail_wake: Notify,
    active: Mutex<Active>,
    #[cfg(test)]
    clock: std::sync::OnceLock<(Instant, i64)>,
}

#[derive(Debug)]
pub(crate) struct RequestError(pub(crate) String);

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for RequestError {}

fn invalid(message: impl Into<String>) -> anyhow::Error {
    RequestError(message.into()).into()
}

pub(crate) fn validate_config(config: &QueueConfig) -> Result<()> {
    let mut settings = Settings::default();
    for (key, value) in [
        (QUEUE_DEFAULT_CONCURRENCY, config.concurrency.map(u64::from)),
        (QUEUE_DEFAULT_MAX_DEPTH, config.max_depth.map(u64::from)),
        (
            QUEUE_DEFAULT_MAX_ATTEMPTS,
            config.max_attempts.map(u64::from),
        ),
        (QUEUE_DEFAULT_LEASE_MS, config.lease_ms),
    ] {
        if let Some(value) = value {
            settings
                .set(key, herdr_mcp_api::settings::Value::Number(value))
                .map_err(|error| invalid(format!("{key}: {error}")))?;
        }
    }
    Ok(())
}

fn number(server: &HerdrMcpServer, key: &str) -> u64 {
    crate::read(&server.settings).number(key)
}

fn duration(server: &HerdrMcpServer, key: &str) -> Duration {
    Duration::from_millis(number(server, key))
}

impl QueueService {
    async fn db<T: Send + 'static>(
        &self,
        server: &HerdrMcpServer,
        operation: impl FnOnce(&mut Queue) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let runtime = self.0.clone();
        let path = server.persistence.data_dir().join("queue.db");
        self.0
            .tracker
            .spawn_blocking(move || {
                let mut db = lock(&runtime.db);
                if db.is_none() {
                    *db = Some(Queue::open(path).context("opening queue database")?);
                }
                operation(db.as_mut().expect("opened above"))
            })
            .await
            .context("queue database task")?
    }

    #[cfg(not(test))]
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    // Keep the epoch stable while Tokio advances time in paused tests.
    #[cfg(test)]
    fn now(&self) -> i64 {
        let (start, epoch) = self
            .0
            .clock
            .get_or_init(|| (Instant::now(), chrono::Utc::now().timestamp_millis()));
        epoch.saturating_add(i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX))
    }

    async fn recipe(server: &HerdrMcpServer, name: &str) -> Result<Recipe> {
        if let Ok(id) = uuid::Uuid::parse_str(name)
            && let Some(recipe) = server.persistence.load_recipe(&id).await?
        {
            return Ok(recipe);
        }
        let mut named = server
            .persistence
            .list_recipes(None)
            .await?
            .into_iter()
            .filter(|r| r.name == name);
        let recipe = named
            .next()
            .ok_or_else(|| invalid("unknown queue collection"))?;
        if named.next().is_some() {
            return Err(invalid("ambiguous queue name; use the collection id"));
        }
        Ok(recipe)
    }

    fn wake(&self, queue: &str) {
        if queue.starts_with("mail-") {
            self.0.mail_wake.notify_one();
        }
        if let Some(tx) = lock(&self.0.wakes).get(queue) {
            // The periodic DB check also picks up pushes from other processes.
            let _ = tx.try_send(());
        }
    }

    pub(crate) async fn push(
        &self,
        server: &HerdrMcpServer,
        queue: &str,
        payload: Map<String, Value>,
        dedupe_key: Option<String>,
        requester: Option<String>,
    ) -> Result<Pushed> {
        if self.0.root.is_cancelled() {
            return Err(ShuttingDown.into());
        }
        let recipe = Self::recipe(server, queue).await?;
        let config = recipe
            .queue
            .ok_or_else(|| invalid("collection has no queue configuration"))?;
        validate_config(&config)?;
        let QueueConfig {
            coalesce,
            max_depth,
            max_attempts,
            ..
        } = config;
        let coalesce_key = coalesce
            .map(|template| {
                let mut value = Value::String(template);
                crate::server::engine::resolve_variables(
                    &mut value,
                    &HashMap::from([("item".to_owned(), Value::Object(payload.clone()))]),
                );
                if !crate::server::engine::unresolved_placeholders(&value).is_empty() {
                    return Err(invalid("unresolved queue coalesce placeholder"));
                }
                Ok(value
                    .as_str()
                    .expect("resolver preserves strings")
                    .to_owned())
            })
            .transpose()?;
        let queue = recipe.id.to_string();
        let push = Push {
            queue: queue.clone(),
            payload,
            dedupe_key,
            coalesce_key,
            requester,
            max_attempts: max_attempts.unwrap_or_else(|| count(server, QUEUE_DEFAULT_MAX_ATTEMPTS)),
            max_depth: Some(max_depth.unwrap_or_else(|| count(server, QUEUE_DEFAULT_MAX_DEPTH))),
        };
        let root = self.0.root.clone();
        let now = self.now();
        let pushed = self
            .db(server, move |db| {
                if root.is_cancelled() {
                    return Err(ShuttingDown.into());
                }
                Ok(db.push(push, now)?)
            })
            .await?;
        self.wake(&queue);
        Ok(pushed)
    }

    pub(crate) async fn list(
        &self,
        server: &HerdrMcpServer,
        queue: Option<String>,
        status: Option<String>,
        limit: Option<u32>,
    ) -> Result<Vec<Item>> {
        let queue = match queue {
            Some(queue) => Some(Self::recipe(server, &queue).await?.id.to_string()),
            None => None,
        };
        let status = status
            .map(|s| s.parse().map_err(|_| invalid("unknown queue status")))
            .transpose()?;
        self.db(server, move |db| {
            Ok(db.list(queue.as_deref(), status, limit.unwrap_or(100).min(1000))?)
        })
        .await
    }

    pub(crate) async fn item(
        &self,
        server: &HerdrMcpServer,
        id: &str,
    ) -> Result<(Item, Vec<QueueEvent>)> {
        let id = id.to_owned();
        self.db(server, move |db| {
            db.item(&id)?.ok_or_else(|| QueueError::NotFound(id).into())
        })
        .await
    }

    pub(crate) async fn mail_list(
        &self,
        server: &HerdrMcpServer,
        query: MailListQuery,
    ) -> Result<Vec<MailItemHistory>> {
        let queue = query
            .to
            .map(|label| {
                if !valid_mail_label(&label) {
                    return Err(invalid("invalid mailbox label"));
                }
                Ok(format!("mail-{label}"))
            })
            .transpose()?;
        let status = query
            .status
            .map(|status| status.parse().map_err(|_| invalid("unknown queue status")))
            .transpose()?;
        let limit = query.limit.unwrap_or(100).min(1000);
        self.db(server, move |db| {
            Ok(db
                .mail_items_with_events(queue.as_deref(), status, limit)?
                .into_iter()
                .map(|(item, events)| MailItemResponse { item, events })
                .collect())
        })
        .await
    }

    pub(crate) async fn mail_item(
        &self,
        server: &HerdrMcpServer,
        id: &str,
    ) -> Result<MailItemHistory> {
        let id = id.to_owned();
        self.db(server, move |db| {
            let (item, events) = db.mail_item(&id)?.ok_or(QueueError::NotFound(id))?;
            Ok(MailItemResponse { item, events })
        })
        .await
    }

    pub(crate) async fn mail_stats(
        &self,
        server: &HerdrMcpServer,
        query: MailStatsQuery,
    ) -> Result<Vec<MailLabelStats>> {
        if query.since.is_some_and(|since| since < 0) {
            return Err(invalid(
                "since must be a nonnegative Unix epoch millisecond",
            ));
        }
        self.db(server, move |db| {
            Ok(db.mail_stats(query.since, |row| {
                // SQLite counts are i64 and never negative.
                let count = |key: &str| {
                    row.get::<_, i64>(key)
                        .map(|n| u64::try_from(n).unwrap_or_default())
                };
                Ok(MailLabelStats {
                    to: row.get("to")?,
                    depth: count("depth")?,
                    in_flight: count("in_flight")?,
                    delivered: count("delivered")?,
                    replied: count("replied")?,
                    failed: count("failed")?,
                    dead: count("dead")?,
                    median_reply_ms: row.get("median_reply_ms")?,
                    p90_reply_ms: row.get("p90_reply_ms")?,
                    last_activity_at: row.get("last_activity_at")?,
                })
            })?)
        })
        .await
    }

    pub(crate) async fn cancel(&self, server: &HerdrMcpServer, id: String) -> Result<Item> {
        let now = self.now();
        let runtime = self.0.clone();
        self.db(server, move |db| {
            let item = db.cancel(&id, now)?;
            if let Some(active) = lock(&runtime.active).get(&(item.id.clone(), item.claims)) {
                active.cancel.cancel();
            }
            Ok(item)
        })
        .await
    }

    pub(crate) async fn retry(&self, server: &HerdrMcpServer, id: String) -> Result<Item> {
        let now = self.now();
        let item = self.db(server, move |db| Ok(db.retry(&id, now)?)).await?;
        self.wake(&item.queue);
        Ok(item)
    }

    pub(crate) async fn depths(&self, server: &HerdrMcpServer) -> Result<Vec<QueueDepth>> {
        self.db(server, |db| Ok(db.depths()?)).await
    }

    pub(crate) async fn prune(&self, server: &HerdrMcpServer, cutoff: i64) -> Result<usize> {
        self.db(server, move |db| Ok(db.prune(cutoff)?)).await
    }

    pub(crate) async fn wait(
        &self,
        server: &HerdrMcpServer,
        id: &str,
        timeout: Option<u64>,
    ) -> Result<Item> {
        let wait = async {
            loop {
                let id = id.to_owned();
                let item = self.db(server, move |db| Ok(db.get(&id)?)).await?;
                if !matches!(item.status, ItemStatus::Queued | ItemStatus::Running) {
                    return Ok(item);
                }
                tokio::select! {
                    () = self.0.root.cancelled() => return Err(ShuttingDown.into()),
                    () = tokio::time::sleep(duration(server, QUEUE_TICK_MS)) => {}
                }
            }
        };
        let timeout = Duration::from_millis(timeout.unwrap_or(30_000).clamp(1, 60_000));
        tokio::time::timeout(timeout, wait)
            .await
            .map_err(|_| WaitTimeout)?
    }
}

fn count(server: &HerdrMcpServer, key: &str) -> u32 {
    u32::try_from(number(server, key)).expect("queue count settings fit u32")
}

#[derive(Debug)]
pub(crate) struct ShuttingDown;
impl std::fmt::Display for ShuttingDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("shutting down")
    }
}
impl std::error::Error for ShuttingDown {}

#[derive(Debug)]
pub(crate) struct WaitTimeout;
impl std::fmt::Display for WaitTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("timed out waiting for queue item")
    }
}
impl std::error::Error for WaitTimeout {}

impl QueueService {
    pub(crate) async fn push_request(
        &self,
        server: &HerdrMcpServer,
        queue: &str,
        req: herdr_mcp_api::recipe::QueuePushRequest,
        wait: bool,
        timeout: Option<u64>,
    ) -> Result<herdr_mcp_api::recipe::QueuePushResponse<Item>> {
        use herdr_mcp_api::recipe::{QueuePushKind, QueuePushResponse};
        let (pushed, mut item) = match self
            .push(server, queue, req.payload, req.dedupe_key, req.requester)
            .await?
        {
            Pushed::New(item) => (QueuePushKind::New, item),
            Pushed::Deduped(item) => (QueuePushKind::Deduped, item),
            Pushed::Coalesced(item) => (QueuePushKind::Coalesced, item),
        };
        if wait {
            item = self.wait(server, &item.id, timeout).await?;
        }
        Ok(QueuePushResponse { pushed, item })
    }
}

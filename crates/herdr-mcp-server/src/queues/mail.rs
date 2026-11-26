//! The postman: herdr-mcp delivers the capsules waiting in queue
//! `mail-<label>` to the agent in the pane labelled `<label>`, one at a time,
//! and closes each delivery on its reply.
//!
//! Behind `mail.enabled`, for the labels in `mail.labels` only, so another
//! postman can keep the rest. Every step is a fenced queue write, visible in
//! the item's events: `claimed` → `delivered {delivery, pane, state_change_seq}`
//! → `done` (reply seen, or a non-task's turn ended) or `dead` (no reply in
//! `mail.reply-wait-ms`; the sender gets one notice capsule).
//!
//! Ownership: one renewable consumer lease per mailbox in queue.db, so two
//! herdr-mcp processes on one DB never both deliver. After a restart the
//! owner adopts its running delivery instead of typing it again.

use super::*;
use futures_util::future::BoxFuture;
use herdr_capsule::{
    Capsule, CapsuleConfig, delivery_text, format::reply_command, no_control_chars,
};
use herdr_queue::{Consumer, Fenced, Lease, Outcome};
use serde_json::json;

/// Statuses in which an agent may be handed a capsule.
const READY: [&str; 2] = ["idle", "done"];

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MailPane {
    pub pane_id: String,
    pub label: String,
    pub status: String,
    /// herdr's per-pane turn counter; moves when the agent starts or ends a turn.
    pub seq: Option<i64>,
}

/// The herdr calls the postman makes; a fake in tests.
pub(crate) trait MailHerdr: Send + Sync {
    fn panes(&self) -> BoxFuture<'_, Result<Vec<MailPane>>>;
    fn prompt<'a>(&'a self, pane: &'a str, text: &'a str) -> BoxFuture<'a, Result<()>>;
    fn enter<'a>(&'a self, pane: &'a str) -> BoxFuture<'a, Result<()>>;
}

/// herdr through its CLI: `agent list` has status and turn counter, `pane
/// list` has labels.
pub(crate) struct CliMailHerdr;

fn result(text: &str) -> Result<Value> {
    let parsed: Value = serde_json::from_str(text).context("herdr output is not JSON")?;
    parsed
        .get("result")
        .cloned()
        .context("herdr output has no result")
}

impl MailHerdr for CliMailHerdr {
    fn panes(&self) -> BoxFuture<'_, Result<Vec<MailPane>>> {
        Box::pin(async {
            let cli = |args: &'static [&'static str]| async move {
                crate::server::herdr_cli(args)
                    .await
                    .map_err(|e| anyhow::anyhow!("herdr {}: {}", args.join(" "), e.message))
            };
            let agents = result(&cli(&["agent", "list"]).await?)?;
            let panes = result(&cli(&["pane", "list"]).await?)?;
            let seqs: HashMap<&str, Option<i64>> = agents["agents"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| Some((a["pane_id"].as_str()?, a["state_change_seq"].as_i64())))
                .collect();
            panes["panes"]
                .as_array()
                .context("herdr pane list has no panes array")?
                .iter()
                .map(|p| {
                    let pane_id = p["pane_id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .context("herdr pane list entry has no pane ID")?;
                    Ok(MailPane {
                        pane_id: pane_id.to_owned(),
                        label: p["label"].as_str().unwrap_or_default().to_owned(),
                        status: p["agent_status"].as_str().unwrap_or("unknown").to_owned(),
                        seq: seqs.get(pane_id).copied().flatten(),
                    })
                })
                .collect()
        })
    }

    fn prompt<'a>(&'a self, pane: &'a str, text: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            crate::server::herdr_cli(&["agent", "prompt", pane, text])
                .await
                .map(drop)
                .map_err(|e| anyhow::anyhow!("herdr agent prompt {pane}: {}", e.message))
        })
    }

    fn enter<'a>(&'a self, pane: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            crate::server::herdr_cli(&["pane", "send-keys", pane, "enter"])
                .await
                .map(drop)
                .map_err(|e| anyhow::anyhow!("herdr pane send-keys {pane}: {}", e.message))
        })
    }
}

/// A delivery this process typed (or adopted) and is waiting on.
#[derive(Debug, Clone)]
struct InFlight {
    lease: Lease,
    capsule: Capsule,
    delivery: String,
    pane: String,
    seq: Option<i64>,
    delivered_at: i64,
    /// When to check that the agent started (and press Enter once if not).
    confirm_at: Option<i64>,
}

/// What one mailbox step decided, for the event log and tests.
#[derive(Debug, PartialEq)]
pub(crate) enum Step {
    NotOwner,
    NoPane,
    Busy,
    Empty,
    Delivered(String),
    Waiting,
    Nudged,
    Reminded,
    Replied(String),
    TurnEnded,
    Dead(String),
    Released(String),
}

pub(crate) struct Postman {
    owner: String,
    herdr: Arc<dyn MailHerdr>,
    flights: Mutex<HashMap<String, InFlight>>,
    /// When each label was first seen with no pane (cleared when one shows).
    /// In memory: a restart restarts the clock, which only delays expiry.
    missing_since: Mutex<HashMap<String, i64>>,
}

impl Postman {
    pub(crate) fn new(herdr: Arc<dyn MailHerdr>) -> Self {
        Self {
            owner: format!("herdr-mcp:{}:{}", std::process::id(), uuid::Uuid::new_v4()),
            herdr,
            flights: Mutex::default(),
            missing_since: Mutex::default(),
        }
    }
}

fn capsule_config(server: &HerdrMcpServer) -> CapsuleConfig {
    // The default names the running binary; the agent's reply runs `capsule`.
    // The work dir is read live: the next delivery text uses the setting.
    CapsuleConfig {
        exe: "capsule".into(),
        work: server.settings().path(MAIL_WORK_DIR),
        ..CapsuleConfig::default()
    }
}

fn parse_capsule(item: &Item) -> std::result::Result<Capsule, String> {
    let cap = Capsule::from_value(&Value::Object(item.payload.clone()))
        .map_err(|e| format!("not a capsule: {e}"))?;
    if cap.id.is_empty() || cap.to.is_empty() {
        return Err("not a capsule: no id or to".into());
    }
    // `send` refuses these; a direct queue push must not reach a terminal.
    // Every field the delivery text types is checked raw ("text" is not
    // whitespace-normalized first, unlike "summary").
    let reply_to = cap.reply_to.clone().unwrap_or_default();
    let fields = [
        ("id", cap.id.as_str()),
        ("task", cap.task.as_str()),
        ("text", cap.summary.as_str()),
        ("to", cap.to.as_str()),
        ("from", cap.from.as_str()),
        ("reply_to", reply_to.as_str()),
        ("status", cap.status.as_str()),
    ];
    let mut refs: Vec<String> = cap.artifacts.iter().map(|a| a.reference.clone()).collect();
    refs.extend(cap.artifacts.iter().filter_map(|a| a.note.clone()));
    no_control_chars(&fields, &refs).map_err(|e| e.to_string())?;
    Ok(cap)
}

fn ms(server: &HerdrMcpServer, key: &str) -> i64 {
    i64::try_from(number(server, key)).unwrap_or(i64::MAX)
}

impl QueueService {
    /// Runs the postman until shutdown; does nothing while `mail.enabled` is off.
    pub(crate) fn start_mail(&self, server: &HerdrMcpServer, herdr: Arc<dyn MailHerdr>) {
        let service = self.clone();
        let server = server.clone();
        let postman = Arc::new(Postman::new(herdr));
        self.0.tracker.spawn(async move {
            while !service.0.root.is_cancelled() {
                if let Err(error) = service.mail_tick(&server, &postman).await {
                    tracing::warn!("mail: {error:#}");
                }
                tokio::select! {
                    () = service.0.root.cancelled() => break,
                    () = tokio::time::sleep(duration(&server, QUEUE_TICK_MS)) => {}
                }
            }
        });
    }

    /// One pass over the mailboxes: new deliveries only for enabled labels,
    /// but every delivery already in flight is followed to its end, even
    /// after `mail.enabled` is switched off or its label removed.
    pub(crate) async fn mail_tick(
        &self,
        server: &HerdrMcpServer,
        postman: &Postman,
    ) -> Result<Vec<(String, Step)>> {
        let (enabled, labels) = {
            let s = crate::read(&server.settings);
            (s.flag(MAIL_ENABLED), s.labels(MAIL_LABELS))
        };
        // `lead` belongs to `capsule watch`: never claim its consumer or
        // deliver its mail, even when an operator lists the label.
        let open: Vec<String> = if enabled {
            labels
                .into_iter()
                .filter(|label| label.as_str() != "lead")
                .collect()
        } else {
            Vec::new()
        };
        let mut all = open.clone();
        all.extend(
            lock(&postman.flights)
                .keys()
                .filter(|l| !open.contains(l))
                .cloned()
                .collect::<Vec<_>>(),
        );
        if all.is_empty() {
            self.release_closed_mailboxes(server, postman, open).await?;
            return Ok(vec![]);
        }
        let panes = match postman.herdr.panes().await {
            Ok(panes) => Some(panes),
            Err(error) => {
                tracing::warn!("mail: reading herdr panes: {error:#}");
                None // still follow flights: replies and deadlines need no pane
            }
        };
        let mut steps = Vec::new();
        for label in all {
            let is_open = open.contains(&label);
            let readable = panes.is_some();
            let panes = panes.as_deref().unwrap_or_default();
            match self
                .mail_step(server, postman, &label, panes, is_open, readable)
                .await
            {
                Ok(step) => {
                    tracing::debug!("mail {label}: {step:?}");
                    steps.push((label, step));
                }
                Err(error) => tracing::warn!("mail {label}: {error:#}"),
            }
        }
        self.release_closed_mailboxes(server, postman, open).await?;
        Ok(steps)
    }

    async fn release_closed_mailboxes(
        &self,
        server: &HerdrMcpServer,
        postman: &Postman,
        mut retained: Vec<String>,
    ) -> Result<()> {
        // This service cannot own a mailbox before it has opened the queue.
        if self.0.db.try_lock().is_ok_and(|db| db.is_none()) {
            return Ok(());
        }
        retained.extend(lock(&postman.flights).keys().cloned());
        let owner = postman.owner.clone();
        let now = self.now();
        self.db(server, move |db| {
            for row in db.consumers()? {
                let consumer = row.consumer;
                if consumer.owner == owner
                    && row.until > now
                    && consumer
                        .queue
                        .strip_prefix("mail-")
                        .is_some_and(|label| !retained.iter().any(|keep| keep == label))
                {
                    db.release_consumer(&consumer)?;
                }
            }
            Ok(())
        })
        .await
    }

    async fn mail_step(
        &self,
        server: &HerdrMcpServer,
        postman: &Postman,
        label: &str,
        panes: &[MailPane],
        is_open: bool,
        readable: bool,
    ) -> Result<Step> {
        let queue = format!("mail-{label}");
        let lease_ms = number(server, MAIL_CONSUMER_LEASE_MS);
        let now = self.now();
        let (q, owner) = (queue.clone(), postman.owner.clone());
        let Some(consumer) = self
            .db(server, move |db| {
                Ok(db.claim_consumer(&q, &owner, lease_ms, now)?)
            })
            .await?
        else {
            lock(&postman.flights).remove(label);
            return Ok(Step::NotOwner);
        };
        let here: Vec<&MailPane> = panes.iter().filter(|p| p.label == label).collect();
        // Missing, or two panes share the label: deliver to neither.
        let pane = if let [pane] = here.as_slice() {
            Some(*pane)
        } else {
            None
        };
        let flight = lock(&postman.flights).get(label).cloned();
        let flight = match flight {
            Some(f) => Some(f),
            None => self.adopt(server, &queue).await?,
        };
        if let Some(flight) = flight {
            let pane_closed = readable
                && !flight.pane.is_empty()
                && !panes.iter().any(|pane| pane.pane_id == flight.pane);
            return self
                .follow(server, postman, label, flight, pane, pane_closed)
                .await;
        }
        if !is_open {
            return Ok(Step::NotOwner);
        }
        if !readable {
            return Ok(Step::Empty); // herdr unreadable: try again next tick
        }
        let missing_for = {
            let mut missing = lock(&postman.missing_since);
            if here.is_empty() {
                let since = *missing.entry(label.to_owned()).or_insert(now);
                now.saturating_sub(since)
            } else {
                missing.remove(label);
                0
            }
        };
        let Some(pane) = pane else {
            // Undeliverable only after the pane has been gone for the whole
            // delay, not merely because old mail met a brief absence.
            return if here.is_empty() && missing_for >= ms(server, MAIL_UNDELIVERABLE_AFTER_MS) {
                self.expire_undeliverable(server, consumer, label).await
            } else {
                Ok(Step::NoPane)
            };
        };
        // No turn counter means no baseline: never type into an unknown turn.
        if !READY.contains(&pane.status.as_str()) || pane.seq.is_none() {
            return Ok(Step::Busy);
        }
        self.deliver(server, postman, &consumer, label, pane).await
    }

    async fn expire_undeliverable(
        &self,
        server: &HerdrMcpServer,
        consumer: Consumer,
        label: &str,
    ) -> Result<Step> {
        let cutoff = self
            .now()
            .saturating_sub(ms(server, MAIL_UNDELIVERABLE_AFTER_MS));
        let queue = consumer.queue.clone();
        let Some(item) = self
            .db(server, move |db| Ok(db.queued_before(&queue, cutoff)?))
            .await?
        else {
            return Ok(Step::NoPane);
        };
        let capsule = parse_capsule(&item).map_err(anyhow::Error::msg)?;
        let reason = format!("undeliverable: no pane with label {label}");
        let notice = Self::notice_push(&capsule, format!("undeliverable-{}", item.id), &reason)?;
        let db_reason = reason.clone();
        let now = self.now();
        let finished = self
            .db(server, move |db| {
                Ok(db.expire_queued_with_notice(
                    &consumer, &item.id, cutoff, &db_reason, notice, now,
                )?)
            })
            .await?;
        Ok(match finished {
            Fenced::Applied(()) => Step::Dead(reason),
            Fenced::Stale => Step::Empty,
        })
    }

    /// A running delivery left by an earlier run of this mailbox's owner (we
    /// hold its consumer lease, so its item lease is ours to continue).
    async fn adopt(&self, server: &HerdrMcpServer, queue: &str) -> Result<Option<InFlight>> {
        let q = queue.to_owned();
        let running = self
            .db(server, move |db| {
                let Some(item) = db
                    .list(Some(&q), Some(ItemStatus::Running), 1)?
                    .into_iter()
                    .next()
                else {
                    return Ok(None);
                };
                let events = db.item(&item.id)?.map(|(_, e)| e).unwrap_or_default();
                Ok(Some((item, events)))
            })
            .await?;
        let Some((item, events)) = running else {
            return Ok(None);
        };
        let lease = Lease {
            id: item.id.clone(),
            claim: item.claims,
        };
        let capsule = match parse_capsule(&item) {
            Ok(c) => c,
            Err(reason) => {
                self.finish_mail(server, &lease, Outcome::Dead(reason))
                    .await?;
                return Ok(None);
            }
        };
        let last = events.iter().rev().find(|e| e.kind == "delivered");
        let data = last.and_then(|e| e.data.clone()).unwrap_or(Value::Null);
        let (Some(delivery), true) = (item.delivery.clone(), data["submitted"] == true) else {
            // Claimed, maybe recorded, never confirmed typed: put it back so
            // it is delivered (again) — the agent may not have seen it.
            let now = self.now();
            self.db(server, move |db| Ok(db.release(&lease, now)?))
                .await?;
            return Ok(None);
        };
        Ok(Some(InFlight {
            lease,
            capsule,
            delivery,
            pane: data["pane"].as_str().unwrap_or_default().to_owned(),
            seq: data["state_change_seq"].as_i64(),
            delivered_at: last.map_or(item.started_at.unwrap_or(0), |e| e.at),
            confirm_at: None,
        }))
    }

    async fn deliver(
        &self,
        server: &HerdrMcpServer,
        postman: &Postman,
        consumer: &Consumer,
        label: &str,
        pane: &MailPane,
    ) -> Result<Step> {
        let now = self.now();
        let c = consumer.clone();
        // Short until it is typed: a crash here must not hold the item long.
        let short = number(server, MAIL_CONSUMER_LEASE_MS);
        let claimed = self
            .db(server, move |db| Ok(db.claim_owned(&c, short, now)?))
            .await?;
        let Fenced::Applied(Some((item, lease))) = claimed else {
            return Ok(Step::Empty);
        };
        let capsule = match parse_capsule(&item) {
            Ok(c) => c,
            Err(reason) => {
                self.finish_mail(server, &lease, Outcome::Dead(reason.clone()))
                    .await?;
                return Ok(Step::Dead(reason));
            }
        };
        let delivery = uuid::Uuid::new_v4().simple().to_string()[..12].to_owned();
        let cfg = capsule_config(server);
        // The report path the agent is told; `capsule reply` reads it back from
        // this event, so the agent's CLI need not share herdr-mcp's work dir.
        let report = herdr_capsule::report_path(&cfg, &capsule.task, &capsule.id);
        let baseline = json!({"pane": pane.pane_id, "state_change_seq": pane.seq,
            "submitted": false, "report": report});
        if !self
            .record_delivery(server, &lease, &delivery, baseline)
            .await?
        {
            return Ok(Step::Empty);
        }
        let text = delivery_text(&capsule, &cfg, Some(&delivery));
        if let Err(error) = postman.herdr.prompt(&pane.pane_id, &text).await {
            let (l, now) = (lease.clone(), self.now());
            self.db(server, move |db| Ok(db.release(&l, now)?)).await?;
            return Ok(Step::Released(format!("{error:#}")));
        }
        let typed = json!({"pane": pane.pane_id, "state_change_seq": pane.seq,
            "submitted": true, "report": report});
        if !self
            .record_delivery(server, &lease, &delivery, typed)
            .await?
        {
            return Ok(Step::Empty);
        }
        let (l, wait, now) = (
            lease.clone(),
            number(server, MAIL_REPLY_WAIT_MS),
            self.now(),
        );
        self.db(server, move |db| Ok(db.heartbeat(&l, wait, now)?))
            .await?;
        let confirm = ms(server, MAIL_SUBMIT_CONFIRM_MS);
        lock(&postman.flights).insert(
            label.to_owned(),
            InFlight {
                lease,
                capsule,
                delivery: delivery.clone(),
                pane: pane.pane_id.clone(),
                seq: pane.seq,
                delivered_at: now,
                confirm_at: Some(now.saturating_add(confirm)),
            },
        );
        Ok(Step::Delivered(delivery))
    }

    /// A fenced `delivered` event; false when the lease went stale.
    async fn record_delivery(
        &self,
        server: &HerdrMcpServer,
        lease: &Lease,
        delivery: &str,
        data: Value,
    ) -> Result<bool> {
        let (l, d, now) = (lease.clone(), delivery.to_owned(), self.now());
        let applied = self
            .db(server, move |db| Ok(db.set_delivery(&l, &d, data, now)?))
            .await?;
        Ok(applied == Fenced::Applied(()))
    }

    async fn follow(
        &self,
        server: &HerdrMcpServer,
        postman: &Postman,
        label: &str,
        mut flight: InFlight,
        pane: Option<&MailPane>,
        pane_closed: bool,
    ) -> Result<Step> {
        let reply_wait = ms(server, MAIL_REPLY_WAIT_MS);
        let (l, wait, now) = (
            flight.lease.clone(),
            number(server, MAIL_REPLY_WAIT_MS),
            self.now(),
        );
        let beat = self
            .db(server, move |db| Ok(db.heartbeat(&l, wait, now)?))
            .await?;
        if beat == Fenced::Stale {
            lock(&postman.flights).remove(label);
            return Ok(Step::Empty); // reaped, cancelled or taken over
        }
        let done = |step: Step| {
            lock(&postman.flights).remove(label);
            step
        };
        let same_pane = pane.filter(|p| p.pane_id == flight.pane);
        // Two known counters are needed to classify a change; missing is neither a
        // change nor equality.
        let known = same_pane.and_then(|p| p.seq).zip(flight.seq);
        let turn_moved = known.is_some_and(|(now, before)| now != before);
        let ready = same_pane.is_some_and(|p| READY.contains(&p.status.as_str()));
        if flight.capsule.status == "task" {
            let (re, d) = (flight.capsule.id.clone(), flight.delivery.clone());
            let reply = self
                .db(server, move |db| Ok(db.find_reply(&re, &d)?))
                .await?;
            if let Some(reply) = reply {
                let result = json!({"reply": reply.id, "delivery": flight.delivery});
                return Ok(
                    if self
                        .finish_mail(server, &flight.lease, Outcome::Done(result))
                        .await?
                    {
                        done(Step::Replied(reply.id))
                    } else {
                        done(Step::Empty)
                    },
                );
            }
        } else if turn_moved && ready {
            let result = json!({"delivery": flight.delivery});
            return Ok(
                if self
                    .finish_mail(server, &flight.lease, Outcome::Done(result))
                    .await?
                {
                    done(Step::TurnEnded)
                } else {
                    done(Step::Empty)
                },
            );
        }
        if pane_closed {
            let reason = format!(
                "pane {} closed during delivery {}",
                flight.pane, flight.delivery
            );
            let (lease, failure) = (flight.lease.clone(), reason.clone());
            let finished = self
                .db(server, move |db| {
                    Ok(db.finish(&lease, Outcome::Failed(failure), now)?)
                })
                .await?;
            return Ok(done(match finished {
                Fenced::Stale => Step::Empty,
                Fenced::Applied(item) if item.status == ItemStatus::Dead => Step::Dead(reason),
                Fenced::Applied(_) => Step::Released(reason),
            }));
        }
        // A live or unconfirmed delivery still uses its original deadline.
        if now.saturating_sub(flight.delivered_at) >= reply_wait {
            let what = if flight.capsule.status == "task" {
                "no reply"
            } else {
                "no turn end"
            };
            let reason = format!(
                "{what} from {label} within {} min (delivery {})",
                reply_wait / 60_000,
                flight.delivery
            );
            let applied = if flight.capsule.status == "task" {
                let notice = Self::timeout_notice(&flight, &reason)?;
                let (lease, reason, now) = (flight.lease.clone(), reason.clone(), self.now());
                self.db(server, move |db| {
                    Ok(matches!(
                        db.finish_dead_with_notice(&lease, reason, notice, now)?,
                        Fenced::Applied(_)
                    ))
                })
                .await?
            } else {
                self.finish_mail(server, &flight.lease, Outcome::Dead(reason.clone()))
                    .await?
            };
            return Ok(done(if applied {
                Step::Dead(reason)
            } else {
                Step::Empty
            }));
        }
        let mut step = Step::Waiting;
        if flight.capsule.status == "task" && ready && turn_moved {
            let lease = flight.lease.clone();
            let re = flight.capsule.id.clone();
            let delivery = flight.delivery.clone();
            let remind_after = ms(server, MAIL_REMIND_AFTER_MS);
            match self
                .db(server, move |db| {
                    Ok(db.reserve_reply_reminder(&lease, &re, &delivery, remind_after, now)?)
                })
                .await?
            {
                Fenced::Stale => return Ok(done(Step::Empty)),
                Fenced::Applied(true) => {
                    let command = reply_command(
                        &flight.capsule,
                        &capsule_config(server),
                        Some(&flight.delivery),
                    );
                    let text = format!(
                        "Reminder: reply to capsule {} with: {command}",
                        flight.capsule.id
                    );
                    match postman.herdr.prompt(&flight.pane, &text).await {
                        Ok(()) => step = Step::Reminded,
                        Err(error) => tracing::warn!("mail {label}: reply reminder: {error:#}"),
                    }
                }
                Fenced::Applied(false) => {}
            }
        }
        if let Some(at) = flight.confirm_at
            && now >= at
        {
            flight.confirm_at = None; // exactly one extra Enter, even if it fails
            if ready && known.is_some() && !turn_moved {
                match postman.herdr.enter(&flight.pane).await {
                    Ok(()) => step = Step::Nudged,
                    Err(error) => tracing::warn!("mail {label}: extra enter: {error:#}"),
                }
            }
        }
        lock(&postman.flights).insert(label.to_owned(), flight);
        Ok(step)
    }

    /// Fenced finish; false when the lease went stale (someone else owns it).
    async fn finish_mail(
        &self,
        server: &HerdrMcpServer,
        lease: &Lease,
        outcome: Outcome,
    ) -> Result<bool> {
        let (l, now) = (lease.clone(), self.now());
        self.db(server, move |db| {
            Ok(matches!(db.finish(&l, outcome, now)?, Fenced::Applied(_)))
        })
        .await
    }

    fn timeout_notice(flight: &InFlight, reason: &str) -> Result<Push> {
        let task = &flight.capsule;
        Self::notice_push(
            task,
            format!("dead-{}-{}", task.id, flight.delivery),
            reason,
        )
    }

    fn notice_push(task: &Capsule, id: String, reason: &str) -> Result<Push> {
        let to = task
            .reply_to
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| task.from.clone());
        let queue = format!("mail-{to}");
        let notice = Capsule {
            v: 2,
            id: id.clone(),
            task: task.task.clone(),
            from: "herdr-mcp".into(),
            to,
            status: "fail".into(),
            summary: reason.chars().take(120).collect(),
            // No `delivery`: find_reply must never mistake the notice for the reply.
            re: Some(task.id.clone()),
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            ..Capsule::default()
        };
        let Value::Object(payload) = serde_json::to_value(&notice)? else {
            anyhow::bail!("a capsule serializes to an object");
        };
        Ok(Push {
            queue,
            payload,
            dedupe_key: Some(id),
            coalesce_key: None,
            requester: Some("herdr-mcp".into()),
            max_attempts: 1,
            max_depth: None,
        })
    }
}

#[cfg(test)]
#[path = "mail_tests.rs"]
mod tests;

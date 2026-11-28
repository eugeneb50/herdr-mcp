use crate::{Capsule, Result, fail, py_repr};
use herdr_queue::{Fenced, Item, ItemStatus, Lease, Outcome, Push, Pushed, Queue, QueueDepth};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
pub struct Store {
    q: Queue,
}
impl Store {
    pub fn open(db: &Path) -> Result<Self> {
        Ok(Self {
            q: herdr_queue::Queue::open(db)?,
        })
    }
    /// Push `payload` reserving its id (see `Queue::push_unique`).
    pub fn push(
        &mut self,
        queue: &str,
        payload: &Capsule,
        requester: &str,
        max_attempts: u32,
    ) -> Result<Pushed> {
        let value = serde_json::to_value(payload)?;
        let payload = value
            .as_object()
            .cloned()
            .ok_or_else(|| fail("payload is not an object"))?;
        let key = payload_id(&payload)?;
        Ok(self.q.push_unique(
            Push {
                queue: queue.to_string(),
                payload,
                dedupe_key: Some(key),
                coalesce_key: None,
                requester: Some(requester.to_string()),
                max_attempts,
                max_depth: None,
            },
            now_ms(),
        )?)
    }
    pub fn list(
        &mut self,
        queue: Option<&str>,
        status: Option<ItemStatus>,
        limit: u32,
    ) -> Result<Vec<Item>> {
        Ok(self.q.list(queue, status, limit)?)
    }
    pub fn depths(&mut self) -> Result<Vec<QueueDepth>> {
        Ok(self.q.depths()?)
    }
    pub fn mailboxes(&mut self) -> Result<Vec<String>> {
        let mut names: Vec<String> = self
            .depths()?
            .iter()
            .filter_map(|d| d.queue.strip_prefix("mail-").map(str::to_string))
            .collect();
        names.sort();
        Ok(names)
    }
    /// The reply already accepted for delivery `delivery` of task `re`.
    pub fn find_reply(&self, re: &str, delivery: &str) -> Result<Option<Item>> {
        Ok(self.q.find_reply(re, delivery)?)
    }
    /// The items in `queue` whose capsule id is `id`.
    pub fn items_with_id(&mut self, queue: &str, id: &str) -> Result<Vec<Item>> {
        Ok(self
            .q
            .find_key(id)?
            .into_iter()
            .map(|(item, _)| item)
            .filter(|item| item.queue == queue)
            .collect())
    }
    /// Mailbox items whose capsule id is `id`, with their events.
    fn mail_items(&mut self, id: &str) -> Result<Vec<(Item, Vec<herdr_queue::QueueEvent>)>> {
        Ok(self
            .q
            .find_key(id)?
            .into_iter()
            .filter(|(item, _)| item.queue.starts_with("mail-"))
            .collect())
    }
    /// The task delivered into `pane` that is still waiting for its reply:
    /// (task capsule, delivery id). Read from the item's last `delivered` event.
    pub fn open_delivery(&mut self, pane: &str) -> Result<Option<(Capsule, String)>> {
        for item in self.q.list_running_delivered()? {
            let Some((_, events)) = self.q.item(&item.id)? else {
                continue;
            };
            let delivered_here = events
                .iter()
                .rev()
                .find(|e| e.kind == "delivered")
                .and_then(|e| e.data.as_ref())
                .is_some_and(|d| d["pane"] == pane);
            if !delivered_here {
                continue;
            }
            if let (Ok(cap), Some(delivery)) = (
                Capsule::from_value(&serde_json::Value::Object(item.payload.clone())),
                item.delivery.clone(),
            ) && cap.status == "task"
            {
                return Ok(Some((cap, delivery)));
            }
        }
        Ok(None)
    }
    /// The report path recorded by the postman for delivery `delivery` of the
    /// task capsule `id`, if any.
    pub fn delivery_report(
        &mut self,
        id: &str,
        delivery: &str,
    ) -> Result<Option<std::path::PathBuf>> {
        for (_, events) in self.mail_items(id)? {
            let found = events
                .iter()
                .rev()
                .filter(|e| e.kind == "delivered")
                .filter_map(|e| e.data.as_ref())
                .find(|d| d["delivery"] == delivery)
                .and_then(|d| d["report"].as_str())
                .map(std::path::PathBuf::from);
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }
    pub fn find_capsule(&mut self, id: &str) -> Result<Option<Capsule>> {
        let mut found = Vec::new();
        for (item, _) in self.mail_items(id)? {
            if let Ok(cap) = Capsule::from_value(&serde_json::Value::Object(item.payload)) {
                found.push(cap);
            }
        }
        if found.len() > 1 {
            return Err(fail(format!(
                "more than one capsule has id {id}; ask the lead"
            )));
        }
        Ok(found.pop())
    }
    /// The item in `queue` whose capsule id is `reference`, else the one
    /// whose queue item id starts with it.
    pub fn find_q(&mut self, queue: &str, reference: &str) -> Result<Item> {
        let by_capsule = self.items_with_id(queue, reference)?;
        let hits: Vec<Item> = if by_capsule.is_empty() {
            self.list(Some(queue), None, u32::MAX)?
                .into_iter()
                .filter(|it| it.id.starts_with(reference))
                .collect()
        } else {
            by_capsule
        };
        if hits.len() > 1 {
            return Err(fail(format!(
                "{} items in {queue} match {}",
                hits.len(),
                py_repr(reference)
            )));
        }
        hits.into_iter()
            .next()
            .ok_or_else(|| fail(format!("no item in {queue} matches {}", py_repr(reference))))
    }
    pub fn claim(&mut self, queue: &str, lease_ms: u64) -> Result<Option<(Item, Lease)>> {
        Ok(self.q.claim(queue, lease_ms, now_ms())?)
    }
    pub fn finish(&mut self, lease: &Lease) -> Result<bool> {
        Ok(matches!(
            self.q
                .finish(lease, Outcome::Done(serde_json::Value::Null), now_ms())?,
            Fenced::Applied(_)
        ))
    }
}
fn payload_id(payload: &serde_json::Map<String, serde_json::Value>) -> Result<String> {
    payload
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| fail("capsule has no id"))
}
/// Push `cap`, drawing a new id from `mint` while the queue says its id is
/// taken. A reply to a delivery that already has one is refused, not retried.
pub fn push_unique<M: FnMut() -> Result<String>>(
    store: &mut Store,
    queue: &str,
    cap: &mut Capsule,
    max_attempts: u32,
    mut mint: M,
) -> Result<()> {
    for _ in 0..20 {
        let from = cap.from.clone();
        match store.push(queue, cap, &from, max_attempts) {
            Ok(Pushed::New(_)) => return Ok(()),
            Ok(other) => return Err(fail(format!("queue did not add the capsule: {other:?}"))),
            Err(crate::Error::Queue(herdr_queue::QueueError::KeyTaken { .. })) => cap.id = mint()?,
            Err(crate::Error::Queue(herdr_queue::QueueError::AlreadyAnswered { reply })) => {
                return Err(fail(format!(
                    "task {} delivery {} already has its reply {reply}; nothing sent",
                    cap.re.as_deref().unwrap_or("?"),
                    cap.delivery.as_deref().unwrap_or("?"),
                )));
            }
            Err(e) => return Err(e),
        }
    }
    Err(fail("could not mint a unique id after 20 tries"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::line;
    fn racing_store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let mut s = Store::open(&d.path().join("q.db")).unwrap();
        let pre = Capsule {
            id: "kalabo".into(),
            ..Capsule::default()
        };
        s.push("mail-a", &pre, "lead", 2).unwrap();
        (d, s)
    }
    fn holders(s: &mut Store, id: &str) -> usize {
        s.q.find_key(id).unwrap().len()
    }
    #[test]
    fn racing_sender_id_is_minted_again() {
        let (_d, mut s) = racing_store();
        let mut c = Capsule {
            id: "kalabo".into(),
            ..Capsule::default()
        };
        let mut ids = ["tirumo"].into_iter();
        // A different mailbox: ids are reserved across all of them.
        push_unique(&mut s, "mail-b", &mut c, 2, || {
            Ok(ids.next().unwrap().into())
        })
        .unwrap();
        assert_eq!(
            (holders(&mut s, "kalabo"), holders(&mut s, "tirumo")),
            (1, 1)
        );
        assert!(line(&c, "cap", &crate::format::Glyphs::defaults()).starts_with("CAP tirumo"));
    }
    #[test]
    fn racing_minter_gives_up() {
        let (_d, mut s) = racing_store();
        let mut c = Capsule {
            id: "kalabo".into(),
            ..Capsule::default()
        };
        let mut draws = 0;
        let e = push_unique(&mut s, "mail-a", &mut c, 2, || {
            draws += 1;
            Ok("kalabo".into())
        })
        .unwrap_err();
        assert!(e.to_string().contains("20 tries"));
        assert_eq!(draws, 20, "one draw per refused push, none after the last");
        assert_eq!(holders(&mut s, "kalabo"), 1);
    }
    #[test]
    fn a_second_reply_is_refused_without_a_new_draw() {
        let (_d, mut s) = racing_store();
        let reply = |id: &str| Capsule {
            id: id.into(),
            status: "done".into(),
            re: Some("kalabo".into()),
            delivery: Some("d1".into()),
            ..Capsule::default()
        };
        push_unique(
            &mut s,
            "mail-lead",
            &mut reply("rotisu"),
            2,
            || unreachable!(),
        )
        .unwrap();
        let e = push_unique(&mut s, "mail-lead", &mut reply("tirumo"), 2, || {
            panic!("an answered delivery must not mint")
        })
        .unwrap_err();
        assert!(
            e.to_string().contains("already has its reply rotisu"),
            "{e}"
        );
        assert_eq!(holders(&mut s, "tirumo"), 0);
    }
}

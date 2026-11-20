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
    pub fn push(
        &mut self,
        queue: &str,
        payload: &Capsule,
        dedupe: &str,
        requester: &str,
        max_attempts: u32,
    ) -> Result<Pushed> {
        let value = serde_json::to_value(payload)?;
        let payload = value
            .as_object()
            .cloned()
            .ok_or_else(|| fail("payload is not an object"))?;
        Ok(self.q.push(
            Push {
                queue: queue.to_string(),
                payload,
                dedupe_key: Some(dedupe.to_string()),
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
    pub fn all_keys(&mut self) -> Result<Vec<String>> {
        Ok(self
            .list(None, None, u32::MAX)?
            .iter()
            .filter_map(|it| it.dedupe_key.clone())
            .collect())
    }
    pub fn find_capsule(&mut self, id: &str) -> Result<Option<Capsule>> {
        let mut found = Vec::new();
        for to in self.mailboxes()? {
            let queue = format!("mail-{to}");
            for item in self.list(Some(&queue), None, u32::MAX)? {
                if item.dedupe_key.as_deref() == Some(id)
                    && let Ok(cap) =
                        Capsule::from_value(&serde_json::Value::Object(item.payload.clone()))
                {
                    found.push(cap);
                }
            }
        }
        if found.len() > 1 {
            return Err(fail(format!(
                "more than one capsule has id {id}; ask the lead"
            )));
        }
        Ok(found.pop())
    }
    pub fn find_q(&mut self, queue: &str, reference: &str) -> Result<Item> {
        let hits: Vec<Item> = self
            .list(Some(queue), None, u32::MAX)?
            .into_iter()
            .filter(|it| it.id.starts_with(reference))
            .collect();
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
    pub fn release(&mut self, lease: &Lease) -> Result<bool> {
        Ok(matches!(
            self.q.release(lease, now_ms())?,
            Fenced::Applied(())
        ))
    }
}
pub fn push_unique<M: FnMut(&mut Store) -> Result<String>>(
    store: &mut Store,
    queue: &str,
    cap: &mut Capsule,
    max_attempts: u32,
    mut mint: M,
) -> Result<()> {
    for _ in 0..20 {
        let dedupe = cap.id.clone();
        let pushed = store.push(queue, cap, &dedupe, &cap.from, max_attempts)?;
        if matches!(pushed, herdr_queue::Pushed::New(_)) {
            return Ok(());
        }
        cap.id = mint(store)?;
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
        s.push("q", &pre, "kalabo", "lead", 2).unwrap();
        (d, s)
    }
    #[test]
    fn racing_sender_id_is_minted_again() {
        let (_d, mut s) = racing_store();
        let mut c = Capsule {
            id: "kalabo".into(),
            ..Capsule::default()
        };
        let mut ids = ["kalabo", "tirumo"].into_iter();
        push_unique(&mut s, "q", &mut c, 2, |_| Ok(ids.next().unwrap().into())).unwrap();
        let keys = s.all_keys().unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"tirumo".to_string()));
        assert!(line(&c, "cap").starts_with("CAP tirumo"));
    }
    #[test]
    fn racing_minter_gives_up() {
        let (_d, mut s) = racing_store();
        let mut c = Capsule {
            id: "kalabo".into(),
            ..Capsule::default()
        };
        let e = push_unique(&mut s, "q", &mut c, 2, |_| Ok("kalabo".into())).unwrap_err();
        assert!(e.to_string().contains("20 tries"));
        assert_eq!(s.all_keys().unwrap().len(), 1);
    }
}

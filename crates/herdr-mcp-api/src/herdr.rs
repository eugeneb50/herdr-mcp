//! herdr's own event stream as the bridge relays it (`GET /api/herdr/events`)
//! and recipe triggers that fire on it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One event from herdr's `events.subscribe` stream. `event` is always the
/// subscription name (`pane.created`, `pane.agent_status_changed`, …): herdr
/// names global events `pane_created` on the wire, which [`HerdrEvent::parse`]
/// maps back. `data` is herdr's payload as-is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HerdrEvent {
    pub event: String,
    #[serde(default)]
    pub data: Value,
}

impl HerdrEvent {
    /// Decode one line of the stream; `None` for acks, errors and blanks.
    pub fn parse(line: &str) -> Option<Self> {
        let mut ev: Self = serde_json::from_str(line).ok()?;
        if !ev.event.contains('.') {
            let (scope, rest) = ev.event.split_once('_')?;
            ev.event = format!("{scope}.{rest}");
        }
        Some(ev)
    }

    /// The workspace the event happened in: `data.workspace_id`, else the
    /// `workspace_id` of the object it carries (`data.pane`, `data.tab`, …).
    pub fn workspace_id(&self) -> Option<&str> {
        let direct = self.data.get("workspace_id").and_then(Value::as_str);
        direct.or_else(|| {
            self.data
                .as_object()?
                .values()
                .find_map(|v| v.get("workspace_id")?.as_str())
        })
    }
}

/// Starts a recipe run when herdr emits a matching event. The run's scope
/// gets `herdr` = the event's `data` plus `event` (so steps use
/// `{{herdr.pane_id}}`, `{{herdr.pane.pane_id}}`, …) and runs in the event's
/// workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeTrigger {
    /// herdr subscription name, e.g. `pane.agent_status_changed`.
    pub on: String,
    /// Every entry must equal the event's value at that dotted path in
    /// `data` (e.g. `agent_status = "done"`, `pane.workspace_id = "w1"`).
    #[serde(default)]
    pub when: serde_json::Map<String, Value>,
}

impl RecipeTrigger {
    pub fn matches(&self, ev: &HerdrEvent) -> bool {
        self.on == ev.event
            && self.when.iter().all(|(path, want)| {
                path.split('.')
                    .try_fold(&ev.data, |v, key| v.get(key))
                    .is_some_and(|got| got == want)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_normalizes_global_event_names() {
        let line = r#"{"data":{"pane":{"pane_id":"w1:p3","workspace_id":"w1"},"type":"pane_created"},"event":"pane_created"}"#;
        let ev = HerdrEvent::parse(line).unwrap();
        assert_eq!(ev.event, "pane.created");
        assert_eq!(ev.workspace_id(), Some("w1"));

        let line = r#"{"data":{"agent_status":"idle","pane_id":"w1:p1","workspace_id":"w2"},"event":"pane.agent_status_changed"}"#;
        let ev = HerdrEvent::parse(line).unwrap();
        assert_eq!(ev.event, "pane.agent_status_changed");
        assert_eq!(ev.workspace_id(), Some("w2"));

        let ev = HerdrEvent::parse(r#"{"data":{},"event":"workspace_metadata_updated"}"#).unwrap();
        assert_eq!(ev.event, "workspace.metadata_updated");
    }

    #[test]
    fn parse_skips_acks() {
        assert!(
            HerdrEvent::parse(r#"{"id":"x","result":{"type":"subscription_started"}}"#).is_none()
        );
        assert!(HerdrEvent::parse("").is_none());
    }

    #[test]
    fn trigger_matches_name_and_paths() {
        let ev = HerdrEvent {
            event: "pane.agent_status_changed".into(),
            data: json!({"agent_status": "done", "agent": "claude", "pane": {"tab_id": "w1:t1"}}),
        };
        let t = |on: &str, when: Value| RecipeTrigger {
            on: on.into(),
            when: when.as_object().cloned().unwrap_or_default(),
        };
        assert!(t("pane.agent_status_changed", json!({})).matches(&ev));
        assert!(t("pane.agent_status_changed", json!({"agent_status": "done"})).matches(&ev));
        assert!(t("pane.agent_status_changed", json!({"pane.tab_id": "w1:t1"})).matches(&ev));
        assert!(!t("pane.agent_status_changed", json!({"agent_status": "idle"})).matches(&ev));
        assert!(!t("pane.agent_status_changed", json!({"missing": "x"})).matches(&ev));
        assert!(!t("pane.created", json!({})).matches(&ev));
    }
}

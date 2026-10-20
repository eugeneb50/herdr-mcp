//! herdr's own data as the bridge relays it: the event stream
//! (`GET /api/herdr/events`) and recipe triggers that fire on it, panes
//! (`GET /api/herdr/panes`) and named sessions (`GET /api/herdr/sessions`).

use schemars::JsonSchema;
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

/// One pane as herdr's `pane list` reports it. herdr owns this data; the
/// bridge only relays it (fields herdr adds later are ignored).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HerdrPane {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    #[serde(default)]
    pub focused: bool,
    /// `idle`, `working`, `blocked`, `done` or `unknown`.
    #[serde(default)]
    pub agent_status: String,
    /// Detected agent (`claude`, `codex`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// cwd of the pane's foreground process, when it differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_title: Option<String>,
}

/// One named herdr session (`herdr session list --json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HerdrSession {
    pub name: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_error: Option<String>,
    pub socket_path: String,
    pub session_dir: String,
    /// The session this bridge talks to (set by the bridge per request, not
    /// by herdr). Writes and trim policies only ever act on this one.
    #[serde(default)]
    pub current: bool,
}

/// Which panes to list: `list_panes` params and `GET /api/herdr/panes`
/// query. Every field narrows; none set means every pane of the session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PaneFilter {
    /// herdr session name (see `list_sessions`); default: the one this server uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    /// Exact agent status: `idle`, `working`, `blocked`, `done`, `unknown`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<String>,
    /// Only the focused pane (`true`) or only unfocused ones (`false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused: Option<bool>,
    /// Case-insensitive substring of the pane's label or agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl PaneFilter {
    pub fn matches(&self, pane: &HerdrPane) -> bool {
        // An empty string filters nothing, as in `panes()`'s herdr args.
        let eq = |want: &Option<String>, got: &str| {
            want.as_deref().is_none_or(|w| w.is_empty() || w == got)
        };
        eq(&self.workspace_id, &pane.workspace_id)
            && eq(&self.tab_id, &pane.tab_id)
            && eq(&self.agent_status, &pane.agent_status)
            && self.focused.is_none_or(|f| f == pane.focused)
            && self.label.as_deref().is_none_or(|want| {
                if want.is_empty() {
                    return true;
                }
                let want = want.to_lowercase();
                [&pane.label, &pane.agent]
                    .into_iter()
                    .flatten()
                    .any(|s| s.to_lowercase().contains(&want))
            })
    }
}

/// `GET /api/herdr/panes` response and `list_panes` result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PanesResponse {
    pub panes: Vec<HerdrPane>,
}

/// `GET /api/herdr/sessions` response and `list_sessions` result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionsResponse {
    pub sessions: Vec<HerdrSession>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pane_decodes_herdr_output_and_filters() {
        // Trimmed from herdr 0.9.5 `pane list`; extra fields are ignored.
        let pane: HerdrPane = serde_json::from_value(json!({
            "agent": "hermes", "agent_status": "idle", "cwd": "/h", "focused": true,
            "label": "Reviewer", "pane_id": "w1:p2", "revision": 1, "tab_id": "w1:t1",
            "scroll": {"viewport_rows": 23}, "workspace_id": "w1"
        }))
        .unwrap();
        assert_eq!(pane.agent.as_deref(), Some("hermes"));
        let f = |v: Value| serde_json::from_value::<PaneFilter>(v).unwrap();
        assert!(f(json!({})).matches(&pane));
        assert!(
            f(json!({"workspace_id": "w1", "tab_id": "w1:t1", "focused": true})).matches(&pane)
        );
        assert!(f(json!({"label": "review"})).matches(&pane));
        assert!(f(json!({"label": "HERM"})).matches(&pane));
        let shell = HerdrPane {
            pane_id: "w1:p3".into(),
            ..Default::default()
        };
        assert!(f(json!({"label": ""})).matches(&shell));
        assert!(!f(json!({"agent_status": "working"})).matches(&pane));
        assert!(!f(json!({"focused": false})).matches(&pane));
        assert!(!f(json!({"workspace_id": "w2"})).matches(&pane));
        assert!(serde_json::from_value::<PaneFilter>(json!({"bogus": 1})).is_err());
    }

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

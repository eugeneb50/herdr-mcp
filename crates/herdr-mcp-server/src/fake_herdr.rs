//! A fake herdr for tests: its API socket (`events.subscribe` and one-shot
//! calls) and a `herdr` CLI script that tools run through `HERDR_BIN`.
//!
//! The CLI logs every call. `pane split` answers with a new pane in tab
//! `w1:t2` and, like herdr, announces it with a `pane.created` event on the
//! socket; `pane list` answers with the panes [`FakeHerdr::set_panes`] gave.
//! Setting `HERDR_BIN` is process-wide, so one fake lives at a time: each
//! holds a process lock until dropped.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

pub struct FakeHerdr {
    dir: tempfile::TempDir,
    events: broadcast::Sender<String>,
    _one_at_a_time: tokio::sync::MutexGuard<'static, ()>,
}

static IN_USE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl FakeHerdr {
    pub async fn start() -> Self {
        let one_at_a_time = IN_USE.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let bin = dir.path().join("herdr");
        std::fs::write(&bin, script(&log)).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        // SAFETY: tests using the fake own their process (see the module doc).
        unsafe { std::env::set_var("HERDR_BIN", &bin) };

        let (events, _) = broadcast::channel(64);
        let listener = UnixListener::bind(dir.path().join("herdr.sock")).unwrap();
        let tx = events.clone();
        let fake_dir = dir.path().to_path_buf();
        tokio::spawn(async move {
            while let Ok((conn, _)) = listener.accept().await {
                tokio::spawn(serve(conn, tx.clone(), fake_dir.clone()));
            }
        });
        tokio::spawn(announce_splits(log, events.clone()));
        Self {
            dir,
            events,
            _one_at_a_time: one_at_a_time,
        }
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.path().join("herdr.sock")
    }

    /// Send `event` to every subscriber, once one is connected.
    pub async fn emit(&self, event: &str, data: Value) {
        wait_for(|| self.events.receiver_count() > 0).await;
        let _ = self
            .events
            .send(json!({ "event": event, "data": data }).to_string());
    }

    /// The panes `pane list` reports from now on.
    pub fn set_panes(&self, panes: Value) {
        let list = json!({ "result": { "panes": panes } });
        std::fs::write(self.dir.path().join("panes.json"), list.to_string()).unwrap();
    }

    /// The CLI calls so far, one line of arguments each.
    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// Poll `ready` for up to 5 s.
pub async fn wait_for(mut ready: impl FnMut() -> bool) {
    for _ in 0..500 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out");
}

fn script(log: &Path) -> String {
    let panes = log.with_file_name("panes.json");
    let panes = panes.display();
    let log = log.display();
    format!(
        r#"#!/bin/sh
echo "$*" >> "{log}"
case "$1 $2" in
"api schema") echo '{{"schemas":{{"request":{{"$defs":{{"Subscription":{{"oneOf":[{{"properties":{{"type":{{"const":"pane.created"}}}},"required":["type"]}},{{"properties":{{"type":{{"const":"pane.closed"}}}},"required":["type"]}},{{"properties":{{"type":{{"const":"workspace.metadata_updated"}}}},"required":["type"]}}]}}}}}}}}}}' ;;
"pane split")
  n=$(grep -c '^pane split' "{log}")
  echo "{{\"result\":{{\"pane\":{{\"pane_id\":\"w1:p$((n + 1))\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t2\"}}}}}}" ;;
"pane list") if [ -f "{panes}" ]; then cat "{panes}"; else echo '{{"result":{{"panes":[]}}}}'; fi ;;
"agent start") echo '{{"result":{{}}}}' ;;
"agent wait") echo '{{"result":{{"agent":{{"agent_status":"idle"}}}}}}' ;;
"agent prompt") if [ "$4" = "block" ]; then echo '{{"result":{{"agent":{{"agent_status":"blocked"}}}}}}'; else echo '{{"result":{{"agent":{{"agent_status":"idle"}}}}}}'; fi ;;
"pane sleep") sleep "$3"; echo '{{"result":{{}}}}' ;;
"pane read") echo 'approval required' ;;
"pane rename") echo '{{"result":{{}}}}' ;;
*) echo "fake herdr: no $*" >&2; exit 1 ;;
esac
"#
    )
}

/// One socket connection: a subscription streams events after its ack;
/// any other method gets an empty result.
async fn serve(conn: UnixStream, events: broadcast::Sender<String>, dir: PathBuf) {
    let (read, mut write) = conn.into_split();
    let mut lines = BufReader::new(read).lines();
    let Ok(Some(line)) = lines.next_line().await else {
        return;
    };
    let req: Value = serde_json::from_str(&line).unwrap_or_default();
    let subscriptions = req["params"]["subscriptions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut rx = events.subscribe();
    if subscriptions
        .iter()
        .any(|s| s["type"] == "pane.agent_status_changed")
    {
        let _ = std::fs::write(dir.join("status_pending"), "pending");
        while dir.join("hold_status").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    let ack = json!({ "id": req["id"], "result": {} });
    if write
        .write_all(format!("{ack}\n").as_bytes())
        .await
        .is_err()
        || req["method"] != "events.subscribe"
    {
        return;
    }
    loop {
        tokio::select! {
            incoming = lines.next_line() => {
                if !matches!(incoming, Ok(Some(_))) { return; }
            }
            message = rx.recv() => {
                let Ok(event) = message else { return };
                let parsed: Value = serde_json::from_str(&event).unwrap_or_default();
                let matches = subscriptions.iter().any(|s| {
                    s["type"] == parsed["event"]
                        && (s.get("pane_id").is_none() || s["pane_id"] == parsed["data"]["pane_id"])
                });
                if matches && write.write_all(format!("{event}\n").as_bytes()).await.is_err() { return; }
            }
        }
    }
}

/// herdr announces every pane it splits off.
async fn announce_splits(log: PathBuf, events: broadcast::Sender<String>) {
    let mut seen = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        let splits = calls
            .lines()
            .filter(|l| l.starts_with("pane split"))
            .count();
        while seen < splits {
            seen += 1;
            let pane = json!({ "pane_id": format!("w1:p{}", seen + 1), "workspace_id": "w1", "tab_id": "w1:t2" });
            let _ = events
                .send(json!({ "event": "pane.created", "data": { "pane": pane } }).to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::herdr_cli_at;
    use herdr_mcp_api::{EventLevel, RecipeStep, RecipeTrigger, RunStatus};
    use std::sync::Arc;

    #[tokio::test]
    async fn metadata_is_not_lost_and_new_pane_status_is_reconciled() {
        let herdr = FakeHerdr::start().await;
        herdr.set_panes(json!([]));
        std::fs::write(herdr.dir.path().join("hold_status"), "hold").unwrap();
        let client = Arc::new(crate::HerdrClient::new(herdr.socket(), 3, 10));
        let mut events = client.events();
        client.spawn_subscriber();
        wait_for(|| client.subscriber_state() == crate::herdr_client::SubscriberPhase::Connected)
            .await;
        herdr.set_panes(json!([{"pane_id":"w1:p1", "workspace_id":"w1", "tab_id":"w1:t1", "agent":"claude", "agent_status":"idle", "label":"worker"}]));
        herdr
            .emit(
                "pane.created",
                json!({"pane":{"pane_id":"w1:p1", "workspace_id":"w1"}}),
            )
            .await;
        wait_for(|| herdr.dir.path().join("status_pending").exists()).await;
        herdr.set_panes(json!([{"pane_id":"w1:p1", "workspace_id":"w1", "tab_id":"w1:t1", "agent":"claude", "agent_status":"blocked", "label":"worker"}]));
        herdr
            .emit(
                "workspace.metadata_updated",
                json!({"workspace_id":"w1", "key":"capsule"}),
            )
            .await;
        std::fs::remove_file(herdr.dir.path().join("hold_status")).unwrap();
        let mut metadata = 0;
        let mut blocked = 0;
        while metadata == 0 || blocked == 0 {
            let ev = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap();
            if ev.event == "workspace.metadata_updated" {
                metadata += 1;
            }
            if ev.event == "pane.agent_status_changed" && ev.data["agent_status"] == "blocked" {
                blocked += 1;
            }
        }
        assert_eq!(metadata, 1);
        assert_eq!(blocked, 1);
        assert_eq!(
            client.registry.get("w1:p1").await.unwrap().status,
            "blocked"
        );
    }

    /// herdr's event reaches a trigger over the socket, the run's step goes
    /// out through the CLI with the event's data filled in, and the pane it
    /// makes comes back as the next event.
    #[tokio::test]
    async fn a_herdr_event_triggers_a_run_end_to_end() {
        let herdr = FakeHerdr::start().await;
        let data = tempfile::tempdir().unwrap();
        let persistence = crate::Persistence::new(data.path().to_path_buf());
        persistence.init().await.unwrap();
        let persistence = Arc::new(persistence);
        let client = Arc::new(crate::HerdrClient::new(herdr.socket(), 3, 10));
        client.spawn_subscriber();
        let server = crate::HerdrMcpServer::new((*persistence).clone(), client.registry.clone())
            .with_herdr_client(client.clone());
        crate::triggers::spawn(server.clone(), client.clone());
        let mut created = client.events();

        let now = chrono::Utc::now();
        let recipe = crate::Recipe {
            id: uuid::Uuid::new_v4(),
            name: "split on new pane".into(),
            description: None,
            flow: herdr_mcp_api::Flow::chain(vec![RecipeStep {
                id: "split".into(),
                tool: "split_pane".into(),
                params: json!({"pane_id": "{{herdr.pane.pane_id}}", "direction": "down", "no_focus": true}),
                description: None,
                extract: Vec::new(),
            }]),
            variables: Default::default(),
            created_at: now,
            updated_at: now,
            is_template: false,
            category: None,
            // Only tab w1:t1, so the pane the run makes (w1:t2) ends it.
            triggers: vec![RecipeTrigger {
                on: "pane.created".into(),
                when: json!({"pane.tab_id": "w1:t1"})
                    .as_object()
                    .cloned()
                    .unwrap(),
            }],
            trail: Default::default(),
            queue: None,
        };
        persistence.save_recipe(&recipe).await.unwrap();

        herdr
            .emit(
                "pane.created",
                json!({"pane": {"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1"}}),
            )
            .await;

        let mut runs = Vec::new();
        for _ in 0..500 {
            runs = crate::runs::list(&server, None).await.unwrap();
            if runs.iter().any(|r| r.status.is_finished()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(runs.len(), 1, "one run: {runs:?}");
        assert_eq!(runs[0].status, RunStatus::Completed);
        assert_eq!(runs[0].session_id, "w1");
        assert!(
            herdr
                .calls()
                .iter()
                .any(|c| c.starts_with("pane split w1:p1 --direction down --no-focus")),
            "{:?}",
            herdr.calls()
        );
        // The split's own pane comes back from herdr.
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(5), created.recv())
                .await
                .unwrap()
                .unwrap();
            if ev.data["pane"]["pane_id"] == "w1:p2" {
                assert_eq!(ev.event, "pane.created");
                break;
            }
        }

        // The Events stream's diagnostics say what happened: subscribed,
        // the trigger fired with the run's id, resubscribed for the new
        // pane, and (debug) why the split's own pane didn't trigger it.
        let mut names = Vec::new();
        for _ in 0..500 {
            let (all, _) = client.recent_and_live(EventLevel::Debug);
            names = all.iter().map(|e| e.event.clone()).collect::<Vec<_>>();
            if let Some(checked) = all.iter().find(|e| e.event == "herdr-mcp.trigger_checked") {
                assert_eq!(checked.level, EventLevel::Debug);
                assert_eq!(
                    checked.data["no_match"],
                    json!([r#"pane.tab_id is "w1:t2", not "w1:t1""#])
                );
                let fired = all
                    .iter()
                    .find(|e| e.event == "herdr-mcp.trigger_fired")
                    .expect("fired");
                assert_eq!(fired.level, EventLevel::Reactions);
                assert_eq!(fired.data["run_id"], json!(runs[0].id));
                assert!(
                    names.iter().any(|n| n == "herdr-mcp.subscribed"),
                    "{names:?}"
                );

                // The default level is herdr's own events only.
                let (herdr_only, _) = client.recent_and_live(EventLevel::Events);
                assert!(
                    herdr_only
                        .iter()
                        .all(|e| !e.event.starts_with("herdr-mcp."))
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no trigger_checked entry: {names:?}");
    }

    /// A label names one agent: spawning it again returns the live pane,
    /// another agent under it is refused, and targets resolve from herdr's
    /// live pane list (a closed pane is not there to match).
    #[tokio::test]
    async fn prompt_ensure_reuses_or_spawns_and_roster_is_live() {
        use crate::security::Origin;
        use crate::server::dispatch_tool;
        let herdr = FakeHerdr::start().await;
        let data = tempfile::tempdir().unwrap();
        let store = crate::Persistence::new(data.path().to_path_buf());
        store.init().await.unwrap();
        let server = crate::HerdrMcpServer::new(store, crate::AgentRegistry::new());
        herdr.set_panes(json!([
            {"pane_id":"w1:p1", "workspace_id":"w1", "tab_id":"w1:t1", "agent":"claude", "agent_status":"idle", "label":"rev"}
        ]));
        server
            .registry
            .upsert("w1:p1", "w1", "w1:t1", "claude", "rev")
            .await;
        server.registry.set_status("w1:p1", "idle").await;
        let scope = crate::runs::scope(&server, "w1", Default::default(), Default::default()).await;
        assert_eq!(
            scope["agents"],
            json!([{"label":"rev","agent":"claude","status":"idle","pane_id":"w1:p1"}])
        );
        let mut prompt = json!("peers: {{ agents }}");
        crate::server::resolve_variables(&mut prompt, &scope);
        assert!(prompt.as_str().unwrap().contains("\"label\":\"rev\""));
        let call = |args| dispatch_tool(&server, "agent_prompt", args, Origin::Api);
        let result = call(json!({"target":"rev", "agent":"claude", "text":"hello"}))
            .await
            .unwrap();
        let payload = herdr_mcp_api::tool_payload(&serde_json::to_value(result).unwrap()).unwrap();
        assert_eq!(payload["spawned"], false);
        assert_eq!(payload["pane_id"], "w1:p1");
        let (_, error) = call(json!({"target":"rev", "agent":"codex", "text":"hello"}))
            .await
            .unwrap_err();
        assert!(error.contains("label `rev` is already pane"), "{error}");
        let result = call(json!({"target":"new", "agent":"claude", "cwd":"/tmp", "text":"hello"}))
            .await
            .unwrap();
        let payload = herdr_mcp_api::tool_payload(&serde_json::to_value(result).unwrap()).unwrap();
        assert_eq!(payload["spawned"], true);
        assert_eq!(payload["label"], "new");
        assert!(
            herdr
                .calls()
                .iter()
                .any(|c| c.starts_with("agent start new --kind claude"))
        );
        assert!(
            herdr
                .calls()
                .iter()
                .any(|c| c.starts_with("pane split") && c.contains("/tmp"))
        );
    }

    #[tokio::test]
    async fn labels_name_one_live_agent() {
        use crate::security::Origin;
        use crate::server::dispatch_tool;

        let herdr = FakeHerdr::start().await;
        let data = tempfile::tempdir().unwrap();
        let server = crate::HerdrMcpServer::new(
            crate::Persistence::new(data.path().to_path_buf()),
            crate::AgentRegistry::new(),
        );
        herdr.set_panes(json!([
            {"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1",
             "agent": "claude", "agent_status": "idle", "label": "rev"},
            {"pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t1", "label": "bare"},
        ]));
        let spawn = |args: Value| dispatch_tool(&server, "agent_spawn", args, Origin::Api);

        let out = spawn(json!({"agent": "claude", "label": "rev"}))
            .await
            .unwrap();
        let out = herdr_mcp_api::tool_payload(&serde_json::to_value(out).unwrap()).unwrap();
        assert_eq!(out["pane_id"], "w1:p1");
        assert_eq!(out["reused"], true);

        let (_, err) = spawn(json!({"agent": "codex", "label": "rev"}))
            .await
            .unwrap_err();
        assert!(
            err.contains("label `rev` is already pane w1:p1 (claude)"),
            "{err}"
        );
        // A plain shell pane is no agent of any profile, `shell` included.
        let (_, err) = spawn(json!({"agent": "shell", "label": "bare"}))
            .await
            .unwrap_err();
        assert!(
            err.contains("label `bare` is already pane w1:p2 (no agent)"),
            "{err}"
        );

        let (_, err) = spawn(json!({"agent": "shell", "needs": ["ghost"]}))
            .await
            .unwrap_err();
        assert!(
            err.contains("needs: No pane found with label 'ghost'"),
            "{err}"
        );
        assert!(
            !herdr.calls().iter().any(|c| c.starts_with("pane split")),
            "{:?}",
            herdr.calls()
        );

        herdr.set_panes(json!([]));
        let read = json!({"target": "rev"});
        let (_, err) = dispatch_tool(&server, "agent_read", read, Origin::Api)
            .await
            .unwrap_err();
        assert!(err.contains("No pane found with label 'rev'"), "{err}");
    }

    /// A wedged herdr (a `pane sleep` that outlives the call's `--timeout`)
    /// is killed and the error names the command. `--timeout 1` plus the
    /// 5 s grace makes our deadline ~5.001 s; the fake sleeps 10 s.
    #[tokio::test]
    async fn herdr_cli_times_out_on_wedged_herdr() {
        let _herdr = FakeHerdr::start().await;
        // A fast call still succeeds.
        let ok = herdr_cli_at(None, &["pane", "list"]).await.unwrap();
        assert!(ok.contains("panes"));
        // A wedged call times out after timeout + grace and names the command.
        let err = herdr_cli_at(None, &["pane", "sleep", "10000", "--timeout", "1"])
            .await
            .unwrap_err();
        let msg = err.message.to_string();
        assert!(
            msg.contains("timed out"),
            "expected a timeout error, got: {msg}"
        );
        assert!(
            msg.contains("pane sleep"),
            "expected the command named, got: {msg}"
        );
        assert!(
            msg.contains("5001ms"),
            "expected the deadline in ms, got: {msg}"
        );
    }
}

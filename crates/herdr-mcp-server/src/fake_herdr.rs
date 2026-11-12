//! A fake herdr for tests: its API socket (`events.subscribe` and one-shot
//! calls) and a `herdr` CLI script that tools run through `HERDR_BIN`.
//!
//! The CLI logs every call. `pane split` answers with a new pane in tab
//! `w1:t2` and, like herdr, announces it with a `pane.created` event on the
//! socket. Setting `HERDR_BIN` is process-wide: tests using this need their
//! own process (nextest).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

pub struct FakeHerdr {
    dir: tempfile::TempDir,
    events: broadcast::Sender<String>,
}

impl FakeHerdr {
    pub async fn start() -> Self {
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
        tokio::spawn(async move {
            while let Ok((conn, _)) = listener.accept().await {
                tokio::spawn(serve(conn, tx.clone()));
            }
        });
        tokio::spawn(announce_splits(log, events.clone()));
        Self { dir, events }
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
    let log = log.display();
    format!(
        r#"#!/bin/sh
echo "$*" >> "{log}"
case "$1 $2" in
"pane split")
  n=$(grep -c '^pane split' "{log}")
  echo "{{\"result\":{{\"pane\":{{\"pane_id\":\"w1:p$((n + 1))\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t2\"}}}}}}" ;;
"pane list") echo '{{"result":{{"panes":[]}}}}' ;;
*) echo "fake herdr: no $*" >&2; exit 1 ;;
esac
"#
    )
}

/// One socket connection: a subscription streams events after its ack;
/// any other method gets an empty result.
async fn serve(conn: UnixStream, events: broadcast::Sender<String>) {
    let (read, mut write) = conn.into_split();
    let mut lines = BufReader::new(read).lines();
    let Ok(Some(line)) = lines.next_line().await else {
        return;
    };
    let req: Value = serde_json::from_str(&line).unwrap_or_default();
    let ack = json!({ "id": req["id"], "result": {} });
    if write
        .write_all(format!("{ack}\n").as_bytes())
        .await
        .is_err()
        || req["method"] != "events.subscribe"
    {
        return;
    }
    let mut rx = events.subscribe();
    while let Ok(event) = rx.recv().await {
        if write
            .write_all(format!("{event}\n").as_bytes())
            .await
            .is_err()
        {
            return;
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
    use herdr_mcp_api::{RecipeStep, RecipeTrigger, RunStatus};
    use std::sync::Arc;

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
        crate::triggers::spawn(server.clone(), client.events());
        let mut created = client.events();

        let now = chrono::Utc::now();
        let recipe = crate::Recipe {
            id: uuid::Uuid::new_v4(),
            name: "split on new pane".into(),
            description: None,
            steps: vec![RecipeStep {
                id: "split".into(),
                tool: "split_pane".into(),
                params: json!({"pane_id": "{{herdr.pane.pane_id}}", "direction": "down", "no_focus": true}),
                description: None,
                extract: Vec::new(),
            }],
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
            gate: None,
            trail: Default::default(),
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
    }
}

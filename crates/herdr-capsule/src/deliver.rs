use crate::{Capsule, CapsuleConfig, Result, Store, fail, format::delivery_text};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    process::{Command, Output},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const DELIVERABLE: [&str; 2] = ["idle", "done"];
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pane {
    pub pane_id: String,
    pub label: String,
    pub agent_status: String,
    #[serde(default)]
    pub state_change_seq: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Fence {
    id: String,
    task: bool,
    at: f64,
    pane: String,
    state_change_seq: Option<i64>,
}
pub trait Herdr {
    fn json(&self, args: &[&str]) -> Result<Value>;
    fn prompt(&self, pane: &str, text: &str) -> Result<(bool, String)>;
    fn send_keys(&self, pane: &str, key: &str) -> Result<()>;
    fn report_metadata(&self, workspace: &str, tokens: &[(&str, &str)]) -> Result<()>;
}
pub struct CliHerdr(pub String);
impl CliHerdr {
    fn output(&self, args: &[&str]) -> Result<Output> {
        Ok(Command::new(&self.0).args(args).output()?)
    }
}
fn err_text(out: &Output) -> String {
    let bytes = if out.stderr.is_empty() {
        &out.stdout
    } else {
        &out.stderr
    };
    String::from_utf8_lossy(bytes).trim().to_string()
}
impl Herdr for CliHerdr {
    fn json(&self, args: &[&str]) -> Result<Value> {
        let out = self.output(args)?;
        if !out.status.success() {
            return Err(fail(format!(
                "herdr {}: {}",
                args.join(" "),
                err_text(&out)
            )));
        }
        let parsed: Value = serde_json::from_slice(&out.stdout)?;
        parsed
            .get("result")
            .cloned()
            .ok_or_else(|| fail(format!("herdr {}: no result", args.join(" "))))
    }
    fn prompt(&self, pane: &str, text: &str) -> Result<(bool, String)> {
        let out = self.output(&["agent", "prompt", pane, text])?;
        Ok((out.status.success(), err_text(&out)))
    }
    fn send_keys(&self, pane: &str, key: &str) -> Result<()> {
        let out = self.output(&["pane", "send-keys", pane, key])?;
        if out.status.success() {
            Ok(())
        } else {
            Err(fail(format!("herdr pane send-keys {pane}: failed")))
        }
    }
    fn report_metadata(&self, workspace: &str, tokens: &[(&str, &str)]) -> Result<()> {
        let mut args = vec![
            "workspace",
            "report-metadata",
            workspace,
            "--source",
            "capsule",
            "--ttl-ms",
            "5000",
        ];
        let pairs: Vec<String> = tokens.iter().map(|(k, v)| format!("{k}={v}")).collect();
        for pair in &pairs {
            args.push("--token");
            args.push(pair);
        }
        let out = self.output(&args)?;
        if out.status.success() {
            Ok(())
        } else {
            Err(fail(format!(
                "herdr workspace report-metadata: {}",
                err_text(&out)
            )))
        }
    }
}
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
fn utc_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
pub fn panes_by_label(herdr: &dyn Herdr) -> Result<std::collections::BTreeMap<String, Pane>> {
    let agents = herdr.json(&["agent", "list"])?;
    let panes = herdr.json(&["pane", "list"])?;
    let mut turns = std::collections::HashMap::new();
    if let Some(list) = agents.get("agents").and_then(Value::as_array) {
        for agent in list {
            if let (Some(id), seq) = (
                agent.get("pane_id").and_then(Value::as_str),
                agent.get("state_change_seq").and_then(Value::as_i64),
            ) {
                turns.insert(id.to_string(), seq);
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    if let Some(list) = panes.get("panes").and_then(Value::as_array) {
        for pane in list {
            let Ok(parsed) = serde_json::from_value::<Pane>(pane.clone()) else {
                continue;
            };
            if !parsed.label.is_empty() {
                let mut parsed = parsed;
                parsed.state_change_seq = turns.get(&parsed.pane_id).copied().flatten();
                out.insert(parsed.label.clone(), parsed);
            }
        }
    }
    Ok(out)
}
pub fn replied(store: &mut Store, id: &str) -> Result<bool> {
    for to in store.mailboxes()? {
        let queue = format!("mail-{to}");
        for item in store.list(Some(&queue), None, u32::MAX)? {
            if item.payload.get("re").and_then(Value::as_str) == Some(id) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn fence_decision(last: &Fence, pane: &Pane, now: f64, reply_wait: f64, has_reply: bool) -> bool {
    // A task's reply proves its turn happened, even when herdr's turn
    // counter never moved (e.g. the agent restarted outside herdr's view).
    if last.task && has_reply {
        return false;
    }
    if last.pane == pane.pane_id && last.state_change_seq == pane.state_change_seq {
        return true;
    }
    if last.task && last.pane == pane.pane_id && now - last.at < reply_wait {
        return !has_reply;
    }
    false
}
pub fn fenced(cfg: &CapsuleConfig, store: &mut Store, queue: &str, pane: &Pane) -> Result<bool> {
    let path = cfg.home.join("fences").join(format!("{queue}.json"));
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(false);
    };
    let last: Fence = serde_json::from_str(&text)?;
    let has_reply = replied(store, &last.id)?;
    Ok(fence_decision(
        &last,
        pane,
        now_secs(),
        cfg.reply_wait_s(),
        has_reply,
    ))
}
pub fn confirm_submitted(cfg: &CapsuleConfig, herdr: &dyn Herdr, pane: &Pane) -> Result<()> {
    let deadline = Instant::now() + cfg.submit_confirm;
    let step = cfg.submit_confirm.min(Duration::from_millis(250));
    while Instant::now() < deadline {
        if step.is_zero() {
            break;
        }
        thread::sleep(step);
        let agents = herdr.json(&["agent", "list"])?;
        let now = agents
            .get("agents")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|v| v.get("pane_id").and_then(Value::as_str) == Some(&pane.pane_id))
            });
        let done = now.is_none()
            || now
                .and_then(|v| v.get("agent_status").and_then(Value::as_str))
                .is_none_or(|s| !DELIVERABLE.contains(&s))
            || now.and_then(|v| v.get("state_change_seq").and_then(Value::as_i64))
                != pane.state_change_seq;
        if done {
            return Ok(());
        }
    }
    herdr.send_keys(&pane.pane_id, "enter")?;
    println!(
        "pressed enter again in {}: the agent did not start",
        pane.pane_id
    );
    Ok(())
}
pub fn deliver_pass(cfg: &CapsuleConfig, store: &mut Store, herdr: &dyn Herdr) -> Result<()> {
    let panes = panes_by_label(herdr)?;
    for to in store.mailboxes()? {
        if to == "lead" {
            continue;
        }
        let Some(pane) = panes.get(&to) else {
            continue;
        };
        if !DELIVERABLE.contains(&pane.agent_status.as_str()) {
            continue;
        }
        let queue = format!("mail-{to}");
        if fenced(cfg, store, &queue, pane)? {
            continue;
        }
        let Some((item, lease)) = store.claim(&queue, 60_000)? else {
            continue;
        };
        let Ok(cap) = Capsule::from_value(&Value::Object(item.payload.clone())) else {
            continue;
        };
        let (ok, msg) = herdr.prompt(&pane.pane_id, &delivery_text(&cap, cfg))?;
        if !ok {
            store.release(&lease)?;
            println!("deliver {} to {to}: {}", cap.id, msg.trim());
            continue;
        }
        confirm_submitted(cfg, herdr, pane)?;
        if !store.finish(&lease)? {
            eprintln!("capsule: could not mark {} read", item.id);
        }
        let fences = cfg.home.join("fences");
        fs::create_dir_all(&fences)?;
        let fence = json!({
            "id": cap.id,
            "task": cap.status == "task",
            "at": now_secs(),
            "pane": pane.pane_id,
            "state_change_seq": pane.state_change_seq,
        });
        fs::write(fences.join(format!("{queue}.json")), fence.to_string())?;
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(cfg.home.join("delivered.jsonl"))?;
        writeln!(
            log,
            "{}",
            json!({"id": cap.id, "to": to, "pane": pane.pane_id, "at": utc_now()})
        )?;
        println!(
            "delivered {} {} {} -> {to} ({})",
            cap.id, cap.task, cap.status, pane.pane_id
        );
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn pane() -> Pane {
        Pane {
            pane_id: "w1:p2".into(),
            label: "builder".into(),
            agent_status: "idle".into(),
            state_change_seq: Some(7),
        }
    }
    fn fence(task: bool) -> Fence {
        Fence {
            id: "rotisu".into(),
            task,
            at: 1000.0,
            pane: "w1:p2".into(),
            state_change_seq: Some(7),
        }
    }
    #[test]
    fn the_same_turn_is_always_fenced() {
        assert!(fence_decision(&fence(false), &pane(), 1001.0, 1800.0, true));
    }
    #[test]
    fn a_replied_task_never_fences_even_if_the_turn_counter_stuck() {
        // hermes restarted after its reply; herdr's seq stayed at delivery's.
        assert!(!fence_decision(&fence(true), &pane(), 1001.0, 1800.0, true));
        assert!(fence_decision(&fence(true), &pane(), 1001.0, 1800.0, false));
    }
    #[test]
    fn a_task_holds_until_its_reply_arrives_or_wait_ends() {
        let mut p = pane();
        p.state_change_seq = Some(8);
        assert!(fence_decision(&fence(true), &p, 1200.0, 1800.0, false));
        assert!(!fence_decision(&fence(true), &p, 1200.0, 1800.0, true));
        assert!(!fence_decision(&fence(true), &p, 4000.0, 1800.0, false));
    }
}

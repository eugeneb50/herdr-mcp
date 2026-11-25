//! Shared sandbox for the capsule CLI tests: a private queue DB, CAPSULE_HOME,
//! CAPSULE_WORK and the same fake herdr script the Python suite uses.

// clippy.toml allows unwrap in test roots; clippy does not treat this helper
// submodule as one, though it is part of the same integration-test target.
#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, File},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

/// The binary this tree builds; CAPSULE_BIN overrides it (e.g. to replay
/// the suite against another implementation).
const DEFAULT_BIN: &str = env!("CARGO_BIN_EXE_capsule");
/// Cargo's per-target scratch dir: outside /tmp, never the real mailbox.
const SCRATCH: &str = env!("CARGO_TARGET_TMPDIR");

/// The herdr-queue CLI: the one a workspace build put next to `capsule`,
/// else whichever is on PATH.
pub fn queue_bin() -> PathBuf {
    let built = Path::new(DEFAULT_BIN).with_file_name("herdr-queue");
    if built.exists() {
        built
    } else {
        PathBuf::from("herdr-queue")
    }
}

const FAKE_HERDR: &str = r#"#!/bin/sh
# Fake herdr: `pane list` prints $FAKE_PANES; `agent prompt` logs its args
# to $FAKE_LOG and fails with agent_blocked for a pane listed in $FAKE_BLOCKED.
case "$1 $2" in
"pane list") cat "$FAKE_PANES" ;;
"agent list") cat "$FAKE_AGENTS" ;;
"agent prompt")
    if [ -n "${FAKE_BLOCKED:-}" ] && [ "$3" = "$FAKE_BLOCKED" ]; then
        echo '{"error":{"code":"agent_blocked"}}' >&2; exit 1
    fi
    printf '%s\t%s\n' "$3" "$4" >> "$FAKE_LOG"
    if [ -n "${FAKE_ON_PROMPT:-}" ]; then sh "$FAKE_ON_PROMPT"; fi ;;
"pane send-keys") printf '%s\t%s\n' "$3" "$4" >> "$FAKE_KEYS" ;;
"workspace report-metadata") : ;;
*) echo "fake herdr: no $*" >&2; exit 1 ;;
esac
"#;

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Minimal POSIX word splitting, enough for the `shlex.quote` output the
/// capsule embeds in its reply command.
pub fn shlex_split(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut open = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                open = true;
                while let Some(&d) = chars.peek() {
                    chars.next();
                    if d == '\'' {
                        break;
                    }
                    cur.push(d);
                }
            }
            '"' => {
                open = true;
                while let Some(&d) = chars.peek() {
                    chars.next();
                    if d == '"' {
                        break;
                    }
                    if d == '\\' {
                        if let Some(&e) = chars.peek() {
                            chars.next();
                            cur.push(e);
                        }
                    } else {
                        cur.push(d);
                    }
                }
            }
            '\\' => {
                open = true;
                if let Some(e) = chars.next() {
                    cur.push(e);
                }
            }
            c if c.is_whitespace() => {
                if open {
                    words.push(std::mem::take(&mut cur));
                    open = false;
                }
            }
            c => {
                open = true;
                cur.push(c);
            }
        }
    }
    if open || !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Resolve a program on PATH, the way Python's `shutil.which` does.
pub fn which(name: &str) -> String {
    let out = Command::new("sh")
        .args(["-c", &format!("command -v {name}")])
        .output()
        .expect("run command -v");
    assert!(out.status.success(), "no {name} on PATH");
    stdout(&out).trim().to_string()
}

pub struct Fixture {
    pub tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub panes: PathBuf,
    pub agents: PathBuf,
    pub log: PathBuf,
    pub keys: PathBuf,
    pub work: PathBuf,
    pub brief: PathBuf,
    pub queue_db: PathBuf,
    env: HashMap<String, String>,
    bin: String,
}

impl Fixture {
    pub fn new() -> Self {
        // Not under /tmp: temp-dir artifacts are refused, and the suite must
        // never touch the real mailbox.
        fs::create_dir_all(SCRATCH).unwrap();
        let tmp = tempfile::Builder::new()
            .prefix("capsule-test-")
            .tempdir_in(SCRATCH)
            .unwrap();
        let root = tmp.path();
        let home = root.join("mailbox");
        let herdr = root.join("herdr");
        fs::write(&herdr, FAKE_HERDR).unwrap();
        fs::set_permissions(&herdr, fs::Permissions::from_mode(0o755)).unwrap();
        let panes = root.join("panes.json");
        let agents = root.join("agents.json");
        let log = root.join("prompts.log");
        File::create(&log).unwrap();
        let keys = root.join("keys.log");
        let work = root.join("work");
        fs::create_dir_all(&work).unwrap();
        let brief = work.join("brief.md");
        fs::write(&brief, "do the thing\n").unwrap();
        let queue_db = root.join("queue.db");

        let mut env: HashMap<String, String> = std::env::vars().collect();
        env.insert("CAPSULE_HOME".into(), home.display().to_string());
        env.insert("HERDR_BIN_PATH".into(), herdr.display().to_string());
        env.insert("FAKE_PANES".into(), panes.display().to_string());
        env.insert("FAKE_AGENTS".into(), agents.display().to_string());
        env.insert("FAKE_LOG".into(), log.display().to_string());
        env.insert("FAKE_KEYS".into(), keys.display().to_string());
        env.insert("CAPSULE_SUBMIT_CONFIRM_S".into(), "0.3".into());
        env.insert("CAPSULE_FROM".into(), "tohitu".into());
        env.insert("CAPSULE_WORK".into(), work.display().to_string());
        // Never the real queue: every test gets its own DB.
        env.insert("HERDR_QUEUE_DB".into(), queue_db.display().to_string());
        env.remove("HERDR_WORKSPACE_ID");
        env.remove("HERDR_PANE_ID");

        let bin = std::env::var("CAPSULE_BIN").unwrap_or_else(|_| DEFAULT_BIN.to_string());
        let f = Fixture {
            tmp,
            home,
            panes,
            agents,
            log,
            keys,
            work,
            brief,
            queue_db,
            env,
            bin,
        };
        f.set_panes(true);
        f
    }

    pub fn set_panes(&self, idle: bool) {
        self.set_panes_opts(idle, "builder", 7, "w1:p2");
    }

    pub fn set_panes_opts(&self, idle: bool, label: &str, turns: u64, pane: &str) {
        let status = if idle { "idle" } else { "working" };
        fs::write(
            &self.panes,
            json!({"result": {"panes": [
                {"pane_id": pane, "label": label, "agent_status": status}]}})
            .to_string(),
        )
        .unwrap();
        fs::write(
            &self.agents,
            json!({"result": {"agents": [
                {"pane_id": pane, "agent_status": status, "state_change_seq": turns}]}})
            .to_string(),
        )
        .unwrap();
    }

    fn command(&self, args: &[&str], overrides: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.args(args);
        // Python passes `env=env`: replace, don't inherit stale HERDR_* vars.
        cmd.env_clear();
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        for (k, v) in overrides {
            cmd.env(k, v);
        }
        cmd
    }

    /// One capsule invocation, unchecked.
    pub fn run(&self, args: &[&str], overrides: &[(&str, &str)]) -> Output {
        self.command(args, overrides).output().expect("run capsule")
    }

    /// One capsule invocation; a non-zero exit fails the test with its stderr.
    pub fn cap(&self, args: &[&str]) -> Output {
        self.cap_env(args, &[])
    }

    pub fn cap_env(&self, args: &[&str], overrides: &[(&str, &str)]) -> Output {
        let out = self.run(args, overrides);
        assert!(
            out.status.success(),
            "capsule {args:?} exited {:?}: {}",
            out.status,
            stderr(&out)
        );
        out
    }

    /// Like `run`, but kills the child and returns `None` after `secs` seconds.
    pub fn run_timeout(
        &self,
        args: &[&str],
        overrides: &[(&str, &str)],
        secs: u64,
    ) -> Option<Output> {
        let mut child = self
            .command(args, overrides)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn capsule");
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            if child.try_wait().expect("try_wait").is_some() {
                return Some(child.wait_with_output().expect("collect output"));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn cid(out: &Output) -> String {
        stdout(out)
            .split_whitespace()
            .nth(1)
            .expect("CAP <id> ...")
            .to_string()
    }

    pub fn task(&self, to: &str) -> String {
        let artifact = format!("file={}::brief", self.brief.display());
        let out = self.cap(&[
            "send",
            "--to",
            to,
            "--task",
            "t1",
            "--status",
            "task",
            "--summary",
            "build it",
            "--artifact",
            &artifact,
            "--reply-check",
            "sh /x/check.sh",
        ]);
        Self::cid(&out)
    }

    pub fn task_with_check(&self, script: &str) -> (String, PathBuf) {
        let chk = self.work.join("check.sh");
        fs::write(&chk, script).unwrap();
        let artifact = format!("file={}::brief", self.brief.display());
        let check = format!("sh {}", chk.display());
        let out = self.cap(&[
            "send",
            "--to",
            "builder",
            "--task",
            "t1",
            "--status",
            "task",
            "--summary",
            "build it",
            "--artifact",
            &artifact,
            "--reply-check",
            &check,
        ]);
        (Self::cid(&out), chk)
    }

    pub fn reply(&self, cid: &str, extra: &[&str]) -> Output {
        let report = self.work.join("r.md");
        fs::write(&report, "done\n").unwrap();
        let artifact = format!("file={}::report", report.display());
        let mut args = vec![
            "send".to_string(),
            "--to".into(),
            "lead".into(),
            "--task".into(),
            "t1".into(),
            "--status".into(),
            "done".into(),
            "--re".into(),
            cid.into(),
            "--summary".into(),
            "did it".into(),
            "--artifact".into(),
            artifact,
        ];
        args.extend(extra.iter().map(|s| (*s).to_string()));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs, &[])
    }

    pub fn items(&self, to: &str, status: Option<&str>) -> Vec<Value> {
        let mut args = vec![
            "--db".to_string(),
            self.queue_db.display().to_string(),
            "list".to_string(),
            "--queue".to_string(),
            format!("mail-{to}"),
        ];
        if let Some(s) = status {
            args.push("--status".into());
            args.push(s.into());
        }
        let out = Command::new(queue_bin())
            .args(&args)
            .output()
            .expect("herdr-queue list");
        assert!(out.status.success(), "herdr-queue list: {}", stderr(&out));
        let text = stdout(&out);
        if text.trim().is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&text).expect("queue json")
        }
    }

    pub fn unread(&self, to: &str) -> usize {
        self.items(to, Some("queued")).len()
    }

    pub fn prompts(&self) -> Vec<(String, String)> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let mut parts = line.splitn(2, '\t');
                let pane = parts.next().unwrap_or("").to_string();
                let text = parts.next().unwrap_or("").to_string();
                (pane, text)
            })
            .collect()
    }

    pub fn keys(&self) -> Vec<String> {
        fs::read_to_string(&self.keys)
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    pub fn maildir(&self, to: &str, sub: &str, cid: &str, summary: &str) {
        let dir = self.home.join(to).join(sub);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("1795000000-{cid}.json")),
            json!({
                "id": cid, "task": "t", "status": "wip", "from": "tohitu",
                "summary": summary, "artifacts": [], "to": to
            })
            .to_string(),
        )
        .unwrap();
    }
}

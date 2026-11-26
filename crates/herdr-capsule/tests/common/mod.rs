//! Shared sandbox for the capsule CLI tests: a private queue DB, CAPSULE_HOME,
//! CAPSULE_WORK and the same fake herdr script the Python suite uses.

// clippy.toml allows unwrap in test roots; clippy does not treat this helper
// submodule as one, though it is part of the same integration-test target.
#![allow(clippy::unwrap_used)]

use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
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
# Fake herdr: the metadata announcement `send` makes after enqueue.
case "$1 $2" in
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
        let work = root.join("work");
        fs::create_dir_all(&work).unwrap();
        let brief = work.join("brief.md");
        fs::write(&brief, "do the thing\n").unwrap();
        let queue_db = root.join("queue.db");

        let mut env: HashMap<String, String> = std::env::vars().collect();
        env.insert("CAPSULE_HOME".into(), home.display().to_string());
        env.insert("HERDR_BIN_PATH".into(), herdr.display().to_string());
        env.insert("CAPSULE_FROM".into(), "tohitu".into());
        env.insert("CAPSULE_WORK".into(), work.display().to_string());
        // Never the real queue: every test gets its own DB.
        env.insert("HERDR_QUEUE_DB".into(), queue_db.display().to_string());
        env.remove("HERDR_WORKSPACE_ID");
        env.remove("HERDR_PANE_ID");

        let bin = std::env::var("CAPSULE_BIN").unwrap_or_else(|_| DEFAULT_BIN.to_string());
        Fixture {
            tmp,
            home,
            work,
            brief,
            queue_db,
            env,
            bin,
        }
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
}

//! Characterization suite for the capsule CLI. One test per Python test in
//! `scripts/test_capsule_deliver.py`, run against the binary in `CAPSULE_BIN`.

mod common;

use common::{Fixture, shlex_split, stderr, stdout, which};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
fn test_work_files_under_tmp_are_refused() {
    let f = Fixture::new();
    // /tmp is wiped on reboot; the capsule is an audit trail pointing at it.
    let junk = std::env::temp_dir().join("capsule-test-report.md");
    fs::write(&junk, "x").unwrap();
    let artifact = format!("file={}::r", junk.display());
    let out = f.run(
        &[
            "send",
            "--to",
            "lead",
            "--task",
            "t",
            "--status",
            "done",
            "--summary",
            "x",
            "--artifact",
            &artifact,
        ],
        &[],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("CAPSULE_WORK"), "{}", stderr(&out));
}

#[test]
fn test_a_work_dir_in_tmp_is_refused_for_the_real_mailbox() {
    let f = Fixture::new();
    // Pointing CAPSULE_WORK at /tmp must not dodge the rule: the temp-dir
    // exemption is only for a sandboxed (non-default) mailbox.
    let tmp = tempfile::Builder::new()
        .tempdir_in(std::env::temp_dir())
        .unwrap();
    let junk = tmp.path().join("r.md");
    fs::write(&junk, "x").unwrap();
    let artifact = format!("file={}::r", junk.display());
    let out = f.run(
        &[
            "send",
            "--to",
            "lead",
            "--task",
            "t",
            "--status",
            "wip",
            "--summary",
            "x",
            "--artifact",
            &artifact,
        ],
        &[("CAPSULE_WORK", tmp.path().to_str().unwrap())],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("temp dirs"), "{}", stderr(&out));
}

#[test]
fn test_delivery_names_a_report_path_in_the_work_dir() {
    let f = Fixture::new();
    f.task("builder");
    f.cap(&["deliver", "--once"]);
    let prompts = f.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0]
            .1
            .contains(&f.work.join("t1").display().to_string())
    );
    let tmp = format!("{}/", std::env::temp_dir().display());
    let mut paths = prompts[0]
        .1
        .split(|c: char| c.is_whitespace() || c == ':' || c == '=');
    assert!(!paths.any(|p| p.starts_with(&tmp)), "{}", prompts[0].1);
}

#[test]
fn test_task_needs_a_brief_artifact() {
    let f = Fixture::new();
    let out = f.run(
        &[
            "send",
            "--to",
            "builder",
            "--task",
            "t",
            "--status",
            "task",
            "--summary",
            "x",
        ],
        &[],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("artifact"), "{}", stderr(&out));
}

#[test]
fn test_idle_agent_gets_the_task_with_its_reply_command() {
    let f = Fixture::new();
    let cid = f.task("builder");
    f.cap(&["deliver", "--once"]);
    let prompts = f.prompts();
    assert_eq!(prompts.len(), 1);
    let (pane, text) = &prompts[0];
    assert_eq!(pane, "w1:p2");
    assert!(text.contains(&cid));
    assert!(text.contains(&format!("--re {cid}")));
    assert!(text.contains("--to lead"));
    assert!(text.contains("sh /x/check.sh"));
    assert_eq!(f.unread("builder"), 0);
    let log = fs::read_to_string(f.home.join("delivered.jsonl")).unwrap();
    let first: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(first["id"].as_str().unwrap(), cid);
}

#[test]
fn test_busy_agent_waits_and_gets_it_once_idle() {
    let f = Fixture::new();
    f.task("builder");
    f.set_panes_opts(false, "builder", 7, "w1:p2");
    f.cap(&["deliver", "--once"]);
    assert!(f.prompts().is_empty());
    f.set_panes(true);
    f.cap(&["deliver", "--once"]);
    assert_eq!(f.prompts().len(), 1);
    f.cap(&["deliver", "--once"]);
    assert_eq!(f.prompts().len(), 1, "delivered twice");
}

#[test]
fn test_next_capsule_waits_for_the_delivered_turn_to_finish() {
    let f = Fixture::new();
    for n in ["one", "two"] {
        f.cap(&[
            "send",
            "--to",
            "builder",
            "--task",
            "n",
            "--status",
            "wip",
            "--summary",
            n,
        ]);
    }
    f.cap(&["deliver", "--once"]);
    f.cap(&["deliver", "--once"]);
    assert_eq!(
        f.prompts().len(),
        1,
        "second capsule typed into a fresh turn"
    );
    f.set_panes_opts(true, "builder", 8, "w1:p2");
    f.cap(&["deliver", "--once"]);
    assert_eq!(f.prompts().len(), 2);
}

#[test]
fn test_a_restarted_agent_in_a_new_pane_is_not_fenced() {
    let f = Fixture::new();
    f.task("builder");
    f.task("builder");
    f.cap(&["deliver", "--once"]);
    f.set_panes_opts(true, "builder", 0, "w1:p9");
    f.cap(&["deliver", "--once"]);
    let prompts = f.prompts();
    let panes: Vec<&str> = prompts.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(panes, ["w1:p2", "w1:p9"]);
}

#[test]
fn test_one_capsule_per_agent_per_pass() {
    let f = Fixture::new();
    f.task("builder");
    f.task("builder");
    f.cap(&["deliver", "--once"]);
    assert_eq!(f.prompts().len(), 1);
    assert_eq!(f.unread("builder"), 1);
}

#[test]
fn test_blocked_agent_is_not_typed_into_and_keeps_its_capsule() {
    let f = Fixture::new();
    f.task("builder");
    let out = f.cap_env(&["deliver", "--once"], &[("FAKE_BLOCKED", "w1:p2")]);
    assert!(f.prompts().is_empty());
    assert_eq!(f.unread("builder"), 1);
    let items = f.items("builder", None);
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0]["attempts"].as_i64().unwrap(),
        0,
        "a refused prompt must refund the attempt"
    );
    let both = format!("{}{}", stdout(&out), stderr(&out));
    assert!(both.contains("agent_blocked"), "{both}");
}

#[test]
fn test_mailbox_without_a_pane_is_left_alone() {
    let f = Fixture::new();
    f.task("nobody");
    f.cap(&["deliver", "--once"]);
    assert!(f.prompts().is_empty());
    assert_eq!(f.unread("nobody"), 1);
}

#[test]
fn test_lead_mailbox_is_never_delivered_to_a_pane() {
    let f = Fixture::new();
    f.set_panes_opts(true, "lead", 7, "w1:p2");
    f.cap(&[
        "send",
        "--to",
        "lead",
        "--task",
        "t",
        "--status",
        "wip",
        "--summary",
        "x",
    ]);
    f.cap(&["deliver", "--once"]);
    assert!(f.prompts().is_empty());
}

#[test]
fn test_watch_prints_each_capsule_once_and_marks_it_read() {
    let f = Fixture::new();
    f.cap(&[
        "send",
        "--to",
        "lead",
        "--task",
        "a",
        "--status",
        "wip",
        "--summary",
        "one",
    ]);
    f.cap(&[
        "send",
        "--to",
        "lead",
        "--task",
        "b",
        "--status",
        "wip",
        "--summary",
        "two",
    ]);
    let out = f.cap(&["watch", "--to", "lead", "--max", "2"]);
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{}", text);
    assert!(lines[0].contains("\"one\""), "{}", lines[0]);
    assert!(lines[1].contains("\"two\""), "{}", lines[1]);
    assert_eq!(f.unread("lead"), 0);
    let items = f.items("lead", None);
    let statuses: HashSet<&str> = items
        .iter()
        .map(|i| i["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, HashSet::from(["done"]));
}

#[test]
fn test_watch_with_nothing_new_prints_nothing_again() {
    let f = Fixture::new();
    f.cap(&[
        "send",
        "--to",
        "lead",
        "--task",
        "a",
        "--status",
        "wip",
        "--summary",
        "one",
    ]);
    f.cap(&["watch", "--to", "lead", "--max", "1"]);
    assert_eq!(f.unread("lead"), 0);
    // A consumed capsule is never printed twice: the second watch blocks.
    let out = f.run_timeout(&["watch", "--to", "lead", "--max", "1"], &[], 3);
    assert!(out.is_none(), "watch printed the consumed capsule again");
}

#[test]
fn test_fence_is_per_agent() {
    let f = Fixture::new();
    // Delivering to a second agent must not lift the first one's fence.
    fs::write(
        &f.panes,
        json!({"result": {"panes": [
            {"pane_id": "w1:p2", "label": "builder", "agent_status": "idle"},
            {"pane_id": "w1:p3", "label": "tester", "agent_status": "idle"}]}})
        .to_string(),
    )
    .unwrap();
    fs::write(
        &f.agents,
        json!({"result": {"agents": [
            {"pane_id": "w1:p2", "agent_status": "idle", "state_change_seq": 7},
            {"pane_id": "w1:p3", "agent_status": "idle", "state_change_seq": 4}]}})
        .to_string(),
    )
    .unwrap();
    f.task("builder");
    f.task("builder");
    f.task("tester");
    f.cap(&["deliver", "--once"]);
    f.cap(&["deliver", "--once"]);
    let mut panes: Vec<String> = f.prompts().iter().map(|(p, _)| p.clone()).collect();
    panes.sort();
    assert_eq!(panes, ["w1:p2", "w1:p3"]);
}

#[test]
fn test_a_reply_runs_the_tasks_check_not_its_own() {
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("exit 3\n");
    let out = f.reply(&cid, &["--check", "true"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("sh "), "{}", stderr(&out));
    let out = f.reply(&cid, &[]);
    assert!(
        !out.status.success(),
        "a done reply passed although the task's check fails"
    );
}

#[test]
fn test_a_reply_passes_when_the_tasks_check_passes() {
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("exit 0\n");
    let out = f.reply(&cid, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains(" ok "), "{}", stdout(&out));
}

#[test]
fn test_a_check_file_edited_after_the_task_was_sent_is_refused() {
    let f = Fixture::new();
    let (cid, chk) = f.task_with_check("exit 3\n");
    fs::write(&chk, "exit 0\n").unwrap();
    let out = f.reply(&cid, &[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("changed"), "{}", stderr(&out));
}

#[test]
fn test_a_follow_up_task_does_not_run_the_earlier_tasks_check() {
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("exit 3\n");
    let artifact = format!("file={}::brief", f.brief.display());
    let out = f.run(
        &[
            "send",
            "--to",
            "builder",
            "--task",
            "t1",
            "--status",
            "task",
            "--re",
            &cid,
            "--summary",
            "more",
            "--artifact",
            &artifact,
        ],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains(" chk:"), "{}", stdout(&out));
}

#[test]
fn test_after_a_task_the_next_capsule_waits_for_its_reply() {
    let f = Fixture::new();
    let first = f.task("builder");
    f.task("builder");
    f.cap(&["deliver", "--once"]);
    f.set_panes_opts(true, "builder", 8, "w1:p2");
    f.cap(&["deliver", "--once"]);
    assert_eq!(
        f.prompts().len(),
        1,
        "next capsule typed into a task still in progress"
    );
    let report = f.work.join("r.md");
    fs::write(&report, "done\n").unwrap();
    let artifact = format!("file={}::report", report.display());
    f.cap(&[
        "send",
        "--to",
        "lead",
        "--task",
        "t1",
        "--status",
        "fail",
        "--re",
        &first,
        "--summary",
        "tried it",
        "--artifact",
        &artifact,
    ]);
    f.set_panes_opts(true, "builder", 9, "w1:p2");
    f.cap(&["deliver", "--once"]);
    assert_eq!(f.prompts().len(), 2);
}

#[test]
fn test_a_task_without_a_reply_stops_holding_after_the_wait() {
    let f = Fixture::new();
    f.task("builder");
    f.task("builder");
    f.cap(&["deliver", "--once"]);
    f.set_panes_opts(true, "builder", 8, "w1:p2");
    f.cap_env(&["deliver", "--once"], &[("CAPSULE_REPLY_WAIT_S", "0")]);
    assert_eq!(f.prompts().len(), 2);
}

#[test]
fn test_a_prompt_the_agent_never_starts_gets_one_more_enter() {
    let f = Fixture::new();
    f.task("builder");
    f.cap(&["deliver", "--once"]);
    assert_eq!(f.prompts().len(), 1);
    assert_eq!(f.keys(), ["w1:p2\tenter"]);
}

#[test]
fn test_a_prompt_the_agent_starts_gets_no_extra_enter() {
    let f = Fixture::new();
    let starts = f.tmp.path().join("starts.sh");
    let working = f.tmp.path().join("working.json");
    fs::write(
        &working,
        json!({"result": {"agents": [
            {"pane_id": "w1:p2", "agent_status": "working", "state_change_seq": 8}]}})
        .to_string(),
    )
    .unwrap();
    fs::write(
        &starts,
        format!("cp \"{}\" \"{}\"\n", working.display(), f.agents.display()),
    )
    .unwrap();
    f.task("builder");
    f.cap_env(
        &["deliver", "--once"],
        &[("FAKE_ON_PROMPT", starts.to_str().unwrap())],
    );
    assert_eq!(f.prompts().len(), 1);
    assert!(f.keys().is_empty(), "{:?}", f.keys());
}

#[test]
fn test_a_forged_copy_of_a_task_cannot_swap_its_check() {
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("exit 3\n");
    let forged = json!({
        "id": cid, "status": "task", "task": "t1", "reply_check": "true", "to": "aaa"
    });
    let out = std::process::Command::new("herdr-queue")
        .args([
            "--db",
            &f.queue_db.display().to_string(),
            "push",
            "--queue",
            "mail-aaa",
            "--payload",
            &forged.to_string(),
            "--dedupe",
            &cid,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let reply = f.reply(&cid, &[]);
    assert!(
        !reply.status.success(),
        "reply passed on a forged task's check"
    );
    assert!(
        stderr(&reply).contains("more than one"),
        "{}",
        stderr(&reply)
    );
}

#[test]
fn test_a_check_given_as_a_symlink_is_refused() {
    let f = Fixture::new();
    let real = f.work.join("real-check.sh");
    fs::write(&real, "exit 0\n").unwrap();
    let link = f.work.join("check-link.sh");
    symlink(&real, &link).unwrap();
    let artifact = format!("file={}::brief", f.brief.display());
    let check = format!("sh {}", link.display());
    let out = f.run(
        &[
            "send",
            "--to",
            "builder",
            "--task",
            "t1",
            "--status",
            "task",
            "--summary",
            "x",
            "--artifact",
            &artifact,
            "--reply-check",
            &check,
        ],
        &[],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("symlink"), "{}", stderr(&out));
}

#[test]
fn test_delivery_quotes_every_field_of_the_reply_command() {
    let f = Fixture::new();
    let sentinel = f.tmp.path().join("pwned");
    let task_name = format!("x;touch {}", sentinel.display());
    let artifact = format!("file={}::brief", f.brief.display());
    f.run(
        &[
            "send",
            "--to",
            "builder",
            "--task",
            &task_name,
            "--status",
            "task",
            "--summary",
            "s",
            "--artifact",
            &artifact,
        ],
        &[],
    );
    f.cap(&["deliver", "--once"]);
    let prompts = f.prompts();
    let text = &prompts[0].1;
    let cmd = text
        .split("reply with ONE command: ")
        .nth(1)
        .unwrap()
        .split(" ; then reply")
        .next()
        .unwrap();
    let words = shlex_split(cmd);
    assert!(
        words.iter().any(|w| w == &task_name),
        "the task name was split or unquoted: {cmd}"
    );
}

#[test]
fn test_control_characters_in_capsule_text_are_refused() {
    let f = Fixture::new();
    // An escape sequence would be printed on the lead's terminal and typed
    // into a pane (OSC 52 writes the clipboard).
    const BAD: &str = "hi\u{1b}]52;c;ZXZpbA==\u{7}there";
    for field in ["--summary", "--task"] {
        let mut summary = "ok".to_string();
        let mut task = "t1".to_string();
        if field == "--summary" {
            summary = BAD.to_string();
        } else {
            task = BAD.to_string();
        }
        let out = f.run(
            &[
                "send",
                "--to",
                "builder",
                "--status",
                "wip",
                "--task",
                &task,
                "--summary",
                &summary,
            ],
            &[],
        );
        assert!(!out.status.success(), "{field}");
        assert!(
            stderr(&out).contains("control character"),
            "{field}: {}",
            stderr(&out)
        );
    }
    assert!(f.items("builder", None).is_empty());
}

#[test]
fn test_a_symlinked_file_artifact_is_refused() {
    let f = Fixture::new();
    let real = f.work.join("real.md");
    fs::write(&real, "report\n").unwrap();
    let link = f.work.join("link.md");
    symlink(&real, &link).unwrap();
    let artifact = format!("file={}::r", link.display());
    let out = f.run(
        &[
            "send",
            "--to",
            "lead",
            "--task",
            "t",
            "--status",
            "done",
            "--summary",
            "x",
            "--artifact",
            &artifact,
        ],
        &[],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("symlink"), "{}", stderr(&out));
    assert!(
        f.items("lead", None).is_empty(),
        "a refused capsule was still stored"
    );
}

#[test]
fn test_an_oversized_file_artifact_is_refused() {
    let f = Fixture::new();
    let big = f.work.join("big.md");
    fs::write(&big, vec![b'x'; 2000]).unwrap();
    let artifact = format!("file={}::r", big.display());
    let out = f.run(
        &[
            "send",
            "--to",
            "lead",
            "--task",
            "t",
            "--status",
            "done",
            "--summary",
            "x",
            "--artifact",
            &artifact,
        ],
        &[("CAPSULE_ARTIFACT_MAX_BYTES", "1000")],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("too large"), "{}", stderr(&out));
    assert!(
        f.items("lead", None).is_empty(),
        "a refused capsule was still stored"
    );
}

#[test]
fn test_a_under_limit_file_is_accepted_with_a_correct_chunked_sha() {
    let f = Fixture::new();
    // Cross a hash-chunk boundary: the recorded sha must still cover all of it.
    let report = f.work.join("report.md");
    let data = vec![b'a'; 2 * 1024 * 1024 + 123];
    fs::write(&report, &data).unwrap();
    let artifact = format!("file={}::r", report.display());
    let max = format!("{}", 4 * 1024 * 1024);
    f.cap_env(
        &[
            "send",
            "--to",
            "lead",
            "--task",
            "t",
            "--status",
            "done",
            "--summary",
            "x",
            "--artifact",
            &artifact,
        ],
        &[("CAPSULE_ARTIFACT_MAX_BYTES", &max)],
    );
    let items = f.items("lead", None);
    assert_eq!(items.len(), 1);
    // sha256(b"a" * (2 * 1024 * 1024 + 123))[:8] — the bytes are fixed.
    assert_eq!(
        items[0]["payload"]["artifacts"][0]["sha"].as_str().unwrap(),
        "84c89c12"
    );
}

#[test]
fn the_read_command_in_a_delivery_finds_the_capsule_by_its_id() {
    // Every delivery says `Read it: capsule card <capsule id> --to <me>`;
    // the lookup used to match only queue item uuids, so it always failed.
    let f = Fixture::new();
    let cid = f.task("builder");
    for view in ["card", "line", "json"] {
        let out = f.cap(&[view, &cid, "--to", "builder"]);
        assert!(stdout(&out).contains(&cid), "{view}: {}", stdout(&out));
    }
    let uuid = f.items("builder", None)[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let out = f.cap(&["line", &uuid[..8], "--to", "builder"]);
    assert!(stdout(&out).contains(&cid), "{}", stdout(&out));
}

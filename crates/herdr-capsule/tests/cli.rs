//! Characterization suite for the capsule CLI. One test per Python test in
//! `scripts/test_capsule_deliver.py`, run against the binary in `CAPSULE_BIN`.

mod common;

use common::{Fixture, stderr, stdout, which};
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
fn test_a_forged_copy_of_a_task_cannot_swap_its_check() {
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("exit 3\n");
    let forged = json!({
        "id": cid, "status": "task", "task": "t1", "reply_check": "true", "to": "aaa"
    });
    let out = std::process::Command::new(common::queue_bin())
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

#[test]
fn a_reply_names_the_delivery_it_answers() {
    // The postman types `send --re <task> --delivery <attempt>`; the reply
    // must carry it so an answer to an older attempt can't close a newer one.
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("true");
    let out = f.reply(&cid, &["--delivery", "d-1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let reply = f
        .items("lead", None)
        .into_iter()
        .find(|i| i["payload"]["re"] == cid)
        .expect("reply in mail-lead");
    assert_eq!(reply["payload"]["delivery"], "d-1");
    let bare = f.run(
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
            "--delivery",
            "d-2",
        ],
        &[],
    );
    assert!(
        !bare.status.success(),
        "--delivery without --re must be refused"
    );
}

/// What the postman does when it types a task: claim it and record the
/// delivery (with its pane) on the item.
fn mark_delivered(f: &Fixture, to: &str, delivery: &str, pane: &str) {
    let mut q = herdr_queue::Queue::open(&f.queue_db).expect("open queue");
    let (_, lease) = q
        .claim(&format!("mail-{to}"), 600_000, 1)
        .expect("claim")
        .expect("a queued task");
    let set = q
        .set_delivery(
            &lease,
            delivery,
            json!({"pane": pane, "submitted": true}),
            2,
        )
        .expect("set delivery");
    assert_eq!(set, herdr_queue::Fenced::Applied(()));
}

#[test]
fn reply_closes_its_task_with_one_short_command() {
    // Weak models drop or mangle `send --re X --delivery Y --to Z --task T`;
    // `reply <id>@<delivery>` takes everything else from the task.
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("true");
    let report = f.work.join("t1").join(format!("{cid}-report.md"));
    fs::create_dir_all(report.parent().unwrap()).unwrap();
    fs::write(&report, "all good\n").unwrap();
    let token = format!("{cid}@d-1");
    let out = f.run(&["reply", &token, "done", "built", "it", "fine"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let reply = f
        .items("lead", None)
        .into_iter()
        .find(|i| i["payload"]["re"] == cid.as_str())
        .expect("reply in mail-lead");
    let p = &reply["payload"];
    assert_eq!(p["delivery"], "d-1");
    assert_eq!(p["task"], "t1");
    assert_eq!(p["status"], "done");
    assert_eq!(p["summary"], "built it fine");
    assert_eq!(p["artifacts"][0]["ref"], report.display().to_string());
    assert_eq!(p["check"]["exit"], 0, "the task's pinned check ran: {p}");
}

#[test]
fn reply_status_ignores_case() {
    // A model that types DONE must not leave its task open until the deadline.
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("true");
    let token = format!("{cid}@d-1");
    let out = f.run(&["reply", &token, "BLOCK", "need", "input"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let reply = f
        .items("lead", None)
        .into_iter()
        .find(|i| i["payload"]["re"] == cid.as_str())
        .expect("reply in mail-lead");
    assert_eq!(reply["payload"]["status"], "block");
}

#[test]
fn a_delivery_takes_one_reply_even_after_the_lead_read_it() {
    // Agents re-ran their reply 2-3 times; each copy reached the lead as new mail.
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("true");
    let token = format!("{cid}@d-1");
    let out = f.run(&["reply", &token, "block", "need", "input"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = f.run(&["watch", "--to", "lead", "--max", "1"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    for again in [
        vec!["reply", &token, "block", "need", "input"],
        vec![
            "send",
            "--to",
            "lead",
            "--task",
            "t1",
            "--status",
            "block",
            "--summary",
            "again",
            "--re",
            &cid,
            "--delivery",
            "d-1",
        ],
    ] {
        let out = f.run(&again, &[]);
        assert!(!out.status.success(), "{again:?} was accepted");
        assert!(
            stderr(&out).contains("already has its reply"),
            "{again:?}: {}",
            stderr(&out)
        );
    }
    let replies = f
        .items("lead", None)
        .into_iter()
        .filter(|i| i["payload"]["re"] == cid.as_str())
        .count();
    assert_eq!(replies, 1);
    // A new delivery of the same task is a new question and takes its own reply.
    let out = f.run(
        &["reply", &format!("{cid}@d-2"), "block", "still", "stuck"],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn reply_done_without_its_report_says_where_to_write_it() {
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("true");
    let token = format!("{cid}@d-1");
    let out = f.run(&["reply", &token, "done", "x"], &[]);
    assert!(!out.status.success());
    let want = format!("t1/{cid}-report.md");
    assert!(stderr(&out).contains(&want), "{}", stderr(&out));
    assert!(f.items("lead", None).is_empty(), "nothing sent");
    // Stuck is fine without a report.
    let out = f.run(&["reply", &token, "block", "need", "access"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn reply_to_an_unknown_capsule_is_refused() {
    let f = Fixture::new();
    let out = f.run(&["reply", "nosuch@d-1", "fail", "x"], &[]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no capsule 'nosuch'"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_task_name_that_is_a_path_is_refused() {
    let f = Fixture::new();
    for task in ["../escape", "a/b", "..", "a::b"] {
        let args = [
            "send",
            "--to",
            "builder",
            "--task",
            task,
            "--status",
            "task",
            "--summary",
            "x",
        ];
        let out = f.run(&args, &[]);
        assert!(!out.status.success(), "{task} accepted");
        assert!(
            stderr(&out).contains("names a directory"),
            "{}",
            stderr(&out)
        );
    }
}

#[test]
fn a_task_whose_reply_to_is_no_mailbox_name_is_refused() {
    let f = Fixture::new();
    let args = [
        "send",
        "--to",
        "builder",
        "--task",
        "t1",
        "--status",
        "task",
        "--summary",
        "x",
        "--reply-to",
        "Bad Label",
    ];
    let out = f.run(&args, &[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--reply-to"), "{}", stderr(&out));
}

#[test]
fn send_without_re_for_a_delivered_task_names_the_reply_command() {
    // Live failure 2026-11-25: hermes answered `send --to lead --task T
    // --status done` without --re; the task stayed open until its deadline.
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("true");
    mark_delivered(&f, "builder", "d-9", "w1:p2");
    let args = [
        "send",
        "--to",
        "lead",
        "--task",
        "t1",
        "--status",
        "fail",
        "--summary",
        "x",
    ];
    let out = f.run(&args, &[("HERDR_PANE_ID", "w1:p2")]);
    assert!(
        !out.status.success(),
        "the open delivery must be answered by reply"
    );
    let want = format!("reply {cid}@d-9 fail");
    assert!(stderr(&out).contains(&want), "{}", stderr(&out));
    // Another pane, or another task, is not this delivery.
    let out = f.run(&args, &[("HERDR_PANE_ID", "w1:p3")]);
    assert!(out.status.success(), "{}", stderr(&out));
    let other = [
        "send",
        "--to",
        "lead",
        "--task",
        "t2",
        "--status",
        "fail",
        "--summary",
        "x",
    ];
    let out = f.run(&other, &[("HERDR_PANE_ID", "w1:p2")]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn reply_finds_the_report_where_the_postman_said_it_is() {
    // herdr-mcp's mail.work-dir may differ from this CLI's CAPSULE_WORK: the
    // delivered event's `report` is the path the agent was told.
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("true");
    let told = f.work.join("elsewhere").join("r.md");
    {
        let mut q = herdr_queue::Queue::open(&f.queue_db).expect("open queue");
        let (_, lease) = q
            .claim("mail-builder", 600_000, 1)
            .expect("claim")
            .expect("a queued task");
        let data = json!({"pane": "w1:p2", "submitted": true, "report": told});
        q.set_delivery(&lease, "d-5", data, 2)
            .expect("set delivery");
    }
    fs::create_dir_all(told.parent().unwrap()).unwrap();
    fs::write(&told, "ok\n").unwrap();
    let token = format!("{cid}@d-5");
    let out = f.run(&["reply", &token, "done", "ok"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let reply = f
        .items("lead", None)
        .into_iter()
        .find(|i| i["payload"]["re"] == cid.as_str())
        .expect("reply");
    assert_eq!(
        reply["payload"]["artifacts"][0]["ref"],
        told.display().to_string()
    );
}

#[test]
fn hung_check_times_out_and_kills_its_children() {
    let f = Fixture::new();
    let pids = f.work.join("sleep-pids");
    let (cid, _) = f.task_with_check(
        r#"trap '' TERM
sh -c 'printf "%s\n" "$$" >> "$1"; exec sleep 300' sh "$CHECK_PID_FILE" &
sh -c 'printf "%s\n" "$$" >> "$1"; exec sleep 300' sh "$CHECK_PID_FILE"
"#,
    );
    let reply = f.run_timeout(
        &["reply", &cid, "fail", "check hung"],
        &[
            ("CAPSULE_CHECK_TIMEOUT_S", "1"),
            ("CHECK_PID_FILE", pids.to_str().unwrap()),
        ],
        10,
    );
    let pids: Vec<_> = fs::read_to_string(pids)
        .unwrap()
        .lines()
        .map(|pid| pid.parse::<i32>().unwrap())
        .collect();
    assert_eq!(pids.len(), 2);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let still_running = loop {
        let live: Vec<_> = pids
            .iter()
            .copied()
            .filter(|pid| {
                let ps = std::process::Command::new("ps")
                    .args(["-o", "state=", "-p", &pid.to_string()])
                    .output()
                    .unwrap();
                // The reparented process may briefly remain as a zombie.
                let state = stdout(&ps);
                let state = state.trim();
                !state.is_empty() && !state.starts_with('Z')
            })
            .collect();
        if live.is_empty() || std::time::Instant::now() >= deadline {
            break live;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    // Keep a failing regression test from leaking long-lived fixture sleeps.
    for pid in &still_running {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(*pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    assert!(
        still_running.is_empty(),
        "surviving sleeps: {still_running:?}"
    );

    let reply = reply.expect("capsule reply must finish within the test deadline");
    assert!(reply.status.success(), "{}", stderr(&reply));
    let replies = f.items("lead", None);
    assert_eq!(replies[0]["payload"]["check"]["exit"], 124);
}

#[test]
fn timed_out_check_refuses_done_but_allows_fail() {
    let f = Fixture::new();
    let (cid, _) = f.task_with_check("printf 'warning: check started\\n'; exec sleep 300\n");
    let report = f.work.join("t1").join(format!("{cid}-report.md"));
    fs::create_dir_all(report.parent().unwrap()).unwrap();
    fs::write(&report, "Report exists before replying.\n").unwrap();
    let env = [("CAPSULE_CHECK_TIMEOUT_S", "1")];

    let done = f
        .run_timeout(&["reply", &cid, "done", "finished"], &env, 10)
        .expect("done reply must finish within the test deadline");
    assert!(!done.status.success());
    let error = stderr(&done);
    assert!(error.contains("warning: check started"), "{error}");
    assert!(error.contains("check timed out after 1 s"), "{error}");
    assert!(error.contains("check failed (exit 124)"), "{error}");
    assert!(f.items("lead", None).is_empty());

    let failure = f
        .run_timeout(&["reply", &cid, "fail", "check hung"], &env, 10)
        .expect("fail reply must finish within the test deadline");
    assert!(failure.status.success(), "{}", stderr(&failure));
    let replies = f.items("lead", None);
    assert_eq!(replies.len(), 1);
    let payload = &replies[0]["payload"];
    assert_eq!(payload["status"], "fail");
    assert_eq!(payload["check"]["exit"], 124);
    assert_eq!(
        payload["check"]["why"].as_array().unwrap().last().unwrap(),
        "check timed out after 1 s"
    );
    let artifacts = payload["artifacts"].as_array().unwrap();
    let log = artifacts
        .iter()
        .find(|artifact| artifact["kind"] == "log")
        .unwrap();
    let text = fs::read_to_string(log["ref"].as_str().unwrap()).unwrap();
    assert!(text.ends_with("check timed out after 1 s"), "{text}");
}

#[test]
fn a_rollup_verifies_its_originals_and_catches_a_changed_one() {
    let f = Fixture::new();
    for summary in ["one", "two"] {
        f.cap(&[
            "send",
            "--to",
            "lead",
            "--task",
            "r",
            "--status",
            "wip",
            "--summary",
            summary,
        ]);
    }
    f.cap(&["watch", "--to", "lead", "--max", "2"]);
    let out = f.cap(&["rollup", "--to", "lead", "--task", "r", "--send"]);
    let rollup = stdout(&out)
        .lines()
        .find_map(|l| {
            l.strip_prefix("CAP ")
                .and_then(|r| r.split_whitespace().next())
        })
        .expect("rollup CAP line")
        .to_string();
    let out = f.run(&["verify", &rollup, "--to", "lead"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("2 originals verified"),
        "{}",
        stdout(&out)
    );
    // A corrupt index line is named, not skipped.
    let index = f.home.join("archive").join("r-001.jsonl");
    let original = fs::read_to_string(&index).unwrap();
    fs::write(&index, format!("{original}not json\n")).unwrap();
    let out = f.run(&["verify", &rollup, "--to", "lead"], &[]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("index line 3 is not JSON"),
        "{}",
        stderr(&out)
    );
    let out = f.cap(&["card", &rollup, "--to", "lead"]);
    assert!(
        stdout(&out).contains("1 index line(s) are not JSON"),
        "{}",
        stdout(&out)
    );
    fs::write(&index, original).unwrap();
    // Change one original after it was archived: verify must say which.
    let db = rusqlite::Connection::open(&f.queue_db).unwrap();
    let changed: String = db
        .query_row(
            "UPDATE queue_items SET payload = json_set(payload, '$.summary', 'forged')
             WHERE json_extract(payload, '$.summary') = 'one' RETURNING json_extract(payload, '$.id')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let out = f.run(&["verify", &rollup, "--to", "lead"], &[]);
    assert!(!out.status.success(), "a changed original passed");
    assert!(stderr(&out).contains(&changed), "{}", stderr(&out));
}

#[test]
fn watch_names_an_item_that_is_not_a_capsule_instead_of_dropping_it() {
    let f = Fixture::new();
    let mut q = herdr_queue::Queue::open(&f.queue_db).unwrap();
    q.push(
        herdr_queue::Push {
            queue: "mail-lead".into(),
            payload: serde_json::json!({"note": "raw queue_push"})
                .as_object()
                .unwrap()
                .clone(),
            dedupe_key: None,
            coalesce_key: None,
            requester: None,
            max_attempts: 1,
            max_depth: None,
        },
        1,
    )
    .unwrap();
    let out = f.cap(&["watch", "--to", "lead", "--max", "1"]);
    assert!(
        stdout(&out).starts_with("BAD ") && stdout(&out).contains("not a capsule"),
        "{}",
        stdout(&out)
    );
}

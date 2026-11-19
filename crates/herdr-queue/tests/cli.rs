use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::{Value, json};

#[test]
fn push_claim_fail_claim_done_events() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("queue.db");
    let run = |args: &[&str]| -> Value {
        let result = cargo_bin_cmd!("herdr-queue")
            .arg("--db")
            .arg(&db)
            .args(args)
            .assert()
            .success();
        serde_json::from_slice(&result.get_output().stdout).unwrap()
    };
    let pushed = run(&[
        "push",
        "--queue",
        "build",
        "--payload",
        r#"{"check":"fmt"}"#,
    ]);
    let id = pushed["id"].as_str().unwrap();
    assert_eq!(run(&["claim", "--queue", "build"])["id"], id);
    assert_eq!(
        run(&["fail", "--id", id, "--lease", "1", "--error", "boom"])["status"],
        "queued"
    );
    assert_eq!(run(&["claim", "--queue", "build"])["attempts"], 2);
    assert_eq!(run(&["done", "--id", id, "--lease", "2"])["status"], "done");
    assert_eq!(run(&["claim", "--queue", "build"]), Value::Null);
    let events = run(&["events", "--id", id]);
    assert_eq!(
        events
            .as_array()
            .unwrap()
            .iter()
            .map(|event| &event["kind"])
            .collect::<Vec<_>>(),
        json!([
            "enqueued", "claimed", "failed", "requeued", "claimed", "done"
        ])
        .as_array()
        .unwrap()
        .iter()
        .collect::<Vec<_>>()
    );
}

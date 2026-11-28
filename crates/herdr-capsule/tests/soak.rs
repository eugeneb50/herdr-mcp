//! Soak: 8 threads × 25 concurrent sends → 200 unique ids, 0 lost.
#![allow(clippy::unwrap_used)]

use std::{collections::HashSet, fs, process::Command, sync::Arc, thread};

const THREADS: usize = 8;
const PER_THREAD: usize = 25;

fn bin() -> String {
    std::env::var("CAPSULE_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_capsule").to_string())
}

fn send(
    db: &std::path::Path,
    home: &std::path::Path,
    work: &std::path::Path,
    summary: &str,
) -> String {
    let out = Command::new(bin())
        .args([
            "send",
            "--to",
            "lead",
            "--task",
            "soak",
            "--status",
            "wip",
            "--summary",
            summary,
        ])
        .env("CAPSULE_HOME", home)
        .env("CAPSULE_WORK", work)
        .env("CAPSULE_FROM", "tohitu")
        .env("HERDR_QUEUE_DB", db)
        .env_remove("HERDR_BIN_PATH")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_PANE_ID")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "send failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn eight_threads_send_two_hundred_unique_capsules() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let home = root.join("mailbox");
    fs::create_dir_all(&home).unwrap();
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    let db = Arc::new(root.join("queue.db"));

    let mut handles = Vec::new();
    for t in 0..THREADS {
        let db = Arc::clone(&db);
        let home = home.clone();
        let work = work.clone();
        handles.push(thread::spawn(move || {
            let mut ids = Vec::new();
            for i in 0..PER_THREAD {
                let line = send(db.as_path(), &home, &work, &format!("thread {t} item {i}"));
                let id = line.split_whitespace().nth(1).unwrap().to_string();
                ids.push(id);
            }
            ids
        }));
    }

    let mut all = Vec::new();
    for h in handles {
        all.extend(h.join().unwrap());
    }
    assert_eq!(all.len(), THREADS * PER_THREAD);
    let unique: HashSet<_> = all.iter().collect();
    assert_eq!(
        unique.len(),
        THREADS * PER_THREAD,
        "expected 200 unique ids, got {}",
        unique.len()
    );

    // 0 lost: the bridge's own list shows every one of them.
    let listed = Command::new(bin())
        .args(["list", "--to", "lead", "--all"])
        .env("CAPSULE_HOME", &home)
        .env("CAPSULE_WORK", &work)
        .env("CAPSULE_FROM", "tohitu")
        .env("HERDR_QUEUE_DB", db.as_path())
        .env_remove("HERDR_BIN_PATH")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_PANE_ID")
        .output()
        .unwrap();
    assert!(listed.status.success());
    let text = String::from_utf8_lossy(&listed.stdout);
    let listed_ids: HashSet<&str> = text
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .collect();
    for id in &all {
        assert!(listed_ids.contains(id.as_str()), "lost {id} from the queue");
    }
}

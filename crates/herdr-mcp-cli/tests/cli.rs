use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

fn daemon() -> Command {
    Command::cargo_bin("herdr-mcp").unwrap()
}

fn tools() -> Command {
    Command::cargo_bin("herdr-trim").unwrap()
}

#[test]
fn cli_daemon_help_lists_serve_only() {
    daemon()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("serve"))
        .stdout(predicate::str::contains("trim").not())
        .stdout(predicate::str::contains("folder-key").not())
        .stdout(predicate::str::contains("dashboard").not());
}

#[test]
fn cli_tools_help_lists_trim_dashboard_folder_key() {
    tools()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("trim"))
        .stdout(predicate::str::contains("folder-key"))
        .stdout(predicate::str::contains("dashboard"))
        .stdout(predicate::str::contains("serve").not());
}

#[test]
fn cli_versions_print_version() {
    for bin in ["herdr-mcp", "herdr-trim"] {
        Command::cargo_bin(bin)
            .unwrap()
            .arg("--version")
            .assert()
            .success()
            .stdout(predicate::str::contains("0.1.0"));
    }
}

#[test]
fn cli_daemon_rejects_moved_subcommands() {
    // Hard break per spec: local tools moved to herdr-trim.
    daemon()
        .args(["trim", "hello"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

#[test]
fn cli_trim_decompress_is_identity() {
    tools()
        .args(["trim", "--decompress", "hello world"])
        .assert()
        .success()
        .stdout("hello world");
}

#[test]
fn cli_trim_pfc1_emits_report() {
    let data = tempfile::tempdir().unwrap();
    tools()
        .args([
            "trim",
            "--stage",
            "pfc1",
            "--data-dir",
            data.path().to_str().unwrap(),
            "the configuration gateway profile session database connection",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("input_bytes"))
        .stdout(predicate::str::contains("output_bytes"));
}

#[test]
fn cli_trim_caveman_emits_report() {
    let data = tempfile::tempdir().unwrap();
    tools()
        .args([
            "trim",
            "--stage",
            "caveman:full",
            "--data-dir",
            data.path().to_str().unwrap(),
            "the quick brown fox jumps over the lazy dog",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("output"));
}

#[test]
fn cli_trim_invalid_stage_fails() {
    let data = tempfile::tempdir().unwrap();
    tools()
        .args([
            "trim",
            "--stage",
            "bogus",
            "--data-dir",
            data.path().to_str().unwrap(),
            "some input",
        ])
        .assert()
        .failure();
}

#[test]
fn cli_folder_key_list_empty() {
    let data = tempfile::tempdir().unwrap();
    tools()
        .args([
            "folder-key",
            "list",
            "--data-dir",
            data.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("folder key(s)"));
}

#[test]
fn cli_folder_key_build_then_show() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let doc = folder.path().join("docs.md");
    let mut f = std::fs::File::create(&doc).unwrap();
    writeln!(
        f,
        "configuration gateway profile session configuration gateway profile session"
    )
    .unwrap();
    drop(f);

    tools()
        .args([
            "folder-key",
            "build",
            folder.path().to_str().unwrap(),
            "--data-dir",
            data.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Built key"));

    tools()
        .args(["folder-key", "show", folder.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("key"));
}

#[test]
fn cli_folder_key_decompress_runs() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let doc = folder.path().join("docs.md");
    let mut f = std::fs::File::create(&doc).unwrap();
    writeln!(f, "configuration gateway profile session").unwrap();
    drop(f);

    tools()
        .args([
            "folder-key",
            "build",
            folder.path().to_str().unwrap(),
            "--data-dir",
            data.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    tools()
        .args([
            "folder-key",
            "decompress",
            folder.path().to_str().unwrap(),
            "hello world",
        ])
        .assert()
        .success();
}

#[test]
fn cli_trim_missing_input_fails() {
    let data = tempfile::tempdir().unwrap();
    tools()
        .args([
            "trim",
            "--stage",
            "pfc1",
            "--data-dir",
            data.path().to_str().unwrap(),
            "",
        ])
        .assert()
        .failure();
}

// ── Proxy CLI tests ─────────────────────────────────────────────────────────

#[test]
fn cli_serve_proxy_port_flag_accepted() {
    let data = tempfile::tempdir().unwrap();
    daemon()
        .args([
            "serve",
            "--proxy-port",
            "0",
            "--http-only",
            "--data-dir",
            data.path().to_str().unwrap(),
        ])
        .timeout(std::time::Duration::from_secs(2))
        .assert()
        // Timeout leaves the binary running — expect exit by signal or assert_line.
        .stderr(predicate::str::contains("Proxy listener auto-started"));
}

#[test]
fn cli_serve_proxy_and_http_together() {
    let data = tempfile::tempdir().unwrap();
    daemon()
        .args([
            "serve",
            "--proxy-port",
            "0",
            "--http",
            "0",
            "--http-only",
            "--data-dir",
            data.path().to_str().unwrap(),
        ])
        .timeout(std::time::Duration::from_secs(2))
        .assert()
        .stderr(predicate::str::contains("Proxy listener auto-started"));
}

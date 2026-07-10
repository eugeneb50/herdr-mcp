use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

fn bin() -> Command {
    Command::cargo_bin("herdr-mcp").unwrap()
}

#[test]
fn cli_help_lists_subcommands() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("trim"))
        .stdout(predicate::str::contains("folder-key"))
        .stdout(predicate::str::contains("serve"))
        .stdout(predicate::str::contains("dashboard"));
}

#[test]
fn cli_version_prints_version() {
    bin()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("0.1.0"));
}

#[test]
fn cli_trim_decompress_is_identity() {
    bin()
        .args(["trim", "--decompress", "hello world"])
        .assert()
        .success()
        .stdout("hello world");
}

#[test]
fn cli_trim_pfc1_emits_report() {
    let data = tempfile::tempdir().unwrap();
    bin()
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
    bin()
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
    bin()
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
    bin()
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

    bin()
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

    bin()
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

    bin()
        .args([
            "folder-key",
            "build",
            folder.path().to_str().unwrap(),
            "--data-dir",
            data.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    bin()
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
    bin()
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

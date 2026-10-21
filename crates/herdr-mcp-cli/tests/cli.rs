use assert_cmd::Command;
use predicates::prelude::*;

fn bin() -> Command {
    Command::cargo_bin("herdr-mcp").unwrap()
}

#[test]
fn cli_help_lists_modes_and_plugin() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--no-tui"))
        .stdout(predicate::str::contains("--http-only"))
        .stdout(predicate::str::contains("plugin"));
}

#[test]
fn cli_version_prints_version() {
    bin()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn cli_rejects_removed_subcommands() {
    // `serve` became the default (flags at the top level); the trim tools
    // and the separate dashboard binary are gone.
    for sub in ["serve", "trim", "dashboard", "folder-key"] {
        bin().arg(sub).assert().failure().stderr(
            predicate::str::contains("unexpected argument")
                .or(predicate::str::contains("unrecognized subcommand")),
        );
    }
}

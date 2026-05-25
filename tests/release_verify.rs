use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn init_fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "release-verify-demo"
version = "0.1.0"
edition = "2024"
publish = false
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## [0.1.0] - 2026-05-25\n\n- Initial release.\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("keys")).unwrap();
    fs::copy(
        common::maintainer_key_path(),
        root.join("keys/maintainers.gpg"),
    )
    .unwrap();

    git(root, &["init"]);
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ],
    );
    git(root, &["-c", "tag.gpgSign=false", "tag", "0.1.0"]);

    temp
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn release_verify_help_lists_verify_flags() {
    let output = simit()
        .args(["release", "verify", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--json"));
    assert!(stdout.contains("--version <VERSION>"));
    assert!(stdout.contains("--push-target <REMOTE>"));
}

#[test]
fn release_verify_json_reports_all_checks_and_exit_code() {
    let temp = init_fixture();
    fs::write(temp.path().join("dirty.txt"), "dirty\n").unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["release", "verify", "--json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["command"], "simit release verify");
    assert_eq!(value["summary"]["exit_code"], 1);

    let results = value["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .any(|item| item["check"] == "worktree clean" && item["status"] == "fail")
    );
    assert!(
        results
            .iter()
            .any(|item| item["check"] == "remote secrets" && item["status"] == "blocked")
    );
}

#[test]
fn release_verify_text_reports_missing_changelog_entry() {
    let temp = init_fixture();

    let output = simit()
        .current_dir(temp.path())
        .args(["release", "verify", "--version", "0.2.0"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[fail"));
    assert!(stdout.contains("CHANGELOG entry exists: no released entry for 0.2.0"));
    assert!(stdout.contains("summary:"));
}

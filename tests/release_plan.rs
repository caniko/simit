use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn fixture_dir(name: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    copy_dir(
        Path::new("tests/fixtures").join(name).as_path(),
        temp.path(),
    );
    temp
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).unwrap();
        }
    }
}

#[test]
fn release_plan_help_lists_plan_flags() {
    let output = simit()
        .args(["release", "plan", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--dry-run-package"));
    assert!(stdout.contains("--package <NAME>"));
    assert!(stdout.contains("--json"));
}

#[test]
fn release_plan_single_crate_repo_prints_single_entry() {
    let cargo_toml =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let version_line = cargo_toml
        .lines()
        .find(|line| line.starts_with("version = "))
        .unwrap();
    let version = version_line
        .split('"')
        .nth(1)
        .expect("package version in Cargo.toml");

    let output = simit()
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["release", "plan"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("publish order (1 crates):"));
    assert!(stdout.contains(&format!("1. simit {version}")));
    assert!(stdout.contains("non-publishable members skipped: (none)"));
}

#[test]
fn release_plan_orders_workspace_and_skips_non_publishable_members() {
    let temp = fixture_dir("release-plan-workspace");

    let output = simit()
        .current_dir(temp.path())
        .args(["release", "plan"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("publish order (3 crates):"));
    assert!(stdout.contains("1. a 0.1.0"));
    assert!(stdout.contains("2. b 0.1.0"));
    assert!(stdout.contains("3. c 0.1.0"));
    assert!(stdout.contains("non-publishable members skipped: xtask"));
}

#[test]
fn release_plan_json_reports_dependency_order() {
    let temp = fixture_dir("release-plan-workspace");

    let output = simit()
        .current_dir(temp.path())
        .args(["release", "plan", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let entries = value.as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["name"], "a");
    assert_eq!(entries[0]["version"], "0.1.0");
    assert_eq!(entries[0]["publish"], true);
    assert_eq!(entries[0]["depends_on"], Value::Array(Vec::new()));
    assert_eq!(entries[1]["name"], "b");
    assert_eq!(entries[1]["depends_on"], serde_json::json!(["a"]));
    assert_eq!(entries[2]["name"], "c");
    assert_eq!(entries[2]["depends_on"], serde_json::json!(["b"]));
}

#[test]
fn release_plan_dry_run_package_fails_fast_in_publish_order() {
    let temp = fixture_dir("release-plan-dry-run-fail");

    let output = simit()
        .current_dir(temp.path())
        .args(["release", "plan", "--dry-run-package"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("dry-run package: a 0.1.0"));
    assert!(stdout.contains("  ok"));
    assert!(stdout.contains("dry-run package: b 0.1.0"));
    assert!(stdout.contains("  fail"));
    assert!(!stdout.contains("dry-run package: c 0.1.0"));
}

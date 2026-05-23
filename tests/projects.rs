use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

type FeatureFixture<'a> = (&'a str, &'a str);
type ProjectFixture<'a> = (&'a Path, &'a str, &'a [FeatureFixture<'a>]);

fn simit_with_data_home(data_home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_simit"));
    command.env("XDG_DATA_HOME", data_home);
    command
}

fn init_package(root: &Path, name: &str) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
"#
        ),
    )
    .unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
}

fn registry_file(data_home: &Path) -> PathBuf {
    data_home.join("simit/projects.toml")
}

fn write_registry(data_home: &Path, projects: &[ProjectFixture<'_>]) {
    let path = registry_file(data_home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = String::from("schema_version = 1\n");
    for (project_path, name, features) in projects {
        text.push_str("\n[[project]]\n");
        text.push_str(&format!("path = \"{}\"\n", project_path.display()));
        text.push_str(&format!("name = \"{name}\"\n"));
        text.push_str("first_seen = \"2026-05-20T00:00:00Z\"\n");
        text.push_str("last_seen = \"2026-05-21T00:00:00Z\"\n");
        if !features.is_empty() {
            text.push_str("[project.features]\n");
            for (feature, status) in *features {
                text.push_str(&format!("{feature} = \"{status}\"\n"));
            }
        }
    }
    fs::write(path, text).unwrap();
}

fn registry_json(data_home: &Path) -> Value {
    let output = simit_with_data_home(data_home)
        .args(["projects", "list", "--json", "--sort", "path"])
        .output()
        .unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn help_lists_clear_state_and_its_flags() {
    let projects_help = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["projects", "--help"])
        .output()
        .unwrap();
    assert!(projects_help.status.success());
    let stdout = String::from_utf8(projects_help.stdout).unwrap();
    assert!(stdout.contains("clear-state"));

    let clear_help = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["projects", "clear-state", "--help"])
        .output()
        .unwrap();
    assert!(clear_help.status.success());
    let stdout = String::from_utf8(clear_help.stdout).unwrap();
    assert!(stdout.contains("--dry-run"));
}

#[test]
fn list_json_outputs_registered_projects() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "demo");
    let project_path = fs::canonicalize(project.path()).unwrap();
    write_registry(
        data_home.path(),
        &[(&project_path, "demo", &[("flake", "managed")])],
    );

    let output = simit_with_data_home(data_home.path())
        .args(["projects", "list", "--json", "--sort", "path"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0]["name"], "demo");
    assert_eq!(value[0]["features"]["flake"], "managed");
}

#[test]
fn show_json_missing_path_errors() {
    let data_home = TempDir::new().unwrap();
    write_registry(data_home.path(), &[]);
    let missing = data_home.path().join("missing-project");

    let output = simit_with_data_home(data_home.path())
        .args(["projects", "show", "--json", missing.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("project is not registered"));
}

#[test]
fn scan_dry_run_does_not_modify_registry_file() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "demo");
    let project_path = fs::canonicalize(project.path()).unwrap();
    write_registry(
        data_home.path(),
        &[(&project_path, "demo", &[("flake", "absent")])],
    );
    let before = fs::read_to_string(registry_file(data_home.path())).unwrap();

    let output = simit_with_data_home(data_home.path())
        .args(["projects", "scan", "--dry-run"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let after = fs::read_to_string(registry_file(data_home.path())).unwrap();
    assert_eq!(after, before);
}

#[test]
fn scan_updates_last_seen_for_existing_projects() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "demo");
    let project_path = fs::canonicalize(project.path()).unwrap();
    write_registry(
        data_home.path(),
        &[(&project_path, "demo", &[("flake", "absent")])],
    );

    let status = simit_with_data_home(data_home.path())
        .args(["projects", "scan"])
        .status()
        .unwrap();
    assert!(status.success());

    let output = simit_with_data_home(data_home.path())
        .args(["projects", "list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_ne!(value[0]["last_seen"], "2026-05-21T00:00:00Z");
}

#[test]
fn scan_prune_removes_missing_paths() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "demo");
    let project_path = fs::canonicalize(project.path()).unwrap();
    let missing = data_home.path().join("missing-project");
    write_registry(
        data_home.path(),
        &[
            (&project_path, "demo", &[("flake", "managed")]),
            (&missing, "missing", &[("ci", "managed")]),
        ],
    );

    let status = simit_with_data_home(data_home.path())
        .args(["projects", "scan", "--prune"])
        .status()
        .unwrap();
    assert!(status.success());

    let output = simit_with_data_home(data_home.path())
        .args(["projects", "list", "--json", "--sort", "path"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0]["name"], "demo");
}

#[test]
fn forget_removes_exactly_one_entry() {
    let data_home = TempDir::new().unwrap();
    let one = TempDir::new().unwrap();
    let two = TempDir::new().unwrap();
    init_package(one.path(), "one");
    init_package(two.path(), "two");
    let one_path = fs::canonicalize(one.path()).unwrap();
    let two_path = fs::canonicalize(two.path()).unwrap();
    write_registry(
        data_home.path(),
        &[
            (&one_path, "one", &[("flake", "managed")]),
            (&two_path, "two", &[("ci", "managed")]),
        ],
    );

    let status = simit_with_data_home(data_home.path())
        .args(["projects", "forget", one_path.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let output = simit_with_data_home(data_home.path())
        .args(["projects", "list", "--json", "--sort", "path"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0]["name"], "two");
}

#[test]
fn clear_state_removes_all_registry_entries() {
    let data_home = TempDir::new().unwrap();
    let one = TempDir::new().unwrap();
    let two = TempDir::new().unwrap();
    init_package(one.path(), "one");
    init_package(two.path(), "two");
    let one_path = fs::canonicalize(one.path()).unwrap();
    let two_path = fs::canonicalize(two.path()).unwrap();
    write_registry(
        data_home.path(),
        &[
            (&one_path, "one", &[("flake", "managed")]),
            (&two_path, "two", &[("ci", "managed")]),
        ],
    );

    let output = simit_with_data_home(data_home.path())
        .args(["projects", "clear-state"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("cleared project registry state"));

    let value = registry_json(data_home.path());
    assert!(value.as_array().unwrap().is_empty());
}

#[test]
fn clear_state_dry_run_does_not_modify_registry_file() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "demo");
    let project_path = fs::canonicalize(project.path()).unwrap();
    write_registry(
        data_home.path(),
        &[(&project_path, "demo", &[("flake", "managed")])],
    );
    let before = fs::read_to_string(registry_file(data_home.path())).unwrap();

    let output = simit_with_data_home(data_home.path())
        .args(["projects", "clear-state", "--dry-run"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("would clear project registry state"));
    let after = fs::read_to_string(registry_file(data_home.path())).unwrap();
    assert_eq!(after, before);
}

#[test]
fn feature_filter_matches_requested_status() {
    let data_home = TempDir::new().unwrap();
    let one = TempDir::new().unwrap();
    let two = TempDir::new().unwrap();
    init_package(one.path(), "one");
    init_package(two.path(), "two");
    let one_path = fs::canonicalize(one.path()).unwrap();
    let two_path = fs::canonicalize(two.path()).unwrap();
    write_registry(
        data_home.path(),
        &[
            (&one_path, "one", &[("flake", "managed")]),
            (&two_path, "two", &[("flake", "drift")]),
        ],
    );

    let output = simit_with_data_home(data_home.path())
        .args(["projects", "list", "--json", "--feature", "flake=managed"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0]["name"], "one");
}

#[test]
fn show_without_path_uses_current_workspace_root() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "demo");
    let project_path = fs::canonicalize(project.path()).unwrap();
    write_registry(
        data_home.path(),
        &[(&project_path, "demo", &[("flake", "managed")])],
    );

    let output = simit_with_data_home(data_home.path())
        .current_dir(project.path())
        .args(["projects", "show"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("name: demo"));
    assert!(stdout.contains("flake       managed"));
}

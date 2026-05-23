use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use simit::project::{self, Languages};
use simit::render::flake;
use tempfile::TempDir;

fn simit_with_data_home(data_home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_simit"));
    command.env("XDG_DATA_HOME", data_home);
    command.env_remove("SIMIT_NO_REGISTRY");
    command
}

fn registry_file(data_home: &Path) -> PathBuf {
    data_home.join("simit/projects.toml")
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
    fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
}

fn init_package_with_broken_registry_dependency(root: &Path, name: &str) {
    init_package(root, name);
    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"

[dependencies]
missing = {{ version = "1", registry = "sparse-crates-io" }}
"#
        ),
    )
    .unwrap();
}

fn add_managed_flake(root: &Path) {
    let files = flake::files(
        &Languages {
            rust: true,
            nix: true,
            toml: true,
            ..Languages::default()
        },
        "2024",
        Some("1.85"),
    );
    project::write_generated_files(root, &files).unwrap();
}

fn canonical(path: &Path) -> String {
    fs::canonicalize(path).unwrap().to_str().unwrap().to_owned()
}

fn registry_json(data_home: &Path) -> Value {
    let output = simit_with_data_home(data_home)
        .args(["projects", "list", "--json", "--sort", "path"])
        .output()
        .unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn first_seen_for(data_home: &Path, project_path: &Path) -> String {
    let value = registry_json(data_home);
    let path = canonical(project_path);
    value
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == path)
        .unwrap()["first_seen"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn help_lists_discover_and_its_flags() {
    let projects_help = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["projects", "--help"])
        .output()
        .unwrap();
    assert!(projects_help.status.success());
    let stdout = String::from_utf8(projects_help.stdout).unwrap();
    assert!(stdout.contains("discover"));

    let discover_help = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["projects", "discover", "--help"])
        .output()
        .unwrap();
    assert!(discover_help.status.success());
    let stdout = String::from_utf8(discover_help.stdout).unwrap();
    for expected in [
        "[ROOT]",
        "--dry-run",
        "--json",
        "--skip",
        "--max-depth",
        "--follow-symlinks",
        "--include-empty",
    ] {
        assert!(
            stdout.contains(expected),
            "missing help text for {expected}"
        );
    }
}

#[test]
fn default_root_is_current_dir_and_registers_managed_project() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "managed");
    add_managed_flake(project.path());

    let output = simit_with_data_home(data_home.path())
        .current_dir(project.path())
        .args(["projects", "discover"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let value = registry_json(data_home.path());
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0]["path"], canonical(project.path()));
    assert_eq!(value[0]["features"]["flake"], "managed");
}

#[test]
fn dry_run_reports_without_creating_registry_file() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "managed");
    add_managed_flake(project.path());

    let output = simit_with_data_home(data_home.path())
        .args([
            "projects",
            "discover",
            project.path().to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Registered:"));
    assert!(stdout.contains(&canonical(project.path())));
    assert!(fs::metadata(registry_file(data_home.path())).is_err());
}

#[test]
fn json_output_has_discover_report_shape() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "managed");
    add_managed_flake(project.path());

    let output = simit_with_data_home(data_home.path())
        .args([
            "projects",
            "discover",
            project.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    for key in ["registered", "refreshed", "skipped_empty", "errors"] {
        assert!(value[key].is_array(), "{key} is not an array");
    }
}

#[test]
fn max_depth_zero_only_inspects_root() {
    let data_home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let project = root.path().join("nested");
    init_package(&project, "nested");
    add_managed_flake(&project);

    let output = simit_with_data_home(data_home.path())
        .args([
            "projects",
            "discover",
            root.path().to_str().unwrap(),
            "--max-depth",
            "0",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        registry_json(data_home.path())
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn include_empty_registers_bare_cargo_crate() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "plain");

    let output = simit_with_data_home(data_home.path())
        .args([
            "projects",
            "discover",
            project.path().to_str().unwrap(),
            "--include-empty",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    let value = registry_json(data_home.path());
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0]["name"], "plain");
    assert_eq!(value[0]["features"]["flake"], "absent");
}

#[test]
fn include_empty_marks_non_simit_registrations_in_human_output() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "plain");

    let output = simit_with_data_home(data_home.path())
        .args([
            "projects",
            "discover",
            project.path().to_str().unwrap(),
            "--include-empty",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[no simit features]"));
}

#[test]
fn generic_project_files_are_skipped_without_include_empty() {
    let data_home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let project = root.path().join("plain");
    init_package(&project, "plain");
    fs::write(project.join("CHANGELOG.md"), "# Changelog\n\n").unwrap();
    fs::write(project.join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();
    fs::create_dir_all(project.join(".github/workflows")).unwrap();
    fs::write(project.join(".github/workflows/ci.yaml"), "name: CI\n").unwrap();

    let output = simit_with_data_home(data_home.path())
        .args(["projects", "discover", root.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());

    let value = registry_json(data_home.path());
    assert!(value.as_array().unwrap().is_empty());
}

#[test]
fn human_output_for_skipped_package_does_not_require_cargo_metadata() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package_with_broken_registry_dependency(project.path(), "plain");

    let output = simit_with_data_home(data_home.path())
        .args(["projects", "discover", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Skipped (no simit features): 1 project"));
    assert!(!stdout.contains("plain"));
    assert!(!stdout.contains("cargo metadata failed"));
}

#[test]
fn skip_excludes_named_directory() {
    let data_home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let skipped = root.path().join("my-vendor-dir");
    let kept = root.path().join("kept");
    init_package(&skipped, "skipped");
    add_managed_flake(&skipped);
    init_package(&kept, "kept");
    add_managed_flake(&kept);

    let output = simit_with_data_home(data_home.path())
        .args([
            "projects",
            "discover",
            root.path().to_str().unwrap(),
            "--skip",
            "my-vendor-dir",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    let value = registry_json(data_home.path());
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0]["name"], "kept");
}

#[test]
fn rerun_does_not_change_first_seen() {
    let data_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "managed");
    add_managed_flake(project.path());

    let first = simit_with_data_home(data_home.path())
        .args(["projects", "discover", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(first.status.success());
    let before = first_seen_for(data_home.path(), project.path());

    let second = simit_with_data_home(data_home.path())
        .args(["projects", "discover", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(second.status.success());
    let after = first_seen_for(data_home.path(), project.path());
    assert_eq!(after, before);
}

#[test]
fn nonexistent_root_exits_two_with_clear_error() {
    let data_home = TempDir::new().unwrap();
    let root = data_home.path().join("missing");
    let output = simit_with_data_home(data_home.path())
        .args(["projects", "discover", root.to_str().unwrap()])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("could not start discovery"));
    assert!(stderr.contains(root.to_str().unwrap()));
}

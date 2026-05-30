use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn init_package(name: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
license = "MIT"
description = "Demo command line"
homepage = "https://example.com/{name}"
"#
        ),
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        root.join("simit.toml"),
        format!(
            r#"[release.codeberg]
repo = "example/{name}"

[release.artifacts]
runner = "atlas"
build_commands = ["mkdir -p release", "printf artifact > release/{name}.txt"]
sign = false

[aur]
download_repo = "example/{name}"

[copr]
download_repo = "example/{name}"
project = "example/{name}"

[apt]
repo_url = "ssh://git@codeberg.org/example/{name}-apt.git"
"#
        ),
    )
    .unwrap();

    temp
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

#[test]
fn bootstraps_release_workflow_for_enabled_channels() {
    let project = init_package("demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "release"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = read(&project.path().join(".forgejo/workflows/release.yml"));
    assert!(workflow.contains("Publish Codeberg release"));
    assert!(workflow.contains("CODEBERG_TOKEN: ${{ secrets.codeberg_token }}"));
    assert!(workflow.contains("find release -maxdepth 1 -type f"));
    assert!(workflow.contains("Publish AUR packages"));
    assert!(workflow.contains("Push SRPM to COPR"));
    assert!(workflow.contains("Publish APT repository"));
    assert!(workflow.contains("mkdir -p release"));
    assert!(workflow.contains("printf artifact > release/demo.txt"));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Generated release workflow"));
    assert!(stdout.contains("git add .forgejo/workflows/release.yml"));
    assert!(stdout.contains("configure the secrets listed at the top of the workflow"));
}

#[test]
fn check_succeeds_after_bootstrap_and_rerender_is_idempotent() {
    let project = init_package("demo");

    for _ in 0..2 {
        let status = simit()
            .current_dir(project.path())
            .args(["init", "release"])
            .status()
            .unwrap();
        assert!(status.success());
    }

    let check = simit()
        .current_dir(project.path())
        .args(["init", "release", "--check"])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn print_matches_checked_workflow_without_writing_file() {
    let project = init_package("demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "release", "--print"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("name: release"));
    assert!(stdout.contains("Publish Codeberg release"));
    assert!(stdout.contains("Publish AUR packages"));
    assert!(stdout.contains("Push SRPM to COPR"));
    assert!(stdout.contains("Publish APT repository"));
    assert!(
        !project
            .path()
            .join(".forgejo/workflows/release.yml")
            .exists()
    );
}

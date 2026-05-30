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
            r#"[apt]
repo_url = "ssh://git@codeberg.org/example/{name}-apt.git"
label = "{name}"
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
fn bootstraps_fresh_apt_distributions_config() {
    let project = init_package("demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "apt"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let distributions = read(&project.path().join("dist/apt/conf/distributions"));
    assert!(distributions.contains("Origin: demo\n"));
    assert!(distributions.contains("Codename: stable\n"));
    assert!(distributions.contains("Architectures: amd64\n"));
    assert!(distributions.contains("Components: main\n"));
    assert!(distributions.contains("SignWith: yes\n"));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Initialised apt repository config"));
    assert!(stdout.contains("commit dist/apt/conf/distributions"));
    assert!(stdout.contains("publish on release via the generated workflow"));
}

#[test]
fn check_succeeds_after_bootstrap_and_rerender_is_idempotent() {
    let project = init_package("demo");

    for _ in 0..2 {
        let status = simit()
            .current_dir(project.path())
            .args(["init", "apt"])
            .status()
            .unwrap();
        assert!(status.success());
    }

    let check = simit()
        .current_dir(project.path())
        .args(["init", "apt", "--check"])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn print_matches_checked_config_without_writing_files() {
    let project = init_package("demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "apt", "--print"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Origin: demo\n"));
    assert!(stdout.contains("Codename: stable\n"));
    assert!(stdout.contains("SignWith: yes\n"));
    assert!(!project.path().join("dist/apt").exists());
}

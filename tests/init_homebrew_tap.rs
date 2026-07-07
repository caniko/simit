use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn git() -> Command {
    Command::new("git")
}

fn init_package(name: &str, with_homebrew: bool) -> TempDir {
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

    if with_homebrew {
        fs::write(
            root.join("simit.toml"),
            format!(
                r#"[homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-{name}.git"
download_repo = "caniko/{name}"
"#
            ),
        )
        .unwrap();
    }

    temp
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

#[test]
fn bootstraps_fresh_tap_repo() {
    let project = init_package("my-app", true);
    let tap_parent = TempDir::new().unwrap();
    let tap = tap_parent.path().join("homebrew-my-app");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "homebrew-tap", "--target", tap.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let formula = read(&tap.join("Formula/my-app.rb"));
    assert!(formula.starts_with("class MyApp < Formula\n"));
    assert_eq!(formula.matches("sha256 :no_check").count(), 3);
    assert!(!formula.contains("my-app-0.1.0-x86_64-darwin.tar.gz"));
    assert!(formula.contains("bin.install \"my-app\""));
    assert!(tap.join(".git").is_dir());

    let remote = git()
        .current_dir(&tap)
        .args(["remote", "get-url", "origin"])
        .output()
        .unwrap();
    assert!(remote.status.success());
    assert_eq!(
        String::from_utf8(remote.stdout).unwrap().trim(),
        "https://codeberg.org/caniko/homebrew-my-app.git"
    );

    let status = git()
        .current_dir(&tap)
        .args(["status", "--porcelain", "--", "Formula/my-app.rb"])
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(
        String::from_utf8(status.stdout)
            .unwrap()
            .starts_with("A  Formula/my-app.rb")
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("git -C"));
    assert!(stdout.contains("commit -m 'Initial my-app formula'"));
    assert!(stdout.contains("push -u origin trunk"));
}

#[test]
fn check_succeeds_after_bootstrap_and_rerender_is_idempotent() {
    let project = init_package("demo", true);
    let tap_parent = TempDir::new().unwrap();
    let tap = tap_parent.path().join("homebrew-demo");

    for _ in 0..2 {
        let status = simit()
            .current_dir(project.path())
            .args(["init", "homebrew-tap", "--target", tap.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    let check = simit()
        .current_dir(project.path())
        .args([
            "init",
            "homebrew-tap",
            "--target",
            tap.to_str().unwrap(),
            "--check",
        ])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn check_fails_when_formula_drifts() {
    let project = init_package("demo", true);
    let tap_parent = TempDir::new().unwrap();
    let tap = tap_parent.path().join("homebrew-demo");

    let write = simit()
        .current_dir(project.path())
        .args(["init", "homebrew-tap", "--target", tap.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(write.success());
    fs::write(tap.join("Formula/demo.rb"), "class Demo < Formula\nend\n").unwrap();

    let output = simit()
        .current_dir(project.path())
        .args([
            "init",
            "homebrew-tap",
            "--target",
            tap.to_str().unwrap(),
            "--check",
            "--diff",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Homebrew formula is not up to date"));
    assert!(stderr.contains("--- "));
    assert!(stderr.contains("Formula/demo.rb differs"));
}

#[test]
fn print_writes_formula_to_stdout_without_creating_target() {
    let project = init_package("demo", true);
    let tap_parent = TempDir::new().unwrap();
    let tap = tap_parent.path().join("homebrew-demo");

    let output = simit()
        .current_dir(project.path())
        .args([
            "init",
            "homebrew-tap",
            "--target",
            tap.to_str().unwrap(),
            "--print",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("class Demo < Formula\n"));
    assert_eq!(stdout.matches("sha256 :no_check").count(), 3);
    assert!(!stdout.contains("demo-0.1.0-x86_64-darwin.tar.gz"));
    assert!(!tap.exists());
}

#[test]
fn no_git_skips_git_initialisation() {
    let project = init_package("demo", true);
    let tap_parent = TempDir::new().unwrap();
    let tap = tap_parent.path().join("homebrew-demo");

    let status = simit()
        .current_dir(project.path())
        .args([
            "init",
            "homebrew-tap",
            "--target",
            tap.to_str().unwrap(),
            "--no-git",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert!(tap.join("Formula/demo.rb").exists());
    assert!(!tap.join(".git").exists());
}

#[test]
fn different_existing_origin_warns_without_overwriting() {
    let project = init_package("demo", true);
    let tap = TempDir::new().unwrap();
    let init = git()
        .current_dir(tap.path())
        .args(["init"])
        .status()
        .unwrap();
    assert!(init.success());
    let remote = git()
        .current_dir(tap.path())
        .args(["remote", "add", "origin", "https://example.com/other.git"])
        .status()
        .unwrap();
    assert!(remote.success());

    let output = simit()
        .current_dir(project.path())
        .args([
            "init",
            "homebrew-tap",
            "--target",
            tap.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("warning: origin already configured"));

    let remote = git()
        .current_dir(tap.path())
        .args(["remote", "get-url", "origin"])
        .output()
        .unwrap();
    assert!(remote.status.success());
    assert_eq!(
        String::from_utf8(remote.stdout).unwrap().trim(),
        "https://example.com/other.git"
    );
}

#[test]
fn missing_tap_url_errors_clearly() {
    let project = init_package("demo", false);
    let tap_parent = TempDir::new().unwrap();
    let tap = tap_parent.path().join("homebrew-demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "homebrew-tap", "--target", tap.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("homebrew.tap_url not set"));
}

#[test]
fn non_empty_non_git_target_is_rejected() {
    let project = init_package("demo", true);
    let tap = TempDir::new().unwrap();
    fs::write(tap.path().join("README.md"), "manual bootstrap\n").unwrap();

    let output = simit()
        .current_dir(project.path())
        .args([
            "init",
            "homebrew-tap",
            "--target",
            tap.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("exists and is not a git repo; refusing to overwrite"));
}

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn simit() -> Command {
    Command::new(env!("CARGO_BIN_EXE_simit"))
}

fn git() -> Command {
    Command::new("git")
}

fn init_package(name: &str, with_scoop: bool) -> TempDir {
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

    if with_scoop {
        fs::write(
            root.join("simit.toml"),
            format!(
                r#"[scoop]
bucket_url = "https://codeberg.org/caniko/scoop-{name}.git"
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
fn bootstraps_fresh_bucket_repo() {
    let project = init_package("my-app", true);
    let bucket_parent = TempDir::new().unwrap();
    let bucket = bucket_parent.path().join("scoop-my-app");

    let output = simit()
        .current_dir(project.path())
        .args(["init-scoop-bucket", "--target", bucket.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = read(&bucket.join("bucket/my-app.json"));
    serde_json::from_str::<serde_json::Value>(&manifest).unwrap();
    assert!(manifest.contains(r#""version": "0.1.0""#));
    assert_eq!(
        manifest
            .matches(
                r#""hash": "0000000000000000000000000000000000000000000000000000000000000000""#
            )
            .count(),
        2
    );
    assert!(bucket.join(".git").is_dir());

    let remote = git()
        .current_dir(&bucket)
        .args(["remote", "get-url", "origin"])
        .output()
        .unwrap();
    assert!(remote.status.success());
    assert_eq!(
        String::from_utf8(remote.stdout).unwrap().trim(),
        "https://codeberg.org/caniko/scoop-my-app.git"
    );

    let status = git()
        .current_dir(&bucket)
        .args(["status", "--porcelain", "--", "bucket/my-app.json"])
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(
        String::from_utf8(status.stdout)
            .unwrap()
            .starts_with("A  bucket/my-app.json")
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("git -C"));
    assert!(stdout.contains("commit -m 'Initial my-app manifest'"));
    assert!(stdout.contains("push -u origin trunk"));
}

#[test]
fn check_succeeds_after_bootstrap_and_rerender_is_idempotent() {
    let project = init_package("demo", true);
    let bucket_parent = TempDir::new().unwrap();
    let bucket = bucket_parent.path().join("scoop-demo");

    for _ in 0..2 {
        let status = simit()
            .current_dir(project.path())
            .args(["init-scoop-bucket", "--target", bucket.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    let check = simit()
        .current_dir(project.path())
        .args([
            "init-scoop-bucket",
            "--target",
            bucket.to_str().unwrap(),
            "--check",
        ])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn check_fails_when_manifest_drifts() {
    let project = init_package("demo", true);
    let bucket_parent = TempDir::new().unwrap();
    let bucket = bucket_parent.path().join("scoop-demo");

    let write = simit()
        .current_dir(project.path())
        .args(["init-scoop-bucket", "--target", bucket.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(write.success());
    fs::write(bucket.join("bucket/demo.json"), "{}\n").unwrap();

    let output = simit()
        .current_dir(project.path())
        .args([
            "init-scoop-bucket",
            "--target",
            bucket.to_str().unwrap(),
            "--check",
            "--diff",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Scoop manifest is not up to date"));
    assert!(stderr.contains("--- "));
    assert!(stderr.contains("bucket/demo.json differs"));
}

#[test]
fn print_writes_manifest_to_stdout_without_creating_target() {
    let project = init_package("demo", true);
    let bucket_parent = TempDir::new().unwrap();
    let bucket = bucket_parent.path().join("scoop-demo");

    let output = simit()
        .current_dir(project.path())
        .args([
            "init-scoop-bucket",
            "--target",
            bucket.to_str().unwrap(),
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
    serde_json::from_str::<serde_json::Value>(&stdout).unwrap();
    assert!(stdout.contains(r#""version": "0.1.0""#));
    assert!(!bucket.exists());
}

#[test]
fn no_git_skips_git_initialisation() {
    let project = init_package("demo", true);
    let bucket_parent = TempDir::new().unwrap();
    let bucket = bucket_parent.path().join("scoop-demo");

    let status = simit()
        .current_dir(project.path())
        .args([
            "init-scoop-bucket",
            "--target",
            bucket.to_str().unwrap(),
            "--no-git",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert!(bucket.join("bucket/demo.json").exists());
    assert!(!bucket.join(".git").exists());
}

#[test]
fn different_existing_origin_warns_without_overwriting() {
    let project = init_package("demo", true);
    let bucket = TempDir::new().unwrap();
    let init = git()
        .current_dir(bucket.path())
        .args(["init"])
        .status()
        .unwrap();
    assert!(init.success());
    let remote = git()
        .current_dir(bucket.path())
        .args(["remote", "add", "origin", "https://example.com/other.git"])
        .status()
        .unwrap();
    assert!(remote.success());

    let output = simit()
        .current_dir(project.path())
        .args([
            "init-scoop-bucket",
            "--target",
            bucket.path().to_str().unwrap(),
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
        .current_dir(bucket.path())
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
fn missing_bucket_url_errors_clearly() {
    let project = init_package("demo", false);
    let bucket_parent = TempDir::new().unwrap();
    let bucket = bucket_parent.path().join("scoop-demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init-scoop-bucket", "--target", bucket.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("scoop.bucket_url not set"));
}

#[test]
fn non_empty_non_git_target_is_rejected() {
    let project = init_package("demo", true);
    let bucket = TempDir::new().unwrap();
    fs::write(bucket.path().join("README.md"), "manual bootstrap\n").unwrap();

    let output = simit()
        .current_dir(project.path())
        .args([
            "init-scoop-bucket",
            "--target",
            bucket.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("exists and is not a git repo; refusing to overwrite"));
}

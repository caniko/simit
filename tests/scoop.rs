use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn init_package(version: &str, arch_config: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "demo-app"
version = "{version}"
edition = "2024"
rust-version = "1.85"
license = "MIT"
description = "Demo application"
homepage = "https://example.com/demo"
"#
        ),
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        root.join("simit.toml"),
        format!(
            r#"[scoop]
name = "demo-app"
bucket_url = "https://codeberg.org/example/scoop-demo-app.git"
download_repo = "example/demo-app"
binaries = ["demo-app"]
{arch_config}
"#
        ),
    )
    .unwrap();

    temp
}

fn fixture_archives(root: &Path) -> Vec<(String, PathBuf)> {
    [
        ("x64", b"x64\n".as_slice()),
        ("arm64", b"arm64\n".as_slice()),
    ]
    .into_iter()
    .map(|(architecture, content)| {
        let path = root.join(format!("{architecture}.zip"));
        fs::write(&path, content).unwrap();
        (architecture.to_owned(), path)
    })
    .collect()
}

fn archive_args(archives: &[(String, PathBuf)]) -> Vec<String> {
    archives
        .iter()
        .flat_map(|(architecture, path)| {
            vec![
                "--archive".to_owned(),
                format!("{architecture}={}", path.display()),
            ]
        })
        .collect()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

fn sha256(path: &Path) -> String {
    simit::sha256::sha256_of_file(path).unwrap()
}

#[test]
fn render_version_outputs_stable_manifest() {
    let temp = init_package("0.9.0", "");

    let output = simit()
        .current_dir(temp.path())
        .args(["dist", "scoop", "render", "--version", "1.2.3"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str::<serde_json::Value>(&stdout).unwrap();
    assert_eq!(
        stdout,
        r#"{
  "version": "1.2.3",
  "description": "Demo application",
  "homepage": "https://example.com/demo",
  "license": "MIT",
  "architecture": {
    "64bit": {
      "url": "https://codeberg.org/example/demo-app/releases/download/1.2.3/demo-app-1.2.3-x86_64-windows.zip",
      "hash": "0000000000000000000000000000000000000000000000000000000000000000",
      "bin": "demo-app"
    },
    "arm64": {
      "url": "https://codeberg.org/example/demo-app/releases/download/1.2.3/demo-app-1.2.3-aarch64-windows.zip",
      "hash": "0000000000000000000000000000000000000000000000000000000000000000",
      "bin": "demo-app"
    }
  }
}
"#
    );
}

#[test]
fn bump_writes_manifest_with_real_sha256s() {
    let temp = init_package("0.9.0", "");
    let bucket = temp.path().join("bucket-repo");
    let archives = fixture_archives(temp.path());
    let mut args = vec![
        "dist".to_owned(),
        "scoop".to_owned(),
        "bump".to_owned(),
        "--version".to_owned(),
        "1.2.3".to_owned(),
        "--bucket".to_owned(),
        bucket.display().to_string(),
    ];
    args.extend(archive_args(&archives));

    let status = simit()
        .current_dir(temp.path())
        .args(args)
        .status()
        .unwrap();

    assert!(status.success());
    let manifest = read(&bucket.join("bucket/demo-app.json"));
    serde_json::from_str::<serde_json::Value>(&manifest).unwrap();
    for (_, path) in archives {
        assert!(manifest.contains(&format!(r#""hash": "{}""#, sha256(&path))));
    }
}

#[test]
fn bump_errors_when_enabled_architecture_archive_is_missing() {
    let temp = init_package("0.9.0", "");
    let bucket = temp.path().join("bucket-repo");
    let archive = temp.path().join("x64.zip");
    fs::write(&archive, b"x64\n").unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args([
            "dist",
            "scoop",
            "bump",
            "--version",
            "1.2.3",
            "--bucket",
            bucket.to_str().unwrap(),
            "--archive",
            &format!("x64={}", archive.display()),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("missing --archive for arm64"));
}

#[test]
fn render_no_arch_arm64_omits_arm64_block() {
    let temp = init_package("0.9.0", "");

    let output = simit()
        .current_dir(temp.path())
        .args([
            "dist",
            "scoop",
            "render",
            "--version",
            "1.2.3",
            "--scoop-no-arch",
            "arm64",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#""64bit""#));
    assert!(!stdout.contains(r#""arm64""#));
    assert!(!stdout.contains("aarch64-windows.zip"));
}

#[test]
fn init_scoop_bucket_print_matches_render() {
    let temp = init_package("0.9.0", "");

    let render = simit()
        .current_dir(temp.path())
        .args(["dist", "scoop", "render"])
        .output()
        .unwrap();
    let init = simit()
        .current_dir(temp.path())
        .args(["init", "scoop-bucket", "--target", "/tmp/unused", "--print"])
        .output()
        .unwrap();

    assert!(render.status.success());
    assert!(init.status.success());
    assert_eq!(render.stdout, init.stdout);
}

#[test]
fn bump_without_push_writes_expected_git_diff() {
    let temp = init_package("0.9.0", "");
    let bucket = temp.path().join("bucket-repo");
    fs::create_dir(&bucket).unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&bucket)
            .arg("init")
            .status()
            .unwrap()
            .success()
    );

    let archives = fixture_archives(temp.path());
    let mut args = vec![
        "dist".to_owned(),
        "scoop".to_owned(),
        "bump".to_owned(),
        "--version".to_owned(),
        "1.2.3".to_owned(),
        "--bucket".to_owned(),
        bucket.display().to_string(),
    ];
    args.extend(archive_args(&archives));

    let status = simit()
        .current_dir(temp.path())
        .args(args)
        .status()
        .unwrap();

    assert!(status.success());
    let git_status = Command::new("git")
        .arg("-C")
        .arg(&bucket)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(git_status.status.success());
    assert_eq!(
        String::from_utf8(git_status.stdout).unwrap(),
        "?? bucket/\n"
    );
}

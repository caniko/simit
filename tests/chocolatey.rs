use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn init_package(version: &str) -> TempDir {
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
authors = ["Can Example"]
description = "Demo & application <for Windows>"
homepage = "https://example.com/demo"
"#
        ),
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        root.join("simit.toml"),
        r#"[chocolatey]
id = "demo-app"
title = "Demo & App"
download_repo = "example/demo-app"
license_url = "https://example.com/demo/license"
tags = "demo cli"
release_notes_url = "https://example.com/demo/releases"
"#,
    )
    .unwrap();

    temp
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

fn sha256(path: &Path) -> String {
    simit::sha256::sha256_of_file(path).unwrap()
}

fn archive(root: &Path, name: &str, content: &[u8]) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, content).unwrap();
    path
}

fn choco_available() -> bool {
    Command::new("choco")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn render_writes_deterministic_package_files() {
    let temp = init_package("0.9.0");
    let output_dir = temp.path().join("pkg");

    let output = simit()
        .current_dir(temp.path())
        .args([
            "dist",
            "chocolatey",
            "render",
            "--version",
            "0.1.0",
            "--output-dir",
            output_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    let nuspec = read(&output_dir.join("demo-app.nuspec"));
    assert!(nuspec.contains("<id>demo-app</id>"));
    assert!(nuspec.contains("<version>0.1.0</version>"));
    assert!(nuspec.contains("<title>Demo &amp; App</title>"));
    assert!(nuspec.contains("<authors>Can Example</authors>"));
    assert!(
        nuspec.contains("<description>Demo &amp; application &lt;for Windows&gt;</description>")
    );
    assert!(nuspec.contains("<file src=\"tools\\chocolateyInstall.ps1\" target=\"tools\" />"));

    let install = read(&output_dir.join("tools/chocolateyInstall.ps1"));
    assert!(install.contains("$url64 = 'https://codeberg.org/example/demo-app/releases/download/0.1.0/demo-app-0.1.0-x86_64-windows.zip'"));
    assert!(install.contains("Install-ChocolateyZipPackage"));
    assert!(!install.contains("-Checksum64"));
    assert!(output_dir.join("tools/chocolateyUninstall.ps1").exists());

    if choco_available() {
        let pack = Command::new("choco")
            .args([
                "pack",
                output_dir.join("demo-app.nuspec").to_str().unwrap(),
                "--output-directory",
                output_dir.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            pack.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&pack.stdout),
            String::from_utf8_lossy(&pack.stderr)
        );
        assert!(output_dir.join("demo-app.0.1.0.nupkg").exists());
    }
}

#[test]
fn init_chocolatey_prints_without_writing() {
    let temp = init_package("0.9.0");
    let target = temp.path().join("skel");

    let output = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "chocolatey",
            "--target",
            target.to_str().unwrap(),
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
    assert!(stdout.contains("==> demo-app.nuspec"));
    assert!(stdout.contains("<version>0.9.0</version>"));
    assert!(stdout.contains("==> tools/chocolateyInstall.ps1"));
    assert!(!target.exists());
}

#[test]
fn bump_writes_real_sha256s() {
    let temp = init_package("0.9.0");
    let package_dir = temp.path().join("pkg");
    let x64 = archive(temp.path(), "x64.zip", b"x64 archive\n");

    let output = simit()
        .current_dir(temp.path())
        .args([
            "dist",
            "chocolatey",
            "bump",
            "--version",
            "0.1.0",
            "--package-dir",
            package_dir.to_str().unwrap(),
            "--archive",
            &format!("x64={}", x64.display()),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let install = read(&package_dir.join("tools/chocolateyInstall.ps1"));
    assert!(install.contains(&format!("$checksum64 = '{}'", sha256(&x64))));
    assert!(install.contains("-Checksum64 $checksum64"));
    assert!(install.contains("-ChecksumType64 'sha256'"));
}

#[test]
fn bump_missing_archive_errors_clearly() {
    let temp = init_package("0.9.0");
    let package_dir = temp.path().join("pkg");

    let output = simit()
        .current_dir(temp.path())
        .args([
            "dist",
            "chocolatey",
            "bump",
            "--version",
            "0.1.0",
            "--package-dir",
            package_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("missing --archive for x64"));
}

#[test]
fn bump_push_requires_api_key_env() {
    let temp = init_package("0.9.0");
    let package_dir = temp.path().join("pkg");
    let x64 = archive(temp.path(), "x64.zip", b"x64 archive\n");

    let output = simit()
        .current_dir(temp.path())
        .args([
            "dist",
            "chocolatey",
            "bump",
            "--version",
            "0.1.0",
            "--package-dir",
            package_dir.to_str().unwrap(),
            "--archive",
            &format!("x64={}", x64.display()),
            "--push",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--push requires --api-key-env"));
}

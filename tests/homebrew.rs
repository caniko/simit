use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn init_package(version: &str, platform_config: &str) -> TempDir {
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
            r#"[homebrew]
name = "demo-app"
tap_url = "https://codeberg.org/example/homebrew-demo-app.git"
download_repo = "example/demo-app"
binaries = ["demo-app"]
{platform_config}
"#
        ),
    )
    .unwrap();

    temp
}

fn fixture_archives(root: &Path) -> Vec<(String, PathBuf)> {
    [
        ("darwin_arm", b"darwin arm\n".as_slice()),
        ("darwin_intel", b"darwin intel\n".as_slice()),
        ("linux_arm", b"linux arm\n".as_slice()),
        ("linux_intel", b"linux intel\n".as_slice()),
    ]
    .into_iter()
    .map(|(platform, content)| {
        let path = root.join(format!("{platform}.tar.gz"));
        fs::write(&path, content).unwrap();
        (platform.to_owned(), path)
    })
    .collect()
}

fn archive_args(archives: &[(String, PathBuf)]) -> Vec<String> {
    archives
        .iter()
        .flat_map(|(platform, path)| {
            vec![
                "--archive".to_owned(),
                format!("{platform}={}", path.display()),
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
fn render_version_outputs_no_check_formula() {
    let temp = init_package("0.9.0", "");

    let output = simit()
        .current_dir(temp.path())
        .args(["dist", "homebrew", "render", "--version", "1.2.3"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().next(), Some("class DemoApp < Formula"));
    assert!(stdout.contains("version \"1.2.3\""));
    assert_eq!(stdout.matches("sha256 :no_check").count(), 4);
}

#[test]
fn render_output_writes_file_without_stdout() {
    let temp = init_package("0.9.0", "");
    let output_path = temp.path().join("Formula/demo-app.rb");

    let output = simit()
        .current_dir(temp.path())
        .args([
            "dist",
            "homebrew",
            "render",
            "--version",
            "1.2.3",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let formula = read(&output_path);
    assert!(formula.contains("version \"1.2.3\""));
    assert_eq!(formula.matches("sha256 :no_check").count(), 4);
}

#[test]
fn render_without_version_uses_package_version() {
    let temp = init_package("0.7.4", "");

    let output = simit()
        .current_dir(temp.path())
        .args(["dist", "homebrew", "render"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("version \"0.7.4\""));
}

#[test]
fn bump_writes_formula_with_real_sha256s() {
    let temp = init_package("0.9.0", "");
    let tap = temp.path().join("tap");
    let archives = fixture_archives(temp.path());
    let mut args = vec![
        "dist".to_owned(),
        "homebrew".to_owned(),
        "bump".to_owned(),
        "--version".to_owned(),
        "1.2.3".to_owned(),
        "--tap".to_owned(),
        tap.display().to_string(),
    ];
    args.extend(archive_args(&archives));

    let status = simit()
        .current_dir(temp.path())
        .args(args)
        .status()
        .unwrap();

    assert!(status.success());
    let formula = read(&tap.join("Formula/demo-app.rb"));
    for (_, path) in archives {
        assert!(formula.contains(&format!("sha256 \"{}\"", sha256(&path))));
    }
}

#[test]
fn bump_omits_disabled_platforms() {
    let temp = init_package(
        "0.9.0",
        r#"
[homebrew.platforms]
linux_arm = false
"#,
    );
    let tap = temp.path().join("tap");
    let archives = fixture_archives(temp.path())
        .into_iter()
        .filter(|(platform, _)| platform != "linux_arm")
        .collect::<Vec<_>>();
    let mut args = vec![
        "dist".to_owned(),
        "homebrew".to_owned(),
        "bump".to_owned(),
        "--version".to_owned(),
        "1.2.3".to_owned(),
        "--tap".to_owned(),
        tap.display().to_string(),
    ];
    args.extend(archive_args(&archives));

    let status = simit()
        .current_dir(temp.path())
        .args(args)
        .status()
        .unwrap();

    assert!(status.success());
    let formula = read(&tap.join("Formula/demo-app.rb"));
    assert_eq!(formula.matches("sha256 \"").count(), 3);
    assert!(!formula.contains("demo-app-1.2.3-aarch64-linux.tar.gz"));
}

#[test]
fn bump_push_rejects_non_git_tap() {
    let temp = init_package("0.9.0", "");
    let tap = temp.path().join("tap");
    let archives = fixture_archives(temp.path());
    let mut args = vec![
        "dist".to_owned(),
        "homebrew".to_owned(),
        "bump".to_owned(),
        "--version".to_owned(),
        "1.2.3".to_owned(),
        "--tap".to_owned(),
        tap.display().to_string(),
        "--push".to_owned(),
    ];
    args.extend(archive_args(&archives));

    let output = simit()
        .current_dir(temp.path())
        .args(args)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("git status --porcelain failed"));
    assert!(stderr.contains("not a git repository"));
}

#[test]
fn bump_push_rejects_unrelated_dirty_file() {
    let temp = init_package("0.9.0", "");
    let tap = temp.path().join("tap");
    fs::create_dir(&tap).unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&tap)
            .arg("init")
            .status()
            .unwrap()
            .success()
    );
    fs::write(tap.join("README.md"), "dirty\n").unwrap();
    let archives = fixture_archives(temp.path());
    let mut args = vec![
        "dist".to_owned(),
        "homebrew".to_owned(),
        "bump".to_owned(),
        "--version".to_owned(),
        "1.2.3".to_owned(),
        "--tap".to_owned(),
        tap.display().to_string(),
        "--push".to_owned(),
    ];
    args.extend(archive_args(&archives));

    let output = simit()
        .current_dir(temp.path())
        .args(args)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("tap working tree has unrelated changes"));
    assert!(stderr.contains("README.md"));
}

#[test]
fn contract_sha256_fields_match_rs_harbor_when_available() {
    if Command::new("rs-harbor").arg("--help").output().is_err() {
        eprintln!("skipping rs-harbor contract test: rs-harbor is not on PATH");
        return;
    }

    let temp = init_package(
        "0.9.0",
        r#"
[homebrew.platforms]
darwin_arm = false
darwin_intel = false
linux_arm = false
"#,
    );
    let tap = temp.path().join("tap");
    let archive = temp.path().join("linux_intel.tar.gz");
    fs::write(&archive, b"hello\n").unwrap();

    let simit_status = simit()
        .current_dir(temp.path())
        .args([
            "dist",
            "homebrew",
            "bump",
            "--version",
            "1.2.3",
            "--tap",
            tap.to_str().unwrap(),
            "--archive",
            &format!("linux_intel={}", archive.display()),
        ])
        .status()
        .unwrap();
    assert!(simit_status.success());

    let url = "https://codeberg.org/example/demo-app/releases/download/1.2.3/demo-app-1.2.3-x86_64-linux.tar.gz";
    let rs_harbor = Command::new("rs-harbor")
        .args([
            "brew",
            "bump",
            "--stdout",
            "--name",
            "demo-app",
            "--version",
            "1.2.3",
            "--description",
            "Demo application",
            "--homepage",
            "https://example.com/demo",
            "--license",
            "MIT",
            "--archive",
            &format!("linux_intel={url},{}", archive.display()),
            "--binary",
            "demo-app",
        ])
        .output()
        .unwrap();
    assert!(rs_harbor.status.success());

    let simit_formula = read(&tap.join("Formula/demo-app.rb"));
    let rs_harbor_formula = String::from_utf8(rs_harbor.stdout).unwrap();
    let simit_sha = simit_formula
        .lines()
        .find(|line| line.trim_start().starts_with("sha256 "))
        .unwrap();
    let rs_harbor_sha = rs_harbor_formula
        .lines()
        .find(|line| line.trim_start().starts_with("sha256 "))
        .unwrap();
    assert_eq!(simit_sha, rs_harbor_sha);
}

#[test]
fn old_top_level_homebrew_command_is_rejected() {
    let output = simit()
        .args(["homebrew", "bump", "--help"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unrecognized subcommand 'homebrew'"));
}

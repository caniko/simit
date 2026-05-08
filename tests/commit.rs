use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn simit() -> Command {
    Command::new(env!("CARGO_BIN_EXE_simit"))
}

fn run(dir: &Path, program: &str, args: &[&str]) {
    let status = Command::new(program)
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap_or_else(|err| panic!("running {program}: {err}"));
    assert!(status.success(), "{program} {args:?} failed");
}

fn output(dir: &Path, program: &str, args: &[&str]) -> String {
    let output = Command::new(program)
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("running {program}: {err}"));
    assert!(output.status.success(), "{program} {args:?} failed");
    String::from_utf8(output.stdout).unwrap()
}

fn init_package() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    run(root, "git", &["init"]);
    run(
        root,
        "git",
        &["config", "user.email", "simit@example.invalid"],
    );
    run(root, "git", &["config", "user.name", "Simit Test"]);
    run(root, "git", &["config", "commit.gpgSign", "false"]);
    run(root, "git", &["config", "tag.gpgSign", "false"]);

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "demo"
version = "0.1.0"
edition = "2024"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    run(root, "cargo", &["generate-lockfile"]);
    run(root, "git", &["add", "."]);
    run(root, "git", &["commit", "-m", "initial"]);

    temp
}

#[test]
fn commit_patch_bumps_version_commits_and_tags() {
    let temp = init_package();
    let root = temp.path();

    let status = simit()
        .current_dir(root)
        .args(["commit", "--no-sign", "patch", "-m", "release patch"])
        .status()
        .unwrap();
    assert!(status.success());

    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("version = \"0.1.1\""));

    let tags = output(root, "git", &["tag", "--list"]);
    assert_eq!(tags.trim(), "0.1.1");

    let subject = output(root, "git", &["log", "-1", "--format=%s"]);
    assert_eq!(subject.trim(), "release patch");
}

#[test]
fn no_tag_skips_tag_creation() {
    let temp = init_package();
    let root = temp.path();

    let status = simit()
        .current_dir(root)
        .args(["commit", "--no-tag", "minor", "-m", "release minor"])
        .status()
        .unwrap();
    assert!(status.success());

    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("version = \"0.2.0\""));

    let tags = output(root, "git", &["tag", "--list"]);
    assert!(tags.trim().is_empty());
}

#[test]
fn preserves_existing_staged_changes() {
    let temp = init_package();
    let root = temp.path();

    fs::write(root.join("CHANGELOG.md"), "next\n").unwrap();
    run(root, "git", &["add", "CHANGELOG.md"]);

    let status = simit()
        .current_dir(root)
        .args([
            "commit",
            "--no-sign",
            "patch",
            "-m",
            "release with changelog",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let names = output(root, "git", &["show", "--name-only", "--format="]);
    assert!(names.lines().any(|line| line == "CHANGELOG.md"));
    assert!(names.lines().any(|line| line == "Cargo.toml"));
    assert!(names.lines().any(|line| line == "Cargo.lock"));
}

#[test]
fn fails_without_cargo_manifest() {
    let temp = TempDir::new().unwrap();
    run(temp.path(), "git", &["init"]);

    let output = simit()
        .current_dir(temp.path())
        .args(["commit", "patch", "-m", "release"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("could not find Cargo.toml"));
}

#[test]
fn fails_multi_package_workspace_without_package_name() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    run(root, "git", &["init"]);
    run(
        root,
        "git",
        &["config", "user.email", "simit@example.invalid"],
    );
    run(root, "git", &["config", "user.name", "Simit Test"]);
    run(root, "git", &["config", "commit.gpgSign", "false"]);
    run(root, "git", &["config", "tag.gpgSign", "false"]);

    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["a", "b"]
resolver = "3"
"#,
    )
    .unwrap();
    for package in ["a", "b"] {
        let package_dir = root.join(package);
        fs::create_dir(&package_dir).unwrap();
        fs::create_dir(package_dir.join("src")).unwrap();
        fs::write(
            package_dir.join("Cargo.toml"),
            format!("[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
        )
        .unwrap();
        fs::write(package_dir.join("src/lib.rs"), "").unwrap();
    }
    run(root, "cargo", &["generate-lockfile"]);
    run(root, "git", &["add", "."]);
    run(root, "git", &["commit", "-m", "initial"]);

    let output = simit()
        .current_dir(root)
        .args(["commit", "patch", "-m", "release"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("workspace has multiple packages"));
}

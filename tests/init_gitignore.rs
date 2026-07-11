use std::fs;
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn rust_project() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

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
    temp
}

#[test]
fn writes_language_aware_template_and_checks_it() {
    let project = rust_project();
    let root = project.path();

    let output = simit()
        .current_dir(root)
        .args(["init", "gitignore"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);

    let content = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(content.contains("# Rust\ntarget/\n"));
    assert!(content.contains(".direnv/\n"));
    assert!(!content.contains("__pycache__/\n"));

    let check = simit()
        .current_dir(root)
        .args(["init", "gitignore", "--check"])
        .output()
        .unwrap();
    assert!(check.status.success(), "{:?}", check);
}

#[test]
fn print_does_not_write_and_diff_reports_drift() {
    let project = rust_project();
    let root = project.path();

    let print = simit()
        .current_dir(root)
        .args(["init", "gitignore", "--print"])
        .output()
        .unwrap();
    assert!(print.status.success(), "{:?}", print);
    assert!(String::from_utf8_lossy(&print.stdout).contains("--- .gitignore"));
    assert!(!root.join(".gitignore").exists());

    fs::write(root.join(".gitignore"), "custom-only/\n").unwrap();
    let check = simit()
        .current_dir(root)
        .args(["init", "gitignore", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(!check.status.success());
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(stderr.contains(".gitignore is not up to date"));
    assert!(stderr.contains("---"));
    assert!(stderr.contains("+++"));
}

#[test]
fn non_cargo_directory_is_supported() {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(temp.path().join("uv.lock"), "").unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();

    let output = simit()
        .current_dir(temp.path().join("src"))
        .args(["init", "gitignore"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);

    let content = fs::read_to_string(temp.path().join(".gitignore")).unwrap();
    assert!(content.contains("# Python\n"));
    assert!(content.contains(".venv/\n"));
    assert!(content.contains("node_modules/\n"));
    assert!(!temp.path().join("src/.gitignore").exists());
}

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn simit() -> Command {
    Command::new(env!("CARGO_BIN_EXE_simit"))
}

fn init_package() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "demo"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();
    fs::write(root.join("README.md"), "# Demo\n").unwrap();
    fs::write(root.join("settings.yaml"), "name: demo\n").unwrap();

    temp
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

#[test]
fn writes_detected_hook_files() {
    let temp = init_package();

    let status = simit()
        .current_dir(temp.path())
        .args(["init-hooks"])
        .status()
        .unwrap();
    assert!(status.success());

    let treefmt = read(&temp.path().join("nix/treefmt.nix"));
    assert!(treefmt.contains("programs.rustfmt.enable = true"));
    assert!(treefmt.contains("programs.alejandra.enable = true"));
    assert!(treefmt.contains("programs.taplo.enable = true"));
    assert!(treefmt.contains("programs.prettier"));
    assert!(treefmt.contains("\"*.md\""));
    assert!(treefmt.contains("\"*.yaml\""));

    let hooks = read(&temp.path().join("nix/pre-commit.nix"));
    assert!(hooks.contains("cargo fmt --all -- --check"));
    assert!(hooks.contains("cargo clippy --all-targets --all-features -- --deny warnings"));
    assert!(hooks.contains(
        "nix --extra-experimental-features 'nix-command flakes' flake check --cores 0 --max-jobs auto --no-update-lock-file"
    ));
}

#[test]
fn detects_uv_python_hooks() {
    let temp = init_package();
    fs::write(
        temp.path().join("pyproject.toml"),
        r#"[project]
name = "demo"
version = "0.1.0"

[tool.uv]
"#,
    )
    .unwrap();
    fs::write(temp.path().join("uv.lock"), "").unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init-hooks"])
        .status()
        .unwrap();
    assert!(status.success());

    let hooks = read(&temp.path().join("nix/pre-commit.nix"));
    assert!(hooks.contains("uv run ruff format --check ."));
    assert!(hooks.contains("uv run mypy ."));
}

#[test]
fn print_outputs_snippets_without_writing() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args(["init-hooks", "--print"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--- nix/treefmt.nix"));
    assert!(stdout.contains("--- nix/pre-commit.nix"));
    assert!(stdout.contains("--- flake.nix integration snippet"));
    assert!(stdout.contains("treefmt-nix.url = \"github:numtide/treefmt-nix\""));
    assert!(!temp.path().join("nix/treefmt.nix").exists());
}

#[test]
fn check_succeeds_when_hooks_are_current() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init-hooks"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let check_status = simit()
        .current_dir(temp.path())
        .args(["init-hooks", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn check_fails_when_hooks_differ() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init-hooks"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(temp.path().join("nix/treefmt.nix"), "{}\n").unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init-hooks", "--check"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("hook files are not up to date"));
    assert!(stderr.contains("nix/treefmt.nix differs"));
}

#[test]
fn check_and_print_are_mutually_exclusive() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args(["init-hooks", "--check", "--print"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("init-hooks accepts only one of --check or --print"));
}

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn simit() -> Command {
    Command::new(env!("CARGO_BIN_EXE_simit"))
}

fn init_package(name: &str, with_chocolatey: bool) -> TempDir {
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
authors = ["Example Maintainers"]
description = "Demo command line"
homepage = "https://example.com/{name}"
"#
        ),
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

    if with_chocolatey {
        fs::write(
            root.join("simit.toml"),
            r#"[chocolatey]
download_repo = "caniko/my-app"
"#,
        )
        .unwrap();
    }

    temp
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

#[test]
fn bootstraps_fresh_package_directory() {
    let project = init_package("my-app", true);
    let target_parent = TempDir::new().unwrap();
    let target = target_parent.path().join("chocolatey-my-app");

    let output = simit()
        .current_dir(project.path())
        .args(["init-chocolatey", "--target", target.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let nuspec = read(&target.join("my-app.nuspec"));
    assert!(nuspec.contains("<id>my-app</id>"));
    assert!(nuspec.contains("<version>0.1.0</version>"));
    assert!(nuspec.contains("<authors>Example Maintainers</authors>"));
    assert!(target.join("tools/chocolateyInstall.ps1").exists());
    assert!(target.join("tools/chocolateyUninstall.ps1").exists());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Initialised Chocolatey package"));
    assert!(stdout.contains("choco pack"));
    assert!(stdout.contains("simit chocolatey bump --version <version>"));
}

#[test]
fn check_succeeds_after_bootstrap_and_rerender_is_idempotent() {
    let project = init_package("demo", true);
    let target_parent = TempDir::new().unwrap();
    let target = target_parent.path().join("chocolatey-demo");

    for _ in 0..2 {
        let status = simit()
            .current_dir(project.path())
            .args(["init-chocolatey", "--target", target.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    let check = simit()
        .current_dir(project.path())
        .args([
            "init-chocolatey",
            "--target",
            target.to_str().unwrap(),
            "--check",
        ])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn check_fails_when_package_drifts() {
    let project = init_package("demo", true);
    let target_parent = TempDir::new().unwrap();
    let target = target_parent.path().join("chocolatey-demo");

    let write = simit()
        .current_dir(project.path())
        .args(["init-chocolatey", "--target", target.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(write.success());
    fs::write(target.join("demo.nuspec"), "<package />\n").unwrap();

    let output = simit()
        .current_dir(project.path())
        .args([
            "init-chocolatey",
            "--target",
            target.to_str().unwrap(),
            "--check",
            "--diff",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Chocolatey package is not up to date"));
    assert!(stderr.contains("--- "));
    assert!(stderr.contains("demo.nuspec differs"));
}

#[test]
fn print_writes_package_to_stdout_without_creating_target() {
    let project = init_package("demo", true);
    let target_parent = TempDir::new().unwrap();
    let target = target_parent.path().join("chocolatey-demo");

    let output = simit()
        .current_dir(project.path())
        .args([
            "init-chocolatey",
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
    assert!(stdout.contains("==> demo.nuspec"));
    assert!(stdout.contains("<version>0.1.0</version>"));
    assert!(stdout.contains("==> tools/chocolateyInstall.ps1"));
    assert!(!target.exists());
}

#[test]
fn missing_chocolatey_config_errors_clearly() {
    let project = init_package("demo", false);
    let target_parent = TempDir::new().unwrap();
    let target = target_parent.path().join("chocolatey-demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init-chocolatey", "--target", target.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("chocolatey.download_repo not set"));
}

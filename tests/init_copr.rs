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
            r#"[copr]
download_repo = "example/{name}"
project = "example/{name}"
build_requires = ["rust >= 1.85", "cargo"]
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
fn bootstraps_fresh_copr_files() {
    let project = init_package("demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "copr"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let spec = read(&project.path().join("demo.spec"));
    assert!(spec.contains("%global crate demo\n"));
    assert!(spec.contains("Version:        0.1.0\n"));
    assert!(spec.contains("BuildRequires:  rust >= 1.85\n"));
    assert!(spec.contains("install -Dm755 target/release/demo %{buildroot}%{_bindir}/demo\n"));
    let makefile = read(&project.path().join(".copr/Makefile"));
    assert!(makefile.contains("srpm:"));
    assert!(makefile.contains("cargo vendor"));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Initialised COPR packaging"));
    assert!(stdout.contains("git add demo.spec .copr/Makefile"));
    assert!(stdout.contains("copr-cli build <owner>/<project> <srpm>"));
}

#[test]
fn check_succeeds_after_bootstrap_and_rerender_is_idempotent() {
    let project = init_package("demo");

    for _ in 0..2 {
        let status = simit()
            .current_dir(project.path())
            .args(["init", "copr"])
            .status()
            .unwrap();
        assert!(status.success());
    }

    let check = simit()
        .current_dir(project.path())
        .args(["init", "copr", "--check"])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn print_matches_checked_files_without_writing_files() {
    let project = init_package("demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "copr", "--print"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("==> demo.spec"));
    assert!(stdout.contains("%global crate demo\n"));
    assert!(stdout.contains("==> .copr/Makefile"));
    assert!(stdout.contains("cargo vendor"));
    assert!(!project.path().join("demo.spec").exists());
    assert!(!project.path().join(".copr").exists());
}

#[test]
fn custom_makefile_path_agrees_across_render_write_check_and_badges() {
    let project = init_package("demo");
    let root = project.path();
    let config = read(&root.join("simit.toml"));
    fs::write(
        root.join("simit.toml"),
        format!("{config}makefile_path = \"dist/copr/Makefile\"\n"),
    )
    .unwrap();
    fs::write(root.join("README.md"), "# Demo\n\nProject documentation.\n").unwrap();

    for command in [
        vec!["dist", "copr", "render"],
        vec!["init", "copr", "--print"],
    ] {
        let output = simit().current_dir(root).args(command).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("==> dist/copr/Makefile"));
        assert!(!root.join("dist/copr/Makefile").exists());
    }

    let output = simit()
        .current_dir(root)
        .args(["init", "copr"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("git add demo.spec dist/copr/Makefile")
    );
    assert!(read(&root.join("dist/copr/Makefile")).contains("srpm:"));
    assert!(!root.join(".copr").exists());
    assert!(read(&root.join("README.md")).contains(")](dist/copr/Makefile)"));
    assert!(
        simit()
            .current_dir(root)
            .args(["init", "copr", "--check"])
            .status()
            .unwrap()
            .success()
    );

    fs::write(root.join("dist/copr/Makefile"), "drift\n").unwrap();
    let check = simit()
        .current_dir(root)
        .args(["init", "copr", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stderr).contains("dist/copr/Makefile"));
    assert_eq!(read(&root.join("dist/copr/Makefile")), "drift\n");
}

#[test]
fn apt_badge_uses_public_url_and_retains_default_fallback() {
    for (setting, expected) in [
        (
            "public_url = \"https://apt.example.com/\"\n",
            "https://apt.example.com/",
        ),
        ("", "dist/apt/conf/distributions"),
    ] {
        let project = init_package("demo");
        let root = project.path();
        let config = read(&root.join("simit.toml"));
        fs::write(root.join("simit.toml"), format!("{config}\n[apt]\nrepo_url = \"ssh://git@codeberg.org/example/demo-apt.git\"\n{setting}")).unwrap();
        fs::write(root.join("README.md"), "# Demo\n").unwrap();
        let output = simit()
            .current_dir(root)
            .args(["init", "copr"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(read(&root.join("README.md")).contains(&format!(")]({expected})")));
    }
}

#[test]
fn custom_makefile_path_rejects_empty_absolute_and_traversing_paths() {
    for path in ["", "/Makefile", "../Makefile", "dist/../Makefile", "."] {
        let project = init_package("demo");
        let root = project.path();
        let config = read(&root.join("simit.toml"));
        fs::write(
            root.join("simit.toml"),
            format!("{config}makefile_path = {path:?}\n"),
        )
        .unwrap();
        let output = simit()
            .current_dir(root)
            .args(["init", "copr"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted invalid path {path:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("makefile_path"));
        assert!(!root.join("demo.spec").exists());
    }
}

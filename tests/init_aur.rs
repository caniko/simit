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
            r#"[aur]
download_repo = "example/{name}"
depends = ["glibc"]
makedepends = ["cargo"]
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
fn bootstraps_fresh_aur_pkgbuilds() {
    let project = init_package("demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "aur"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source = read(&project.path().join("dist/aur/demo/PKGBUILD"));
    assert!(source.contains("pkgname=demo\n"));
    assert!(source.contains("pkgver=0.1.0\n"));
    assert!(source.contains("makedepends=('cargo')\n"));
    let binary = read(&project.path().join("dist/aur/demo-bin/PKGBUILD"));
    assert!(binary.contains("pkgname=demo-bin\n"));
    assert!(binary.contains("depends=('glibc')\n"));
    let vcs = read(&project.path().join("dist/aur/demo-git/PKGBUILD"));
    assert!(vcs.contains("pkgname=demo-git\n"));
    assert!(vcs.contains("source=('git+https://codeberg.org/example/demo.git')\n"));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Initialised AUR PKGBUILDs"));
    assert!(stdout.contains("git add dist/aur"));
    assert!(stdout.contains("publish on release via the generated workflow"));
}

#[test]
fn check_succeeds_after_bootstrap_and_rerender_is_idempotent() {
    let project = init_package("demo");

    for _ in 0..2 {
        let status = simit()
            .current_dir(project.path())
            .args(["init", "aur"])
            .status()
            .unwrap();
        assert!(status.success());
    }

    let check = simit()
        .current_dir(project.path())
        .args(["init", "aur", "--check"])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn print_matches_checked_pkgbuilds_without_writing_files() {
    let project = init_package("demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "aur", "--print"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("==> dist/aur/demo/PKGBUILD"));
    assert!(stdout.contains("pkgname=demo\n"));
    assert!(stdout.contains("==> dist/aur/demo-bin/PKGBUILD"));
    assert!(stdout.contains("pkgname=demo-bin\n"));
    assert!(stdout.contains("==> dist/aur/demo-git/PKGBUILD"));
    assert!(stdout.contains("pkgname=demo-git\n"));
    assert!(!project.path().join("dist/aur").exists());
}

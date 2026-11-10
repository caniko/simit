//! `simit binstall sync` against a fake flake and a stubbed `nix` binary.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

mod common;

const RENDERED: &str = r#"[package.metadata.binstall.overrides.x86_64-unknown-linux-gnu]
pkg-url = "{ repo }/releases/download/{ version }/demo-{ version }-x86_64-linux-musl.tar.gz"
pkg-fmt = "tgz"
bin-dir = "bin/{ bin }{ binary-ext }"
[package.metadata.binstall.overrides.x86_64-unknown-linux-musl]
pkg-url = "{ repo }/releases/download/{ version }/demo-{ version }-x86_64-linux-musl.tar.gz"
pkg-fmt = "tgz"
bin-dir = "bin/{ bin }{ binary-ext }"
"#;

const STALE: &str =
    "[package.metadata.binstall.overrides.x86_64-unknown-linux-gnu]\npkg-url = \"stale\"\n";

fn manifest(extra: &str) -> String {
    format!(
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\nrepository = \"https://github.com/caniko/demo\"\n\n[[bin]]\nname = \"demo\"\npath = \"src/main.rs\"\n\n{extra}"
    )
}

fn workspace(extra: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::write(root.join("Cargo.toml"), manifest(extra)).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    temp
}

/// A `nix` stub that renders `RENDERED` for `attr` and fails everything else.
fn stub_nix(dir: &Path, attrs: &[&str]) {
    let mut arms = String::new();
    for attr in attrs {
        arms.push_str(&format!(
            "  *{attr}*) printf '%s' \"$SIMIT_STUB_TOML\" ;;\n"
        ));
    }
    let path = dir.join("nix");
    fs::write(
        &path,
        format!("#!/bin/sh\ncase \"$*\" in\n{arms}  *) exit 1 ;;\nesac\n"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn simit(root: &Path, shim: &Path, args: &[&str]) -> Output {
    let path = format!("{}:{}", shim.display(), std::env::var("PATH").unwrap());
    common::simit()
        .current_dir(root)
        .env("PATH", path)
        .env("SIMIT_STUB_TOML", RENDERED)
        .args(args)
        .output()
        .unwrap()
}

fn binstall(root: &Path, shim: &Path, args: &[&str]) -> Output {
    let mut all = vec!["binstall", "sync", "--system", "x86_64-linux"];
    all.extend_from_slice(args);
    simit(root, shim, &all)
}

fn manifest_text(root: &Path) -> String {
    fs::read_to_string(root.join("Cargo.toml")).unwrap()
}

#[test]
fn check_reports_drift_and_sync_restores_the_rendered_block() {
    let temp = workspace(STALE);
    let shim = TempDir::new().unwrap();
    stub_nix(
        shim.path(),
        &["checks.x86_64-linux.binstall-metadata-demo.toml"],
    );
    let before = manifest_text(temp.path());

    let check = binstall(temp.path(), shim.path(), &["--check"]);
    assert_eq!(check.status.code(), Some(1));
    let stdout = String::from_utf8(check.stdout).unwrap();
    assert!(stdout.contains("--- demo: a/Cargo.toml"), "{stdout}");
    assert!(stdout.contains("-pkg-url = \"stale\""), "{stdout}");
    assert!(
        stdout.contains("+bin-dir = \"bin/{ bin }{ binary-ext }\""),
        "{stdout}"
    );
    assert_eq!(manifest_text(temp.path()), before, "check must not write");

    let sync = binstall(temp.path(), shim.path(), &[]);
    assert!(sync.status.success(), "{sync:?}");
    let written = manifest_text(temp.path());
    assert!(written.contains(RENDERED), "{written}");
    assert!(!written.contains("stale"), "{written}");
    assert!(written.contains("name = \"demo\""), "{written}");

    let clean = binstall(temp.path(), shim.path(), &["--check"]);
    assert!(clean.status.success(), "{clean:?}");
    assert!(
        String::from_utf8(clean.stdout)
            .unwrap()
            .contains("metadata in sync")
    );
}

#[test]
fn missing_checks_are_reported_with_the_flake_hint() {
    let temp = workspace("");
    let shim = TempDir::new().unwrap();
    stub_nix(shim.path(), &[]);
    let before = manifest_text(temp.path());

    let check = binstall(temp.path(), shim.path(), &["--check"]);
    assert_eq!(check.status.code(), Some(1));
    let stdout = String::from_utf8(check.stdout).unwrap();
    assert!(stdout.contains("missing metadata"), "{stdout}");
    assert!(
        stdout.contains("expose mkBinstallRelease checks in flake.nix"),
        "{stdout}"
    );

    // Write mode reports the same gap without touching the manifest.
    let sync = binstall(temp.path(), shim.path(), &[]);
    assert!(sync.status.success(), "{sync:?}");
    assert!(
        String::from_utf8(sync.stdout)
            .unwrap()
            .contains("expose mkBinstallRelease checks in flake.nix")
    );
    assert_eq!(manifest_text(temp.path()), before);
}

#[test]
fn package_filters_select_only_those_manifests() {
    let temp = workspace("");
    let shim = TempDir::new().unwrap();
    stub_nix(
        shim.path(),
        &["checks.x86_64-linux.binstall-metadata-demo.toml"],
    );

    let unknown = binstall(temp.path(), shim.path(), &["--package", "other"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(
        String::from_utf8(unknown.stderr)
            .unwrap()
            .contains("--package other is not a workspace member")
    );

    let sync = binstall(temp.path(), shim.path(), &["--package", "demo"]);
    assert!(sync.status.success(), "{sync:?}");
    assert!(manifest_text(temp.path()).contains(RENDERED));
}

#[test]
fn single_binary_workspaces_fall_back_to_the_unsuffixed_check() {
    let temp = workspace("");
    let shim = TempDir::new().unwrap();
    stub_nix(shim.path(), &["checks.x86_64-linux.binstall-metadata.toml"]);

    let sync = binstall(temp.path(), shim.path(), &[]);
    assert!(sync.status.success(), "{sync:?}");
    assert!(manifest_text(temp.path()).contains(RENDERED));
}

#[test]
fn help_lists_sync_flags() {
    let stdout = String::from_utf8(
        common::simit()
            .args(["binstall", "sync", "--help"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    for flag in ["--check", "--package <NAME>", "--system <SYS>"] {
        assert!(stdout.contains(flag), "{flag}: {stdout}");
    }
}

#[test]
fn release_verify_checks_binstall_metadata_only_when_opted_in() {
    let temp = workspace("");
    let shim = TempDir::new().unwrap();
    stub_nix(
        shim.path(),
        &["checks.x86_64-linux.binstall-metadata-demo.toml"],
    );
    let root = temp.path();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init"]);
    git(&["config", "core.hooksPath", "/dev/null"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Test User",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-m",
        "initial",
    ]);
    fs::create_dir_all(root.join("keys")).unwrap();
    fs::copy(
        common::maintainer_key_path(),
        root.join("keys/maintainers.gpg"),
    )
    .unwrap();
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## [0.1.0] - 2026-05-25\n\n- Initial release.\n",
    )
    .unwrap();
    // A hosted target keeps `release verify` off crates.io.
    let config = "[package.metadata.simit.release.github]\nrepo = \"caniko/demo\"\n";
    let verify = |artifacts: &str| {
        fs::write(
            root.join("Cargo.toml"),
            manifest(&format!("{config}{artifacts}")),
        )
        .unwrap();
        simit(root, shim.path(), &["release", "verify", "--json"])
    };
    let reported = |output: &Output| {
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        value["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["check"] == "cargo-binstall metadata")
    };

    let advisory = verify("");
    assert!(!reported(&advisory), "{advisory:?}");

    let fatal = verify(
        "[package.metadata.simit.release.artifacts]\nprebuild_binaries = true\nbinstall = true\n",
    );
    assert_eq!(fatal.status.code(), Some(1));
    assert!(reported(&fatal), "{fatal:?}");
}

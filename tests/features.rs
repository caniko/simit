use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
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
rust-version = "1.85"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    run(root, "cargo", &["generate-lockfile"]);
    run(root, "git", &["add", "."]);
    run(root, "git", &["commit", "-m", "initial"]);
    temp
}

fn init_workspace(a_version: &str, b_version: &str) -> TempDir {
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
    for (package, version) in [("a", a_version), ("b", b_version)] {
        let package_dir = root.join(package);
        fs::create_dir(&package_dir).unwrap();
        fs::create_dir(package_dir.join("src")).unwrap();
        fs::write(
            package_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package}\"\nversion = \"{version}\"\nedition = \"2024\"\nrust-version = \"1.85\"\n"
            ),
        )
        .unwrap();
        fs::write(package_dir.join("src/lib.rs"), "").unwrap();
    }
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    run(root, "cargo", &["generate-lockfile"]);
    run(root, "git", &["add", "."]);
    run(root, "git", &["commit", "-m", "initial"]);
    temp
}

#[test]
fn dry_run_does_not_mutate_version_commit_or_tag() {
    let temp = init_package();
    let root = temp.path();
    let before_manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let before_head = output(root, "git", &["rev-parse", "HEAD"]);

    let status = simit()
        .current_dir(root)
        .args([
            "commit",
            "--dry-run",
            "--no-sign",
            "patch",
            "-m",
            "release patch",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    assert_eq!(
        fs::read_to_string(root.join("Cargo.toml")).unwrap(),
        before_manifest
    );
    assert_eq!(output(root, "git", &["rev-parse", "HEAD"]), before_head);
    assert!(output(root, "git", &["tag", "--list"]).trim().is_empty());
}

#[test]
fn prerelease_bump_uses_semver_prerelease() {
    let temp = init_package();
    let root = temp.path();

    let status = simit()
        .current_dir(root)
        .args([
            "commit",
            "--no-sign",
            "--no-tag",
            "patch",
            "--pre",
            "rc.1",
            "-m",
            "release candidate",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("version = \"0.1.1-rc.1\""));
}

#[test]
fn workspace_batch_release_creates_one_version_tag() {
    let temp = init_workspace("0.1.0", "0.1.0");
    let root = temp.path();

    let status = simit()
        .current_dir(root)
        .args([
            "commit",
            "--workspace",
            "--no-sign",
            "patch",
            "-m",
            "workspace release",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    assert!(
        fs::read_to_string(root.join("a/Cargo.toml"))
            .unwrap()
            .contains("version = \"0.1.1\"")
    );
    assert!(
        fs::read_to_string(root.join("b/Cargo.toml"))
            .unwrap()
            .contains("version = \"0.1.1\"")
    );
    assert_eq!(output(root, "git", &["tag", "--list"]).trim(), "0.1.1");
}

#[test]
fn workspace_batch_fails_when_versions_diverge() {
    let temp = init_workspace("0.1.0", "0.2.0");
    let root = temp.path();

    let output = simit()
        .current_dir(root)
        .args([
            "commit",
            "--workspace",
            "--no-sign",
            "patch",
            "-m",
            "workspace release",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("selected packages do not resolve to one release version"));
}

#[test]
fn release_updates_keep_a_changelog() {
    let temp = init_package();
    let root = temp.path();
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n\n- Add release flow\n",
    )
    .unwrap();
    run(root, "git", &["add", "CHANGELOG.md"]);
    run(root, "git", &["commit", "-m", "add changelog"]);

    let status = simit()
        .current_dir(root)
        .args(["release", "--no-sign", "patch", "-m", "release patch"])
        .status()
        .unwrap();
    assert!(status.success());

    let changelog = fs::read_to_string(root.join("CHANGELOG.md")).unwrap();
    assert!(changelog.contains("## [Unreleased]"));
    assert!(changelog.contains("## [0.1.1] - "));
    assert!(changelog.contains("- Add release flow"));
    assert_eq!(output(root, "git", &["tag", "--list"]).trim(), "0.1.1");
}

#[test]
fn release_sync_up_moves_current_version_tag_to_head() {
    let temp = init_package();
    let root = temp.path();
    run(root, "git", &["tag", "0.1.0"]);
    let old_target = output(root, "git", &["rev-parse", "0.1.0^{commit}"]);
    fs::write(
        root.join("src/main.rs"),
        "fn main() { println!(\"fixed\"); }\n",
    )
    .unwrap();
    run(root, "git", &["add", "src/main.rs"]);
    run(root, "git", &["commit", "-m", "fix release ci"]);
    let head = output(root, "git", &["rev-parse", "HEAD"]);

    let status = simit()
        .current_dir(root)
        .args(["release", "sync-up", "--no-sign"])
        .status()
        .unwrap();
    assert!(status.success());

    assert_ne!(old_target, head);
    assert_eq!(output(root, "git", &["rev-parse", "0.1.0^{commit}"]), head);
}

#[test]
fn release_sync_up_dry_run_does_not_move_tag() {
    let temp = init_package();
    let root = temp.path();
    run(root, "git", &["tag", "0.1.0"]);
    let old_target = output(root, "git", &["rev-parse", "0.1.0^{commit}"]);
    fs::write(
        root.join("src/main.rs"),
        "fn main() { println!(\"fixed\"); }\n",
    )
    .unwrap();
    run(root, "git", &["add", "src/main.rs"]);
    run(root, "git", &["commit", "-m", "fix release ci"]);

    let status = simit()
        .current_dir(root)
        .args(["release", "sync-up", "--no-sign", "--dry-run"])
        .status()
        .unwrap();
    assert!(status.success());

    assert_eq!(
        output(root, "git", &["rev-parse", "0.1.0^{commit}"]),
        old_target
    );
}

#[test]
fn release_sync_up_rejects_dirty_worktree() {
    let temp = init_package();
    let root = temp.path();
    run(root, "git", &["tag", "0.1.0"]);
    fs::write(
        root.join("src/main.rs"),
        "fn main() { println!(\"dirty\"); }\n",
    )
    .unwrap();

    let output = simit()
        .current_dir(root)
        .args(["release", "sync-up", "--no-sign"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("worktree has uncommitted changes")
    );
}

#[test]
fn release_sync_up_rejects_missing_current_version_tag() {
    let temp = init_package();
    let root = temp.path();

    let output = simit()
        .current_dir(root)
        .args(["release", "sync-up", "--no-sign"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("tag 0.1.0 does not exist")
    );
}

#[test]
fn release_sync_up_rejects_workspace_version_disagreement() {
    let temp = init_workspace("0.1.0", "0.2.0");
    let root = temp.path();
    run(root, "git", &["tag", "0.1.0"]);

    let output = simit()
        .current_dir(root)
        .args(["release", "sync-up", "--workspace", "--no-sign"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("selected packages do not have one current release version")
    );
}

#[test]
fn release_sync_up_pushes_moved_tag_to_remote() {
    let temp = init_package();
    let root = temp.path();
    let remote = TempDir::new().unwrap();
    run(remote.path(), "git", &["init", "--bare"]);
    run(
        root,
        "git",
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    run(root, "git", &["tag", "0.1.0"]);
    run(root, "git", &["push", "origin", "HEAD", "refs/tags/0.1.0"]);
    fs::write(
        root.join("src/main.rs"),
        "fn main() { println!(\"fixed\"); }\n",
    )
    .unwrap();
    run(root, "git", &["add", "src/main.rs"]);
    run(root, "git", &["commit", "-m", "fix release ci"]);
    let head = output(root, "git", &["rev-parse", "HEAD"]);

    let status = simit()
        .current_dir(root)
        .args(["release", "sync-up", "--no-sign", "--push"])
        .status()
        .unwrap();
    assert!(status.success());

    assert_eq!(
        output(root, "git", &["ls-remote", "origin", "refs/tags/0.1.0"])
            .split_whitespace()
            .next()
            .unwrap(),
        head.trim()
    );
}

#[test]
fn release_sync_up_can_push_after_local_sync() {
    let temp = init_package();
    let root = temp.path();
    let remote = TempDir::new().unwrap();
    run(remote.path(), "git", &["init", "--bare"]);
    run(
        root,
        "git",
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    run(root, "git", &["tag", "0.1.0"]);
    run(root, "git", &["push", "origin", "HEAD", "refs/tags/0.1.0"]);
    fs::write(
        root.join("src/main.rs"),
        "fn main() { println!(\"fixed\"); }\n",
    )
    .unwrap();
    run(root, "git", &["add", "src/main.rs"]);
    run(root, "git", &["commit", "-m", "fix release ci"]);
    let head = output(root, "git", &["rev-parse", "HEAD"]);

    let local_status = simit()
        .current_dir(root)
        .args(["release", "sync-up", "--no-sign"])
        .status()
        .unwrap();
    assert!(local_status.success());

    let push_status = simit()
        .current_dir(root)
        .args(["release", "sync-up", "--no-sign", "--push"])
        .status()
        .unwrap();
    assert!(push_status.success());

    assert_eq!(
        output(root, "git", &["ls-remote", "origin", "refs/tags/0.1.0"])
            .split_whitespace()
            .next()
            .unwrap(),
        head.trim()
    );
}

#[test]
fn release_preserves_existing_staged_changes() {
    let temp = init_package();
    let root = temp.path();
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n\n- Add release flow\n",
    )
    .unwrap();
    fs::write(root.join("NOTES.md"), "ship it\n").unwrap();
    run(root, "git", &["add", "CHANGELOG.md", "NOTES.md"]);
    run(root, "git", &["commit", "-m", "add release notes"]);
    fs::write(root.join("NOTES.md"), "ship it\nwith staged context\n").unwrap();
    run(root, "git", &["add", "NOTES.md"]);

    let status = simit()
        .current_dir(root)
        .args(["release", "--no-sign", "patch", "-m", "release patch"])
        .status()
        .unwrap();
    assert!(status.success());

    let names = output(root, "git", &["show", "--name-only", "--format="]);
    assert!(names.lines().any(|line| line == "NOTES.md"));
    assert!(names.lines().any(|line| line == "Cargo.toml"));
    assert!(names.lines().any(|line| line == "Cargo.lock"));
    assert!(names.lines().any(|line| line == "CHANGELOG.md"));
}

#[test]
fn release_allows_staged_version_file_changes() {
    let temp = init_package();
    let root = temp.path();
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n\n- Add release flow\n",
    )
    .unwrap();
    run(root, "git", &["add", "CHANGELOG.md"]);
    run(root, "git", &["commit", "-m", "add changelog"]);

    let mut manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    manifest.push_str("description = \"demo package\"\n");
    fs::write(root.join("Cargo.toml"), manifest).unwrap();
    run(root, "git", &["add", "Cargo.toml"]);

    let status = simit()
        .current_dir(root)
        .args(["release", "--no-sign", "patch", "-m", "release patch"])
        .status()
        .unwrap();
    assert!(status.success());

    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("version = \"0.1.1\""));
    assert!(manifest.contains("description = \"demo package\""));

    let show = output(root, "git", &["show", "--format=", "--", "Cargo.toml"]);
    assert!(show.contains("+description = \"demo package\""));
    assert!(show.contains("+version = \"0.1.1\""));
}

#[test]
fn release_fails_without_unreleased_changelog_section() {
    let temp = init_package();
    let root = temp.path();
    let before_manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(root.join("CHANGELOG.md"), "# Changelog\n").unwrap();
    run(root, "git", &["add", "CHANGELOG.md"]);
    run(root, "git", &["commit", "-m", "add changelog"]);

    let output = simit()
        .current_dir(root)
        .args(["release", "--no-sign", "patch", "-m", "release patch"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("CHANGELOG.md must contain `## [Unreleased]`"));
    assert_eq!(
        fs::read_to_string(root.join("Cargo.toml")).unwrap(),
        before_manifest
    );
}

#[test]
fn dirty_version_file_fails_preflight() {
    let temp = init_package();
    let root = temp.path();
    let mut manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    manifest.push_str("\n# dirty\n");
    fs::write(root.join("Cargo.toml"), manifest).unwrap();

    let output = simit()
        .current_dir(root)
        .args(["commit", "--no-sign", "patch", "-m", "release patch"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("version files have uncommitted changes"));
}

#[test]
fn preflight_reports_existing_tag_detached_head_and_missing_signing_key() {
    let existing_tag = init_package();
    let root = existing_tag.path();
    run(root, "git", &["tag", "0.1.1"]);
    let output = simit()
        .current_dir(root)
        .args(["commit", "--no-sign", "patch", "-m", "release patch"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("tag 0.1.1 already exists")
    );

    let detached = init_package();
    let root = detached.path();
    run(root, "git", &["checkout", "--detach"]);
    let output = simit()
        .current_dir(root)
        .args(["commit", "--no-sign", "patch", "-m", "release patch"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("git HEAD is detached")
    );

    let unsigned = init_package();
    let root = unsigned.path();
    let output = simit()
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", root.join("missing-global"))
        .args(["commit", "patch", "-m", "release patch"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("signed tags require git config user.signingkey")
    );
}

#[test]
fn release_preflight_reports_existing_tag_detached_head_and_missing_signing_key() {
    let existing_tag = init_package();
    let root = existing_tag.path();
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n\n- Add release flow\n",
    )
    .unwrap();
    run(root, "git", &["add", "CHANGELOG.md"]);
    run(root, "git", &["commit", "-m", "add changelog"]);
    run(root, "git", &["tag", "0.1.1"]);
    let output = simit()
        .current_dir(root)
        .args(["release", "--no-sign", "patch", "-m", "release patch"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("tag 0.1.1 already exists")
    );

    let detached = init_package();
    let root = detached.path();
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n\n- Add release flow\n",
    )
    .unwrap();
    run(root, "git", &["add", "CHANGELOG.md"]);
    run(root, "git", &["commit", "-m", "add changelog"]);
    run(root, "git", &["checkout", "--detach"]);
    let output = simit()
        .current_dir(root)
        .args(["release", "--no-sign", "patch", "-m", "release patch"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("git HEAD is detached")
    );

    let unsigned = init_package();
    let root = unsigned.path();
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n\n- Add release flow\n",
    )
    .unwrap();
    run(root, "git", &["add", "CHANGELOG.md"]);
    run(root, "git", &["commit", "-m", "add changelog"]);
    let output = simit()
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", root.join("missing-global"))
        .args(["release", "patch", "-m", "release patch"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("signed tags require git config user.signingkey")
    );
}

#[test]
fn init_flake_creates_prints_checks_and_refuses_unpatchable_existing_flake() {
    let temp = init_package();
    let root = temp.path();

    let print = simit()
        .current_dir(root)
        .args(["init", "flake", "--print"])
        .output()
        .unwrap();
    assert!(print.status.success());
    assert!(
        String::from_utf8(print.stdout)
            .unwrap()
            .contains("--- nix/pre-commit.nix")
    );
    assert!(!root.join("flake.nix").exists());
    assert!(!root.join("nix/treefmt.nix").exists());

    let status = simit()
        .current_dir(root)
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!root.join("flake.nix").exists());
    assert!(!root.join("nix/treefmt.nix").exists());
    assert!(root.join("nix/pre-commit.nix").exists());

    let check = simit()
        .current_dir(root)
        .args(["init", "flake", "--check"])
        .status()
        .unwrap();
    assert!(check.success());

    fs::write(root.join("flake.nix"), "{}\n").unwrap();
    let output = simit()
        .current_dir(root)
        .args(["init", "flake", "--scope", "full"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("cannot patch flake.nix")
    );
}

#[test]
fn ci_options_emit_expected_steps_and_msrv_requires_rust_version() {
    let temp = init_package();
    let root = temp.path();

    let status = simit()
        .current_dir(root)
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-nextest",
            "--with-msrv",
            "--with-audit",
            "--with-deny",
            "--with-docs",
            "--with-artifacts",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = fs::read_to_string(root.join(".github/workflows/ci.yaml")).unwrap();
    assert!(ci.contains("cargo nextest run --all-features"));
    assert!(ci.contains("cargo +1.85 check --all-targets"));
    assert!(ci.contains("cargo audit"));
    assert!(ci.contains("cargo deny check bans licenses sources"));
    assert!(ci.contains("cargo doc --no-deps --all-features"));
    let deny = fs::read_to_string(root.join("deny.toml")).unwrap();
    assert!(deny.contains("\"MIT\""));
    assert!(deny.contains("\"BSD-2-Clause\""));
    assert!(deny.contains("\"BSL-1.0\""));
    assert!(deny.contains("\"CC0-1.0\""));
    assert!(deny.contains("\"ISC\""));
    assert!(deny.contains("\"LicenseRef-UFL-1.0\""));
    assert!(deny.contains("\"Apache-2.0\""));
    assert!(deny.contains("\"OFL-1.1\""));
    assert!(deny.contains("\"Zlib\""));
    assert!(
        root.join(".github/workflows/release-artifacts.yaml")
            .exists()
    );

    let no_msrv = TempDir::new().unwrap();
    let root = no_msrv.path();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let output = simit()
        .current_dir(root)
        .args(["init", "ci", "--platform", "github", "--with-msrv"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("--with-msrv requires package.rust-version")
    );
}

#[test]
fn generated_diff_is_printed_for_stale_hooks() {
    let temp = init_package();
    let root = temp.path();

    let status = simit()
        .current_dir(root)
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(status.success());
    fs::write(root.join("nix/treefmt.nix"), "{}\n").unwrap();

    let output = simit()
        .current_dir(root)
        .args(["init", "flake", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--- nix/treefmt.nix"));
    assert!(stderr.contains("+++ nix/treefmt.nix"));
}

#[test]
fn completions_and_man_are_non_empty() {
    let temp = init_package();
    let root = temp.path();

    let completions = simit()
        .current_dir(root)
        .args(["completions", "bash"])
        .output()
        .unwrap();
    assert!(completions.status.success());
    assert!(!completions.stdout.is_empty());

    let man = simit().current_dir(root).args(["man"]).output().unwrap();
    assert!(man.status.success());
    assert!(!man.stdout.is_empty());
}

#[test]
fn init_help_is_grouped() {
    let temp = init_package();
    let root = temp.path();

    let top_help = simit().current_dir(root).arg("--help").output().unwrap();
    assert!(top_help.status.success());
    let top_help = String::from_utf8(top_help.stdout).unwrap();
    for old in [
        "init-ci",
        "init-flake",
        "init-homebrew-tap",
        "init-chocolatey",
        "init-scoop-bucket",
    ] {
        assert!(!top_help.contains(old), "top-level help still lists {old}");
    }
    assert!(top_help.contains("init"));

    let init_help = simit()
        .current_dir(root)
        .args(["init", "--help"])
        .output()
        .unwrap();
    assert!(init_help.status.success());
    let init_help = String::from_utf8(init_help.stdout).unwrap();
    let commands: Vec<_> = init_help
        .lines()
        .skip_while(|line| line.trim() != "Commands:")
        .skip(1)
        .take_while(|line| line.trim() != "Options:")
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(
        commands,
        ["ci", "flake", "homebrew-tap", "chocolatey", "scoop-bucket"]
    );
}

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
rust-version = "1.85"
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
        .args(["init-flake", "--print"])
        .output()
        .unwrap();
    assert!(print.status.success());
    assert!(
        String::from_utf8(print.stdout)
            .unwrap()
            .contains("--- flake.nix")
    );
    assert!(!root.join("flake.nix").exists());
    assert!(!root.join("nix/treefmt.nix").exists());

    let status = simit()
        .current_dir(root)
        .args(["init-flake"])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(root.join("flake.nix").exists());
    assert!(root.join("nix/treefmt.nix").exists());
    assert!(root.join("nix/pre-commit.nix").exists());

    let check = simit()
        .current_dir(root)
        .args(["init-flake", "--check"])
        .status()
        .unwrap();
    assert!(check.success());

    fs::write(root.join("flake.nix"), "{}\n").unwrap();
    let output = simit()
        .current_dir(root)
        .args(["init-flake"])
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
            "init-ci",
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
    assert!(ci.contains("cargo deny check"));
    assert!(ci.contains("cargo doc --no-deps --all-features"));
    let deny = fs::read_to_string(root.join("deny.toml")).unwrap();
    assert!(deny.contains("\"MIT\""));
    assert!(deny.contains("\"Apache-2.0\""));
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
        .args(["init-ci", "--platform", "github", "--with-msrv"])
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
        .args(["init-flake"])
        .status()
        .unwrap();
    assert!(status.success());
    fs::write(root.join("nix/treefmt.nix"), "{}\n").unwrap();

    let output = simit()
        .current_dir(root)
        .args(["init-flake", "--check", "--diff"])
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

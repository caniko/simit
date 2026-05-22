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

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

fn init_release_repo() -> TempDir {
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

#[test]
fn init_creates_keep_a_changelog_skeleton() {
    let temp = TempDir::new().unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["changelog", "init"])
        .status()
        .unwrap();
    assert!(status.success());

    let changelog = read(&temp.path().join("CHANGELOG.md"));
    assert!(changelog.starts_with(
        "# Changelog\n\nAll notable changes to this project will be documented in this file.\n\n"
    ));
    assert!(changelog.contains("The format is based on [Keep a Changelog]"));
    assert!(changelog.ends_with("\n## [Unreleased]\n"));
}

#[test]
fn add_creates_missing_sections_and_appends_existing_ones() {
    let temp = TempDir::new().unwrap();
    run(
        temp.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "init"],
    );

    let status = simit()
        .current_dir(temp.path())
        .args(["changelog", "add", "fixed", "Fix panic on empty input"])
        .status()
        .unwrap();
    assert!(status.success());
    let status = simit()
        .current_dir(temp.path())
        .args(["changelog", "add", "added", "Support a new API"])
        .status()
        .unwrap();
    assert!(status.success());
    let status = simit()
        .current_dir(temp.path())
        .args(["changelog", "add", "fixed", "Fix race in release flow"])
        .status()
        .unwrap();
    assert!(status.success());

    let changelog = read(&temp.path().join("CHANGELOG.md"));
    assert!(changelog.contains("### Added\n\n- Support a new API\n\n### Fixed\n\n- Fix panic on empty input\n- Fix race in release flow\n"));
}

#[test]
fn release_promotes_unreleased_and_updates_compare_links() {
    let temp = TempDir::new().unwrap();
    run(
        temp.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "init"],
    );
    run(
        temp.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "add", "added", "Ship the first release"],
    );

    let status = simit()
        .current_dir(temp.path())
        .args([
            "changelog",
            "release",
            "0.1.0",
            "--date",
            "2026-05-20",
            "--repo-url",
            "https://example.com/acme/demo",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let changelog = read(&temp.path().join("CHANGELOG.md"));
    assert!(changelog.contains(
        "## [Unreleased]\n\n## [0.1.0] - 2026-05-20\n\n### Added\n\n- Ship the first release\n"
    ));
    assert!(changelog.contains("[Unreleased]: https://example.com/acme/demo/compare/0.1.0...HEAD"));

    run(
        temp.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "add", "fixed", "Patch the first release"],
    );
    let status = simit()
        .current_dir(temp.path())
        .args([
            "changelog",
            "release",
            "0.1.1",
            "--date",
            "2026-05-21",
            "--repo-url",
            "https://example.com/acme/demo",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let changelog = read(&temp.path().join("CHANGELOG.md"));
    assert!(changelog.contains("## [0.1.1] - 2026-05-21"));
    assert!(changelog.contains("[Unreleased]: https://example.com/acme/demo/compare/0.1.1...HEAD"));
    assert!(changelog.contains("[0.1.1]: https://example.com/acme/demo/compare/0.1.0...0.1.1"));
}

#[test]
fn release_rejects_empty_unreleased() {
    let temp = TempDir::new().unwrap();
    run(
        temp.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "init"],
    );

    let output = simit()
        .current_dir(temp.path())
        .args(["changelog", "release", "0.1.0", "--date", "2026-05-20"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("[Unreleased] is empty")
    );
}

#[test]
fn release_rejects_invalid_calendar_dates() {
    let temp = TempDir::new().unwrap();
    run(
        temp.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "init"],
    );
    run(
        temp.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "add", "fixed", "Prepare release"],
    );

    let output = simit()
        .current_dir(temp.path())
        .args(["changelog", "release", "0.1.0", "--date", "2025-02-29"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("parsing calendar date `2025-02-29`")
    );
}

#[test]
fn check_rejects_noncanonical_header_malformed_versions_and_out_of_order_sections() {
    let bad_header = TempDir::new().unwrap();
    fs::write(
        bad_header.path().join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n",
    )
    .unwrap();
    let output = simit()
        .current_dir(bad_header.path())
        .args(["changelog", "check"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("canonical Keep a Changelog header")
    );

    let malformed = TempDir::new().unwrap();
    fs::write(
        malformed.path().join("CHANGELOG.md"),
        "# Changelog\n\nAll notable changes to this project will be documented in this file.\n\nThe format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),\nand this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).\n\n## [Unreleased]\n\n## [oops] - 2026-05-20\n",
    )
    .unwrap();
    let output = simit()
        .current_dir(malformed.path())
        .args(["changelog", "check"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("parsing changelog version")
    );

    let out_of_order = TempDir::new().unwrap();
    fs::write(
        out_of_order.path().join("CHANGELOG.md"),
        "# Changelog\n\nAll notable changes to this project will be documented in this file.\n\nThe format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),\nand this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).\n\n## [Unreleased]\n\n## [0.1.0] - 2026-05-20\n\n- First\n\n## [0.2.0] - 2026-05-21\n\n- Second\n",
    )
    .unwrap();
    let output = simit()
        .current_dir(out_of_order.path())
        .args(["changelog", "check"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("descending semver order")
    );
}

#[test]
fn show_prints_the_requested_section_body() {
    let temp = TempDir::new().unwrap();
    run(
        temp.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "init"],
    );
    run(
        temp.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "add", "added", "Ship alpha"],
    );
    run(
        temp.path(),
        env!("CARGO_BIN_EXE_simit"),
        &[
            "changelog",
            "release",
            "0.1.0",
            "--date",
            "2026-05-20",
            "--repo-url",
            "https://example.com/acme/demo",
        ],
    );
    run(
        temp.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "add", "fixed", "Patch alpha"],
    );

    let released = simit()
        .current_dir(temp.path())
        .args(["changelog", "show", "0.1.0"])
        .output()
        .unwrap();
    assert!(released.status.success());
    assert_eq!(
        String::from_utf8(released.stdout).unwrap(),
        "### Added\n\n- Ship alpha\n"
    );

    let unreleased = simit()
        .current_dir(temp.path())
        .args(["changelog", "show"])
        .output()
        .unwrap();
    assert!(unreleased.status.success());
    assert_eq!(
        String::from_utf8(unreleased.stdout).unwrap(),
        "### Fixed\n\n- Patch alpha\n"
    );
}

#[test]
fn release_command_promotes_changelog_and_honors_no_changelog() {
    let promoted = init_release_repo();
    run(
        promoted.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "init"],
    );
    run(
        promoted.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "add", "added", "Prepare release automation"],
    );
    run(promoted.path(), "git", &["add", "CHANGELOG.md"]);
    run(promoted.path(), "git", &["commit", "-m", "add changelog"]);

    let status = simit()
        .current_dir(promoted.path())
        .args(["release", "--no-sign", "patch", "-m", "release patch"])
        .status()
        .unwrap();
    assert!(status.success());

    let changelog = read(&promoted.path().join("CHANGELOG.md"));
    assert!(changelog.contains("## [0.1.1] - "));
    assert!(output(promoted.path(), "git", &["tag", "--list"]).contains("0.1.1"));

    let skipped = init_release_repo();
    run(
        skipped.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "init"],
    );
    run(
        skipped.path(),
        env!("CARGO_BIN_EXE_simit"),
        &["changelog", "add", "fixed", "Keep unreleased notes intact"],
    );
    run(skipped.path(), "git", &["add", "CHANGELOG.md"]);
    run(skipped.path(), "git", &["commit", "-m", "add changelog"]);

    let status = simit()
        .current_dir(skipped.path())
        .args([
            "release",
            "--no-sign",
            "--no-changelog",
            "patch",
            "-m",
            "release patch",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let changelog = read(&skipped.path().join("CHANGELOG.md"));
    assert!(changelog.contains("### Fixed\n\n- Keep unreleased notes intact"));
    assert!(!changelog.contains("## [0.1.1] - "));
}

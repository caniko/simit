use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

fn simit() -> Command {
    Command::new(env!("CARGO_BIN_EXE_simit"))
}

fn simit_with_data_home(data_home: &Path) -> Command {
    let mut command = simit();
    command.env("XDG_DATA_HOME", data_home);
    command
}

fn init_package(root: &Path, name: &str) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "MIT"
"#
        ),
    )
    .unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("README.md"), format!("# {name}\n\nbody\n")).unwrap();
}

fn init_pages_project(name: &str) -> TempDir {
    let project = non_tmp_project(name);
    fs::write(
        project.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "MIT"
repository = "https://codeberg.org/caniko/{name}"
"#
        ),
    )
    .unwrap();
    fs::write(
        project.path().join("flake.nix"),
        r#"{ outputs = { self }: { apps.x86_64-linux.deploy-pages = {}; }; }"#,
    )
    .unwrap();
    fs::create_dir_all(project.path().join(".forgejo/workflows")).unwrap();
    fs::write(
        project.path().join(".forgejo/workflows/pages.yaml"),
        r#"---
name: Publish Pages

"on":
  push:
    branches: [trunk]

jobs:
  publish:
    runs-on: atlas-nix-trusted
    steps:
      - name: Checkout
        uses: https://code.forgejo.org/actions/checkout@v4

      - name: Validate Pages domain
        run: |
          nix build .#site --no-link --out-link result-pages-site
          test -f result-pages-site/.domains
          grep -qx upgrade-pages.tartanoglu.com result-pages-site/.domains

      - name: Deploy Codeberg Pages
        run: nix run .#deploy-pages
"#,
    )
    .unwrap();
    project
}

fn non_tmp_project(name: &str) -> TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/upgrade-fixtures");
    fs::create_dir_all(&root).unwrap();
    let project = tempfile::Builder::new()
        .prefix(name)
        .tempdir_in(root)
        .unwrap();
    init_package(project.path(), name);
    project
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

type RegistryProject<'a> = (&'a Path, &'a str, &'a [(&'a str, &'a str)]);

fn write_registry(data_home: &Path, projects: &[RegistryProject<'_>]) {
    let path = data_home.join("simit/projects.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = String::from("schema_version = 1\n");
    for (project_path, name, features) in projects {
        text.push_str("\n[[project]]\n");
        text.push_str(&format!("path = \"{}\"\n", project_path.display()));
        text.push_str(&format!("name = \"{name}\"\n"));
        text.push_str("first_seen = \"2026-05-20T00:00:00Z\"\n");
        text.push_str("last_seen = \"2026-05-21T00:00:00Z\"\n");
        text.push_str("[project.features]\n");
        for (feature, status) in *features {
            text.push_str(&format!("{feature} = \"{status}\"\n"));
        }
    }
    fs::write(path, text).unwrap();
}

#[test]
fn upgrade_current_workspace_writes_managed_badge_block() {
    let project = non_tmp_project("upgrade-current");

    let output = simit()
        .current_dir(project.path())
        .args(["upgrade"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let readme = read(&project.path().join("README.md"));
    assert!(readme.contains("<!-- simit:badges:start -->"));
    assert!(readme.contains("https://img.shields.io/badge/crates.io-ready-f46623"));
    assert!(readme.contains("# upgrade-current\n\n<!-- simit:badges:start -->"));
}

#[test]
fn upgrade_path_targets_explicit_workspace() {
    let project = non_tmp_project("upgrade-path");

    let output = simit()
        .args(["upgrade", "--path", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(read(&project.path().join("README.md")).contains("simit:badges:start"));
}

#[test]
fn dry_run_diff_reports_without_writing() {
    let project = non_tmp_project("upgrade-dry-run");

    let output = simit()
        .current_dir(project.path())
        .args(["upgrade", "--dry-run", "--diff"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("---"));
    assert!(stdout.contains("would upgrade"));
    assert!(!read(&project.path().join("README.md")).contains("simit:badges:start"));
}

#[test]
fn check_exits_nonzero_when_readme_needs_upgrade() {
    let project = non_tmp_project("upgrade-check");

    let output = simit()
        .current_dir(project.path())
        .args(["upgrade", "--check"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("need `simit upgrade`"));
}

#[test]
fn rejects_missing_readme() {
    let project = non_tmp_project("upgrade-missing-readme");
    fs::remove_file(project.path().join("README.md")).unwrap();

    let output = simit()
        .current_dir(project.path())
        .args(["upgrade"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("README.md is required"));
}

#[test]
fn all_upgrades_registered_managed_projects_and_skips_hand_rolled() {
    let data_home = TempDir::new().unwrap();
    let managed = non_tmp_project("upgrade-managed");
    let hand_rolled = non_tmp_project("upgrade-hand-rolled");
    let managed_path = fs::canonicalize(managed.path()).unwrap();
    let hand_rolled_path = fs::canonicalize(hand_rolled.path()).unwrap();
    write_registry(
        data_home.path(),
        &[
            (&managed_path, "upgrade-managed", &[("ci", "managed")]),
            (
                &hand_rolled_path,
                "upgrade-hand-rolled",
                &[("ci", "hand-rolled")],
            ),
        ],
    );

    let output = simit_with_data_home(data_home.path())
        .args(["upgrade", "--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("upgraded"));
    assert!(read(&managed.path().join("README.md")).contains("simit:badges:start"));
    assert!(!read(&hand_rolled.path().join("README.md")).contains("simit:badges:start"));
}

#[test]
fn all_reports_partial_failures_without_skipping_later_projects() {
    let data_home = TempDir::new().unwrap();
    let broken = non_tmp_project("upgrade-broken");
    let good = non_tmp_project("upgrade-good");
    fs::write(broken.path().join("README.md"), "no heading\n").unwrap();
    let broken_path = fs::canonicalize(broken.path()).unwrap();
    let good_path = fs::canonicalize(good.path()).unwrap();
    write_registry(
        data_home.path(),
        &[
            (&broken_path, "upgrade-broken", &[("ci", "managed")]),
            (&good_path, "upgrade-good", &[("ci", "managed")]),
        ],
    );

    let output = simit_with_data_home(data_home.path())
        .args(["upgrade", "--all"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("failed"));
    assert!(stdout.contains("upgraded"));
    assert!(read(&good.path().join("README.md")).contains("simit:badges:start"));
}

#[test]
fn upgrade_rewrites_codeberg_pages_workflow_to_managed_hook() {
    let project = init_pages_project("upgrade-pages");

    let output = simit()
        .current_dir(project.path())
        .args(["upgrade"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let workflow = read(&project.path().join(".forgejo/workflows/pages.yaml"));
    assert!(workflow.contains("# Generated by simit."));
    assert!(workflow.contains("group: ${{ codeberg.workflow }}-${{ codeberg.ref }}"));
    assert!(workflow.contains("CODEBERG_TOKEN: ${{ secrets.codeberg_token }}"));
    assert!(workflow.contains("nix build .#site --out-link result-pages-site"));
    assert!(workflow.contains("grep -qx upgrade-pages.tartanoglu.com result-pages-site/.domains"));
    assert!(workflow.contains(
        "git remote add pages-origin \"https://caniko:${CODEBERG_TOKEN}@codeberg.org/caniko/upgrade-pages.git\""
    ));
    assert!(workflow.contains("DEPLOY_REMOTE=pages-origin nix run .#deploy-pages"));
    assert!(!workflow.contains("        run: nix run .#deploy-pages\n"));
}

#[test]
fn upgrade_check_reports_stale_codeberg_pages_workflow() {
    let project = init_pages_project("upgrade-pages-check");

    let output = simit()
        .current_dir(project.path())
        .args(["upgrade", "--check"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("need `simit upgrade`"));
    let workflow = read(&project.path().join(".forgejo/workflows/pages.yaml"));
    assert!(!workflow.contains("CODEBERG_TOKEN"));
}

#[test]
fn pages_only_upgrade_skips_readme_badges() {
    let project = init_pages_project("upgrade-pages-only");

    let output = simit()
        .current_dir(project.path())
        .args(["upgrade", "--pages-only"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let readme = read(&project.path().join("README.md"));
    assert!(!readme.contains("simit:badges:start"));
    let workflow = read(&project.path().join(".forgejo/workflows/pages.yaml"));
    assert!(workflow.contains("CODEBERG_TOKEN: ${{ secrets.codeberg_token }}"));
}

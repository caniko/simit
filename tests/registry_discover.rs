use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use camino::Utf8PathBuf;
use simit::project::{self, Languages};
use simit::registry::{self, DiscoverOptions, FeatureStatus};
use simit::render::flake;
use tempfile::TempDir;

#[allow(dead_code)]
mod common;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
    data_home: TempDir,
    old_xdg_data_home: Option<OsString>,
    old_no_registry: Option<OsString>,
}

impl EnvGuard {
    fn new() -> Self {
        let guard = ENV_LOCK.lock().unwrap();
        let data_home = TempDir::new().unwrap();
        let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
        let old_no_registry = std::env::var_os("SIMIT_NO_REGISTRY");
        // SAFETY: tests in this file serialize environment mutation with
        // ENV_LOCK and restore the original values in Drop.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", data_home.path());
            std::env::remove_var("SIMIT_NO_REGISTRY");
        }
        Self {
            _guard: guard,
            data_home,
            old_xdg_data_home,
            old_no_registry,
        }
    }

    fn registry_file(&self) -> PathBuf {
        self.data_home.path().join("simit/projects.toml")
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        restore_env("XDG_DATA_HOME", self.old_xdg_data_home.as_ref());
        restore_env("SIMIT_NO_REGISTRY", self.old_no_registry.as_ref());
    }
}

fn restore_env(key: &str, value: Option<&OsString>) {
    // SAFETY: callers hold ENV_LOCK through EnvGuard while restoring process
    // environment variables for this test module.
    unsafe {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
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
"#
        ),
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
}

fn init_package_with_broken_registry_dependency(root: &Path, name: &str) {
    init_package(root, name);
    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"

[dependencies]
missing = {{ version = "1", registry = "sparse-crates-io" }}
"#
        ),
    )
    .unwrap();
}

fn init_workspace(root: &Path, member: &str) {
    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[workspace]
members = ["{member}"]
resolver = "3"
"#
        ),
    )
    .unwrap();
    init_package(&root.join(member), member);
}

fn init_managed_github_ci(root: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_simit"))
        .env("SIMIT_NO_REGISTRY", "1")
        .env("SIMIT_MAINTAINERS_GPG", common::maintainer_key_path())
        .current_dir(root)
        .args(["init", "ci", "--platform", "github"])
        .status()
        .unwrap();
    assert!(status.success());
}

fn add_managed_flake(root: &Path) {
    let files = flake::files(
        &Languages {
            rust: true,
            nix: true,
            toml: true,
            ..Languages::default()
        },
        "2024",
        Some("1.85"),
        None,
        flake::AuditTools {
            audit: true,
            deny: false,
        },
    );
    project::write_generated_files(root, &files).unwrap();
}

fn add_hooks_only_flake(root: &Path) {
    let files = flake::files(
        &Languages {
            rust: true,
            nix: true,
            toml: true,
            ..Languages::default()
        },
        "2024",
        Some("1.85"),
        None,
        flake::AuditTools {
            audit: true,
            deny: false,
        },
    );
    let hook_files = files
        .into_iter()
        .filter(|file| file.relative_path == Path::new("nix/pre-commit.nix"))
        .collect::<Vec<_>>();
    project::write_generated_files(root, &hook_files).unwrap();
}

fn canonical(path: &Path) -> Utf8PathBuf {
    Utf8PathBuf::from_path_buf(fs::canonicalize(path).unwrap()).unwrap()
}

fn discover(root: &Path, opts: DiscoverOptions) -> registry::DiscoverReport {
    registry::discover_under(root, &opts).unwrap()
}

#[test]
fn empty_temp_dir_returns_empty_report() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();

    let report = discover(root.path(), DiscoverOptions::default());

    assert!(report.registered.is_empty());
    assert!(report.refreshed.is_empty());
    assert!(report.skipped_empty.is_empty());
    assert!(report.errors.is_empty());
}

#[test]
fn simit_managed_crate_is_registered_with_managed_flake() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "managed");
    add_managed_flake(root.path());

    let report = discover(root.path(), DiscoverOptions::default());
    let key = canonical(root.path());
    let registry = registry::load().unwrap();

    assert_eq!(report.registered.as_slice(), std::slice::from_ref(&key));
    assert_eq!(
        registry.projects[&key].features["flake"],
        FeatureStatus::Managed
    );
}

#[test]
fn hooks_only_crate_is_registered_with_managed_flake() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "managed-hooks");
    fs::write(root.path().join("flake.nix"), "{ custom = \"owned\"; }\n").unwrap();
    add_hooks_only_flake(root.path());

    let report = discover(root.path(), DiscoverOptions::default());
    let key = canonical(root.path());
    let registry = registry::load().unwrap();

    assert_eq!(report.registered.as_slice(), std::slice::from_ref(&key));
    assert_eq!(
        registry.projects[&key].features["flake"],
        FeatureStatus::Managed
    );
}

#[test]
fn unsimit_crate_is_skipped_as_empty() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "plain");

    let report = discover(root.path(), DiscoverOptions::default());

    assert!(report.registered.is_empty());
    assert_eq!(report.skipped_empty, [canonical(root.path())]);
    assert!(registry::load().unwrap().projects.is_empty());
}

#[test]
fn package_name_does_not_require_cargo_metadata_for_skipped_crate() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package_with_broken_registry_dependency(root.path(), "plain");

    let report = discover(root.path(), DiscoverOptions::default());

    assert!(report.registered.is_empty());
    assert_eq!(report.skipped_empty, [canonical(root.path())]);
    assert!(report.errors.is_empty());
}

#[test]
fn package_name_does_not_require_cargo_metadata_for_include_empty_crate() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package_with_broken_registry_dependency(root.path(), "plain");

    let report = discover(
        root.path(),
        DiscoverOptions {
            include_empty: true,
            ..DiscoverOptions::default()
        },
    );
    let key = canonical(root.path());
    let registry = registry::load().unwrap();

    assert_eq!(report.registered.as_slice(), std::slice::from_ref(&key));
    assert!(report.errors.is_empty());
    assert_eq!(registry.projects[&key].name, "plain");
}

#[test]
fn generic_changelog_does_not_make_crate_simit_managed() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "plain");
    fs::write(root.path().join("CHANGELOG.md"), "# Changelog\n\n").unwrap();

    let report = discover(root.path(), DiscoverOptions::default());

    assert!(report.registered.is_empty());
    assert_eq!(report.skipped_empty, [canonical(root.path())]);
    assert!(registry::load().unwrap().projects.is_empty());
}

#[test]
fn generic_flake_does_not_make_crate_simit_managed() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "plain");
    fs::write(root.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();

    let report = discover(root.path(), DiscoverOptions::default());

    assert!(report.registered.is_empty());
    assert_eq!(report.skipped_empty, [canonical(root.path())]);
    assert!(registry::load().unwrap().projects.is_empty());
}

#[test]
fn generic_ci_registers_as_hand_rolled() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "plain");
    fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
    fs::write(
        root.path().join(".github/workflows/ci.yaml"),
        "name: CI\n\non: [push]\n",
    )
    .unwrap();

    let report = discover(root.path(), DiscoverOptions::default());

    assert_eq!(report.registered, [canonical(root.path())]);
    assert!(report.skipped_empty.is_empty());
    let registry = registry::load().unwrap();
    let entry = registry.projects.get(&canonical(root.path())).unwrap();
    assert_eq!(entry.features["ci"], FeatureStatus::HandRolled);
}

#[test]
fn marked_ci_with_unmarked_supplementary_workflow_registers_as_managed_extra() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "plain");
    init_managed_github_ci(root.path());
    fs::write(
        root.path().join(".github/workflows/pages.yaml"),
        "name: Pages\non: [push]\n",
    )
    .unwrap();

    let report = discover(root.path(), DiscoverOptions::default());

    assert_eq!(report.registered, [canonical(root.path())]);
    let registry = registry::load().unwrap();
    let entry = registry.projects.get(&canonical(root.path())).unwrap();
    assert_eq!(entry.features["ci"], FeatureStatus::ManagedExtra);
}

#[test]
fn edited_marked_ci_with_unmarked_supplementary_workflow_stays_drift() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "plain");
    init_managed_github_ci(root.path());
    let ci_path = root.path().join(".github/workflows/ci.yaml");
    let ci = fs::read_to_string(&ci_path).unwrap();
    fs::write(
        &ci_path,
        ci.replace("cargo test --all-features", "cargo test"),
    )
    .unwrap();
    fs::write(
        root.path().join(".github/workflows/pages.yaml"),
        "name: Pages\non: [push]\n",
    )
    .unwrap();

    let report = discover(root.path(), DiscoverOptions::default());

    assert_eq!(report.registered, [canonical(root.path())]);
    let registry = registry::load().unwrap();
    let entry = registry.projects.get(&canonical(root.path())).unwrap();
    assert_eq!(entry.features["ci"], FeatureStatus::Drift);
}

#[test]
fn hand_rolled_ci_registers_with_include_empty() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "plain");
    fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
    fs::write(
        root.path().join(".github/workflows/ci.yaml"),
        "name: CI\n\non: [push]\n",
    )
    .unwrap();

    let report = discover(
        root.path(),
        DiscoverOptions {
            include_empty: true,
            ..DiscoverOptions::default()
        },
    );

    assert_eq!(report.registered, [canonical(root.path())]);
    let registry = registry::load().unwrap();
    let entry = registry.projects.get(&canonical(root.path())).unwrap();
    assert_eq!(entry.features["ci"], FeatureStatus::HandRolled);
}

#[test]
fn include_empty_registers_unsimit_crate() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "plain");

    let report = discover(
        root.path(),
        DiscoverOptions {
            include_empty: true,
            ..DiscoverOptions::default()
        },
    );

    assert_eq!(report.registered, [canonical(root.path())]);
    assert!(report.skipped_empty.is_empty());
}

#[test]
fn nested_workspace_registers_parent_and_does_not_visit_members() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_workspace(root.path(), "member");

    let report = discover(
        root.path(),
        DiscoverOptions {
            include_empty: true,
            ..DiscoverOptions::default()
        },
    );

    assert_eq!(report.registered, [canonical(root.path())]);
    assert!(
        !report
            .registered
            .contains(&canonical(&root.path().join("member")))
    );
}

#[test]
fn sibling_crates_are_both_registered() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(&root.path().join("one"), "one");
    init_package(&root.path().join("two"), "two");

    let report = discover(
        root.path(),
        DiscoverOptions {
            include_empty: true,
            ..DiscoverOptions::default()
        },
    );

    assert_eq!(
        report.registered,
        [
            canonical(&root.path().join("one")),
            canonical(&root.path().join("two"))
        ]
    );
}

#[test]
fn max_depth_zero_only_checks_root() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(&root.path().join("child"), "child");

    let report = discover(
        root.path(),
        DiscoverOptions {
            max_depth: 0,
            include_empty: true,
            ..DiscoverOptions::default()
        },
    );

    assert!(report.registered.is_empty());
}

#[test]
fn deny_listed_target_dir_is_skipped() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(&root.path().join("target").join("fake"), "fake");

    let report = discover(
        root.path(),
        DiscoverOptions {
            include_empty: true,
            ..DiscoverOptions::default()
        },
    );

    assert!(report.registered.is_empty());
}

#[test]
fn hidden_dir_is_skipped() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(&root.path().join(".attic").join("fake"), "fake");

    let report = discover(
        root.path(),
        DiscoverOptions {
            include_empty: true,
            ..DiscoverOptions::default()
        },
    );

    assert!(report.registered.is_empty());
}

#[cfg(unix)]
#[test]
fn symlink_loop_with_follow_symlinks_terminates() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    let child = root.path().join("child");
    fs::create_dir_all(&child).unwrap();
    std::os::unix::fs::symlink(root.path(), child.join("loop")).unwrap();

    let report = discover(
        root.path(),
        DiscoverOptions {
            follow_symlinks: true,
            include_empty: true,
            ..DiscoverOptions::default()
        },
    );

    assert!(report.registered.is_empty());
    assert!(report.errors.is_empty());
}

#[test]
fn dry_run_does_not_write_registry_file() {
    let env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "plain");

    let report = registry::discover_under_dry_run(
        root.path(),
        &DiscoverOptions {
            include_empty: true,
            ..DiscoverOptions::default()
        },
    )
    .unwrap();

    assert_eq!(report.registered, [canonical(root.path())]);
    assert!(fs::metadata(env.registry_file()).is_err());
}

#[test]
fn rerun_preserves_first_seen_for_existing_entry() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    init_package(root.path(), "plain");
    let opts = DiscoverOptions {
        include_empty: true,
        ..DiscoverOptions::default()
    };

    let first_report = discover(root.path(), opts.clone());
    let key = first_report.registered[0].clone();
    let first_seen = registry::load().unwrap().projects[&key].first_seen;
    thread::sleep(Duration::from_millis(5));
    let second_report = discover(root.path(), opts);
    let second_seen = registry::load().unwrap().projects[&key].first_seen;

    assert_eq!(second_report.refreshed, [key]);
    assert_eq!(first_seen, second_seen);
}

#[test]
fn broken_manifest_is_reported_without_aborting_walk() {
    let _env = EnvGuard::new();
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("broken")).unwrap();
    fs::write(root.path().join("broken/Cargo.toml"), "[package\n").unwrap();
    init_package(&root.path().join("valid"), "valid");

    let report = discover(
        root.path(),
        DiscoverOptions {
            include_empty: true,
            ..DiscoverOptions::default()
        },
    );

    assert_eq!(report.registered, [canonical(&root.path().join("valid"))]);
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].1.contains("parsing"));
}

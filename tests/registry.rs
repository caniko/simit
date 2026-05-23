use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread;

use simit::registry::{self, FeatureStatus};
use tempfile::TempDir;

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

    fn path(&self) -> &Path {
        self.data_home.path()
    }

    fn disable_registry(&self) {
        // SAFETY: guarded by ENV_LOCK for the lifetime of EnvGuard.
        unsafe {
            std::env::set_var("SIMIT_NO_REGISTRY", "1");
        }
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
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
}

fn registry_file(data_home: &Path) -> PathBuf {
    data_home.join("simit/projects.toml")
}

#[test]
fn round_trips_load_save_with_fixture_project() {
    let env = EnvGuard::new();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "demo");

    registry::touch(
        project.path(),
        "demo",
        [
            ("flake", FeatureStatus::Managed),
            ("hooks", FeatureStatus::Installed),
        ],
    )
    .unwrap();

    let loaded = registry::load().unwrap();
    let key = fs::canonicalize(project.path()).unwrap();
    let key = camino::Utf8PathBuf::from_path_buf(key).unwrap();
    let entry = loaded.projects.get(&key).unwrap();
    assert_eq!(entry.name, "demo");
    assert_eq!(entry.features["flake"], FeatureStatus::Managed);
    assert_eq!(entry.features["hooks"], FeatureStatus::Installed);

    let text = fs::read_to_string(registry_file(env.path())).unwrap();
    assert!(text.contains("schema_version = 1"));
    assert!(text.contains("[[project]]"));
}

#[test]
fn atomic_write_leaves_no_tmp_on_success() {
    let env = EnvGuard::new();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "demo");

    registry::touch(project.path(), "demo", [("ci", FeatureStatus::Managed)]).unwrap();
    let loaded = registry::load().unwrap();
    registry::save(&loaded).unwrap();

    assert!(registry_file(env.path()).exists());
    assert!(!env.path().join("simit/projects.toml.tmp").exists());
}

#[test]
fn concurrent_touch_calls_preserve_both_updates() {
    let _env = EnvGuard::new();
    let one = TempDir::new().unwrap();
    let two = TempDir::new().unwrap();
    init_package(one.path(), "one");
    init_package(two.path(), "two");

    let one_path = one.path().to_path_buf();
    let two_path = two.path().to_path_buf();
    let left = thread::spawn(move || {
        registry::touch(&one_path, "one", [("flake", FeatureStatus::Managed)]).unwrap();
    });
    let right = thread::spawn(move || {
        registry::touch(&two_path, "two", [("ci", FeatureStatus::Managed)]).unwrap();
    });
    left.join().unwrap();
    right.join().unwrap();

    let loaded = registry::load().unwrap();
    assert_eq!(loaded.projects.len(), 2);
    assert!(loaded.projects.values().any(|entry| entry.name == "one"));
    assert!(loaded.projects.values().any(|entry| entry.name == "two"));
}

#[test]
fn schema_version_mismatch_errors_with_required_version() {
    let env = EnvGuard::new();
    let path = registry_file(env.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "schema_version = 999\n").unwrap();

    let err = registry::load().unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("schema_version 999"));
    assert!(message.contains("required version 1"));
}

#[test]
fn no_registry_env_suppresses_touch_io() {
    let env = EnvGuard::new();
    env.disable_registry();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "demo");

    registry::touch(project.path(), "demo", [("flake", FeatureStatus::Managed)]).unwrap();

    assert!(!registry_file(env.path()).exists());
}

#[test]
fn mutating_command_writes_registry_under_xdg_data_home() {
    let env = EnvGuard::new();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "demo");

    let status = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(project.path())
        .env("XDG_DATA_HOME", env.path())
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(status.success());

    let text = fs::read_to_string(registry_file(env.path())).unwrap();
    assert!(text.contains("name = \"demo\""));
    assert!(text.contains("flake = \"managed\""));
    assert!(text.contains("hooks = \"installed\""));
}

#[test]
fn no_registry_env_suppresses_command_io() {
    let env = EnvGuard::new();
    let project = TempDir::new().unwrap();
    init_package(project.path(), "demo");

    let status = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(project.path())
        .env("XDG_DATA_HOME", env.path())
        .env("SIMIT_NO_REGISTRY", "1")
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(status.success());

    assert!(!registry_file(env.path()).exists());
}

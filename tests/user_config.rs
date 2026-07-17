use std::env;
use std::fs;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use simit::cli::{Platform, Runtime};
use simit::user_config::UserConfig;
use tempfile::TempDir;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn valid_config() -> &'static str {
    r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true

[ci.runners.windows_atlas]
platform = "forgejo"
labels = ["windows-atlas"]
os = "windows"
arch = "x86_64"
runtimes = ["cargo"]

[ci.defaults.forgejo]
cargo = "atlas"
nix = "atlas"
release = "atlas"
windows = "windows_atlas"
"#
}

fn write_config(root: &TempDir, text: &str) {
    let dir = root.path().join("simit");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), text).unwrap();
}

fn simit_with_xdg(root: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_simit"));
    command
        .env("XDG_CONFIG_HOME", root.path())
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("HOME", root.path().join("home"));
    command
}

#[test]
fn user_config_path_uses_xdg_config_home() {
    let _guard = env_lock().lock().unwrap();
    let temp = TempDir::new().unwrap();
    let old_xdg = env::var_os("XDG_CONFIG_HOME");
    let old_home = env::var_os("HOME");
    unsafe {
        env::set_var("XDG_CONFIG_HOME", temp.path());
        env::set_var("HOME", temp.path().join("home"));
    }

    assert_eq!(UserConfig::path(), temp.path().join("simit/config.toml"));

    unsafe {
        match old_xdg {
            Some(value) => env::set_var("XDG_CONFIG_HOME", value),
            None => env::remove_var("XDG_CONFIG_HOME"),
        }
        match old_home {
            Some(value) => env::set_var("HOME", value),
            None => env::remove_var("HOME"),
        }
    }
}

#[test]
fn user_config_path_falls_back_to_home_config() {
    let _guard = env_lock().lock().unwrap();
    let temp = TempDir::new().unwrap();
    let old_xdg = env::var_os("XDG_CONFIG_HOME");
    let old_home = env::var_os("HOME");
    unsafe {
        env::remove_var("XDG_CONFIG_HOME");
        env::set_var("HOME", temp.path());
    }

    assert_eq!(
        UserConfig::path(),
        temp.path().join(".config/simit/config.toml")
    );

    unsafe {
        match old_xdg {
            Some(value) => env::set_var("XDG_CONFIG_HOME", value),
            None => env::remove_var("XDG_CONFIG_HOME"),
        }
        match old_home {
            Some(value) => env::set_var("HOME", value),
            None => env::remove_var("HOME"),
        }
    }
}

#[test]
fn validates_and_resolves_multi_runtime_runner() {
    let config: UserConfig = toml_edit::de::from_str(valid_config()).unwrap();
    config.validate().unwrap();

    let runners = config
        .resolve_ci_runners(Platform::Forgejo, Runtime::Nix, None, None, true)
        .unwrap();

    assert_eq!(runners.ci.labels, vec!["atlas"]);
    assert_eq!(runners.release.labels, vec!["atlas"]);
    assert_eq!(
        runners.windows.as_ref().unwrap().labels,
        vec!["windows-atlas"]
    );
}

#[test]
fn rejects_nix_default_that_is_not_trusted() {
    // An untrusted runner is fine for cargo CI, but the Nix runner pushes
    // builds to the Attic cache and must be trusted.
    let config = valid_config().replace(
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true"#,
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = false"#,
    );
    let config: UserConfig = toml_edit::de::from_str(&config).unwrap();
    config.validate().unwrap();

    let err = config
        .resolve_ci_runners(Platform::Forgejo, Runtime::Nix, None, None, false)
        .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("must be trusted"), "got: {message}");
    assert!(message.contains("Attic cache"), "got: {message}");
}

#[test]
fn untrusted_cargo_ci_runner_resolves_when_release_runner_is_trusted() {
    // The plain cargo CI runner does not push to Attic, so it may stay
    // untrusted as long as the release runner (which does push) is trusted.
    let config = valid_config().replace(
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true"#,
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo"]
trusted = false

[ci.runners.atlas_release]
platform = "forgejo"
labels = ["atlas-nix-trusted"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true"#,
    );
    let config = config
        .replace("nix = \"atlas\"", "nix = \"atlas_release\"")
        .replace("release = \"atlas\"", "release = \"atlas_release\"");
    let config: UserConfig = toml_edit::de::from_str(&config).unwrap();
    config.validate().unwrap();

    let runners = config
        .resolve_ci_runners(Platform::Forgejo, Runtime::Cargo, None, None, false)
        .unwrap();
    // CI (cargo) runner stays untrusted; release runner is the trusted one.
    assert_eq!(runners.ci.labels, vec!["atlas"]);
    assert_eq!(runners.release.labels, vec!["atlas-nix-trusted"]);
}

#[test]
fn detects_explicit_trusted_forgejo_nix_runner_label() {
    let config = valid_config().replace(
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true"#,
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo"]
trusted = false

[ci.runners.atlas_release]
platform = "forgejo"
labels = ["atlas-nix-trusted"]
os = "linux"
arch = "x86_64"
runtimes = ["nix"]
trusted = true"#,
    );
    let config = config.replace("nix = \"atlas\"", "nix = \"atlas_release\"");
    let config: UserConfig = toml_edit::de::from_str(&config).unwrap();
    config.validate().unwrap();

    assert!(
        config
            .explicit_label_is_trusted_forgejo_nix_runner("atlas-nix-trusted")
            .unwrap()
    );
    assert!(
        !config
            .explicit_label_is_trusted_forgejo_nix_runner("atlas")
            .unwrap()
    );
}

#[test]
fn rejects_default_that_points_to_wrong_runtime() {
    let config =
        valid_config().replace("runtimes = [\"cargo\", \"nix\"]", "runtimes = [\"cargo\"]");
    let config: UserConfig = toml_edit::de::from_str(&config).unwrap();

    let err = config.validate().unwrap_err();

    assert!(format!("{err:#}").contains("does not support runtime `nix`"));
}

#[test]
fn rejects_empty_labels() {
    let config = valid_config().replace("labels = [\"atlas\"]", "labels = []");
    let config: UserConfig = toml_edit::de::from_str(&config).unwrap();

    let err = config.validate().unwrap_err();

    assert!(format!("{err:#}").contains("must define at least one label"));
}

#[test]
fn config_cli_path_show_check_and_init_work() {
    let temp = TempDir::new().unwrap();

    let path_output = simit_with_xdg(&temp)
        .args(["config", "path"])
        .output()
        .unwrap();
    assert!(path_output.status.success());
    assert_eq!(
        String::from_utf8(path_output.stdout).unwrap().trim(),
        temp.path().join("simit/config.toml").display().to_string()
    );

    let init_output = simit_with_xdg(&temp)
        .args(["config", "init"])
        .output()
        .unwrap();
    assert!(init_output.status.success());
    assert!(temp.path().join("simit/config.toml").exists());

    let check_output = simit_with_xdg(&temp)
        .args(["config", "check"])
        .output()
        .unwrap();
    assert!(check_output.status.success());
    assert!(
        String::from_utf8(check_output.stdout)
            .unwrap()
            .contains("is valid")
    );

    let show_output = simit_with_xdg(&temp)
        .args(["config", "show"])
        .output()
        .unwrap();
    assert!(show_output.status.success());
    let show = String::from_utf8(show_output.stdout).unwrap();
    assert!(show.contains("[ci.runners.forgejo_sccache_trusted]"));

    let second_init = simit_with_xdg(&temp)
        .args(["config", "init"])
        .output()
        .unwrap();
    assert!(!second_init.status.success());
    assert!(
        String::from_utf8(second_init.stderr)
            .unwrap()
            .contains("already exists")
    );
}

#[test]
fn config_cli_check_rejects_invalid_config() {
    let temp = TempDir::new().unwrap();
    write_config(
        &temp,
        r#"[ci.runners.bad]
platform = "forgejo"
labels = []
os = "linux"
arch = "x86_64"
runtimes = ["cargo"]
"#,
    );

    let output = simit_with_xdg(&temp)
        .args(["config", "check"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("must define at least one label")
    );
}

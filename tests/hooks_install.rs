use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn isolated_git_env() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join("xdg")).unwrap();
    temp
}

fn with_isolated_git_env(command: &mut Command, env: &TempDir) {
    command
        .env("GIT_CONFIG_GLOBAL", env.path().join("global.gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", env.path())
        .env("XDG_CONFIG_HOME", env.path().join("xdg"));
}

fn run(dir: &Path, program: &str, args: &[&str]) {
    let status = Command::new(program)
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap_or_else(|err| panic!("running {program}: {err}"));
    assert!(status.success(), "{program} {args:?} failed");
}

fn init_package() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    run(root, "git", &["init", "-q"]);
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
    fs::create_dir(root.join("nix")).unwrap();
    fs::write(
        root.join("nix/pre-commit.nix"),
        "{ pkgs, treefmtWrapper }: {}\n",
    )
    .unwrap();
    temp
}

fn git_config_snapshot_with_env(root: &Path, env: &TempDir) -> String {
    let mut command = Command::new("git");
    with_isolated_git_env(&mut command, env);
    let output = command
        .current_dir(root)
        .args(["config", "--list", "--show-scope"])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

fn git_config_with_env(root: &Path, env: &TempDir, args: &[&str]) -> std::process::Output {
    let mut command = Command::new("git");
    with_isolated_git_env(&mut command, env);
    command.current_dir(root).args(args).output().unwrap()
}

fn write_canix_dispatcher(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::write(path.join("dispatched-by-canix"), "").unwrap();
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path).unwrap().permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[test]
fn hooks_install_rejects_custom_hooks_path_without_dispatcher() {
    let temp = init_package();
    let root = temp.path();
    let git_env = isolated_git_env();
    let foreign_hooks = root.join("foreign-hooks");
    fs::create_dir(&foreign_hooks).unwrap();
    let status = {
        let mut command = Command::new("git");
        with_isolated_git_env(&mut command, &git_env);
        command
            .current_dir(root)
            .args(["config", "--local", "core.hooksPath"])
            .arg(&foreign_hooks)
            .status()
            .unwrap()
    };
    assert!(status.success());
    let before = git_config_snapshot_with_env(root, &git_env);

    let mut command = simit();
    with_isolated_git_env(&mut command, &git_env);
    let install_output = command
        .current_dir(root)
        .args(["hooks", "install"])
        .output()
        .unwrap();
    assert!(!install_output.status.success());
    let stderr = String::from_utf8_lossy(&install_output.stderr);
    assert!(stderr.contains("core.hooksPath"));
    assert!(stderr.contains("is not a canix dispatcher"));

    let pre_commit = root.join(".git/hooks/pre-commit");
    assert!(pre_commit.exists());
    assert!(is_executable(&pre_commit));
    assert!(root.join(".git/hooks/pre-push").exists());
    assert!(root.join(".git/hooks/commit-msg").exists());
    assert_eq!(
        String::from_utf8(
            git_config_with_env(root, &git_env, &["config", "--get", "core.hooksPath"]).stdout
        )
        .unwrap()
        .trim(),
        foreign_hooks.to_str().unwrap()
    );
    assert_eq!(git_config_snapshot_with_env(root, &git_env), before);
}

#[test]
fn hooks_install_repairs_local_override_when_global_canix_dispatcher_exists() {
    let temp = init_package();
    let root = temp.path();
    let git_env = isolated_git_env();
    let dispatcher = root.join("global-hooks");
    write_canix_dispatcher(&dispatcher);
    let status = {
        let mut command = Command::new("git");
        with_isolated_git_env(&mut command, &git_env);
        command
            .current_dir(root)
            .args(["config", "--global", "core.hooksPath"])
            .arg(&dispatcher)
            .status()
            .unwrap()
    };
    assert!(status.success());
    let status = {
        let mut command = Command::new("git");
        with_isolated_git_env(&mut command, &git_env);
        command
            .current_dir(root)
            .args(["config", "--local", "core.hooksPath", ".git/hooks"])
            .status()
            .unwrap()
    };
    assert!(status.success());

    let mut command = simit();
    with_isolated_git_env(&mut command, &git_env);
    let install_output = command
        .current_dir(root)
        .args(["hooks", "install"])
        .output()
        .unwrap();
    assert!(
        install_output.status.success(),
        "{}",
        String::from_utf8_lossy(&install_output.stderr)
    );
    let stderr = String::from_utf8_lossy(&install_output.stderr);
    assert!(stderr.contains("repaired local core.hooksPath override"));

    let local = git_config_with_env(
        root,
        &git_env,
        &["config", "--local", "--get", "core.hooksPath"],
    );
    assert!(!local.status.success());
    assert!(git_config_snapshot_with_env(root, &git_env).contains(dispatcher.to_str().unwrap()));
    assert!(is_executable(&root.join(".git/hooks/pre-commit")));
    assert!(is_executable(&root.join(".git/hooks/pre-push")));
    assert!(is_executable(&root.join(".git/hooks/commit-msg")));
}

#[test]
fn hooks_install_check_and_diff_report_drift() {
    let temp = init_package();
    let root = temp.path();
    let git_env = isolated_git_env();
    let mut install = simit();
    with_isolated_git_env(&mut install, &git_env);
    let install = install
        .current_dir(root)
        .args(["hooks", "install"])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );

    let mut clean_check = simit();
    with_isolated_git_env(&mut clean_check, &git_env);
    let clean_check = clean_check
        .current_dir(root)
        .args(["hooks", "install", "--check"])
        .output()
        .unwrap();
    assert!(
        clean_check.status.success(),
        "{}",
        String::from_utf8_lossy(&clean_check.stderr)
    );

    let mut clean_diff = simit();
    with_isolated_git_env(&mut clean_diff, &git_env);
    let clean_diff = clean_diff
        .current_dir(root)
        .args(["hooks", "install", "--diff"])
        .output()
        .unwrap();
    assert!(
        clean_diff.status.success(),
        "{}",
        String::from_utf8_lossy(&clean_diff.stderr)
    );
    assert!(clean_diff.stdout.is_empty());

    fs::remove_file(root.join(".git/hooks/pre-commit")).unwrap();
    let mut drift_check = simit();
    with_isolated_git_env(&mut drift_check, &git_env);
    let drift_check = drift_check
        .current_dir(root)
        .args(["hooks", "install", "--check"])
        .output()
        .unwrap();
    assert!(!drift_check.status.success());
    assert!(String::from_utf8_lossy(&drift_check.stderr).contains("pre-commit"));

    let mut drift_diff = simit();
    with_isolated_git_env(&mut drift_diff, &git_env);
    let drift_diff = drift_diff
        .current_dir(root)
        .args(["hooks", "install", "--diff"])
        .output()
        .unwrap();
    assert!(!drift_diff.status.success());
    let stderr = String::from_utf8_lossy(&drift_diff.stderr);
    assert!(stderr.contains("--- "));
    assert!(stderr.contains("+++ "));
    assert!(
        stderr.contains("ARGS=(hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit")
    );
}

#[test]
fn hooks_install_check_fails_when_local_override_bypasses_dispatcher() {
    let temp = init_package();
    let root = temp.path();
    let git_env = isolated_git_env();
    let dispatcher = root.join("global-hooks");
    write_canix_dispatcher(&dispatcher);
    let status = {
        let mut command = Command::new("git");
        with_isolated_git_env(&mut command, &git_env);
        command
            .current_dir(root)
            .args(["config", "--global", "core.hooksPath"])
            .arg(&dispatcher)
            .status()
            .unwrap()
    };
    assert!(status.success());

    let mut install = simit();
    with_isolated_git_env(&mut install, &git_env);
    let install = install
        .current_dir(root)
        .args(["hooks", "install"])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );

    let status = {
        let mut command = Command::new("git");
        with_isolated_git_env(&mut command, &git_env);
        command
            .current_dir(root)
            .args(["config", "--local", "core.hooksPath", ".git/hooks"])
            .status()
            .unwrap()
    };
    assert!(status.success());

    let mut check = simit();
    with_isolated_git_env(&mut check, &git_env);
    let check = check
        .current_dir(root)
        .args(["hooks", "install", "--check"])
        .output()
        .unwrap();
    assert!(!check.status.success());
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(stderr.contains("bypasses canix dispatcher"));
}

#[test]
fn hooks_install_help_lists_install_subcommand() {
    let output = simit()
        .args(["hooks", "install", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage: simit hooks install"));

    let output = simit().args(["hooks", "--help"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage: simit hooks <COMMAND>"));
    assert!(stdout.contains("install"));
}

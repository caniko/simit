#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn runner(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_simit"));
    command.current_dir(root).env("TMPDIR", root);
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT");
    command.env("GIT_CONFIG_GLOBAL", root.join("global"));
    command
}

#[test]
fn isolates_fixture_commits_but_preserves_checkout_policy_and_cleans_up() {
    let root = tempfile::Builder::new()
        .prefix("simit-test-[scope] ")
        .tempdir()
        .unwrap();
    let root = root.path();
    fs::create_dir(root.join("hooks")).unwrap();
    fs::write(
        root.join("hooks/pre-commit"),
        "#!/bin/sh\nprintf blocked > hook-ran\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(
        root.join("hooks/pre-commit"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let global = format!(
        "[include]\npath = identity\n[core]\nhooksPath = \"{}\"\n[commit]\ngpgSign = true\n[tag]\ngpgSign = true\n",
        root.join("hooks").display()
    );
    fs::write(root.join("global"), &global).unwrap();
    fs::write(
        root.join("identity"),
        "[user]\nname = Fixture\nemail = fixture@example.invalid\n",
    )
    .unwrap();
    let script = r#"
git init -q checkout
before=$(git -C checkout config --get core.hooksPath)
test "$(git -C checkout config --get commit.gpgSign)" = true
if git -C checkout commit --allow-empty -m blocked; then exit 31; fi
test -f checkout/hook-ran
git init -q "$TMPDIR/fixture"
git -C "$TMPDIR/fixture" commit --allow-empty -qm fixture
git -C "$TMPDIR/fixture" tag -am fixture fixture
test "$(git -C checkout config --get core.hooksPath)" = "$before"
test "$(git -C checkout config --get tag.gpgSign)" = true
git init -q --separate-git-dir "$PWD/outside.git" "$TMPDIR/linked"
test "$(git -C "$TMPDIR/linked" config --get core.hooksPath)" = "$before"
test "$(git -C "$TMPDIR/linked" config --get commit.gpgSign)" = true
test "$TMPDIR" = "$TMP"
test "$TMPDIR" = "$TEMP"
printf '%s' "$TMPDIR" > fixture-root
"#;
    let output = runner(root)
        .args(["test", "--git-fixtures", "--", "sh", "-eu", "-c", script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fixture_root = fs::read_to_string(root.join("fixture-root")).unwrap();
    assert!(!Path::new(&fixture_root).exists());
    assert_eq!(fs::read_to_string(root.join("global")).unwrap(), global);
    let checkout_config = fs::read_to_string(root.join("checkout/.git/config")).unwrap();
    assert!(!checkout_config.contains("hooksPath"));
    assert!(!checkout_config.contains("gpgSign"));
}

#[test]
fn forwards_arguments_and_exit_status_and_cleans_up_after_failure() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("global"), "").unwrap();
    let output = runner(root.path()).args([
        "test", "--git-fixtures", "--", "sh", "-c",
        "printf '%s' \"$TMPDIR\" > fixture-root\ntest \"$1\" = 'literal $(false)' || exit 99\nexit 37",
        "test", "literal $(false)",
    ]).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(37),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!Path::new(&fs::read_to_string(root.path().join("fixture-root")).unwrap()).exists());
}

#[test]
fn leaves_environment_unchanged_without_fixture_opt_in() {
    let root = tempfile::tempdir().unwrap();
    let output = runner(root.path())
        .args([
            "test",
            "--",
            "sh",
            "-eu",
            "-c",
            "test \"$TMPDIR\" = \"$PWD\"\ntest \"$GIT_CONFIG_GLOBAL\" = \"$PWD/global\"",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_missing_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["test", "--git-fixtures"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn preserves_explicit_local_and_command_scope_git_policy() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("global"), "").unwrap();
    let output = runner(root.path())
        .args([
            "test",
            "--git-fixtures",
            "--",
            "sh",
            "-eu",
            "-c",
            r#"
git init -q "$TMPDIR/fixture"
git -C "$TMPDIR/fixture" config core.hooksPath /fixture/local-policy
test "$(git -C "$TMPDIR/fixture" config core.hooksPath)" = /fixture/local-policy
test "$(git -C "$TMPDIR/fixture" config commit.gpgSign)" = true
"#,
        ])
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgSign")
        .env("GIT_CONFIG_VALUE_0", "true")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cleans_up_when_the_executable_is_missing() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("global"), "").unwrap();
    let output = runner(root.path())
        .args([
            "test",
            "--git-fixtures",
            "--",
            "/nonexistent/simit-test-command",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

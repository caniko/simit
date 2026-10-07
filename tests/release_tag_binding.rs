#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

mod common;

#[test]
fn publish_uses_default_branch_keys_and_rejects_a_different_checkout() {
    for member_scoped in [false, true] {
        verify_publish_binding(member_scoped);
    }
}

fn verify_publish_binding(member_scoped: bool) {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT\"\n\n[workspace]\nmembers = []\n",
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    let mut command = common::simit();
    command.current_dir(temp.path()).args([
        "init",
        "ci",
        "--platform",
        "github",
        "--runtime",
        "cargo",
        "--publish-crates",
    ]);
    if member_scoped {
        command.arg("--workspace");
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let name = if member_scoped {
        "publish-crate-demo.yaml"
    } else {
        "publish-crate.yaml"
    };
    let workflow: serde_yaml::Value = serde_yaml::from_str(
        &fs::read_to_string(temp.path().join(".github/workflows").join(name)).unwrap(),
    )
    .unwrap();
    let script = workflow["jobs"]["publish"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"].as_str() == Some("Validate signed release tag"))
        .unwrap()["run"]
        .as_str()
        .unwrap();

    // The release tree supplies an unrelated key. The fake verifier only
    // accepts the independently fetched default-branch trust root.
    fs::create_dir_all(temp.path().join("keys")).unwrap();
    fs::write(temp.path().join("keys/maintainers.gpg"), "release-tree-key").unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    for (name, body) in [
        (
            "git",
            r#"case "$1" in
fetch) exit 0;;
show) [ "$2" = 'FETCH_HEAD:keys/maintainers.gpg' ] || exit 90; [ "$TEST_KEY_AVAILABLE" = 1 ] || exit 91; printf default-branch-key;;
verify-tag) exit 0;;
rev-parse) case "$3" in HEAD) printf '%s\n' "$TEST_CHECKOUT_SHA";; refs/tags/*) printf '%s\n' "$TEST_TAG_SHA";; *) exit 92;; esac;;
*) exit 99;;
esac"#,
        ),
        (
            "gpg",
            r#"[ "$1" = --batch ] && [ "$2" = --import ] && [ "$(cat "$3")" = default-branch-key ]"#,
        ),
        ("cargo", "touch version-checked; printf 'demo@0.1.0\\n'"),
    ] {
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let checkout = "1111111111111111111111111111111111111111";
    for (tag, key_available, accepted) in [
        ("2222222222222222222222222222222222222222", "1", false),
        (checkout, "0", false),
        (checkout, "1", true),
    ] {
        let output = Command::new("bash")
            .current_dir(temp.path())
            .args(["-c", script])
            .env("PATH", &path)
            .env("TMPDIR", temp.path())
            .env("GITHUB_REF_NAME", "0.1.0")
            .env("TEST_CHECKOUT_SHA", checkout)
            .env("TEST_TAG_SHA", tag)
            .env("TEST_KEY_AVAILABLE", key_available)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), accepted, "{output:?}");
        assert_eq!(temp.path().join("version-checked").exists(), accepted);
    }
}

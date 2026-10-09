#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

mod common;

#[test]
fn source_manifests_hash_dash_and_option_like_names_as_files() {
    use sha2::Digest;

    let temp = tempfile::tempdir().unwrap();
    for name in ["-", "--help", "source.rs"] {
        fs::write(temp.path().join(name), format!("fixture {name}\n")).unwrap();
    }
    for args in [vec!["init", "--quiet"], vec!["add", "--", "."]] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(temp.path())
                .status()
                .unwrap()
                .success()
        );
    }
    for path in [
        ".github/workflows/qualify-release-generator.yaml",
        ".github/workflows/review-compatibility.yml",
    ] {
        let workflow =
            fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
                .unwrap();
        let command = workflow
            .lines()
            .find(|line| line.contains("git ls-files -z"))
            .unwrap()
            .split(" > ")
            .next()
            .unwrap()
            .trim();
        let output = Command::new("sh")
            .args(["-c", command])
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{path}: {output:?}");
        let manifest = String::from_utf8(output.stdout).unwrap();
        assert_eq!(manifest.lines().count(), 3, "{path}: {manifest}");
        for name in ["-", "--help", "source.rs"] {
            let expected = format!(
                "{}  ./{name}",
                hex::encode(sha2::Sha256::digest(format!("fixture {name}\n")))
            );
            assert!(
                manifest.lines().any(|line| line == expected),
                "{path}: {manifest}"
            );
        }
    }
}

#[test]
fn publishers_validate_before_project_setup_without_persisting_credentials() {
    for platform in ["github", "forgejo"] {
        for runtime in ["cargo", "nix"] {
            let temp = tempfile::tempdir().unwrap();
            fs::write(
                temp.path().join("Cargo.toml"),
                "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\nlicense='MIT'\n",
            )
            .unwrap();
            fs::create_dir(temp.path().join("src")).unwrap();
            fs::write(temp.path().join("src/lib.rs"), "").unwrap();
            fs::write(temp.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();
            fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform='{platform}'\nruntime='{runtime}'\nrunner='fixture-linux'\npublish_crates=true\nextra_setup=['echo checkout-controlled-setup']\n")).unwrap();
            let output = common::simit()
                .current_dir(temp.path())
                .args(["init", "ci"])
                .output()
                .unwrap();
            assert!(output.status.success(), "{platform}/{runtime}: {output:?}");
            let workflow = fs::read_to_string(
                temp.path()
                    .join(format!(".{platform}/workflows/publish-crate.yaml")),
            )
            .unwrap();
            let parsed: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
            let steps = parsed["jobs"]["publish"]["steps"].as_sequence().unwrap();
            let validation = steps
                .iter()
                .position(|step| step["name"] == "Validate signed release tag")
                .unwrap();
            let setup = steps
                .iter()
                .position(|step| step["run"] == "echo checkout-controlled-setup")
                .unwrap();
            assert!(validation < setup, "{platform}/{runtime}: {workflow}");
            let checkout = steps
                .iter()
                .find(|step| step["name"] == "Checkout")
                .unwrap();
            assert_eq!(checkout["with"]["persist-credentials"], false);
            assert_eq!(checkout["with"]["ref"], "${{ github.sha }}");
            let run = steps[validation]["run"].as_str().unwrap();
            let binding = run
                .find("if [ \"$validated_sha\" != \"$checkout_sha\" ]")
                .unwrap();
            assert!(binding < run.find("cargo pkgid").unwrap());
        }
    }
}

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
    if member_scoped {
        // Package-scoped workflows require more than one real workspace member.
        let helper = temp.path().join("helper");
        fs::create_dir_all(helper.join("src")).unwrap();
        fs::write(
            helper.join("Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n",
        )
        .unwrap();
        fs::write(helper.join("src/lib.rs"), "").unwrap();
        let manifest = fs::read_to_string(temp.path().join("Cargo.toml")).unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            manifest.replace("members = []", "members = [\"helper\"]"),
        )
        .unwrap();
    }
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
            r#"authenticated=0
if [ "$1" = -c ]; then
  [ "$2" = 'http.extraHeader=AUTHORIZATION: basic eC1hY2Nlc3MtdG9rZW46Zml4dHVyZS1yZWFkLXRva2Vu' ] || exit 94
  authenticated=1; shift 2
fi
case "$1" in
fetch) [ "$authenticated" = 1 ] || exit 95;;
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
            .env("GITHUB_TOKEN", "fixture-read-token")
            .env("TEST_CHECKOUT_SHA", checkout)
            .env("TEST_TAG_SHA", tag)
            .env("TEST_KEY_AVAILABLE", key_available)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), accepted, "{output:?}");
        assert_eq!(temp.path().join("version-checked").exists(), accepted);
    }
}

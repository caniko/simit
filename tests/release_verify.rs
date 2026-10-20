use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn init_fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "release-verify-demo"
version = "0.1.0"
edition = "2024"
publish = false
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## [0.1.0] - 2026-05-25\n\n- Initial release.\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("keys")).unwrap();
    fs::copy(
        common::maintainer_key_path(),
        root.join("keys/maintainers.gpg"),
    )
    .unwrap();

    git(root, &["init"]);
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ],
    );
    git(root, &["-c", "tag.gpgSign=false", "tag", "0.1.0"]);

    temp
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn release_verify_help_lists_verify_flags() {
    let output = simit()
        .args(["release", "verify", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--json"));
    assert!(stdout.contains("--version <VERSION>"));
    assert!(stdout.contains("--push-target <REMOTE>"));
}

#[test]
fn release_verify_json_reports_all_checks_and_exit_code() {
    let temp = init_fixture();
    fs::write(temp.path().join("dirty.txt"), "dirty\n").unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["release", "verify", "--json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["command"], "simit release verify");
    assert_eq!(value["summary"]["exit_code"], 1);

    let results = value["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .any(|item| item["check"] == "worktree clean" && item["status"] == "fail")
    );
    assert!(!results.iter().any(|item| item["check"] == "remote secrets"));
}

#[test]
fn release_verify_text_reports_missing_changelog_entry() {
    let temp = init_fixture();

    let output = simit()
        .current_dir(temp.path())
        .args(["release", "verify", "--version", "0.2.0"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[fail"));
    assert!(stdout.contains("CHANGELOG entry exists: no released entry for 0.2.0"));
    assert!(stdout.contains("summary:"));
}

#[cfg(unix)]
#[test]
fn release_plz_verification_uses_the_configured_credential_contract_once() {
    use std::os::unix::fs::PermissionsExt;

    let tools = tempfile::tempdir().unwrap();
    let curl = tools.path().join("curl");
    fs::write(
        &curl,
        "#!/bin/sh\nprintf '%s\\n%s' '{\"versions\":[{\"num\":\"0.1.0\"}]}' '200'\n",
    )
    .unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        tools.path().display(),
        std::env::var("PATH").unwrap()
    );
    for hosted in [false, true] {
        for authentication in [
            "token_secret = 'CUSTOM_PAT'",
            "app_id_variable = 'CUSTOM_APP_ID'\napp_private_key_secret = 'CUSTOM_APP_KEY'",
        ] {
            let temp = init_fixture();
            let manifest = temp.path().join("Cargo.toml");
            fs::write(
                &manifest,
                fs::read_to_string(&manifest)
                    .unwrap()
                    .replace("publish = false\n", ""),
            )
            .unwrap();
            let target = if hosted {
                "[release.github]\nrepo = 'example/demo'\n"
            } else {
                ""
            };
            fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = 'github'\npublish_crates = true\npublish_strategy = 'release-plz'\n[ci.release_plz]\nregistry_token_secret = 'CUSTOM_REGISTRY_TOKEN'\n{authentication}\n{target}")).unwrap();
            let contract = simit()
                .current_dir(temp.path())
                .args(["release", "secrets", "contract", "--json"])
                .output()
                .unwrap();
            assert!(contract.status.success(), "{contract:?}");
            let contract: Value = serde_json::from_slice(&contract.stdout).unwrap();
            let output = simit()
                .current_dir(temp.path())
                .env("PATH", &path)
                .args(["release", "verify", "--json"])
                .output()
                .unwrap();
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            let remote: Vec<_> = report["results"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["check"] == "remote secrets")
                .collect();
            assert_eq!(remote.len(), 1, "{hosted}: {report}");
            let message = remote[0]["message"].as_str().unwrap();
            for entry in contract.as_array().unwrap().iter().filter(|entry| {
                entry["required"] == true && (hosted || entry["channel"] == "release-plz")
            }) {
                assert!(
                    message.contains(entry["name"].as_str().unwrap()),
                    "{report}"
                );
            }
            for unused in [
                "CRATES_IO_API_TOKEN",
                "RELEASE_PLZ_TOKEN",
                "RELEASE_PLZ_GPG_PASSPHRASE",
            ] {
                assert!(!message.contains(unused), "{report}");
            }
            if !hosted {
                assert!(!message.contains("MINISIGN"), "{report}");
            }
            assert!(
                report["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["check"] == "crates.io release-verify-demo"
                        && item["status"] == "pass"),
                "{report}"
            );
        }
    }
}

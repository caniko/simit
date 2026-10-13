use serde_json::Value;
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;
mod common;

fn write(root: &Path, name: &str, data: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

#[test]
fn hosted_verification_and_credentials_use_selected_nonfirst_member_and_same_backend() {
    for (target, ci, expected) in [
        (
            "[release.github]\nrepo = 'example/selected'\n",
            "",
            ".github",
        ),
        (
            "[release.codeberg]\nrepo = 'example/selected'\n",
            "",
            ".forgejo",
        ),
        (
            "[release.github]\nrepo = 'example/selected'\n[release.codeberg]\nrepo = 'example/selected'\n",
            "",
            ".forgejo",
        ),
        (
            "[release.github]\nrepo = 'example/selected'\n[release.codeberg]\nrepo = 'example/selected'\n",
            "[ci]\nplatform = 'github'\n",
            ".github",
        ),
    ] {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = ['first', 'selected']\nresolver = '3'\n",
        );
        for name in ["first", "selected"] {
            write(
                root,
                &format!("{name}/Cargo.toml"),
                &format!(
                    "[package]\nname = '{name}'\nversion = '1.2.3'\nedition = '2024'\npublish = false\n"
                ),
            );
            write(root, &format!("{name}/src/main.rs"), "fn main() {}\n");
        }
        write(
            root,
            "simit.toml",
            &format!(
                "{ci}{target}[release.artifacts]\nrunner = 'ubuntu-latest'\nprebuild_binaries = true\nsign = false\n"
            ),
        );
        write(
            root,
            "CHANGELOG.md",
            "# Changelog\n\n## [1.2.3] - 2026-10-13\n\n- Initial release.\n",
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["add", "."])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.com",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-qm",
                    "fixture"
                ])
                .status()
                .unwrap()
                .success()
        );
        let generated = common::simit()
            .current_dir(root)
            .args(["init", "release", "--package", "selected"])
            .output()
            .unwrap();
        assert!(generated.status.success(), "{generated:?}");
        assert!(
            root.join(format!("{expected}/workflows/release.yml"))
                .is_file()
        );
        let contract = common::simit()
            .current_dir(root)
            .args([
                "release",
                "secrets",
                "contract",
                "--package",
                "selected",
                "--json",
            ])
            .output()
            .unwrap();
        assert!(contract.status.success(), "{contract:?}");
        let verified = common::simit()
            .current_dir(root)
            .args(["release", "verify", "--package", "selected", "--json"])
            .output()
            .unwrap();
        let report: Value = serde_json::from_slice(&verified.stdout)
            .unwrap_or_else(|error| panic!("{error}: {verified:?}"));
        let check = report["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["check"] == "release workflow managed")
            .unwrap();
        assert_eq!(check["status"], "pass", "{report:#}");
    }
}

#[test]
fn ci_artifact_gates_validate_full_tag_but_parse_configured_version_prefix() {
    for prefix in ["", "v"] {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            root,
            "Cargo.toml",
            "[package]\nname = 'selected'\nversion = '1.2.3'\nedition = '2024'\n",
        );
        write(root, "src/main.rs", "fn main() {}\n");
        write(
            root,
            "simit.toml",
            &format!("[release]\ntag_prefix = '{prefix}'\n"),
        );
        let generated = common::simit()
            .current_dir(root)
            .args([
                "init",
                "ci",
                "--platform",
                "github",
                "--runtime",
                "cargo",
                "--with-artifacts",
            ])
            .output()
            .unwrap();
        assert!(generated.status.success(), "{generated:?}");
        let workflow: serde_yaml::Value = serde_yaml::from_str(
            &fs::read_to_string(root.join(".github/workflows/release-artifacts.yaml")).unwrap(),
        )
        .unwrap();
        let script = workflow["jobs"]["build"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["name"] == "Validate signed release tag")
            .unwrap()["run"]
            .as_str()
            .unwrap();
        assert!(script.contains("git verify-tag \"$tag\""));
        assert!(script.contains("refs/tags/${tag}"));
        // Execute the actual generated extraction/semver validation slice.
        let start = script
            .find("case \"$tag\"")
            .unwrap_or_else(|| script.find("if ! printf").unwrap());
        let end = script.find("if ! command -v gpg").unwrap();
        for (tag, success) in [
            (format!("{prefix}1.2.3"), true),
            (
                if prefix.is_empty() {
                    "v1.2.3".into()
                } else {
                    "1.2.3".into()
                },
                false,
            ),
        ] {
            let output = Command::new("bash")
                .args(["-euo", "pipefail", "-c", &script[start..end]])
                .env("tag", tag)
                .output()
                .unwrap();
            assert_eq!(output.status.success(), success, "{output:?}");
        }
        let source = fs::read_to_string(root.join(".github/workflows/publish-crate.yaml")).unwrap();
        assert!(source.contains(&format!("\"{prefix}[0-9]*\"")));
        let publisher =
            fs::read_to_string(root.join(".github/workflows/trusted-publish-crate.yaml")).unwrap();
        assert!(publisher.contains(&format!("--publisher-tag-prefix '{prefix}'")));
    }
}

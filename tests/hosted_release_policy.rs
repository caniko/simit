use std::{fs, path::Path, process::Command};

use tempfile::TempDir;

mod common;

fn project(config: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"player\"\nversion = \"0.2.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("simit.toml"), config).unwrap();
    dir
}

fn render(root: &Path) -> String {
    let output = common::simit()
        .current_dir(root)
        .args(["init", "release", "--print"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

const HOSTED: &str = "[release.codeberg]\nrepo = \"example/player\"\n\n[release.artifacts]\nrunner = \"atlas-nix-trusted\"\nprebuild_binaries = true\nsign = false\n";

#[test]
fn prebuild_requires_gates_and_uses_effective_legacy_notes_policy() {
    let dir = project(
        "[ci]\nrequired_gates = [{id='integration',run='false | tee gate.log',timeout_minutes=7}]\n[release.github]\nrepo='example/player'\nbody_from_changelog=false\n[release.artifacts]\nrunner='ubuntu-24.04'\nprebuild_binaries=true\nsign=false\n[prebuild]\nrelease_archives=true\npublish_attic=true\nattic_app='push-flake-inputs'\nsystem_runners={x86_64-linux='ubuntu-24.04'}\n[release.attic]\nurl='https://cache.example.invalid'\ncache='release'\ntoken_name='release'\ntoken_secret='ATTIC_TOKEN'\n",
    );
    let workflow = render(dir.path());
    let doc: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    assert_eq!(doc["jobs"]["prebuild"]["needs"], "validate");
    assert!(
        doc["jobs"]["release"]["needs"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|job| job == "validate")
    );
    let steps = doc["jobs"]["validate"]["steps"].as_sequence().unwrap();
    let validation = steps
        .iter()
        .position(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.contains("git verify-tag"))
        })
        .unwrap();
    let gate = steps
        .iter()
        .position(|step| step["name"] == "Required gate integration")
        .unwrap();
    assert!(validation < gate);
    assert_eq!(steps[gate]["timeout-minutes"], 7);
    assert!(
        steps[gate]["run"]
            .as_str()
            .unwrap()
            .contains("false | tee gate.log")
    );
    assert!(!workflow.contains("CHANGELOG.md"), "{workflow}");
    assert_eq!(workflow.matches("Required gate integration").count(), 1);
}

#[test]
fn prefixed_tags_keep_manifest_versions_unprefixed() {
    let dir = project(&format!("[release]\ntag_prefix = \"v\"\n\n{HOSTED}"));
    let workflow = render(dir.path());
    assert!(workflow.contains("\"v[0-9]*\""));
    assert!(workflow.contains("git verify-tag \"$TAG\""));
    assert!(workflow.contains("--arg tag \"$TAG\""));
    assert!(workflow.contains("(.version == $version)"));
    assert!(workflow.contains("VERSION=\"${TAG#v}\""));
}

#[test]
fn reusable_prebuild_uses_the_same_effective_notes_policy() {
    for (notes, legacy, changelog) in [
        (None, false, false),
        (Some("git"), true, false),
        (Some("none"), true, false),
        (Some("changelog"), false, true),
    ] {
        let notes = notes.map_or_else(String::new, |source| {
            format!("[release.notes]\nsource='{source}'\n")
        });
        let dir = project(&format!(
            "[release]\ntag_prefix='v'\n{notes}[ci]\nplatform='github'\nprovider='actions'\nruntime='nix'\n[release.github]\nrepo='example/player'\nbody_from_changelog={legacy}\n[release.artifacts]\nrunner='ubuntu-24.04'\nprebuild_binaries=true\nsign=false\n[prebuild]\nrelease_archives=true\npublish_attic=false\nsystem_runners={{x86_64-linux='ubuntu-24.04'}}\n"
        ));
        fs::write(dir.path().join("flake.nix"), "{}\n").unwrap();
        let release = render(dir.path());
        assert_eq!(release.contains("CHANGELOG.md"), changelog);
        let output = common::simit()
            .current_dir(dir.path())
            .args(["init", "ci", "--prebuild-only"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let prebuild =
            fs::read_to_string(dir.path().join(".github/workflows/prebuild.yaml")).unwrap();
        assert_eq!(prebuild.contains("CHANGELOG.md"), changelog, "{prebuild}");
        assert!(prebuild.contains("VERSION=\"${TAG#v}\""));
    }
}

#[test]
fn disabling_changelog_body_removes_the_changelog_precondition() {
    let dir = project(&HOSTED.replace(
        "repo = \"example/player\"",
        "repo = \"example/player\"\nbody_from_changelog = false",
    ));
    let workflow = render(dir.path());
    assert!(!workflow.contains("CHANGELOG.md"));
    assert!(workflow.contains("git verify-tag"));
}

#[test]
fn required_gates_block_publication_on_the_validated_revision() {
    let dir = project(&format!(
        "[ci]\nrequired_gates = [{{ id = \"player-tests\", run = \"nix run .#player-tests\", timeout_minutes = 90, env = {{ WOW_DATA = \"\" }} }}]\n\n{HOSTED}"
    ));
    let workflow = render(dir.path());
    let checkout = workflow.find("git checkout --detach").unwrap();
    let gate = workflow.find("nix run .#player-tests").unwrap();
    let upload = workflow.find("Publish Forgejo release").unwrap();
    assert!(checkout < gate && gate < upload);
    assert!(workflow.contains("timeout-minutes: 90"));
    assert!(workflow.contains("WOW_DATA:"));
}

#[test]
fn release_artifact_jobs_do_not_reuse_required_gate_workspaces() {
    for platform in ["github", "codeberg"] {
        let dir = project(&format!(
            "[ci]\nrequired_gates = [{{id='mutate',run='echo gate > src/main.rs; touch gate-generated.rs; git commit --allow-empty -m gate',timeout_minutes=9}}]\n[release.{platform}]\nrepo='example/player'\n[release.artifacts]\nrunner='ubuntu-24.04'\nprebuild_binaries=true\nsign=false\n"
        ));
        let workflow = render(dir.path());
        let doc: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
        assert_eq!(
            doc["jobs"]["release"]["needs"], "validate",
            "{platform}: {workflow}"
        );
        let gates = doc["jobs"]["validate"]["steps"].as_sequence().unwrap();
        assert!(
            gates
                .iter()
                .any(|step| step["name"] == "Required gate mutate")
        );
        let artifact_steps = doc["jobs"]["release"]["steps"].as_sequence().unwrap();
        assert!(
            !artifact_steps
                .iter()
                .any(|step| step["name"] == "Required gate mutate")
        );
        assert!(
            artifact_steps
                .iter()
                .any(|step| step["with"]["ref"] == "${{ github.sha }}")
        );
        assert!(artifact_steps.iter().any(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.contains("git verify-tag") && run.contains("GITHUB_SHA"))
        }));
        assert!(
            !serde_yaml::to_string(&doc["jobs"]["validate"])
                .unwrap()
                .contains("secrets.")
        );
    }
}

#[test]
fn crow_required_release_gates_are_not_silently_omitted() {
    let dir = project(
        "[ci]\nprovider='crow'\nplatform='forgejo'\nrequired_gates=[{id='required',run='exit 99',timeout_minutes=7}]\n[release.codeberg]\nrepo='example/player'\n[release.artifacts]\nrunner='crow-default'\nbuild_commands=['mkdir -p release']\nsign=false\n",
    );
    let output = common::simit()
        .current_dir(dir.path())
        .args(["init", "release"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Crow release workflows do not support required gates"),
        "{output:?}"
    );
    assert!(!dir.path().join(".crow/release.yaml").exists());
}

#[test]
fn crow_rejects_unsupported_release_notes_and_prefix() {
    for (policy, diagnostic) in [
        (
            "[release.notes]\nsource='git'\n",
            "Crow release workflows do not support release notes",
        ),
        (
            "[release.notes]\nsource='changelog'\n",
            "Crow release workflows do not support release notes",
        ),
        (
            "[release]\ntag_prefix='v'\n",
            "Crow release workflows do not support prefixed tags",
        ),
    ] {
        let dir = project(&format!(
            "{policy}[ci]\nprovider='crow'\nplatform='forgejo'\n[release.codeberg]\nrepo='example/player'\n[release.artifacts]\nrunner='crow-default'\nbuild_commands=['mkdir -p release']\nsign=false\n"
        ));
        let output = common::simit()
            .current_dir(dir.path())
            .args(["init", "release"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "{output:?}"
        );
        assert!(!dir.path().join(".crow/release.yaml").exists());
    }
}

#[cfg(unix)]
#[test]
fn comprehensive_release_signed_tag_names_reject_aliases() {
    let key = TempDir::new().unwrap();
    let output = Command::new("gpg")
        .env("GNUPGHOME", key.path())
        .args([
            "--batch",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "",
            "--quick-generate-key",
            "Release fixture <release@example.invalid>",
            "ed25519",
            "sign",
            "0",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let public_key = Command::new("gpg")
        .env("GNUPGHOME", key.path())
        .args(["--export", "release@example.invalid"])
        .output()
        .unwrap();
    assert!(public_key.status.success());
    for platform in ["github", "codeberg"] {
        for prefix in ["", "v"] {
            let dir = project(&format!(
                "[release]\ntag_prefix='{prefix}'\n[release.notes]\nsource='none'\n[release.{platform}]\nrepo='example/player'\n[release.artifacts]\nrunner='ubuntu-24.04'\nprebuild_binaries=true\nsign=false\n"
            ));
            let root = dir.path();
            fs::create_dir(root.join("keys")).unwrap();
            fs::write(root.join("keys/maintainers.gpg"), &public_key.stdout).unwrap();
            let git = |args: &[&str]| {
                let output = Command::new("git")
                    .current_dir(root)
                    .env("GNUPGHOME", key.path())
                    .args([
                        "-c",
                        "user.name=Release fixture",
                        "-c",
                        "user.email=release@example.invalid",
                        "-c",
                        "user.signingkey=release@example.invalid",
                    ])
                    .args(args)
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{output:?}");
                output
            };
            git(&["init"]);
            git(&["add", "."]);
            git(&["commit", "-m", "fixture"]);
            git(&["remote", "add", "origin", "."]);
            let head = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout).unwrap();
            git(&["tag", "-s", "component/v0.2.0", "-m", "component release"]);
            let object =
                String::from_utf8(git(&["rev-parse", "refs/tags/component/v0.2.0"]).stdout)
                    .unwrap();
            let tag = format!("{prefix}0.2.0");
            git(&["update-ref", &format!("refs/tags/{tag}"), object.trim()]);
            git(&["verify-tag", &format!("refs/tags/{tag}")]);
            let workflow = render(root);
            let doc: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
            let script = doc["jobs"]["release"]["steps"]
                .as_sequence()
                .unwrap()
                .iter()
                .find(|step| step["name"] == "Validate tag")
                .unwrap()["run"]
                .as_str()
                .unwrap();
            let validate = || {
                Command::new("bash")
                    .current_dir(root)
                    .env("GITHUB_REF_NAME", &tag)
                    .env("GITHUB_SHA", head.trim())
                    .env("GITHUB_ENV", root.join("environment"))
                    .args(["-c", &format!("{script}\nprintf validated > admitted")])
                    .output()
                    .unwrap()
            };
            let output = validate();
            assert!(!output.status.success(), "{platform}/{prefix}: {output:?}");
            assert!(!root.join("admitted").exists());
            git(&["tag", "-s", "-f", &tag, "-m", "correct release"]);
            let output = validate();
            assert!(output.status.success(), "{output:?}");
            assert!(root.join("admitted").exists());
        }
    }
}

#[test]
fn unsafe_tag_prefix_is_rejected() {
    let dir = project(&format!("[release]\ntag_prefix = \"../\"\n\n{HOSTED}"));
    let output = common::simit()
        .current_dir(dir.path())
        .args(["init", "release", "--print"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("tag_prefix"));
}

#[test]
fn binary_verification_does_not_request_crate_publication() {
    let dir = project(&format!("[release.notes]\nsource = \"git\"\n\n{HOSTED}"));
    let output = common::simit()
        .current_dir(dir.path())
        .args(["release", "verify", "--json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let checks = report["results"].as_array().unwrap();
    assert!(
        !checks
            .iter()
            .any(|c| c["check"].as_str().unwrap().starts_with("crates.io"))
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("CRATES_IO_API_TOKEN"));
    assert!(
        !checks
            .iter()
            .any(|c| c["check"] == "CHANGELOG entry exists")
    );
}

#[test]
fn git_notes_are_bounded_by_the_release_tag() {
    let dir = project(&format!(
        "[release]\ntag_prefix = \"v\"\n\n[release.notes]\nsource = \"git\"\n\n{HOSTED}"
    ));
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "core.hooksPath", "/dev/null"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "commit.gpgSign=false",
        "commit",
        "-qm",
        "Initial player",
    ]);
    git(&["-c", "tag.gpgSign=false", "tag", "v0.1.0"]);
    fs::write(dir.path().join("change"), "release").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "commit.gpgSign=false",
        "commit",
        "-qm",
        "Improve lighting",
    ]);
    git(&["-c", "tag.gpgSign=false", "tag", "v0.2.0"]);
    git(&["-c", "tag.gpgSign=false", "tag", "v0.2.1"]);
    fs::write(dir.path().join("change"), "later").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "commit.gpgSign=false",
        "commit",
        "-qm",
        "Unreleased work",
    ]);
    git(&["-c", "tag.gpgSign=false", "tag", "v0.1.9"]);
    let workflow = render(dir.path());
    let start = workflow
        .find("      - name: Generate release notes\n")
        .unwrap();
    let step = &workflow[start..];
    let end = step[1..].find("\n      - ").map_or(step.len(), |i| i + 1);
    let script = step[..end]
        .split("        run: |\n")
        .nth(1)
        .unwrap()
        .lines()
        .map(|l| l.strip_prefix("          ").unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        dir.path().join("release-env"),
        "TAG=v0.2.0\nVERSION=0.2.0\nIS_PRERELEASE=false\n",
    )
    .unwrap();
    let output = Command::new("bash")
        .current_dir(dir.path())
        .args(["-c", &script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let notes = fs::read_to_string(dir.path().join("release-notes.md")).unwrap();
    assert!(notes.contains("Improve lighting"));
    assert!(!notes.contains("Initial player"));
    assert!(!notes.contains("Unreleased work"));
    fs::write(
        dir.path().join("release-env"),
        "TAG=v0.1.0\nVERSION=0.1.0\nIS_PRERELEASE=false\n",
    )
    .unwrap();
    let output = Command::new("bash")
        .current_dir(dir.path())
        .args(["-c", &script])
        .output()
        .unwrap();
    assert!(output.status.success());
    let notes = fs::read_to_string(dir.path().join("release-notes.md")).unwrap();
    assert!(notes.contains("Initial player"));
    assert!(!notes.contains("Improve lighting"));
}

#[test]
fn native_git_notes_start_after_supported_prerelease_tags() {
    for previous in ["1.2.3-dev.1", "1.2.3-alpha", "1.2.3-beta", "1.2.3-rc.2"] {
        let dir = project("");
        let root = dir.path();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Simit note fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        git(&["config", "commit.gpgsign", "false"]);
        git(&["config", "tag.gpgsign", "false"]);
        git(&["add", "."]);
        git(&["commit", "-qm", "Prior prerelease history"]);
        git(&["tag", &format!("native/v{previous}")]);
        fs::write(
            root.join("src/main.rs"),
            "fn main() { println!(\"new\"); }\n",
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "New component fix"]);
        git(&["tag", "native/v1.2.3"]);
        let script =
            simit::release_notes::component_git_notes_script("native", &["src".into()]).unwrap();
        let output = Command::new("bash")
            .current_dir(root)
            .args(["-c", &script])
            .env("TAG", "native/v1.2.3")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let notes = String::from_utf8(output.stdout).unwrap();
        assert!(notes.contains("New component fix"), "{notes}");
        assert!(
            !notes.contains("Prior prerelease history"),
            "{previous}: {notes}"
        );
    }
}

#[test]
fn package_downloads_use_tags_but_archive_names_use_versions() {
    let dir = project(
        "[release]\ntag_prefix = \"v\"\n\n[homebrew]\ndescription = \"Player\"\nhomepage = \"https://example.com/player\"\nlicense = \"MIT\"\ndownload_repo = \"example/player\"\ntap_url = \"https://example.com/tap.git\"\n",
    );
    let output = common::simit()
        .current_dir(dir.path())
        .args(["dist", "homebrew", "render", "--version", "0.2.0"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let formula = String::from_utf8(output.stdout).unwrap();
    assert!(formula.contains("/releases/download/v0.2.0/player-0.2.0-"));
}

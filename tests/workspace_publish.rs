use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn fixture_dir(name: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    copy_dir(
        Path::new("tests/fixtures").join(name).as_path(),
        temp.path(),
    );
    temp
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).unwrap();
        }
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

fn assert_yaml_parses(text: &str) {
    serde_yaml::from_str::<serde_yaml::Value>(text).unwrap();
}

fn write_minimal_package(root: &Path, name: &str) {
    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "MIT"
"#
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
}

fn write_workspace_simit_toml(root: &Path, extra: &str) {
    fs::write(
        root.join("simit.toml"),
        format!(
            r#"[ci]
platform = "github"
provider = "actions"
runtime = "nix"
runner = "ubuntu-latest"
workspace = true
workspace_strategy = "aggregate"
publish_crates = true
publish_strategy = "coordinated"
{extra}
"#
        ),
    )
    .unwrap();
}

// --- Release-plan graph fixtures (offline, cargo metadata only) ---

#[test]
fn release_plan_diamond_orders_deterministically() {
    let temp = fixture_dir("release-plan-diamond");

    let output = simit()
        .current_dir(temp.path())
        .args(["release", "plan", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let names: Vec<String> = value
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect();
    // `a` first, `d` last; `b`/`c` alphabetical tie-break.
    assert_eq!(names, vec!["a", "b", "c", "d"]);
    let d = value
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "d")
        .unwrap();
    assert_eq!(d["depends_on"], serde_json::json!(["b", "c"]));
}

#[test]
fn release_plan_advanced_handles_renamed_optional_target_and_dev_deps() {
    let temp = fixture_dir("release-plan-advanced");

    let output = simit()
        .current_dir(temp.path())
        .args(["release", "plan", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let names: Vec<String> = value
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect();
    // `helper` (publish=false, dev-only) excluded; `liba` before `libb`
    // (renamed) before `app` (optional + target-specific normal deps).
    assert!(!names.contains(&"helper".to_owned()));
    assert_eq!(names, vec!["liba", "libb", "app"]);
}

#[test]
fn release_plan_rejects_non_publishable_normal_dependency() {
    let temp = fixture_dir("release-plan-nonpublishable");

    let output = simit()
        .current_dir(temp.path())
        .args(["release", "plan"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("depends on non-publishable workspace member"));
}

#[test]
fn release_plan_reports_cycle() {
    let temp = fixture_dir("release-plan-cycle");

    let output = simit()
        .current_dir(temp.path())
        .args(["release", "plan"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("local dependency cycle"));
}

// --- Coordinated publish generation (GitHub, offline fixtures) ---

fn init_coordinated_workspace(extra_gates: &str) -> TempDir {
    let temp = fixture_dir("release-plan-diamond");
    // Add a project-owned custom flake (hooks-only composition, not generated).
    fs::write(
        temp.path().join("flake.nix"),
        "{ outputs = { self }: {}; }\n",
    )
    .unwrap();
    write_workspace_simit_toml(temp.path(), extra_gates);
    temp
}

#[test]
fn coordinated_publish_generates_ordered_gated_workflow() {
    let temp = init_coordinated_workspace(
        r#"[[ci.required_gates]]
id = "gel-integration"
run = "nix run .#test-gel"
timeout_minutes = 30
"#,
    );

    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
            "--publish-crates",
            "--coordinated-publish",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let audit = simit::registry::audit_ci(temp.path()).unwrap();
    assert_eq!(
        audit.status,
        simit::registry::FeatureStatus::Managed,
        "{audit:?}"
    );

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_yaml_parses(&ci);
    assert!(
        !ci.contains("\n\n\n"),
        "generated CI must remain treefmt-stable"
    );
    // Required gate is a dedicated CI job with scoped identity + timeout.
    assert!(ci.contains("- name: Required gate gel-integration"));
    assert!(ci.contains("run: nix run .#test-gel"));
    assert!(ci.contains("timeout-minutes: 30"));
    // No release secrets in ordinary CI jobs.
    assert!(!ci.contains("CRATES_IO_API_TOKEN"));
    assert!(!ci.contains("MINISIGN_SECRET_KEY"));

    let publish = read(&temp.path().join(".github/workflows/publish-workspace.yaml"));
    assert_yaml_parses(&publish);
    let trusted = read(
        &temp
            .path()
            .join(".github/workflows/trusted-publish-workspace.yaml"),
    );
    assert_yaml_parses(&trusted);
    // Tag-only triggers: no publish-on-PR.
    assert!(publish.contains("tags:\n      - \"[0-9]*\""));
    assert!(!publish.contains("pull_request"));
    assert!(!publish.contains("workflow_dispatch"));
    assert!(trusted.contains("resume: rerun the original tag-push run"));
    assert!(!publish.contains("re-dispatch"));
    // Least-privilege permissions.
    assert!(publish.contains("permissions:\n  contents: read"));
    assert!(!publish.contains("CRATES_IO_API_TOKEN"));
    assert!(trusted.contains("actions: read"));
    assert!(!trusted.contains("id-token: write"));
    // Serialize conflicting attempts.
    assert!(publish.contains("cancel-in-progress: false"));
    // Signed-tag + trust-root gating preserved (never disabled when missing).
    assert!(publish.contains("git verify-tag \"$tag\""));
    assert!(publish.contains("test -s \"$GNUPGHOME/maintainers.gpg\""));
    assert!(publish.contains("$RUNNER_TEMP/simit-package-target/package/a-$version.crate"));
    assert!(publish.contains("--target-dir \"$RUNNER_TEMP/simit-package-target\""));
    assert!(!publish.contains(" | jq"));
    // Lockstep validation for every publishable member.
    assert!(publish.contains("cargo pkgid --locked -p a"));
    assert!(publish.contains("cargo pkgid --locked -p d"));
    assert!(publish.contains("$(nix develop -c cargo pkgid --locked -p a"));
    let parsed: serde_yaml::Value = serde_yaml::from_str(&publish).unwrap();
    // The tag may move after validation. Every job that reads source, including
    // required gates and transitive dependent publishers, retains the event SHA
    // that validation requires the signed tag to match.
    for (name, job) in parsed["jobs"].as_mapping().unwrap() {
        let checkout = job["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["name"].as_str() == Some("Checkout"))
            .unwrap();
        assert_eq!(
            checkout["with"]["ref"].as_str(),
            Some("${{ github.sha }}"),
            "job {name:?} must not resolve a mutable release ref"
        );
    }
    assert_eq!(
        parsed["jobs"]["build"]["env"]["CARGO_HOME"].as_str(),
        Some("/tmp/.cargo")
    );
    // Gate failure blocks publication via needs chain.
    let parsed: serde_yaml::Value = serde_yaml::from_str(&trusted).unwrap();
    assert!(
        parsed["jobs"]["verify-a"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|step| step["name"] == "Required gate gel-integration"
                && step["timeout-minutes"] == 30)
    );
    assert_eq!(parsed["jobs"]["publish-a"]["needs"][0], "verify-a");
    // Dependency order via explicit needs: b/c need a, d needs b+c.
    let b_job = trusted.find("verify-b:").expect("verify-b job");
    let b_needs = &trusted[b_job..b_job + 400];
    assert!(b_needs.contains("publish-a"));
    let d_job = trusted.find("verify-d:").expect("verify-d job");
    let d_needs = &trusted[d_job..d_job + 500];
    assert!(d_needs.contains("publish-b"));
    assert!(d_needs.contains("publish-c"));
    // Archive construction stays unprivileged; packaged-content verification
    // precedes fresh-runner API upload and the engine's exact registry wait.
    assert!(publish.contains("cargo package --locked --allow-dirty --no-verify -p"));
    assert!(trusted.contains("cargo publish --dry-run --locked --manifest-path"));
    assert!(trusted.contains("--publisher-stage publish"));
    // Auditable non-publishing summary.
    assert!(trusted.contains("publish-report"));
    assert!(trusted.contains("if: always()"));
}

#[test]
fn member_workflows_have_unique_check_names_for_single_split_and_gate_jobs() {
    for (platform, split) in [("github", false), ("forgejo", false), ("forgejo", true)] {
        let temp = fixture_dir("release-plan-diamond");
        let runner_map = if split {
            "[ci.step_runners]\ncargo-clippy = 'lint-runner'\ncargo-test = 'test-runner'\n"
        } else {
            ""
        };
        fs::write(
            temp.path().join("simit.toml"),
            format!(
                "[ci]\nplatform = '{platform}'\nruntime = 'cargo'\nrunner = 'fixture-linux'\nworkspace = false\nworkspace_strategy = 'members'\npackages = ['a', 'b']\nall_features = false\n{runner_map}\n[[ci.required_gates]]\nid = 'integration'\nrun = 'cargo test --no-default-features'\n"
            ),
        )
        .unwrap();
        let output = simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let mut names = std::collections::BTreeSet::new();
        for member in ["a", "b"] {
            let path = temp
                .path()
                .join(format!(".{platform}/workflows/ci-{member}.yaml"));
            let text = read(&path);
            let workflow: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
            assert_eq!(
                workflow["name"].as_str(),
                Some(format!("CI ({member})").as_str())
            );
            let jobs = workflow["jobs"].as_mapping().unwrap();
            assert_eq!(
                jobs.len(),
                match (member, split) {
                    ("a", true) => 4,
                    ("a", false) => 2,
                    (_, true) => 3,
                    (_, false) => 1,
                }
            );
            for (id, job) in jobs {
                let name = job["name"]
                    .as_str()
                    .expect("every member job needs a distinct check name");
                assert_eq!(name, format!("{member} / {}", id.as_str().unwrap()));
                assert!(
                    names.insert(name.to_owned()),
                    "ambiguous member check: {name}"
                );
            }
            assert!(!text.contains("--all-features"));
            assert!(text.contains(&format!("cargo test -p {member}")));
            assert_eq!(
                text.matches("run: cargo test --no-default-features")
                    .count(),
                usize::from(member == "a")
            );
        }
        let check = simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--check", "--diff"])
            .output()
            .unwrap();
        assert!(check.status.success(), "{check:?}");
        assert_eq!(
            simit::registry::audit_ci(temp.path()).unwrap().status,
            simit::registry::FeatureStatus::Managed
        );
    }
}

#[test]
fn coordinated_publication_preserves_five_member_ci_policies_and_dependency_order() {
    let temp = init_coordinated_workspace(
        "all_features = true\nwith_docs = true\n[[ci.required_gates]]\nid = 'member-contract'\nrun = 'cargo test --all-features'\n",
    );
    let config_path = temp.path().join("simit.toml");
    let config = read(&config_path)
        .replace(
            "workspace_strategy = \"aggregate\"",
            "workspace_strategy = \"members\"",
        )
        .replace(
            "publish_strategy = \"coordinated\"",
            "publish_strategy = \"members\"",
        );
    fs::write(&config_path, config).unwrap();
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .status()
            .unwrap()
            .success()
    );
    let mut member_ci = std::collections::BTreeMap::new();
    for name in ["a", "b", "c", "d", "tool"] {
        let ci = temp
            .path()
            .join(format!(".github/workflows/ci-{name}.yaml"));
        let content = read(&ci);
        assert!(content.contains(&format!("cargo test -p {name} --all-features")));
        assert!(content.contains("cargo doc"));
        assert_eq!(
            content.matches("gate-member-contract:").count(),
            usize::from(name == "a")
        );
        member_ci.insert(ci, content);
    }
    // Only publication switches to a dependency-ordered carrier. CI policy,
    // member selectors, check names and a project-owned workflow are retained.
    let foreign = temp.path().join(".github/workflows/project-owned.yml");
    fs::write(&foreign, "name: Project-owned\non: [push]\njobs: {}\n").unwrap();
    let output = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--coordinated-publish"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    for (path, content) in &member_ci {
        assert_eq!(
            read(path),
            *content,
            "member CI must not change: {}",
            path.display()
        );
    }
    assert_eq!(
        read(&foreign),
        "name: Project-owned\non: [push]\njobs: {}\n"
    );
    // Persistence omits default values. Check the effective policy, so an
    // absent default spelling still has to mean member CI and ordered publish.
    let config = simit::config::ProjectConfig::load(temp.path()).unwrap();
    assert_eq!(
        config.ci.workspace_strategy,
        simit::cli::WorkspaceStrategy::Members
    );
    assert_eq!(
        config.ci.publish_strategy,
        simit::config::PublishStrategy::Coordinated
    );
    let publish: serde_yaml::Value = serde_yaml::from_str(&read(
        &temp
            .path()
            .join(".github/workflows/trusted-publish-workspace.yaml"),
    ))
    .unwrap();
    let jobs = &publish["jobs"];
    assert!(jobs["publish-tool"].is_null());
    for member in ["a", "b", "c", "d"] {
        assert!(
            !temp
                .path()
                .join(format!(".github/workflows/publish-crate-{member}.yaml"))
                .exists()
        );
    }
    assert!(
        jobs["verify-b"]["needs"]
            .as_sequence()
            .unwrap()
            .contains(&serde_yaml::Value::from("publish-a"))
    );
    assert!(
        jobs["verify-d"]["needs"]
            .as_sequence()
            .unwrap()
            .contains(&serde_yaml::Value::from("publish-b"))
    );
    assert!(
        jobs["verify-d"]["needs"]
            .as_sequence()
            .unwrap()
            .contains(&serde_yaml::Value::from("publish-c"))
    );
    assert!(
        jobs["verify-a"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|step| step["name"] == "Required gate member-contract")
    );
    assert_eq!(jobs["publish-a"]["needs"][0], "verify-a");
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--check", "--diff"])
            .status()
            .unwrap()
            .success()
    );
    let audit = simit::registry::audit_ci(temp.path()).unwrap();
    assert_eq!(
        audit.status,
        simit::registry::FeatureStatus::ManagedExtra,
        "{audit:?}"
    );
}

#[cfg(unix)]
#[test]
fn coordinated_tag_prefixes_validate_source_and_archive_versions() {
    use std::os::unix::fs::PermissionsExt;

    for prefix in ["", "v"] {
        let temp = init_coordinated_workspace("");
        if !prefix.is_empty() {
            let config = read(&temp.path().join("simit.toml"));
            fs::write(
                temp.path().join("simit.toml"),
                format!("{config}\n[release]\ntag_prefix = '{prefix}'\n"),
            )
            .unwrap();
        }
        let output = simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--runtime", "cargo"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let workflow: serde_yaml::Value = serde_yaml::from_str(&read(
            &temp.path().join(".github/workflows/publish-workspace.yaml"),
        ))
        .unwrap();
        assert_eq!(workflow["on"]["push"]["tags"][0], format!("{prefix}[0-9]*"));
        let script = workflow["jobs"]["build"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["name"].as_str() == Some("Validate signed release tag"))
            .unwrap()["run"]
            .as_str()
            .unwrap();

        // Exercise the rendered shell boundary without contacting GitHub or requiring
        // an operator signing key. Signature verification succeeds in this fixture;
        // only the binding between the tag commit and the immutable event SHA varies.
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        fs::create_dir_all(temp.path().join("keys")).unwrap();
        fs::write(
            temp.path().join("keys/maintainers.gpg"),
            "fixture trust root",
        )
        .unwrap();
        for (name, body) in [
            ("gpg", "exit 0"),
            (
                "git",
                "case \"$1\" in\nfetch|verify-tag) exit 0;;\nshow) printf 'fixture trust root';;\ncat-file) printf 'tag %s\\n\\n' \"$GITHUB_REF_NAME\";;\nrev-parse) case \"$3\" in HEAD) printf '%s\\n' \"$GITHUB_SHA\";; *) printf '%s\\n' \"$TEST_TAG_SHA\";; esac;;\ncheckout) touch checkout-ran;;\n*) exit 99;;\nesac",
            ),
            ("cargo", "printf 'fixture@%s\\n' \"${TEST_VERSION:-0.1.0}\""),
        ] {
            let path = bin.join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let event_sha = "1111111111111111111111111111111111111111";
        for tag_sha in ["2222222222222222222222222222222222222222", event_sha] {
            let output = Command::new("bash")
                .current_dir(temp.path())
                .args(["-c", script])
                .env("PATH", &path)
                .env("TMPDIR", temp.path())
                .env("GITHUB_REF_NAME", format!("{prefix}0.1.0"))
                .env("GITHUB_SHA", event_sha)
                .env("TEST_TAG_SHA", tag_sha)
                .output()
                .unwrap();
            assert_eq!(output.status.success(), tag_sha == event_sha, "{output:?}");
            assert!(
                !temp.path().join("checkout-ran").exists(),
                "the tag validator must never checkout a mutable tag"
            );
        }
        for step in workflow["jobs"]["build"]["steps"].as_sequence().unwrap() {
            let Some(name) = step["name"]
                .as_str()
                .and_then(|name| name.strip_prefix("Build immutable archive "))
            else {
                continue;
            };
            let packaged = temp
                .path()
                .join(format!("simit-package-target/package/{name}-0.1.0.crate"));
            fs::create_dir_all(packaged.parent().unwrap()).unwrap();
            fs::write(packaged, "fixture archive").unwrap();
            for version in ["0.1.0", "0.2.0"] {
                let output = Command::new("bash")
                    .current_dir(temp.path())
                    .args(["-c", step["run"].as_str().unwrap()])
                    .env("PATH", &path)
                    .env("RUNNER_TEMP", temp.path())
                    .env("GITHUB_REF_NAME", format!("{prefix}0.1.0"))
                    .env("TEST_VERSION", version)
                    .output()
                    .unwrap();
                assert_eq!(
                    output.status.success(),
                    version == "0.1.0",
                    "{prefix}/{name}/{version}: {output:?}"
                );
            }
            assert_eq!(
                read(
                    &temp
                        .path()
                        .join(format!("simit-crates/{name}/{name}-0.1.0.crate"))
                ),
                "fixture archive"
            );
        }
    }
}

#[test]
fn coordinated_publish_replaces_per_member_outputs_only() {
    let temp = init_coordinated_workspace("");
    // Stale per-member output with the generated marker is superseded.
    fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
    fs::write(
        temp.path().join(".github/workflows/publish-crate-a.yaml"),
        format!(
            "{}\nname: stale\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        ),
    )
    .unwrap();
    fs::write(
        temp.path().join(".github/workflows/ci-a.yaml"),
        format!(
            "{}\nname: stale\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        ),
    )
    .unwrap();
    // Handwritten outputs are never deleted.
    fs::write(
        temp.path().join(".github/workflows/handwritten.yaml"),
        "name: handwritten\n",
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
            "--publish-crates",
            "--coordinated-publish",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        !temp
            .path()
            .join(".github/workflows/publish-crate.yaml")
            .exists(),
        "coordinated publication must have exactly one publisher workflow"
    );
    assert!(
        !temp.path().join(".github/workflows/ci-a.yaml").exists(),
        "aggregate CI supersedes generated member CI"
    );
    assert!(
        temp.path()
            .join(".github/workflows/publish-workspace.yaml")
            .exists()
    );
    assert!(
        !temp
            .path()
            .join(".github/workflows/publish-crate-a.yaml")
            .exists()
    );
    assert_eq!(
        read(&temp.path().join(".github/workflows/handwritten.yaml")),
        "name: handwritten\n"
    );
}

#[test]
fn coordinated_publish_check_detects_drift_and_is_stable() {
    let temp = init_coordinated_workspace("");

    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
            "--publish-crates",
            "--coordinated-publish",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let before = read(&temp.path().join(".github/workflows/publish-workspace.yaml"));
    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
            "--publish-crates",
            "--coordinated-publish",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        before,
        read(&temp.path().join(".github/workflows/publish-workspace.yaml")),
        "generator output must be stable"
    );

    // Altered file detected.
    fs::write(
        temp.path().join(".github/workflows/publish-workspace.yaml"),
        format!("{before}\n# drift\n"),
    )
    .unwrap();
    let check = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--check"])
        .output()
        .unwrap();
    assert!(!check.status.success());
    assert!(
        String::from_utf8_lossy(&check.stderr).contains("differs")
            || String::from_utf8_lossy(&check.stdout).contains("differs")
            || !check.status.success()
    );

    // Missing file detected.
    fs::remove_file(temp.path().join(".github/workflows/publish-workspace.yaml")).unwrap();
    let check = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--check"])
        .output()
        .unwrap();
    assert!(!check.status.success());
}

#[test]
fn coordinated_publish_rejects_non_github_backends() {
    let temp = init_coordinated_workspace("");
    let output = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--runner",
            "atlas",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
            "--publish-crates",
            "--coordinated-publish",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires --platform github"));
}

#[test]
fn required_gates_reject_invalid_config() {
    let temp = TempDir::new().unwrap();
    write_minimal_package(temp.path(), "demo");
    fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
    // Duplicate id.
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
runtime = "nix"

[[ci.required_gates]]
id = "gel"
run = "nix run .#test-gel"

[[ci.required_gates]]
id = "gel"
run = "nix run .#other"
"#,
    )
    .unwrap();
    let output = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("duplicate id"));

    // Distinct raw ids that sanitize to the same job name must also fail.
    let temp = TempDir::new().unwrap();
    write_minimal_package(temp.path(), "demo");
    fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
runtime = "nix"

[[ci.required_gates]]
id = "a_b"
run = "nix run .#test-gel"

[[ci.required_gates]]
id = "a-b"
run = "nix run .#other"
"#,
    )
    .unwrap();
    let output = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("collides after sanitization"));
}

#[test]
fn coordinated_publish_preserves_signing_trust_when_config_missing() {
    // No [release.signing] in simit.toml; generation uses the ephemeral
    // SIMIT_MAINTAINERS_GPG key, but the workflow must still verify the
    // committed trust root at runtime (never silently disable signing).
    let temp = init_coordinated_workspace("");
    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
            "--publish-crates",
            "--coordinated-publish",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let publish = read(&temp.path().join(".github/workflows/publish-workspace.yaml"));
    assert!(publish.contains("git show \"FETCH_HEAD:keys/maintainers.gpg\""));
    assert!(publish.contains("gpg --batch --import \"$GNUPGHOME/maintainers.gpg\""));
    assert!(publish.contains("git verify-tag \"$tag\""));
}

#[test]
fn coordinated_publish_bounds_propagation_and_distinguishes_failures() {
    let temp = init_coordinated_workspace("");
    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
            "--publish-crates",
            "--coordinated-publish",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    use simit::release_publisher::execution::wait_for_registry_with;
    let mut attempts = 0;
    let mut waits = 0;
    let result = wait_for_registry_with(
        "a",
        "0.1.0",
        "checksum",
        || {
            attempts += 1;
            Ok(None)
        },
        || panic!("absent API version cannot be resolved"),
        || {
            waits += 1;
        },
    );
    assert!(result.is_err());
    assert_eq!(attempts, 20);
    assert_eq!(waits, 19);
    for status in [401, 403, 503] {
        let result = wait_for_registry_with(
            "a",
            "0.1.0",
            "checksum",
            || anyhow::bail!("HTTP {status}"),
            || panic!("provider failure cannot become acceptance"),
            || panic!("transport failure must fail fast"),
        );
        assert!(result.is_err());
    }
}

#[cfg(unix)]
#[test]
fn coordinated_publication_waits_for_exact_cargo_resolution_even_when_api_is_ready() {
    use simit::release_publisher::execution::{
        registry_resolution_matches, wait_for_registry_with,
    };
    let checksum = "a".repeat(64);
    let record = serde_json::json!({"version":{"crate":"a","num":"0.1.0","yanked":false,"checksum":checksum}});
    for (mode, success, attempts) in [
        ("delayed", true, 2),
        ("binary-only", true, 1),
        ("missing", false, 20),
        ("foreign", false, 20),
        ("wrong-version", false, 20),
        ("wrong-checksum", false, 20),
    ] {
        let mut count = 0;
        let result = wait_for_registry_with(
            "a",
            "0.1.0",
            &checksum,
            || Ok(Some(record.clone())),
            || {
                count += 1;
                if mode == "missing" || (mode == "delayed" && count == 1) {
                    return Ok(false);
                }
                let source = if mode == "foreign" {
                    "path+file:///checkout/a"
                } else {
                    "registry+https://github.com/rust-lang/crates.io-index"
                };
                let version = if mode == "wrong-version" {
                    "0.1.1"
                } else {
                    "0.1.0"
                };
                let digest = if mode == "wrong-checksum" {
                    "wrong"
                } else {
                    &checksum
                };
                registry_resolution_matches(
                    &format!(
                        "version=4\n[[package]]\nname='a'\nversion='{version}'\nsource='{source}'\nchecksum='{digest}'\n[[package]]\nname='probe'\nversion='0.0.0'\n"
                    ),
                    "a",
                    "0.1.0",
                    &checksum,
                )
            },
            || {},
        );
        assert_eq!(result.is_ok(), success, "{mode}: {result:?}");
        assert_eq!(count, attempts, "{mode}");
    }
}

#[cfg(unix)]
#[test]
fn single_and_coordinated_publication_require_usable_checksum_bound_registry_archives() {
    use sha2::Digest;
    use simit::release_publisher::execution::{publish_archive_with, wait_for_registry_with};

    for runtime in ["cargo", "nix"] {
        let mut readiness = Vec::new();
        for coordinated in [false, true] {
            let temp = if coordinated {
                init_coordinated_workspace("")
            } else {
                let temp = TempDir::new().unwrap();
                write_minimal_package(temp.path(), "a");
                fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
                fs::write(
                    temp.path().join("simit.toml"),
                    "[ci]\nplatform='github'\nrunner='ubuntu-24.04'\npublish_crates=true\n",
                )
                .unwrap();
                temp
            };
            let output = simit()
                .current_dir(temp.path())
                .args(["init", "ci", "--runtime", runtime])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let (workflow, job) = if coordinated {
                ("trusted-publish-workspace.yaml", "publish-a")
            } else {
                ("trusted-publish-crate.yaml", "publish-a")
            };
            let yaml: serde_yaml::Value =
                serde_yaml::from_str(&read(&temp.path().join(".github/workflows").join(workflow)))
                    .unwrap();
            let steps = yaml["jobs"][job]["steps"].as_sequence().unwrap();
            let publish_index = steps
                .iter()
                .position(|step| step["name"] == "Upload independently verified archive a")
                .unwrap();
            let command = steps[publish_index]["run"].as_str().unwrap();
            assert!(command.contains("simit release publisher --publisher-stage publish"));
            assert!(!command.contains("cargo publish"));
            readiness.push("simit release publisher --publisher-stage publish");
            let target = temp.path().join("custom target/package");
            fs::create_dir_all(&target).unwrap();
            let archive = b"intended verified release archive\n";
            fs::write(target.join("a-0.1.0.crate"), archive).unwrap();
            let checksum = hex::encode(sha2::Sha256::digest(archive));
            let valid = serde_json::json!({"version":{"crate":"a","num":"0.1.0","yanked":false,"checksum":checksum}});
            for (mode, status, token, success, uploaded) in [
                ("matching", "200", "", true, false),
                ("conflict", "200", "fixture", false, false),
                ("yanked", "200", "fixture", false, false),
                ("wrong-version", "200", "fixture", false, false),
                ("wrong-crate", "200", "fixture", false, false),
                ("missing-yanked", "200", "fixture", false, false),
                ("missing-checksum", "200", "fixture", false, false),
                ("malformed", "200", "fixture", false, false),
                ("absent", "404", "fixture", true, true),
                ("no-token", "404", "", false, false),
                ("unauthorized", "401", "fixture", false, false),
                ("forbidden", "403", "fixture", false, false),
                ("server-error", "503", "fixture", false, false),
                ("network", "network", "fixture", false, false),
            ] {
                let state = temp.path().join(mode);
                fs::create_dir(&state).unwrap();
                let mut response = valid.clone();
                match mode {
                    "conflict" => response["version"]["checksum"] = "0".repeat(64).into(),
                    "yanked" => response["version"]["yanked"] = true.into(),
                    "wrong-version" => response["version"]["num"] = "0.1.1".into(),
                    "wrong-crate" => response["version"]["crate"] = "other".into(),
                    "missing-yanked" => {
                        response["version"]
                            .as_object_mut()
                            .unwrap()
                            .remove("yanked");
                    }
                    "missing-checksum" => {
                        response["version"]
                            .as_object_mut()
                            .unwrap()
                            .remove("checksum");
                    }
                    _ => {}
                }
                let json = if mode == "malformed" {
                    "{invalid".to_owned()
                } else {
                    serde_json::to_string(&response).unwrap()
                };
                fs::write(state.join("registry.json"), &json).unwrap();
                let output = publish_archive_with(
                    &serde_json::json!({"name":"a","vers":"0.1.0"}),
                    archive,
                    &checksum,
                    Some(token),
                    |name, version| {
                        assert_eq!((name, version), ("a", "0.1.0"));
                        match status {
                            "200" => Ok(Some(serde_json::from_str(&json)?)),
                            "404" => Ok(None),
                            _ => anyhow::bail!("provider failure {status}"),
                        }
                    },
                    |actual_token, metadata, actual_archive| {
                        assert_eq!(actual_token, "fixture");
                        assert_eq!(metadata["name"], "a");
                        assert_eq!(metadata["vers"], "0.1.0");
                        assert_eq!(actual_archive, archive);
                        fs::write(state.join("published"), "verified-archive-upload")?;
                        Ok(())
                    },
                );
                assert_eq!(
                    output.is_ok(),
                    success,
                    "{runtime}/{coordinated}/{mode}: {output:?}"
                );
                assert_eq!(
                    state.join("published").exists(),
                    uploaded,
                    "{runtime}/{coordinated}/{mode}"
                );
                if uploaded {
                    assert_eq!(read(&state.join("published")), "verified-archive-upload");
                }
                if success {
                    let mut resolutions = 0;
                    wait_for_registry_with(
                        "a",
                        "0.1.0",
                        &checksum,
                        || Ok(Some(valid.clone())),
                        || {
                            resolutions += 1;
                            Ok(true)
                        },
                        || panic!("already-ready exact archive must resolve immediately"),
                    )
                    .unwrap();
                    assert_eq!(
                        resolutions, 1,
                        "both uploads and checksum resumes need Cargo registry resolution"
                    );
                }
            }
        }
        assert_eq!(
            readiness[0], readiness[1],
            "single and coordinated publishers must use the same readiness gate"
        );
    }
}

#[test]
fn coordinated_publish_conflict_is_not_success_and_resume_is_auditable() {
    let temp = init_coordinated_workspace("");
    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
            "--publish-crates",
            "--coordinated-publish",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let publish = read(
        &temp
            .path()
            .join(".github/workflows/trusted-publish-workspace.yaml"),
    );
    // The same engine used in the runtime cases above owns checksum conflicts.
    assert!(publish.contains("--publisher-stage publish"));
    // Resume guidance + always-run auditable report (not atomic rollback).
    assert!(publish.contains("resume: rerun the original tag-push run"));
    assert!(!publish.contains("re-dispatch"));
    assert!(publish.contains("already-published crates with matching checksums exit 0"));
    assert!(publish.contains("see per-crate job statuses for the auditable result"));
}

#[test]
fn check_mode_leaves_lockfiles_untouched() {
    let temp = init_coordinated_workspace("");
    fs::write(temp.path().join("Cargo.lock"), "lock-stub\n").unwrap();
    fs::write(temp.path().join("flake.lock"), "flake-stub\n").unwrap();
    let cargo_before = read(&temp.path().join("Cargo.lock"));
    let flake_before = read(&temp.path().join("flake.lock"));

    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
            "--publish-crates",
            "--coordinated-publish",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    // Generation writes workflows + simit.toml trust/config, never lockfiles.
    assert_eq!(read(&temp.path().join("Cargo.lock")), cargo_before);
    assert_eq!(read(&temp.path().join("flake.lock")), flake_before);

    let check = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--check"])
        .output()
        .unwrap();
    // May pass or fail depending on trust-root availability, but lockfiles
    // must remain untouched either way (read-only check mode).
    assert_eq!(read(&temp.path().join("Cargo.lock")), cargo_before);
    assert_eq!(read(&temp.path().join("flake.lock")), flake_before);
    let _ = check;
}

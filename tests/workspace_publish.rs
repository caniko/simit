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
    assert!(publish.contains("coordinated workspace publish"));
    // Tag-only triggers: no publish-on-PR.
    assert!(publish.contains("tags:\n      - \"[0-9]*\""));
    assert!(!publish.contains("pull_request"));
    assert!(!publish.contains("workflow_dispatch"));
    assert!(publish.contains("resume: rerun the original tag-push run"));
    assert!(!publish.contains("re-dispatch"));
    // Least-privilege permissions.
    assert!(publish.contains("permissions:\n      contents: read\n      id-token: write"));
    // Serialize conflicting attempts.
    assert!(publish.contains("cancel-in-progress: false"));
    // Signed-tag + trust-root gating preserved (never disabled when missing).
    assert!(publish.contains("git verify-tag \"$tag\""));
    assert!(publish.contains("test -s \"$GNUPGHOME/maintainers.gpg\""));
    assert!(
        publish.contains("local_crate=\"${target_dir}/package/${crate_name}-${version}.crate\"")
    );
    assert!(publish.contains("cargo metadata --no-deps --format-version 1"));
    // Lockstep validation for every publishable member.
    assert!(publish.contains("cargo pkgid -p a"));
    assert!(publish.contains("cargo pkgid -p d"));
    assert!(publish.contains("$(nix develop -c cargo pkgid -p a"));
    assert!(publish.contains(
        ".version | select(.crate == $name and .num == $v and .yanked == false) | .checksum"
    ));
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
    for job in ["validate", "gate-gel-integration", "publish-a"] {
        assert_eq!(
            parsed["jobs"][job]["env"]["CARGO_HOME"].as_str(),
            Some("/tmp/.cargo")
        );
    }
    // Gate failure blocks publication via needs chain.
    assert!(publish.contains("gate-gel-integration"));
    assert!(publish.contains("needs: [validate]"));
    // Dependency order via explicit needs: b/c need a, d needs b+c.
    let b_job = publish.find("publish-b:").expect("publish-b job");
    let b_needs = &publish[b_job..b_job + 400];
    assert!(b_needs.contains("publish-a"));
    let d_job = publish.find("publish-d:").expect("publish-d job");
    let d_needs = &publish[d_job..d_job + 500];
    assert!(d_needs.contains("publish-b"));
    assert!(d_needs.contains("publish-c"));
    // Honest conflict handling: checksum resume vs conflict failure.
    assert!(publish.contains("already exists on crates.io; verifying it is the intended release"));
    assert!(publish.contains("refusing to treat as success"));
    // Upload preflight still fails on auth/ownership; dependencies await
    // exact Cargo registry resolution in a separate step.
    assert!(
        publish
            .contains("waiting for exact registry resolution of ${crate_name} ${version} (attempt")
    );
    assert!(publish.contains("authorization/ownership failure checking"));
    // Packaging stages distinguished: archive+verify, dry-run, publish.
    assert!(publish.contains("cargo package -p"));
    assert!(publish.contains("cargo publish -p"));
    assert!(publish.contains("--dry-run"));
    // Auditable non-publishing summary.
    assert!(publish.contains("publish-report"));
    assert!(publish.contains("if: always()"));
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
            assert_eq!(jobs.len(), if split { 4 } else { 2 });
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
            assert!(text.contains("cargo test --no-default-features"));
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
        assert!(content.contains("member-contract"));
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
    let config = read(&config_path);
    assert!(config.contains("workspace_strategy = \"members\""));
    assert!(config.contains("publish_strategy = \"coordinated\""));
    let publish: serde_yaml::Value = serde_yaml::from_str(&read(
        &temp.path().join(".github/workflows/publish-workspace.yaml"),
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
        jobs["publish-b"]["needs"]
            .as_sequence()
            .unwrap()
            .contains(&serde_yaml::Value::from("publish-a"))
    );
    assert!(
        jobs["publish-d"]["needs"]
            .as_sequence()
            .unwrap()
            .contains(&serde_yaml::Value::from("publish-b"))
    );
    assert!(
        jobs["publish-d"]["needs"]
            .as_sequence()
            .unwrap()
            .contains(&serde_yaml::Value::from("publish-c"))
    );
    assert!(
        jobs["gate-member-contract"]["needs"]
            .as_sequence()
            .unwrap()
            .contains(&serde_yaml::Value::from("validate"))
    );
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--check", "--diff"])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::ManagedExtra
    );
}

#[cfg(unix)]
#[test]
fn coordinated_tag_validation_rejects_a_moved_tag_before_checkout() {
    use std::os::unix::fs::PermissionsExt;

    let temp = init_coordinated_workspace("");
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
    let script = workflow["jobs"]["validate"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| {
            step["name"].as_str() == Some("Validate signed release tag and lockstep versions")
        })
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
            "case \"$1\" in\nfetch|verify-tag) exit 0;;\nshow) printf 'fixture trust root';;\nrev-list) printf '%s\\n' \"$TEST_TAG_SHA\";;\ncheckout) touch checkout-ran;;\n*) exit 99;;\nesac",
        ),
        ("cargo", "printf 'fixture@0.1.0\\n'"),
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
            .env("GITHUB_REF_NAME", "0.1.0")
            .env("GITHUB_SHA", event_sha)
            .env("TEST_TAG_SHA", tag_sha)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), tag_sha == event_sha, "{output:?}");
        assert_eq!(
            temp.path().join("checkout-ran").exists(),
            tag_sha == event_sha
        );
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
    let publish = read(&temp.path().join(".github/workflows/publish-workspace.yaml"));
    // Bounded retries: 20 attempts, 60s calls, 30s sleep, explicit failure.
    assert!(publish.contains("for attempt in $(seq 1 20)"));
    assert!(publish.contains("sleep 30"));
    assert!(publish.contains("registry resolution timeout for"));
    assert!(publish.contains("timeout --kill-after=5s 60s"));
    assert!(publish.contains("cargo fetch --manifest-path"));
    // Upload auth/ownership still fails fast; API readiness never replaces
    // dependency resolution, including when resuming an existing upload.
    assert!(publish.contains("401|403)"));
    assert!(publish.contains("authorization/ownership failure checking"));
    assert!(!publish.contains("simit publish-workspace propagation"));
}

#[cfg(unix)]
#[test]
fn coordinated_publication_waits_for_exact_cargo_resolution_even_when_api_is_ready() {
    use sha2::Digest;
    use std::os::unix::fs::PermissionsExt;
    let temp = init_coordinated_workspace("");
    assert!(
        simit()
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
            .unwrap()
            .success()
    );
    let yaml: serde_yaml::Value = serde_yaml::from_str(&read(
        &temp.path().join(".github/workflows/publish-workspace.yaml"),
    ))
    .unwrap();
    let steps = yaml["jobs"]["publish-a"]["steps"].as_sequence().unwrap();
    let publish = steps
        .iter()
        .position(|step| step["name"].as_str() == Some("Publish"))
        .unwrap();
    let wait = steps.iter().position(|step| step["name"].as_str() == Some("Wait for exact registry resolution")).expect("publication must await Cargo index resolution in a separate step, including checksum resumes");
    assert!(wait > publish);
    let script = steps[wait]["run"].as_str().unwrap();
    let target = temp.path().join("custom target/package");
    fs::create_dir_all(&target).unwrap();
    let archive = b"verified intended release\n";
    fs::write(target.join("a-0.1.0.crate"), archive).unwrap();
    let checksum = hex::encode(sha2::Sha256::digest(archive));
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    // Runtime-created fixtures are not processed by Nix's patchShebangs.
    // Resolve Bash through the build environment rather than assuming /usr.
    let bash = Command::new("bash")
        .args(["-c", "printf '%s' \"$BASH\""])
        .output()
        .unwrap();
    assert!(bash.status.success());
    let interpreter = String::from_utf8(bash.stdout).unwrap();
    assert!(std::path::Path::new(&interpreter).is_absolute());
    for (name, contents) in [
        (
            "curl",
            "#!/usr/bin/env bash\ntouch \"$TEST_STATE/api-probed\"\nprintf '200'\n",
        ),
        ("sleep", "#!/usr/bin/env bash\nexit 0\n"),
        (
            "timeout",
            "#!/usr/bin/env bash\n[[ \"$1\" == --kill-after=5s && \"$2\" == 60s ]] || exit 90\nshift 2\nexec \"$@\"\n",
        ),
        (
            "nix",
            "#!/usr/bin/env bash\n[[ \"$1\" == develop && \"$2\" == -c ]] || exit 91\nshift 2\nexec \"$@\"\n",
        ),
        (
            "cargo",
            r##"#!/usr/bin/env bash
set -euo pipefail
if [[ "$*" == "metadata --no-deps --format-version 1" ]]; then printf '{"target_directory":"%s"}\n' "$TEST_TARGET"; exit 0; fi
[[ "$*" == "fetch --manifest-path "* ]] || exit 92
grep -F 'a = { version = "=0.1.0", registry = "crates-io", default-features = false }' "$3" >/dev/null || exit 93
[[ ! -e "${3%/*}/Cargo.lock" ]] || exit 94
count=0; [[ ! -f "$TEST_STATE/count" ]] || count=$(cat "$TEST_STATE/count")
count=$((count + 1)); printf '%s' "$count" > "$TEST_STATE/count"
if [[ "$TEST_MODE" == missing || ( "$TEST_MODE" == delayed && "$count" == 1 ) ]]; then touch "${3%/*}/Cargo.lock"; echo 'no matching version in registry index' >&2; exit 1; fi
source='registry+https://github.com/rust-lang/crates.io-index'
[[ "$TEST_MODE" != foreign ]] || source='path+file:///checkout/a'
version=0.1.0; [[ "$TEST_MODE" != wrong-version ]] || version=0.1.1
checksum="$TEST_CHECKSUM"; [[ "$TEST_MODE" != wrong-checksum ]] || checksum=wrong
printf 'version = 4\n\n[[package]]\nname = "a"\nversion = "%s"\nsource = "%s"\nchecksum = "%s"\n\n[[package]]\nname = "probe"\nversion = "0.0.0"\n' "$version" "$source" "$checksum" > "${3%/*}/Cargo.lock"
"##,
        ),
    ] {
        let path = bin.join(name);
        fs::write(
            &path,
            contents.replacen("#!/usr/bin/env bash", &format!("#!{interpreter}"), 1),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    for (mode, success, attempts) in [
        ("delayed", true, 2),
        ("binary-only", true, 1),
        ("missing", false, 20),
        ("foreign", false, 20),
        ("wrong-version", false, 20),
        ("wrong-checksum", false, 20),
    ] {
        let state = temp.path().join(mode);
        fs::create_dir(&state).unwrap();
        let output = Command::new("bash")
            .args(["-c", script])
            .current_dir(temp.path())
            .env("PATH", &path)
            .env("GITHUB_REF_NAME", "0.1.0")
            .env("TEST_MODE", mode)
            .env("TEST_STATE", &state)
            .env("TEST_TARGET", target.parent().unwrap())
            .env("TEST_CHECKSUM", &checksum)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(read(&state.join("count")), attempts.to_string());
        assert!(
            !state.join("api-probed").exists(),
            "API readiness cannot establish registry resolution"
        );
    }
}

#[cfg(unix)]
#[test]
fn single_and_coordinated_publication_require_usable_checksum_bound_registry_archives() {
    use sha2::Digest;
    use std::os::unix::fs::PermissionsExt;

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
                ("publish-workspace.yaml", "publish-a")
            } else {
                ("publish-crate.yaml", "publish")
            };
            let yaml: serde_yaml::Value =
                serde_yaml::from_str(&read(&temp.path().join(".github/workflows").join(workflow)))
                    .unwrap();
            let steps = yaml["jobs"][job]["steps"].as_sequence().unwrap();
            let publish_index = steps
                .iter()
                .position(|step| step["name"] == "Publish")
                .unwrap();
            let wait_index = steps
                .iter()
                .position(|step| step["name"] == "Wait for exact registry resolution")
                .unwrap();
            assert!(
                wait_index > publish_index,
                "checksum resumes must still await registry resolution"
            );
            readiness.push(steps[wait_index]["run"].as_str().unwrap().to_owned());
            let script = steps[publish_index]["run"].as_str().unwrap();
            let target = temp.path().join("custom target/package");
            fs::create_dir_all(&target).unwrap();
            let archive = b"intended verified release archive\n";
            fs::write(target.join("a-0.1.0.crate"), archive).unwrap();
            let checksum = hex::encode(sha2::Sha256::digest(archive));
            let valid = serde_json::json!({"version":{"crate":"a","num":"0.1.0","yanked":false,"checksum":checksum}});
            let bin = temp.path().join("bin");
            fs::create_dir(&bin).unwrap();
            for (name, contents) in [
                (
                    "nix",
                    "#!/bin/sh\ntest \"$1\" = develop && test \"$2\" = -c || exit 91\nshift 2\nexec \"$@\"\n",
                ),
                (
                    "cargo",
                    "#!/bin/sh\nset -eu\nif [ \"$*\" = 'metadata --no-deps --format-version 1' ]; then printf '{\"target_directory\":\"%s\"}\\n' \"$TEST_TARGET\"; exit 0; fi\ntest \"$1\" = publish || exit 92\nprintf '%s\\n' \"$*\" > \"$TEST_STATE/published\"\n",
                ),
                (
                    "curl",
                    "#!/bin/sh\nset -eu\nwhile [ \"$#\" -gt 0 ]; do case \"$1\" in -o) output=$2; shift;; https:*) test \"$1\" = 'https://crates.io/api/v1/crates/a/0.1.0' || exit 93;; esac; shift; done\ncp \"$TEST_STATE/registry.json\" \"$output\"\nif [ \"$TEST_HTTP\" = network ]; then exit 7; fi\nprintf '%s' \"$TEST_HTTP\"\n",
                ),
            ] {
                let path = bin.join(name);
                fs::write(&path, contents).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
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
                fs::write(state.join("registry.json"), json).unwrap();
                let output = Command::new("bash")
                    .args(["-c", script])
                    .current_dir(temp.path())
                    .env("PATH", &path)
                    .env("GITHUB_REF_NAME", "0.1.0")
                    .env("CRATES_IO_API_TOKEN", token)
                    .env_remove("CARGO_REGISTRY_TOKEN")
                    .env("TEST_TARGET", target.parent().unwrap())
                    .env("TEST_STATE", &state)
                    .env("TEST_HTTP", status)
                    .output()
                    .unwrap();
                assert_eq!(
                    output.status.success(),
                    success,
                    "{runtime}/{coordinated}/{mode}: {output:?}"
                );
                assert_eq!(
                    state.join("published").exists(),
                    uploaded,
                    "{runtime}/{coordinated}/{mode}"
                );
                if uploaded {
                    assert_eq!(
                        read(&state.join("published")).trim(),
                        if coordinated {
                            "publish -p a"
                        } else {
                            "publish"
                        }
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
    let publish = read(&temp.path().join(".github/workflows/publish-workspace.yaml"));
    // Existing version requires checksum establishment; mismatch fails.
    assert!(publish.contains(".checksum"));
    assert!(publish.contains("checksum matches; resuming (already published)"));
    assert!(publish.contains("conflict:"));
    assert!(publish.contains("refusing to treat as success"));
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

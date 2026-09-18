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
    copy_dir(Path::new("tests/fixtures").join(name).as_path(), temp.path());
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
    // Diamond fixture has no src/ or flake; add minimal package sources and a
    // project-owned custom flake (hooks-only composition, not generated).
    for member in ["a", "b", "c", "d"] {
        let dir = temp.path().join(member);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        // Minimal lib target so `cargo metadata` stays offline-clean.
        let manifest = read(&dir.join("Cargo.toml"));
        fs::write(
            dir.join("Cargo.toml"),
            format!("{manifest}\n[lib]\npath = \"src/lib.rs\"\n"),
        )
        .unwrap();
    }
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

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_yaml_parses(&ci);
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
    // Least-privilege permissions.
    assert!(publish.contains("permissions:\n      contents: read\n      id-token: write"));
    // Serialize conflicting attempts.
    assert!(publish.contains("cancel-in-progress: false"));
    // Signed-tag + trust-root gating preserved (never disabled when missing).
    assert!(publish.contains("git verify-tag \"$tag\""));
    assert!(publish.contains("test -s keys/maintainers.gpg"));
    // Lockstep validation for every publishable member.
    assert!(publish.contains("cargo pkgid -p a"));
    assert!(publish.contains("cargo pkgid -p d"));
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
    // Bounded propagation wait distinguishes delay (404 retry) from
    // auth/ownership (401/403 fast fail) and unexpected statuses.
    assert!(publish.contains("waiting for ${crate_name} ${version} (attempt"));
    assert!(publish.contains("authorization failure while waiting"));
    // Packaging stages distinguished: archive+verify, dry-run, publish.
    assert!(publish.contains("cargo package -p"));
    assert!(publish.contains("cargo publish -p"));
    assert!(publish.contains("--dry-run"));
    // Auditable non-publishing summary.
    assert!(publish.contains("publish-report"));
    assert!(publish.contains("if: always()"));
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
    assert!(temp
        .path()
        .join(".github/workflows/publish-workspace.yaml")
        .exists());
    assert!(!temp
        .path()
        .join(".github/workflows/publish-crate-a.yaml")
        .exists());
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
    assert!(String::from_utf8_lossy(&check.stderr).contains("differs")
        || String::from_utf8_lossy(&check.stdout).contains("differs")
        || !check.status.success());

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
    assert!(publish.contains("test -s keys/maintainers.gpg"));
    assert!(publish.contains("gpg --batch --import keys/maintainers.gpg"));
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
    // Bounded retries: 20 attempts, 30s sleep, explicit timeout failure.
    assert!(publish.contains("for attempt in $(seq 1 20)"));
    assert!(publish.contains("sleep 30"));
    assert!(publish.contains("propagation timeout for"));
    // Propagation delay (404) retries; auth/ownership (401/403) fails fast;
    // unexpected statuses fail fast (not misclassified as delay).
    assert!(publish.contains("404) echo \"waiting for"));
    assert!(publish.contains("401|403) echo \"authorization failure while waiting"));
    assert!(publish.contains("unexpected registry status"));
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
    assert!(publish.contains("re-dispatch this workflow"));
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

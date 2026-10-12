use serde_yaml::Value;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

mod common;

fn project() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("flake.nix"),
        "{ outputs = { self }: {}; }\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        r#"
[ci]
platform = "github"
provider = "actions"
runtime = "nix"
runner = "ubuntu-24.04"
[ci.custody]
repository = "owner/repo"
producer_attempt = 1
status_context = "simit/producer-custody"
[ci.custody.producers.CI]
workflow = ".github/workflows/ci.yaml"
jobs = ["test"]
[ci.custody.producers."Nix installable builds"]
workflow = ".github/workflows/nix-builds.yaml"
jobs = ["build (runtime)"]
artifacts = ["nix-build-0"]
"#,
    )
    .unwrap();
    temp
}

#[test]
fn completion_custody_uses_default_branch_code_without_executing_producer_sources() {
    let temp = project();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let file = temp.path().join(".github/workflows/producer-custody.yaml");
    let value: Value = serde_yaml::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(value["on"]["workflow_run"]["types"][0], "completed");
    assert_eq!(
        value["on"]["workflow_run"]["workflows"]
            .as_sequence()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(value["permissions"]["statuses"], "write");
    assert_eq!(value["permissions"]["actions"], "read");
    assert_eq!(value["concurrency"]["cancel-in-progress"], false);
    let steps = value["jobs"]["collect"]["steps"].as_sequence().unwrap();
    assert!(!steps.iter().any(|s| s["name"] == "Checkout"));
    assert!(!fs::read_to_string(&file).unwrap().contains("gh run watch"));
    let report = steps
        .iter()
        .find(|s| s["name"] == "Report custody on producer source head")
        .unwrap();
    assert_eq!(report["if"], "always()");
    assert_eq!(
        report["env"]["SIMIT_CUSTODY_UPLOAD_OUTCOME"],
        "${{ steps.custody-upload.outcome }}"
    );
    assert!(
        common::simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--check", "--diff"])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
}

#[test]
fn generated_collector_keeps_missing_originals_pending_and_reports_complete_source_bound_custody() {
    let temp = project();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_yaml::from_str(
        &fs::read_to_string(temp.path().join(".github/workflows/producer-custody.yaml")).unwrap(),
    )
    .unwrap();
    let steps = value["jobs"]["collect"]["steps"].as_sequence().unwrap();
    let step = steps
        .iter()
        .find(|s| s["name"] == "Capture completed original producers")
        .unwrap();
    let script = temp.path().join("custody.py");
    fs::write(&script, step["run"].as_str().unwrap()).unwrap();
    let output = Command::new("python3")
        .arg("-c")
        .arg(include_str!("fixtures/ci-custody-contract.py"))
        .arg(&script)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

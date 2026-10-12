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
    assert!(value.get("concurrency").is_none());
    assert_eq!(
        value["jobs"]["collect"]["concurrency"]["cancel-in-progress"],
        false
    );
    assert_eq!(
        value["jobs"]["collect"]["concurrency"]["group"],
        "custody-${{ needs.identity.outputs.group }}"
    );
    assert_eq!(value["jobs"]["collect"]["needs"][0], "identity");
    assert_eq!(value["jobs"]["identity"]["permissions"]["contents"], "read");
    assert!(
        value["jobs"]["identity"]["permissions"]
            .get("statuses")
            .is_none()
    );
    assert_eq!(
        value["jobs"]["identity"]["outputs"]["group"],
        "${{ steps.identity.outputs.group }}"
    );
    assert_eq!(value["jobs"]["collect"]["timeout-minutes"], 25);
    let steps = value["jobs"]["collect"]["steps"].as_sequence().unwrap();
    let upload = steps
        .iter()
        .find(|step| step["id"] == "custody-upload")
        .unwrap();
    assert_eq!(upload["timeout-minutes"], 5);
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
fn unsupported_effective_custody_backends_reject_before_writing_any_language_workflow() {
    for language in ["nix", "rust", "python"] {
        let temp = project();
        let config = temp.path().join("simit.toml");
        let before = fs::read_to_string(&config)
            .unwrap()
            .replace("platform = \"github\"\n", "")
            .replace("provider = \"actions\"\n", "");
        fs::write(&config, &before).unwrap();
        if language == "rust" {
            fs::write(
                temp.path().join("Cargo.toml"),
                "[package]\nname = \"client\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )
            .unwrap();
            fs::create_dir(temp.path().join("src")).unwrap();
            fs::write(temp.path().join("src/lib.rs"), "pub fn client() {}\n").unwrap();
        } else if language == "python" {
            fs::write(
                temp.path().join("pyproject.toml"),
                "[project]\nname = \"client\"\nversion = \"0.1.0\"\nrequires-python = \">=3.11\"\n",
            )
            .unwrap();
        }
        let output = common::simit()
            .current_dir(temp.path())
            .args([
                "init",
                "ci",
                "--platform",
                "forgejo",
                "--ci-provider",
                "crow",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{language}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("effective GitHub Actions backend"),
            "{language}: {output:?}"
        );
        assert_eq!(fs::read_to_string(&config).unwrap(), before);
        assert!(!temp.path().join(".crow").exists());
        assert!(!temp.path().join(".github/workflows").exists());
    }
}

#[test]
fn composed_custody_context_limit_rejects_before_generated_effects() {
    for (prefix, accepted) in [
        ("a".repeat(22), true),
        ("a".repeat(23), false),
        ("é".repeat(11), true),
        ("é".repeat(12), false),
    ] {
        let temp = project();
        let config = temp.path().join("simit.toml");
        let before = fs::read_to_string(&config)
            .unwrap()
            .replace("simit/producer-custody", &prefix);
        fs::write(&config, &before).unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert_eq!(output.status.success(), accepted, "{output:?}");
        if !accepted {
            assert_eq!(fs::read_to_string(&config).unwrap(), before);
            assert!(!temp.path().join(".github/workflows").exists());
            assert!(String::from_utf8_lossy(&output.stderr).contains("composed 100-byte context"));
        }
    }
}

#[test]
fn oversized_enrolled_capture_inventory_rejects_before_workflow_effects() {
    let temp = project();
    let config = temp.path().join("simit.toml");
    let jobs: Vec<_> = (0..101).map(|i| format!("job-{i}")).collect();
    let source = fs::read_to_string(&config)
        .unwrap()
        .replace("jobs = [\"test\"]", &format!("jobs = {jobs:?}"));
    fs::write(&config, &source).unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("at most 100 jobs and artifacts"));
    assert_eq!(fs::read_to_string(&config).unwrap(), source);
    assert!(!temp.path().join(".github/workflows").exists());
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

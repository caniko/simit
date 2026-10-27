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
fn strict_custody_dedupes_completions_per_commit_but_sealed_keeps_every_run() {
    let temp = project();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let workflow = temp.path().join(".github/workflows/producer-custody.yaml");
    let value: Value = serde_yaml::from_str(&fs::read_to_string(&workflow).unwrap()).unwrap();
    assert_eq!(
        value["concurrency"]["group"],
        "producer-custody-${{ github.event.workflow_run.workflow_id }}-${{ github.event.workflow_run.head_repository.id }}-${{ github.event.workflow_run.head_sha }}-${{ github.event.workflow_run.event }}-${{ github.event.workflow_run.head_branch }}-${{ github.event.workflow_run.pull_requests[0].number }}"
    );
    assert_eq!(value["concurrency"]["cancel-in-progress"], false);

    let config = temp.path().join("simit.toml");
    fs::write(
        &config,
        fs::read_to_string(&config)
            .unwrap()
            .replace("[ci.custody]\n", "[ci.custody]\nprotocol = \"sealed-v2\"\n"),
    )
    .unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_yaml::from_str(&fs::read_to_string(&workflow).unwrap()).unwrap();
    assert!(
        value.get("concurrency").is_none(),
        "sealed custody seals every completion and must not drop pending runs"
    );
}

#[test]
fn sealed_custody_is_explicit_and_uploads_capsules_before_aggregate_reporting() {
    let temp = project();
    let config = temp.path().join("simit.toml");
    fs::write(
        &config,
        fs::read_to_string(&config)
            .unwrap()
            .replace("[ci.custody]\n", "[ci.custody]\nprotocol = \"sealed-v2\"\n"),
    )
    .unwrap();
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
    let seal = &value["jobs"]["seal"];
    let aggregate = &value["jobs"]["aggregate"];
    let expected = format!(
        "{}{}",
        include_str!("../src/render/ci/custody.py")
            .split("if __name__ ==")
            .next()
            .unwrap(),
        include_str!("../src/render/ci/custody_sealed.py")
    );
    let mut programs = 0;
    for job in ["identity", "seal", "aggregate"] {
        for step in value["jobs"][job]["steps"].as_sequence().unwrap() {
            if let Some(program) = step["run"].as_str() {
                assert_eq!(program, expected);
                programs += 1;
            }
        }
    }
    assert_eq!(programs, 4);
    assert_eq!(seal["permissions"]["checks"], "read");
    assert!(seal["permissions"].get("statuses").is_none());
    assert!(
        seal["concurrency"]["group"]
            .as_str()
            .unwrap()
            .contains("workflow_run.id")
    );
    assert_eq!(seal["concurrency"]["cancel-in-progress"], false);
    assert_eq!(aggregate["permissions"]["statuses"], "write");
    let phases: Vec<_> = aggregate["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .filter_map(|s| s["env"]["SIMIT_CUSTODY_PHASE"].as_str())
        .collect();
    assert_eq!(phases, ["aggregate", "report"]);
    let script = temp.path().join("sealed.py");
    fs::write(&script, seal["steps"][0]["run"].as_str().unwrap()).unwrap();
    let output = Command::new("python3")
        .arg("-c")
        .arg(include_str!("fixtures/ci-sealed-custody-contract.py"))
        .arg(&script)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn custody_protocol_survives_ci_option_updates_and_regeneration() {
    use simit::config::ProjectConfig;
    use simit::config::custody::Protocol;

    for rust_project in [false, true] {
        for (protocol, expected) in [
            ("strict-v1", Protocol::StrictV1),
            ("sealed-v2", Protocol::SealedV2),
        ] {
            let temp = project();
            if rust_project {
                fs::write(
                    temp.path().join("Cargo.toml"),
                    "[package]\nname = \"custody-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
                )
                .unwrap();
                fs::create_dir(temp.path().join("src")).unwrap();
                fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
            }
            let path = temp.path().join("simit.toml");
            fs::write(
                &path,
                fs::read_to_string(&path).unwrap().replace(
                    "[ci.custody]\n",
                    &format!("[ci.custody]\nprotocol = \"{protocol}\"\n"),
                ),
            )
            .unwrap();
            let before = ProjectConfig::load(temp.path()).unwrap().ci.custody;
            let workflow = temp.path().join(".github/workflows/producer-custody.yaml");
            let initial = common::simit()
                .current_dir(temp.path())
                .args(["init", "ci"])
                .output()
                .unwrap();
            assert!(initial.status.success(), "{protocol}: {initial:?}");
            let observer = fs::read(&workflow).unwrap();

            let update = common::simit()
                .current_dir(temp.path())
                .args(["init", "ci", "--runner", "ubuntu-latest"])
                .output()
                .unwrap();
            assert!(update.status.success(), "{protocol}: {update:?}");
            let reloaded = ProjectConfig::load(temp.path()).unwrap();
            assert_eq!(reloaded.ci.runner.as_deref(), Some("ubuntu-latest"));
            assert_eq!(reloaded.ci.custody, before);
            assert_eq!(reloaded.ci.custody.as_ref().unwrap().protocol, expected);
            assert_eq!(fs::read(&workflow).unwrap(), observer);
            if expected == Protocol::StrictV1 {
                assert!(!fs::read_to_string(&path).unwrap().contains("protocol ="));
            }
            for args in [
                vec!["init", "ci", "--check", "--diff"],
                vec!["init", "ci"],
                vec!["init", "ci", "--check", "--diff"],
            ] {
                let output = common::simit()
                    .current_dir(temp.path())
                    .args(args)
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{protocol}: {output:?}");
                assert_eq!(fs::read(&workflow).unwrap(), observer);
                assert_eq!(ProjectConfig::load(temp.path()).unwrap().ci.custody, before);
            }
        }
    }
}

#[test]
fn custody_protocol_is_versioned_without_changing_default_generation() {
    use simit::config::custody::Protocol;
    let temp = project();
    let default = simit::config::ProjectConfig::load(temp.path())
        .unwrap()
        .ci
        .custody
        .unwrap();
    assert_eq!(default.protocol, Protocol::StrictV1);
    let explicit: simit::config::custody::Config = serde_json::from_value({
        let mut value = serde_json::to_value(&default).unwrap();
        value["protocol"] = serde_json::json!("strict-v1");
        value
    })
    .unwrap();
    assert_eq!(
        simit::render::ci::custody::file(&default).unwrap().content,
        simit::render::ci::custody::file(&explicit).unwrap().content
    );
    let config = temp.path().join("simit.toml");
    let before = fs::read_to_string(&config)
        .unwrap()
        .replace("[ci.custody]\n", "[ci.custody]\nprotocol = \"sealed-v3\"\n");
    fs::write(&config, &before).unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&config).unwrap(), before);
    assert!(!temp.path().join(".github/workflows").exists());
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
    assert!(value["permissions"].get("checks").is_none());
    assert_eq!(
        value["jobs"]["collect"]["permissions"],
        serde_yaml::from_str::<Value>(
            "contents: read\nactions: read\nchecks: read\nstatuses: write\n"
        )
        .unwrap()
    );
    assert_eq!(value["concurrency"]["cancel-in-progress"], false);
    assert_eq!(
        value["jobs"]["collect"]["concurrency"]["cancel-in-progress"],
        false
    );
    assert_eq!(
        value["jobs"]["collect"]["concurrency"]["group"],
        "custody-${{ needs.identity.outputs.group }}"
    );
    assert_eq!(value["jobs"]["collect"]["needs"][0], "identity");
    assert_eq!(
        value["jobs"]["collect"]["if"],
        "needs.identity.outputs.eligible == 'true'"
    );
    assert_eq!(
        value["jobs"]["identity"]["outputs"]["eligible"],
        "${{ steps.identity.outputs.eligible }}"
    );
    assert_eq!(value["jobs"]["identity"]["permissions"]["contents"], "read");
    assert!(
        value["jobs"]["identity"]["permissions"]
            .get("statuses")
            .is_none()
    );
    assert!(
        value["jobs"]["identity"]["permissions"]
            .get("checks")
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
fn rendered_custody_workflow_admits_exact_utf8_byte_boundary_with_full_inventory() {
    use simit::config::custody::Protocol;
    for (protocol, copies, job) in [
        (Protocol::StrictV1, 2, "collect"),
        (Protocol::SealedV2, 3, "aggregate"),
    ] {
        for token in ["x", "é", "\"", "\\", "'"] {
            let temp = project();
            let mut config = simit::config::ProjectConfig::load(temp.path())
                .unwrap()
                .ci
                .custody
                .unwrap();
            config.protocol = protocol;
            let mut producer = config.producers.remove("CI").unwrap();
            producer.jobs = (0..50).map(|i| format!("job-{i}-{token}")).collect();
            producer.artifacts = (0..50).map(|i| format!("artifact-{i}-{token}")).collect();
            config.producers.clear();
            config.producers.insert(format!("CI {token}"), producer);
            let baseline = simit::render::ci::custody::file(&config)
                .unwrap()
                .content
                .len();
            let remaining = 500_000 - baseline;
            // Names have one trigger copy in addition to the repeated policies.
            // Consecutive weights admit an exact boundary under either protocol.
            let name_pad = remaining % copies;
            let (name, mut producer) = config.producers.pop_first().unwrap();
            producer.jobs[0].push_str(&"x".repeat((remaining - (copies + 1) * name_pad) / copies));
            config
                .producers
                .insert(name + &"x".repeat(name_pad), producer);
            let rendered = simit::render::ci::custody::file(&config).unwrap();
            assert_eq!(rendered.content.len(), 500_000, "{token:?}");
            let value: Value = serde_yaml::from_str(&rendered.content).unwrap();
            let policy: serde_json::Value = serde_json::from_str(
                value["jobs"][job]["env"]["SIMIT_CUSTODY_CONFIG"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(policy, serde_json::to_value(&config).unwrap());
            let (name, mut producer) = config.producers.pop_first().unwrap();
            producer.jobs[0].pop();
            config.producers.insert(name + "x", producer);
            config.validate().unwrap();
            let error = simit::render::ci::custody::file(&config).unwrap_err();
            assert!(error.to_string().contains("500001 bytes"), "{error:#}");
        }
    }
}

#[test]
fn oversized_rendered_custody_policy_rejects_without_persistence_or_reconciliation() {
    for (language, protocol) in [
        ("nix", "strict-v1"),
        ("rust", "strict-v1"),
        ("python", "strict-v1"),
        ("nix", "sealed-v2"),
        ("rust", "sealed-v2"),
        ("python", "sealed-v2"),
    ] {
        let temp = project();
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
        let config = temp.path().join("simit.toml");
        let source = fs::read_to_string(&config)
            .unwrap()
            .replace(
                "[ci.custody]\n",
                &format!("[ci.custody]\nprotocol = \"{protocol}\"\n"),
            )
            .replace(
                "jobs = [\"test\"]",
                &format!("jobs = [\"{}\"]", "x".repeat(200_000)),
            );
        fs::write(&config, &source).unwrap();
        let workflows = temp.path().join(".github/workflows");
        fs::create_dir_all(&workflows).unwrap();
        let prior = "# Generated by simit\nname: prior\non: push\njobs: {}\n";
        fs::write(workflows.join("ci.yaml"), prior).unwrap();
        for args in [
            vec!["init", "ci", "--runner", "ubuntu-latest"],
            vec!["init", "ci", "--check", "--diff"],
        ] {
            let output = common::simit()
                .current_dir(temp.path())
                .args(args)
                .output()
                .unwrap();
            assert!(!output.status.success(), "{output:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("500000-byte workflow limit"),
                "{output:?}"
            );
            assert_eq!(fs::read_to_string(&config).unwrap(), source);
            assert_eq!(
                fs::read_to_string(workflows.join("ci.yaml")).unwrap(),
                prior
            );
            assert_eq!(fs::read_dir(&workflows).unwrap().count(), 1);
        }
    }
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

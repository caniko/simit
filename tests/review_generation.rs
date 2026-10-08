use simit::registry::{FeatureStatus, audit_ci};
use std::process::Command;

fn generate(root: &std::path::Path, check: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_simit"));
    command.args(["init", "ci", "--review-only"]);
    if check {
        command.args(["--check", "--diff"]);
    }
    command
        .current_dir(root)
        .env("XDG_CONFIG_HOME", root.join("state"))
        .output()
        .unwrap()
}

#[test]
fn controller_generation_round_trips_and_audits_bootstrap_drift() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("simit.toml"), "[review]\nrole = 'controller'\n").unwrap();
    std::fs::write(root.join("flake.nix"), "{}\n").unwrap();
    std::fs::write(root.join("policy.json"), "{\"publication_enabled\":false}").unwrap();
    let generated = generate(root, false);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    assert!(generate(root, true).status.success());
    assert_eq!(audit_ci(root).unwrap().status, FeatureStatus::Managed);
    std::fs::write(root.join("simit.toml"),"[review]\nrole='controller'\n[ci]\nplatform='github'\nruntime='nix'\nnix_builds=['.#default']\n[ci.nix_build]\nonly=true\n").unwrap();
    let primary = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["init", "ci"])
        .current_dir(root)
        .env("XDG_CONFIG_HOME", root.join("state"))
        .output()
        .unwrap();
    assert!(
        primary.status.success(),
        "{}",
        String::from_utf8_lossy(&primary.stderr)
    );
    assert!(generate(root, true).status.success());
    assert_eq!(audit_ci(root).unwrap().status, FeatureStatus::Managed);
    let workflow =
        std::fs::read_to_string(root.join(".github/workflows/review-repository.yml")).unwrap();
    assert!(workflow.contains("verify-engine --root controller"));
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let secret = &yaml["on"]["workflow_call"]["secrets"]["GH_TOKEN"];
    assert!(
        secret.is_mapping(),
        "callers must be able to forward the report-only token"
    );
    assert_eq!(secret["required"].as_bool(), Some(false));
    for trigger in ["workflow_dispatch", "workflow_call"] {
        assert_eq!(
            yaml["on"][trigger]["inputs"]["controller_revision"]["required"].as_bool(),
            Some(false)
        );
    }
    assert_eq!(
        std::fs::read_to_string(root.join("policy.json")).unwrap(),
        "{\"publication_enabled\":false}"
    );
    std::fs::write(
        root.join(".github/actions/setup-nix/action.yml"),
        "name: drift\n",
    )
    .unwrap();
    assert!(!generate(root, true).status.success());
    assert_eq!(audit_ci(root).unwrap().status, FeatureStatus::Drift);
}

#[test]
fn client_generation_requires_exact_controller_pin_without_cargo_or_flake() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let config = format!(
        "[review]\nrole='client'\ncontroller='caniko/nixpkgs-review-gha'\nrevision='{}'\nrequest='{}'\n",
        "a".repeat(40),
        serde_json::to_string(&simit::review::contract::example()).unwrap()
    );
    std::fs::write(root.join("simit.toml"), &config).unwrap();
    let generated = generate(root, false);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    assert!(generate(root, true).status.success());
    assert_eq!(audit_ci(root).unwrap().status, FeatureStatus::Managed);
    let workflow =
        std::fs::read_to_string(root.join(".github/workflows/review-client.yml")).unwrap();
    assert!(workflow.contains(&format!("review-repository.yml@{}", "a".repeat(40))));
    assert!(!workflow.contains("secrets: inherit"));
    std::fs::write(
        root.join("simit.toml"),
        config.replace(&"a".repeat(40), "main"),
    )
    .unwrap();
    assert!(!generate(root, false).status.success());
}

#[test]
fn controller_identity_rejects_ref_movement_before_publishing_checkout_outputs() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("simit.toml"),
        "[review]\nrole='controller'\n",
    )
    .unwrap();
    std::fs::write(root.path().join("flake.nix"), "{}\n").unwrap();
    let generated = generate(root.path(), false);
    assert!(generated.status.success(), "{generated:?}");
    let yaml: serde_yaml::Value = serde_yaml::from_str(
        &std::fs::read_to_string(root.path().join(".github/workflows/review-repository.yml"))
            .unwrap(),
    )
    .unwrap();
    let step = &yaml["jobs"]["controller"]["steps"][0];
    assert_eq!(
        step["env"]["EXPECTED_CONTROLLER_REVISION"].as_str(),
        Some("${{ inputs.controller_revision }}")
    );
    let script = step["run"]
        .as_str()
        .unwrap()
        .strip_prefix("node <<'NODE'\n")
        .unwrap()
        .strip_suffix("NODE\n")
        .unwrap();
    let actual = "a".repeat(40);
    for (expected, accepted) in [
        (actual.clone(), true),
        ("b".repeat(40), false),
        ("unsafe\nrevision".into(), false),
        (String::new(), true),
    ] {
        let output_path = root.path().join("identity.out");
        let claims = serde_json::json!({
            "iss":"https://token.actions.githubusercontent.com",
            "aud":"repo-review-controller",
            "workflow_ref":"caniko/controller/.github/workflows/review-repository.yml@refs/heads/reviewed-release",
            "workflow_sha":actual
        });
        let harness = format!(
            "global.fetch = async () => ({{ok:true,json:async () => ({{value:'header.'+Buffer.from(JSON.stringify({claims})).toString('base64url')+'.signature'}})}});\n{script}"
        );
        let output = Command::new("node")
            .args(["-e", &harness])
            .env("EXPECTED_CONTROLLER_REVISION", expected)
            .env("GITHUB_OUTPUT", &output_path)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), accepted, "{output:?}");
        assert_eq!(output_path.exists(), accepted);
        if accepted {
            assert_eq!(
                std::fs::read_to_string(&output_path).unwrap(),
                format!("repository=caniko/controller\nrevision={actual}\n")
            );
            std::fs::remove_file(output_path).unwrap();
        }
    }
}

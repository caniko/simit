use simit::registry::{FeatureStatus, audit_ci, infer_project_ci_target};
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
    let report = &yaml["jobs"]["report"];
    assert!(!report["if"].as_str().unwrap().contains("github.repository"));
    assert!(
        report["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|step| step["env"]["GH_TOKEN"].as_str() == Some("${{ secrets.GH_TOKEN }}"))
    );
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

#[cfg(unix)]
#[test]
fn reusable_report_consumes_the_explicit_token_only_when_posting_is_requested() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("simit.toml"),
        "[review]\nrole='controller'\n",
    )
    .unwrap();
    std::fs::write(root.path().join("flake.nix"), "{}\n").unwrap();
    let output = generate(root.path(), false);
    assert!(output.status.success(), "{output:?}");
    let yaml: serde_yaml::Value = serde_yaml::from_str(
        &std::fs::read_to_string(root.path().join(".github/workflows/review-repository.yml"))
            .unwrap(),
    )
    .unwrap();
    let steps = yaml["jobs"]["report"]["steps"].as_sequence().unwrap();
    let script = steps
        .iter()
        .find(|step| {
            step["name"].as_str() == Some("Optional exact-head comment (report-only credential)")
        })
        .unwrap()["run"]
        .as_str()
        .unwrap();
    let bin = root.path().join("bin");
    let tools = root.path().join("tools/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&tools).unwrap();
    for (path, body) in [
        (bin.join("jq"), "[ \"$POST_REQUESTED\" = true ]"),
        (
            tools.join("repo-review"),
            "[ \"$GH_TOKEN\" = fixture-report-token ] && [ \"$PLAN_DIGEST\" = fixture-plan-digest ] || exit 1; touch report-posted; printf '%s\\n' '{\"posted\":true}'",
        ),
    ] {
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    for requested in ["false", "true"] {
        let output = Command::new("bash")
            .current_dir(root.path())
            .args(["-c", script])
            .env("PATH", &path)
            .env("POST_REQUESTED", requested)
            .env("GH_TOKEN", "fixture-report-token")
            .env("PLAN_DIGEST", "fixture-plan-digest")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            root.path().join("report-posted").exists(),
            requested == "true"
        );
    }
}

#[test]
fn review_workflows_preserve_forgejo_and_crow_ci_inference_and_auditing() {
    for provider in ["actions", "crow"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='review-fixture'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        std::fs::write(root.join("flake.nix"), "{}\n").unwrap();
        std::fs::write(
            root.join("simit.toml"),
            format!("[review]\nrole='controller'\n[ci]\nplatform='forgejo'\nprovider='{provider}'\nruntime='nix'\nrunner='atlas'\n"),
        )
        .unwrap();
        let generated = Command::new(env!("CARGO_BIN_EXE_simit"))
            .args(["init", "ci"])
            .current_dir(root)
            .env("XDG_CONFIG_HOME", root.join("state"))
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let audit = audit_ci(root).unwrap();
        assert_eq!(
            audit.status,
            FeatureStatus::Managed,
            "{provider}: {audit:?}"
        );
        assert_eq!(
            audit.platform.as_deref(),
            Some(if provider == "crow" {
                "crow"
            } else {
                "forgejo"
            })
        );
        // Exercise workflow inference without the explicit provider shortcut.
        let config = std::fs::read_to_string(root.join("simit.toml")).unwrap();
        std::fs::write(
            root.join("simit.toml"),
            config
                .replace(&format!("provider = \"{provider}\"\n"), "")
                .replace(&format!("provider='{provider}'\n"), ""),
        )
        .unwrap();
        assert!(
            simit::config::ProjectConfig::load(root)
                .unwrap()
                .ci
                .provider
                .is_none()
        );
        let backend = infer_project_ci_target(root).unwrap().unwrap();
        assert_eq!(backend.platform(), simit::cli::Platform::Forgejo);
        assert_eq!(
            backend.provider(),
            if provider == "crow" {
                simit::cli::CiProvider::Crow
            } else {
                simit::cli::CiProvider::Actions
            }
        );
        assert_eq!(audit_ci(root).unwrap().status, FeatureStatus::Managed);
        std::fs::write(
            root.join(".github/actions/setup-nix/action.yml"),
            "name: drift\n",
        )
        .unwrap();
        assert_eq!(audit_ci(root).unwrap().status, FeatureStatus::Drift);
    }
}

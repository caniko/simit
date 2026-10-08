use std::{fs, process::Output};

use simit::registry::{FeatureStatus, audit_ci};
use tempfile::TempDir;

mod common;

fn project() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();
    fs::create_dir_all(temp.path().join(".simit/templates")).unwrap();
    fs::write(temp.path().join(".simit/templates/tests.yml"),
        "name: Project tests\non: [push]\njobs:\n  test:\n    runs-on: '@simit(runner)@'\n    steps:\n      - run: echo '${{ github.sha }}'\n").unwrap();
    config(
        &temp,
        ".github/workflows/tests.yml",
        ".simit/templates/tests.yml",
        "ubuntu-24.04",
    );
    temp
}

fn config(temp: &TempDir, output: &str, source: &str, runner: &str) {
    fs::write(temp.path().join("simit.toml"), format!(
        "[ci]\nplatform='github'\nprovider='actions'\nruntime='nix'\nrunner='ubuntu-24.04'\nnix_builds=['.#default']\n[ci.nix_build]\nonly=true\n[ci.workflow_templates]\n'{output}'='{source}'\n[ci.workflow_variables]\nrunner='{runner}'\n")).unwrap();
}

fn generate(temp: &TempDir, args: &[&str]) -> Output {
    common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn project_templates_round_trip_and_share_generation_drift_and_registry_ownership() {
    let temp = project();
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    let path = temp.path().join(".github/workflows/tests.yml");
    let original = fs::read_to_string(&path).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&original).unwrap();
    assert_eq!(yaml["jobs"]["test"]["runs-on"], "ubuntu-24.04");
    assert_eq!(
        yaml["jobs"]["test"]["steps"][0]["run"],
        "echo '${{ github.sha }}'"
    );
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(
        audit_ci(temp.path()).unwrap().status,
        FeatureStatus::Managed
    );
    fs::write(
        &path,
        original.replace("ubuntu-24.04", "unavailable-runner"),
    )
    .unwrap();
    assert!(!generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(audit_ci(temp.path()).unwrap().status, FeatureStatus::Drift);
    assert!(generate(&temp, &[]).status.success());
    assert_eq!(original, fs::read_to_string(&path).unwrap());
    config(
        &temp,
        ".github/workflows/tests.yml",
        ".simit/templates/tests.yml",
        "windows-11-arm",
    );
    assert!(generate(&temp, &[]).status.success());
    let updated: serde_yaml::Value =
        serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(updated["jobs"]["test"]["runs-on"], "windows-11-arm");
    // Removing a declaration retires its marked output, preserving foreign files.
    let cfg = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        cfg.split("[ci.workflow_templates]").next().unwrap(),
    )
    .unwrap();
    let foreign = temp.path().join(".github/workflows/foreign.yml");
    fs::write(&foreign, "name: foreign\n").unwrap();
    assert!(!generate(&temp, &["--check"]).status.success());
    assert!(generate(&temp, &[]).status.success());
    assert!(!path.exists());
    assert_eq!(fs::read_to_string(foreign).unwrap(), "name: foreign\n");
}

#[test]
fn invalid_templates_fail_before_any_outputs_are_written() {
    for (output, source, template) in [
        ("../escape.yml", ".simit/templates/tests.yml", "name: bad\n"),
        (
            ".github/workflows/tests.yml",
            "../escape.yml",
            "name: bad\n",
        ),
        (
            ".github/workflows/nix-builds.yaml",
            ".simit/templates/tests.yml",
            "name: bad\n",
        ),
        (
            ".github/workflows/tests.yml",
            ".simit/templates/tests.yml",
            "name: '@simit(missing)@'\n",
        ),
        (
            ".github/workflows/tests.yml",
            ".simit/templates/tests.yml",
            "name: [\n",
        ),
        (
            ".forgejo/workflows/tests.yml",
            ".simit/templates/tests.yml",
            "name: bad\n",
        ),
        (
            ".github/workflows/tests.yml",
            ".simit/templates/tests.yml",
            "name: '@simit(unterminated'\n",
        ),
    ] {
        let temp = project();
        config(&temp, output, source, "ubuntu-24.04");
        fs::write(temp.path().join(".simit/templates/tests.yml"), template).unwrap();
        let result = generate(&temp, &[]);
        assert!(!result.status.success(), "{result:?}");
        assert!(
            !temp
                .path()
                .join(".github/workflows/nix-builds.yaml")
                .exists()
        );
        assert!(!temp.path().join(".github/workflows/tests.yml").exists());
    }
}

#[test]
fn template_retirement_preserves_the_opt_in_review_policy_and_builtin_outputs() {
    let temp = project();
    let policy = "\n[review_policy]\ntoolbelt_version='0.2.0'\napp_id_secret='APP_ID'\napp_private_key_secret='APP_KEY'\ncredential_environment='review-policy'\n";
    let cfg_path = temp.path().join("simit.toml");
    fs::write(&cfg_path, fs::read_to_string(&cfg_path).unwrap() + policy).unwrap();
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    let policy_path = temp.path().join(".github/workflows/review-policy.yaml");
    let builtin_path = temp.path().join(".github/workflows/nix-builds.yaml");
    let policy_output = fs::read_to_string(&policy_path).unwrap();
    let builtin_output = fs::read_to_string(&builtin_path).unwrap();
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(
        audit_ci(temp.path()).unwrap().status,
        FeatureStatus::Managed
    );

    let config = fs::read_to_string(&cfg_path).unwrap();
    let without_template = config
        .split("[ci.workflow_templates]")
        .next()
        .unwrap()
        .to_owned()
        + policy;
    fs::write(cfg_path, without_template).unwrap();
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    assert!(!temp.path().join(".github/workflows/tests.yml").exists());
    assert_eq!(fs::read_to_string(policy_path).unwrap(), policy_output);
    assert_eq!(fs::read_to_string(builtin_path).unwrap(), builtin_output);
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(
        audit_ci(temp.path()).unwrap().status,
        FeatureStatus::Managed
    );
}

#[test]
fn release_owned_workflows_cannot_be_claimed_by_project_templates() {
    for platform in ["github", "forgejo"] {
        for name in [
            "release.yaml",
            "release.yml",
            "publish-vscode-extension.yaml",
            "publish-jetbrains-plugin.yaml",
        ] {
            let temp = project();
            let output = format!(".{platform}/workflows/{name}");
            config(&temp, &output, ".simit/templates/tests.yml", "ubuntu-24.04");
            let cfg = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
            fs::write(
                temp.path().join("simit.toml"),
                cfg.replace("platform='github'", &format!("platform='{platform}'")),
            )
            .unwrap();
            let path = temp.path().join(&output);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let release = "# Generated by simit init release\nname: Release\n";
            fs::write(&path, release).unwrap();
            let result = generate(&temp, &[]);
            assert!(!result.status.success(), "{result:?}");
            assert!(String::from_utf8_lossy(&result.stderr).contains("release-owned"));
            assert_eq!(fs::read_to_string(&path).unwrap(), release);
            assert!(
                !temp
                    .path()
                    .join(format!(".{platform}/workflows/nix-builds.yaml"))
                    .exists()
            );
        }
    }
}

#[test]
fn project_templates_do_not_infer_builtin_checks_packages_or_runners() {
    for output in ["tests.yml", "ci-other.yaml", "release-artifacts.yaml"] {
        let temp = project();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname='template-inference'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let config = format!(
            "[ci]\nplatform='github'\nprovider='actions'\nruntime='cargo'\n[ci.workflow_templates]\n'.github/workflows/{output}'='.simit/templates/tests.yml'\n"
        );
        fs::write(temp.path().join("simit.toml"), &config).unwrap();
        fs::write(temp.path().join(".simit/templates/tests.yml"), "name: Project-only policy\non: [push]\njobs:\n  policy:\n    runs-on: windows-2022\n    steps:\n      - run: cargo deny check bans licenses sources\n").unwrap();
        let result = generate(&temp, &[]);
        assert!(result.status.success(), "{result:?}");
        // Keep optional settings absent so registry inference must use only built-ins.
        fs::write(temp.path().join("simit.toml"), &config).unwrap();
        let built_in = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
        assert!(!built_in.contains("cargo deny check"));
        assert!(!built_in.contains("windows-2022"));
        let result = generate(&temp, &["--check", "--diff"]);
        assert!(result.status.success(), "{result:?}");
        let audit = audit_ci(temp.path()).unwrap();
        assert_eq!(audit.status, FeatureStatus::Managed, "{output}: {audit:?}");
    }
}

#[test]
fn builtin_workflows_cannot_be_sources_even_when_the_previous_output_exists() {
    let temp = project();
    assert!(generate(&temp, &[]).status.success());
    let source = ".github/workflows/nix-builds.yaml";
    let previous = fs::read_to_string(temp.path().join(source)).unwrap();
    config(&temp, ".github/workflows/tests.yml", source, "ubuntu-24.04");
    let cfg = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        cfg.replace("nix_builds=['.#default']", "nix_builds=['.#changed']"),
    )
    .unwrap();
    let output = generate(&temp, &[]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("built-in generated output"));
    assert_eq!(
        fs::read_to_string(temp.path().join(source)).unwrap(),
        previous
    );
}

#[test]
fn template_outputs_reject_portable_case_collisions_without_writes() {
    for output in [
        ".github/workflows/NIX-BUILDS.yaml",
        ".github/workflows/RELEASE.yml",
    ] {
        let temp = project();
        config(&temp, output, ".simit/templates/tests.yml", "ubuntu-24.04");
        let before = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
        for args in [vec![], vec!["--check", "--diff"]] {
            let result = generate(&temp, &args);
            assert!(!result.status.success(), "{output}: {result:?}");
            assert!(String::from_utf8_lossy(&result.stderr).contains("collides"));
            assert_eq!(
                fs::read_to_string(temp.path().join("simit.toml")).unwrap(),
                before
            );
            assert!(
                !temp
                    .path()
                    .join(".github/workflows/nix-builds.yaml")
                    .exists()
            );
        }
    }
    let temp = project();
    let mut cfg = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    cfg = cfg.replace(
        "[ci.workflow_variables]",
        "'.github/workflows/Tests.yml'='.simit/templates/tests.yml'\n[ci.workflow_variables]",
    );
    fs::write(temp.path().join("simit.toml"), cfg).unwrap();
    let result = generate(&temp, &[]);
    assert!(!result.status.success(), "{result:?}");
    assert!(String::from_utf8_lossy(&result.stderr).contains("case-insensitive"));
}

#[test]
fn edited_template_headers_do_not_infer_or_persist_builtin_gates() {
    let temp = project();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='edited-template'\nversion='0.1.0'\nedition='2024'\n",
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(temp.path().join("simit.toml"), "[ci]\nplatform='github'\nprovider='actions'\nruntime='cargo'\n[ci.workflow_templates]\n'.github/workflows/ci-other.yaml'='.simit/templates/tests.yml'\n").unwrap();
    fs::write(temp.path().join(".simit/templates/tests.yml"), "name: Custom\non: [push]\njobs:\n  project:\n    runs-on: windows-2022\n    steps:\n      - run: cargo deny check\n").unwrap();
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    let cfg_path = temp.path().join("simit.toml");
    let baseline = fs::read_to_string(&cfg_path).unwrap();
    // Leave the option omitted so a contaminated inference would persist true.
    fs::write(
        &cfg_path,
        baseline
            .lines()
            .filter(|line| !line.starts_with("with_deny = "))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let output = temp.path().join(".github/workflows/ci-other.yaml");
    let original = fs::read_to_string(&output).unwrap();
    fs::write(
        &output,
        "\n# Project note before the generated header\n".to_owned()
            + &original.replace("# Simit workflow template:", "# Edited template header:"),
    )
    .unwrap();
    assert_eq!(audit_ci(temp.path()).unwrap().status, FeatureStatus::Drift);
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    assert!(
        !fs::read_to_string(temp.path().join(".github/workflows/ci.yaml"))
            .unwrap()
            .contains("cargo deny check")
    );
    assert!(
        !simit::config::ProjectConfig::load(temp.path())
            .unwrap()
            .ci
            .with_deny
    );
    assert_eq!(fs::read_to_string(output).unwrap(), original);
    let result = generate(&temp, &["--check", "--diff"]);
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        audit_ci(temp.path()).unwrap().status,
        FeatureStatus::Managed
    );
}

#[test]
fn inactive_platform_generated_sources_are_rejected_before_reconciliation() {
    let temp = project();
    let source = ".forgejo/workflows/ci.yaml";
    let previous =
        "# Generated by simit. Manual edits will be reported as ci=drift.\nname: CI\non: [push]\n";
    fs::create_dir_all(temp.path().join(".forgejo/workflows")).unwrap();
    fs::write(temp.path().join(source), previous).unwrap();
    config(&temp, ".github/workflows/tests.yml", source, "ubuntu-24.04");
    for args in [vec![], vec!["--check", "--diff"]] {
        let output = generate(&temp, &args);
        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("built-in generated output"));
        assert_eq!(
            fs::read_to_string(temp.path().join(source)).unwrap(),
            previous
        );
        assert!(
            !temp
                .path()
                .join(".github/workflows/nix-builds.yaml")
                .exists()
        );
    }
}

#[test]
fn cli_gitlab_template_mismatch_rejects_both_check_and_write() {
    let temp = project();
    // Establish a matching built-in GitLab file before adding the Actions mapping.
    fs::write(temp.path().join("simit.toml"), "[ci]\nruntime='nix'\n").unwrap();
    let output = generate(&temp, &["--platform", "gitlab"]);
    assert!(output.status.success(), "{output:?}");
    let before = fs::read_to_string(temp.path().join(".gitlab-ci.yml")).unwrap();
    fs::write(temp.path().join("simit.toml"), "[ci]\nruntime='nix'\n[ci.workflow_templates]\n'.github/workflows/tests.yml'='.simit/templates/tests.yml'\n").unwrap();
    for args in [
        vec!["--platform", "gitlab"],
        vec!["--platform", "gitlab", "--check", "--diff"],
    ] {
        let output = generate(&temp, &args);
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("requires GitHub or Forgejo Actions")
        );
        assert_eq!(
            fs::read_to_string(temp.path().join(".gitlab-ci.yml")).unwrap(),
            before
        );
    }
}

#[test]
fn missing_builtins_report_drift_when_only_template_outputs_remain() {
    for rust in [false, true] {
        let temp = project();
        let builtin = if rust {
            fs::write(
                temp.path().join("Cargo.toml"),
                "[package]\nname='missing-builtins'\nversion='0.1.0'\nedition='2024'\n",
            )
            .unwrap();
            fs::create_dir(temp.path().join("src")).unwrap();
            fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
            fs::write(temp.path().join("simit.toml"), "[ci]\nplatform='github'\nprovider='actions'\nruntime='cargo'\nrunner='ubuntu-24.04'\n[ci.workflow_templates]\n'.github/workflows/tests.yml'='.simit/templates/tests.yml'\n[ci.workflow_variables]\nrunner='windows-2022'\n").unwrap();
            ".github/workflows/ci.yaml"
        } else {
            ".github/workflows/nix-builds.yaml"
        };
        let output = generate(&temp, &[]);
        assert!(output.status.success(), "{output:?}");
        fs::remove_file(temp.path().join(builtin)).unwrap();
        let audit = audit_ci(temp.path()).unwrap();
        assert_eq!(audit.status, FeatureStatus::Drift, "{audit:?}");
        let output = generate(&temp, &["--check", "--diff"]);
        assert!(!output.status.success(), "{output:?}");
        let output = generate(&temp, &[]);
        assert!(output.status.success(), "{output:?}");
        assert!(temp.path().join(builtin).is_file());
        assert_eq!(
            audit_ci(temp.path()).unwrap().status,
            FeatureStatus::Managed
        );
    }
}

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

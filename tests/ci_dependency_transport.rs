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
nix_builds = [".#checks.x86_64-linux.runtime"]
[ci.cache]
substituters = ["https://ci-cache.example"]
trusted_public_keys = ["ci-cache.example:YWJj"]
[ci.nix_build]
only = true
capture_results = true
check_systems = ["x86_64-linux"]
dependency_installables = [".#ci-dependency"]
dependency_max_mib = 256
fresh_installables = [".#checks.x86_64-linux.runtime"]
[ci.nix_build.groups]
runtime = [".#checks.x86_64-linux.runtime"]
"#,
    )
    .unwrap();
    temp
}

#[test]
fn dependency_transport_is_same_run_and_runtime_jobs_still_execute_fresh() {
    let temp = project();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_yaml::from_str(
        &fs::read_to_string(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap(),
    )
    .unwrap();
    assert_eq!(value["jobs"]["build"]["needs"][0], "dependencies");
    let steps = value["jobs"]["dependencies"]["steps"]
        .as_sequence()
        .unwrap();
    let export = steps
        .iter()
        .find(|s| s["name"] == "Build and export dependency closure")
        .unwrap();
    let python = steps
        .iter()
        .position(|s| s["name"] == "Provision dependency-transport Python")
        .unwrap();
    assert!(
        python
            < steps
                .iter()
                .position(|s| s["name"] == "Build and export dependency closure")
                .unwrap()
    );
    assert!(
        steps[python]["uses"]
            .as_str()
            .unwrap()
            .starts_with("actions/setup-python@")
    );
    assert_eq!(export["shell"], "python");
    assert!(export["run"].as_str().unwrap().contains("nix-store"));
    let steps = value["jobs"]["build"]["steps"].as_sequence().unwrap();
    let download = steps
        .iter()
        .find(|s| s["name"] == "Download same-run dependencies")
        .unwrap();
    assert!(download["with"]["run-id"].is_null());
    assert!(download["with"]["github-token"].is_null());
    assert!(
        steps
            .iter()
            .any(|s| s["name"] == "Verify and import dependency closure")
    );
    let build = steps
        .iter()
        .find(|s| s["name"] == "Build Nix group")
        .unwrap();
    assert!(build["run"].as_str().unwrap().contains("--rebuild"));
    assert!(
        fs::read_to_string(temp.path().join(".github/workflows/nix-builds.yaml"))
            .unwrap()
            .contains("https://ci-cache.example")
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
fn qualification_outputs_cannot_be_enrolled_as_transport_dependencies() {
    let temp = project();
    let path = temp.path().join("simit.toml");
    let before = fs::read_to_string(&path).unwrap().replace(
        "dependency_installables = [\".#ci-dependency\"]",
        "dependency_installables = [\".#checks.x86_64-linux.runtime\"]",
    );
    fs::write(&path, &before).unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("dependency_installables"),
        "{output:?}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    assert!(!temp.path().join(".github/workflows").exists());
}

#[test]
fn downloaded_dependency_provenance_is_checked_before_any_store_import() {
    let temp = project();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_yaml::from_str(
        &fs::read_to_string(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap(),
    )
    .unwrap();
    let steps = value["jobs"]["build"]["steps"].as_sequence().unwrap();
    let step = steps
        .iter()
        .find(|s| s["name"] == "Verify and import dependency closure")
        .unwrap();
    let program = temp.path().join("transport.py");
    fs::write(&program, step["run"].as_str().unwrap()).unwrap();
    let output = Command::new("python3")
        .arg("-c")
        .arg(include_str!("fixtures/nix-dependency-contract.py"))
        .arg(&program)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn explicit_ci_cache_policy_preserves_release_cache_policy_and_rejects_credentials() {
    let temp = project();
    let config = temp.path().join("simit.toml");
    let base = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("{base}\n[release.artifacts]\nsubstituters = [\"https://release-cache.example\"]\ntrusted_public_keys = [\"release-cache.example:YWJj\"]\n")).unwrap();
    let cfg = simit::config::ProjectConfig::load(temp.path()).unwrap();
    assert_eq!(
        cfg.ci_cache_artifacts().substituters,
        ["https://ci-cache.example"]
    );
    assert_eq!(
        cfg.release.artifacts.substituters,
        ["https://release-cache.example"]
    );
    fs::write(
        &config,
        base.replace("https://ci-cache.example", "https://token@ci-cache.example"),
    )
    .unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("credential-free HTTPS"));
    assert!(!temp.path().join(".github/workflows").exists());
}

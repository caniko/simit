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
    for job in ["dependencies", "build"] {
        assert_eq!(
            value["jobs"][job]["env"]["NIX_CONFIG"],
            "experimental-features = nix-command flakes"
        );
    }
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
    assert_eq!(download["id"], "simit-dependency-download");
    assert!(
        download["with"]["name"]
            .as_str()
            .unwrap()
            .contains("github.run_attempt")
    );
    let guidance = steps
        .iter()
        .find(|s| s["name"] == "Explain missing attempt-bound dependencies")
        .unwrap();
    assert!(
        guidance["if"]
            .as_str()
            .unwrap()
            .contains("steps.simit-dependency-download.outcome == 'failure'")
    );
    assert!(
        guidance["run"]
            .as_str()
            .unwrap()
            .contains("Re-run all jobs")
    );
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
fn dependency_producer_preserves_configured_nix_config() {
    let temp = project();
    let path = temp.path().join("simit.toml");
    let mut config = fs::read_to_string(&path)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    let nix_config = "system-features = kvm big-parallel";
    let other_env = "retained \"quoted\" \\literal";
    config["ci"]["nix_build"]["extra_env"]["NIX_CONFIG"] = toml_edit::value(nix_config);
    config["ci"]["nix_build"]["extra_env"]["TRANSPORT_TEST"] = toml_edit::value(other_env);
    fs::write(&path, config.to_string()).unwrap();
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
    for job in ["dependencies", "build"] {
        assert_eq!(value["jobs"][job]["env"]["NIX_CONFIG"], nix_config);
        assert_eq!(value["jobs"][job]["env"]["TRANSPORT_TEST"], other_env);
    }
}

#[test]
fn dependency_producer_enables_kvm_before_setup_and_export_when_requested() {
    for enabled in [false, true] {
        let temp = project();
        let path = temp.path().join("simit.toml");
        let mut config = fs::read_to_string(&path)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        config["ci"]["nix_build"]["kvm"] = toml_edit::value(enabled);
        let mut setup = toml_edit::Array::new();
        setup.push("echo dependency-kvm-setup");
        config["ci"]["nix_build"]["extra_setup"] = toml_edit::value(setup);
        fs::write(&path, config.to_string()).unwrap();
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
        let mut kvm_runs = Vec::new();
        for job in ["dependencies", "build"] {
            let steps = value["jobs"][job]["steps"].as_sequence().unwrap();
            let kvm: Vec<_> = steps
                .iter()
                .enumerate()
                .filter(|(_, step)| step["name"] == "Enable hosted KVM")
                .collect();
            assert_eq!(kvm.len(), usize::from(enabled), "job={job}");
            if enabled {
                let setup = steps
                    .iter()
                    .position(|step| {
                        step["run"]
                            .as_str()
                            .is_some_and(|run| run.contains("echo dependency-kvm-setup"))
                    })
                    .unwrap();
                assert!(kvm[0].0 < setup, "job={job}");
                kvm_runs.push(kvm[0].1["run"].as_str().unwrap().to_owned());
            }
        }
        if enabled {
            assert_eq!(kvm_runs[0], kvm_runs[1]);
            assert!(kvm_runs[0].contains("test -c /dev/kvm"));
            assert!(kvm_runs[0].contains("test -r /dev/kvm && test -w /dev/kvm"));
        }
    }
}

#[test]
fn qualification_outputs_cannot_be_enrolled_as_transport_dependencies() {
    for installable in [".#checks", ".#checks.x86_64-linux.runtime"] {
        let temp = project();
        let path = temp.path().join("simit.toml");
        let before = fs::read_to_string(&path).unwrap().replace(
            "dependency_installables = [\".#ci-dependency\"]",
            &format!("dependency_installables = [\"{installable}\"]"),
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
}

#[test]
fn dependency_names_outside_the_checks_namespace_remain_usable() {
    for installable in [".#mychecks.foo", ".#packages.x86_64-linux.prechecks.tool"] {
        let temp = project();
        let path = temp.path().join("simit.toml");
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace(".#ci-dependency", installable),
        )
        .unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{installable}: {output:?}");
        assert!(
            common::simit()
                .current_dir(temp.path())
                .args(["init", "ci", "--check", "--diff"])
                .status()
                .unwrap()
                .success()
        );
    }
}

#[test]
fn dependency_output_selectors_reject_before_generation() {
    for selector in ["dev", "out", "*"] {
        let temp = project();
        let path = temp.path().join("simit.toml");
        let before = fs::read_to_string(&path)
            .unwrap()
            .replace(".#ci-dependency", &format!(".#ci-dependency^{selector}"));
        fs::write(&path, &before).unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(diagnostic.contains("dependency_installables"), "{output:?}");
        assert!(diagnostic.contains("output selectors"), "{output:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        assert!(!temp.path().join(".github/workflows").exists());
    }
}

#[test]
fn qualification_output_selectors_reject_only_when_transport_is_enabled() {
    for selector in ["dev", "out", "*"] {
        for transport in [true, false] {
            let temp = project();
            let path = temp.path().join("simit.toml");
            let mut before = fs::read_to_string(&path).unwrap().replace(
                ".#checks.x86_64-linux.runtime",
                &format!(".#runtime^{selector}"),
            );
            if !transport {
                before = before
                    .replace("dependency_installables = [\".#ci-dependency\"]\n", "")
                    .replace("dependency_max_mib = 256\n", "");
            }
            fs::write(&path, &before).unwrap();
            let output = common::simit()
                .current_dir(temp.path())
                .args(["init", "ci"])
                .output()
                .unwrap();
            assert_eq!(output.status.success(), !transport, "{output:?}");
            if transport {
                let diagnostic = String::from_utf8_lossy(&output.stderr);
                assert!(diagnostic.contains("nix_builds"), "{output:?}");
                assert!(diagnostic.contains("output selectors"), "{output:?}");
                assert_eq!(fs::read_to_string(&path).unwrap(), before);
                assert!(!temp.path().join(".github/workflows").exists());
            }
        }
    }
}

#[test]
fn transport_qualification_plan_requires_local_source_without_writes() {
    for installable in ["github:owner/repo#runtime", "path:../other#runtime", ".#"] {
        for transport in [true, false] {
            let temp = project();
            let path = temp.path().join("simit.toml");
            let mut before = fs::read_to_string(&path)
                .unwrap()
                .replace(".#checks.x86_64-linux.runtime", installable);
            if !transport {
                before = before
                    .replace("dependency_installables = [\".#ci-dependency\"]\n", "")
                    .replace("dependency_max_mib = 256\n", "");
            }
            fs::write(&path, &before).unwrap();
            let output = common::simit()
                .current_dir(temp.path())
                .args(["init", "ci"])
                .output()
                .unwrap();
            assert_eq!(output.status.success(), !transport, "{output:?}");
            if transport {
                let diagnostic = String::from_utf8_lossy(&output.stderr);
                assert!(diagnostic.contains("nix_builds"), "{output:?}");
                assert!(diagnostic.contains("local derivations"), "{output:?}");
                assert_eq!(fs::read_to_string(&path).unwrap(), before);
                assert!(!temp.path().join(".github/workflows").exists());
            }
        }
    }
}

#[test]
fn dependency_timeout_preserves_omitted_and_explicit_runner_limits() {
    for timeout in [None, Some(240)] {
        let temp = project();
        let path = temp.path().join("simit.toml");
        let mut config = fs::read_to_string(&path)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        if let Some(value) = timeout {
            config["ci"]["nix_build"]["timeout_minutes"] = toml_edit::value(value);
        }
        fs::write(&path, config.to_string()).unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let workflow: Value = serde_yaml::from_str(
            &fs::read_to_string(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap(),
        )
        .unwrap();
        for job in ["dependencies", "build"] {
            if let Some(value) = timeout {
                assert_eq!(
                    workflow["jobs"][job]["timeout-minutes"].as_i64(),
                    Some(value)
                );
            } else {
                assert!(workflow["jobs"][job]["timeout-minutes"].is_null());
            }
        }
    }
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

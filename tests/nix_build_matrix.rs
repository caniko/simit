use std::fs;
use std::process::{Command, Output};

use serde_yaml::Value;
use tempfile::TempDir;

mod common;

fn project(options: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("flake.nix"),
        "{ outputs = { self }: {}; }\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        format!(
            "[ci]\nplatform = \"github\"\nprovider = \"actions\"\nruntime = \"nix\"\nrunner = \"ubuntu-24.04\"\nnix_builds = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]\n{options}\n"
        ),
    )
    .unwrap();
    temp
}

fn generate(temp: &TempDir, extra: &[&str]) -> Output {
    common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .args(extra)
        .output()
        .unwrap()
}

fn workflow(temp: &TempDir) -> Value {
    serde_yaml::from_str(
        &fs::read_to_string(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap(),
    )
    .unwrap()
}

#[test]
fn grouped_installables_share_a_runner_with_complete_per_output_evidence() {
    let temp = project(
        "[ci.nix_build]\nonly = true\ncapture_results = true\nseparate_events = true\nmax_parallel = 4\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]",
    );
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    let value = workflow(&temp);
    let job = &value["jobs"]["build"];
    assert!(job["timeout-minutes"].is_null());
    assert_eq!(job["strategy"]["max-parallel"], 4);
    assert_eq!(job["strategy"]["matrix"]["include"][0]["group"], "network");
    let steps = job["steps"].as_sequence().unwrap();
    let checkout = steps.iter().find(|s| s["name"] == "Checkout").unwrap();
    assert_eq!(
        checkout["with"]["ref"],
        "${{ github.event.pull_request.head.sha || github.sha }}"
    );
    assert_eq!(
        checkout["with"]["repository"],
        "${{ github.event.pull_request.head.repo.full_name || github.repository }}"
    );
    assert_eq!(checkout["with"]["persist-credentials"], false);
    assert!(
        value["concurrency"]["group"]
            .as_str()
            .unwrap()
            .contains("github.event_name")
    );
    assert!(
        value["concurrency"]["group"]
            .as_str()
            .unwrap()
            .contains("head.repo.full_name")
    );
    let build = steps
        .iter()
        .find(|s| s["name"] == "Build Nix group")
        .unwrap();
    assert_eq!(build["shell"], "bash");
    let setup = steps
        .iter()
        .position(|s| s["name"] == "Provision grouped-build Rust engine")
        .unwrap();
    assert_eq!(
        steps[setup]["env"]["SIMIT_CI_ENGINE_REVISION"],
        "${{ vars.SIMIT_CI_ENGINE_REVISION }}"
    );
    assert!(
        steps[setup]["run"]
            .as_str()
            .unwrap()
            .contains("github:caniko/simit/$SIMIT_CI_ENGINE_REVISION#default")
    );
    assert!(!steps[setup]["run"].as_str().unwrap().contains("cargo run"));
    assert!(!steps[setup]["run"].as_str().unwrap().contains("sha256sum"));
    assert!(
        steps[setup]["run"]
            .as_str()
            .unwrap()
            .contains("nix hash file")
    );
    assert!(
        setup
            < steps
                .iter()
                .position(|s| s["name"] == "Build Nix group")
                .unwrap()
    );
    assert!(
        build["run"]
            .as_str()
            .unwrap()
            .contains("\"$SIMIT_CI_ENGINE\" ci nix-group")
    );
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert!(generate(&temp, &[]).status.success());
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
}

#[test]
fn unsupported_intel_macos_grouped_runners_reject_before_writes() {
    for runner in [
        "macos-13",
        "macos-15-intel",
        "macos-14-large",
        "macos-15-large",
        "macos-26-intel",
    ] {
        let temp = project(
            "[ci.nix_build]\nonly = true\ncapture_results = true\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]",
        );
        let before = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
        fs::write(
            temp.path().join("simit.toml"),
            before.replace("ubuntu-24.04", runner),
        )
        .unwrap();
        let configured = fs::read(temp.path().join("simit.toml")).unwrap();
        let output = generate(&temp, &[]);
        assert!(!output.status.success(), "{runner}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unsupported grouped engine runner"),
            "{output:?}"
        );
        assert_eq!(
            fs::read(temp.path().join("simit.toml")).unwrap(),
            configured
        );
        assert!(!temp.path().join(".github").exists());
    }
}

#[test]
fn invalid_group_coverage_rejects_before_any_workflow_or_config_write() {
    for groups in [
        "network = [\".#checks.x86_64-linux.client\"]",
        "network = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\", \".#unknown\"]",
        "network = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]\nother = [\".#checks.x86_64-linux.client\"]",
        "network = []",
    ] {
        let temp = project(&format!(
            "[ci.nix_build]\nonly = true\ncapture_results = true\n[ci.nix_build.groups]\n{groups}"
        ));
        let before = fs::read(temp.path().join("simit.toml")).unwrap();
        let result = generate(&temp, &[]);
        assert!(!result.status.success(), "{groups}: {result:?}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("groups"),
            "{result:?}"
        );
        assert_eq!(before, fs::read(temp.path().join("simit.toml")).unwrap());
        assert!(!temp.path().join(".github/workflows").exists());
    }
}

#[cfg(unix)]
#[test]
fn generated_group_invocation_and_fallback_execute_the_packaged_rust_cli() {
    generated_group_fixture(None);
}

#[cfg(unix)]
fn generated_group_fixture(hook_size: Option<usize>) {
    let success = "test -s \"$SIMIT_NIX_BUILD_RESULTS/result.json\" && test -s \"$SIMIT_NIX_BUILD_RESULTS/build.log\" && printf '%s' \"$INSTALLABLE\" > \"$SIMIT_NIX_BUILD_RESULTS/hook-member\"";
    let diagnostic = "printf '%s' \"$INSTALLABLE\" > \"$SIMIT_NIX_BUILD_RESULTS/hook-diagnostic\"";
    let pad = |script: &str| match hook_size {
        Some(size) => {
            let body = format!("\ntest \"$0\" = bash; test \"$#\" = 0; {script}");
            let hook = format!("#{}{body}", "x".repeat(size - body.len() - 1));
            assert_eq!(hook.len(), size);
            hook
        }
        None => script.to_owned(),
    };
    let temp = project(&format!(
        "[ci.nix_build]\nonly = true\ncapture_results = true\ncheck_systems = [\"x86_64-linux\"]\nfresh_installables = [\".#checks.x86_64-linux.client\"]\npost_build = [{}]\npost_build_always = [{}]\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]",
        serde_json::to_string(&pad(success)).unwrap(),
        serde_json::to_string(&pad(diagnostic)).unwrap()
    ));
    assert!(generate(&temp, &[]).status.success());
    let value = workflow(&temp);
    let steps = value["jobs"]["build"]["steps"].as_sequence().unwrap();
    let build = steps
        .iter()
        .find(|s| s["name"] == "Build Nix group")
        .unwrap();
    let source = build["run"].as_str().unwrap();
    let diagnostics = steps
        .iter()
        .find(|s| s["name"] == "Retain uncollected group diagnostics")
        .unwrap();
    assert_eq!(diagnostics["if"], "${{ !cancelled() }}");
    let fallback = diagnostics["run"].as_str().unwrap();
    let source_file = temp.path().join("build-step.sh");
    let fallback_file = temp.path().join("fallback-step.sh");
    fs::write(&source_file, source).unwrap();
    fs::write(&fallback_file, fallback).unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    std::os::unix::fs::symlink(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/group-nix.sh"),
        bin.join("nix"),
    )
    .unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        assert!(
            Command::new("git")
                .current_dir(temp.path())
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .status()
                .unwrap()
                .success()
        );
    }
    let head = String::from_utf8(
        Command::new("git")
            .current_dir(temp.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    for (case, setup_failed) in [
        ("success", false),
        ("fail", false),
        ("missing", false),
        ("success", true),
    ] {
        let root = temp.path().join(format!("results-{case}-{setup_failed}"));
        fs::create_dir(&root).unwrap();
        let mut command = Command::new("bash");
        command
            .current_dir(temp.path())
            .args(["-e", "-o", "pipefail"]);
        if setup_failed {
            command.args(["-c", "exit 1"]);
        } else {
            command.arg(&source_file);
        }
        command
            .env("SIMIT_CI_ENGINE", env!("CARGO_BIN_EXE_simit"))
            .env(
                "INSTALLABLES",
                value["jobs"]["build"]["strategy"]["matrix"]["include"][0]["installables"]
                    .as_str()
                    .unwrap(),
            )
            .env("SIMIT_NIX_BUILD_RESULTS", &root)
            .env("SIMIT_NIX_GROUP", "network")
            .env("GITHUB_SHA", head.trim())
            .env("GITHUB_EVENT_NAME", "push")
            .env("GITHUB_RUN_ID", "123")
            .env("GITHUB_RUN_ATTEMPT", "1")
            .env("GROUP_CALLS", root.join("calls"))
            .env("GROUP_CASE", case)
            .env(
                "PATH",
                std::env::join_paths(
                    std::iter::once(bin.clone())
                        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                )
                .unwrap(),
            );
        for (key, value) in build["env"].as_mapping().unwrap() {
            if !value.as_str().unwrap().contains("${{") {
                command.env(key.as_str().unwrap(), value.as_str().unwrap());
            }
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.success(),
            case == "success" && !setup_failed,
            "{output:?}"
        );
        for _ in 0..2 {
            let result = Command::new("bash")
                .current_dir(temp.path())
                .args(["-e", "-o", "pipefail"])
                .arg(&fallback_file)
                .env("SIMIT_NIX_BUILD_RESULTS", &root)
                .env("SIMIT_NIX_GROUP", "network")
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
        }
        for index in 0..2 {
            assert!(root.join(format!("{index}/hook-diagnostic")).exists());
            assert_eq!(
                root.join(format!("{index}/fallback-diagnostic-0.status"))
                    .exists(),
                setup_failed || case == "missing"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn generated_large_hooks_cross_file_boundaries_in_engine_and_fallback() {
    for size in [128 * 1024 + 1, 1024 * 1024] {
        generated_group_fixture(Some(size));
    }
}

#[cfg(unix)]
#[test]
fn engine_bootstrap_never_sources_producer_shell_startup_or_nix_configuration() {
    let temp = project(
        "[ci.nix_build]\nonly = true\ncapture_results = true\nextra_setup = [\"test -n \\\"$TEST_BINDING\\\"\"]\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]\n[ci.nix_build.extra_env]\nBASH_ENV = \"./candidate-startup.sh\"\nENV = \"./candidate-startup.sh\"\nNIX_CONFIG = \"include ./candidate-nix.conf\"\nTEST_BINDING = \"producer-value\"",
    );
    assert!(generate(&temp, &[]).status.success());
    let value = workflow(&temp);
    let job = &value["jobs"]["build"];
    assert!(job["env"]["BASH_ENV"].is_null());
    assert!(job["env"]["TEST_BINDING"].is_null());
    let steps = job["steps"].as_sequence().unwrap();
    for name in ["Project setup", "Build Nix group"] {
        assert_eq!(
            steps.iter().find(|s| s["name"] == name).unwrap()["env"]["BASH_ENV"],
            "./candidate-startup.sh"
        );
    }
    let step = steps
        .iter()
        .find(|s| s["name"] == "Provision grouped-build Rust engine")
        .unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    std::os::unix::fs::symlink(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/group-nix.sh"),
        bin.join("nix"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/group-startup.sh"),
        temp.path().join("candidate-startup.sh"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/group-platform.sh"),
        bin.join("uname"),
    )
    .unwrap();
    let marker = temp.path().join("candidate-marker");
    let observation = temp.path().join("bootstrap-observation");
    for (pin, os, arch, admitted) in [
        ("", "Linux", "x86_64", false),
        ("trunk", "Linux", "x86_64", false),
        (&*"a".repeat(40), "Linux", "x86_64", true),
        (&*"a".repeat(40), "Darwin", "arm64", true),
        (&*"a".repeat(40), "Darwin", "x86_64", false),
    ] {
        if observation.exists() {
            fs::remove_file(&observation).unwrap();
        }
        let mut command = Command::new("bash");
        command
            .current_dir(temp.path())
            .args([
                "--noprofile",
                "--norc",
                "-e",
                "-o",
                "pipefail",
                "-c",
                step["run"].as_str().unwrap(),
            ])
            .env("GROUP_CANDIDATE_MARKER", &marker)
            .env("GROUP_BOOTSTRAP_OBSERVATION", &observation)
            .env("GROUP_CALLS", temp.path().join("bootstrap-calls"))
            .env("GROUP_CASE", "bootstrap")
            .env("GROUP_OS", os)
            .env("GROUP_ARCH", arch)
            .env(
                "PATH",
                std::env::join_paths(
                    std::iter::once(bin.clone())
                        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                )
                .unwrap(),
            );
        for (key, entry) in job["env"]
            .as_mapping()
            .unwrap()
            .iter()
            .chain(step["env"].as_mapping().unwrap())
        {
            command.env(key.as_str().unwrap(), entry.as_str().unwrap());
        }
        let output = command
            .env("SIMIT_CI_ENGINE_REVISION", pin)
            .output()
            .unwrap();
        // Controlled Nix intentionally returns an invalid store path; bootstrap cannot enroll it.
        assert!(!output.status.success(), "{output:?}");
        assert!(!marker.exists());
        assert_eq!(observation.exists(), admitted, "{output:?}");
    }
}

#[test]
fn grouped_configuration_cannot_override_engine_or_generated_execution_inputs() {
    for key in [
        "SIMIT_CI_ENGINE",
        "SIMIT_CI_ENGINE_REVISION",
        "SIMIT_NIX_PLAN",
        "INSTALLABLES",
    ] {
        let temp = project(&format!(
            "[ci.nix_build]\nonly = true\ncapture_results = true\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]\n[ci.nix_build.extra_env]\n{key} = \"hostile\""
        ));
        let output = generate(&temp, &[]);
        assert!(!output.status.success(), "{key}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("reserved runtime variable"));
        assert!(!temp.path().join(".github/workflows").exists());
    }
}

#[test]
fn independent_rust_evaluation_and_realization_lanes_preserve_all_steps() {
    let temp = project(
        "independent_lanes = true\nwith_docs = true\nall_features = true\nextra_setup = [\"cargo-installed-tool\"]\n[[ci.required_gates]]\nid = \"contract\"\nrun = \"nix develop -c cargo test\"",
    );
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"client\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/lib.rs"), "pub fn client() {}\n").unwrap();
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_yaml::from_str(
        &fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap(),
    )
    .unwrap();
    for name in ["rust", "evaluation", "realization"] {
        assert!(value["jobs"][name]["steps"].is_sequence(), "{value:?}");
        assert!(value["jobs"][name]["needs"].is_null());
        let steps = value["jobs"][name]["steps"].as_sequence().unwrap();
        let cargo_path = steps
            .iter()
            .position(|s| s["name"] == "Add cargo bin to PATH")
            .unwrap();
        let setup = steps
            .iter()
            .position(|s| s["name"] == "Project setup")
            .unwrap();
        assert!(cargo_path < setup);
    }
    let gate = value["jobs"]["gate-contract"]["steps"]
        .as_sequence()
        .unwrap();
    let checkout = gate.iter().find(|s| s["name"] == "Checkout").unwrap();
    assert_eq!(
        checkout["with"]["ref"],
        "${{ github.event.pull_request.head.sha || github.sha }}"
    );
    assert_eq!(checkout["with"]["persist-credentials"], false);
    assert!(
        value["jobs"]["evaluation"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|s| s["run"] == "nix flake check --no-build --all-systems --no-update-lock-file")
    );
    let rust = value["jobs"]["rust"]["steps"].as_sequence().unwrap();
    assert!(
        rust.iter()
            .any(|s| s["run"] == "nix develop -c cargo test --all-features")
    );
    assert!(
        rust.iter()
            .any(|s| s["run"] == "nix develop .#docs -c cargo doc --no-deps --all-features")
    );
    assert!(rust.iter().any(|s| s["name"] == "Clippy all features"));
    assert!(!rust.iter().any(|s| s["name"] == "Check flake"));
    assert_eq!(value["on"]["push"]["branches"][0], "**");
    assert!(
        value["on"]
            .as_mapping()
            .unwrap()
            .contains_key(Value::String("pull_request".into()))
    );
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
}

#[test]
fn independent_lanes_can_retire_realization_without_retiring_evaluation() {
    let temp = project(
        "independent_lanes = true\nnix_flake_check = false\ncheck_command = \"cargo test --locked\"",
    );
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"client\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/lib.rs"), "pub fn client() {}\n").unwrap();
    assert!(generate(&temp, &[]).status.success());
    let value: Value = serde_yaml::from_str(
        &fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap(),
    )
    .unwrap();
    assert!(value["jobs"]["realization"].is_null());
    assert!(value["jobs"]["evaluation"]["steps"].is_sequence());
    assert!(
        value["jobs"]["rust"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|s| s["run"]
                .as_str()
                .is_some_and(|run| run.trim_end() == "nix develop -c sh -c 'cargo test --locked'"))
    );
}

#[test]
fn independent_lanes_reject_configured_step_runners_before_normalization() {
    let temp = project("independent_lanes = true\nstep_runners = { test = \"ubuntu-24.04\" }");
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"client\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/lib.rs"), "pub fn client() {}\n").unwrap();
    let before = fs::read(temp.path().join("simit.toml")).unwrap();
    let output = generate(&temp, &[]);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("step_runners"),
        "{output:?}"
    );
    assert_eq!(fs::read(temp.path().join("simit.toml")).unwrap(), before);
    assert!(!temp.path().join(".github/workflows").exists());
}

#[test]
fn independent_lane_audits_reject_step_runners_without_writes() {
    let temp = project("independent_lanes = true");
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"client\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/lib.rs"), "pub fn client() {}\n").unwrap();
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
    let workflows = fs::read_dir(temp.path().join(".github/workflows"))
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    let path = temp.path().join("simit.toml");
    let mut config = fs::read_to_string(&path)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    config["ci"]["step_runners"]["test"] = toml_edit::value("ubuntu-24.04");
    let before = config.to_string();
    fs::write(&path, &before).unwrap();
    let error = simit::registry::audit_ci(temp.path()).unwrap_err();
    assert!(format!("{error:#}").contains("step_runners"), "{error:#}");
    assert_eq!(
        simit::registry::detect_feature_status(temp.path())["ci"],
        simit::registry::FeatureStatus::Drift
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    for (path, bytes) in workflows {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn effective_backend_overrides_cannot_silently_drop_group_admission() {
    let temp = project(
        "[ci.nix_build]\ncapture_results = true\ncheck_systems = [\"x86_64-linux\"]\n[ci.nix_build.groups]\nall = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]",
    );
    let config = temp.path().join("simit.toml");
    let before = fs::read_to_string(&config)
        .unwrap()
        .replace("platform = \"github\"\n", "")
        .replace("provider = \"actions\"\n", "");
    fs::write(&config, &before).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"client\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/lib.rs"), "pub fn client() {}\n").unwrap();
    let output = generate(&temp, &["--platform", "forgejo", "--ci-provider", "crow"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("effective GitHub Actions backend"),
        "{output:?}"
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), before);
    assert!(!temp.path().join(".crow").exists());
    assert!(!temp.path().join(".github/workflows").exists());
}

#[test]
fn unsupported_lane_backends_reject_generation_and_existing_workflow_audits() {
    for (platform, provider) in [("forgejo", "crow"), ("gitlab", "actions")] {
        let temp = project("");
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"client\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/lib.rs"), "pub fn client() {}\n").unwrap();
        let config = temp.path().join("simit.toml");
        let supported = format!(
            "[ci]\nplatform = \"{platform}\"\nprovider = \"{provider}\"\nruntime = \"nix\"\nrunner = \"codefloe-global\"\n"
        );
        fs::write(&config, &supported).unwrap();
        assert!(generate(&temp, &[]).status.success());
        let changed = format!("{supported}independent_lanes = true\n");
        fs::write(&config, &changed).unwrap();
        let error = simit::registry::audit_ci(temp.path()).unwrap_err();
        assert!(
            format!("{error:#}").contains("effective GitHub Actions backend"),
            "{error:#}"
        );
        let output = generate(&temp, &[]);
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("independent_lanes"),
            "{output:?}"
        );
        assert_eq!(fs::read_to_string(&config).unwrap(), changed);
    }
    let temp = project("independent_lanes = true");
    fs::write(
        temp.path().join("pyproject.toml"),
        "[project]\nname = \"client\"\nversion = \"0.1.0\"\nrequires-python = \">=3.12\"\n",
    )
    .unwrap();
    fs::write(temp.path().join("uv.lock"), "version = 1\n").unwrap();
    let before = fs::read(temp.path().join("simit.toml")).unwrap();
    let output = generate(&temp, &["--platform", "forgejo", "--ci-provider", "crow"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("independent_lanes"),
        "{output:?}"
    );
    assert_eq!(fs::read(temp.path().join("simit.toml")).unwrap(), before);
    assert!(!temp.path().join(".crow").exists());
}

#[test]
fn independent_nix_producer_events_survive_regeneration_without_cancelling_each_other() {
    let temp =
        project("[ci.nix_build]\nonly = true\ncapture_results = true\nseparate_events = true");
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let value = workflow(&temp);
    assert_eq!(
        value["concurrency"]["group"],
        "${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.head_ref || github.ref_name }}"
    );
    assert_eq!(value["concurrency"]["cancel-in-progress"], true);
    assert!(
        fs::read_to_string(temp.path().join("simit.toml"))
            .unwrap()
            .contains("separate_events = true")
    );
    let original = fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap();
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert!(generate(&temp, &[]).status.success());
    assert_eq!(
        original,
        fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap()
    );
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
}

#[test]
fn nix_matrix_qualifies_branch_pushes_and_pull_requests_without_tag_releases() {
    for options in ["", "[ci.nix_build]\nonly = true\ncapture_results = true"] {
        let temp = project(options);
        let output = generate(&temp, &[]);
        assert!(output.status.success(), "{output:?}");
        let original = fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap();
        let value = workflow(&temp);
        let events = value["on"].as_mapping().unwrap();
        assert!(events.contains_key("pull_request"));
        assert!(events.contains_key("workflow_dispatch"));
        assert_eq!(value["on"]["push"]["branches"][0], "**");
        assert!(value["on"]["push"]["tags-ignore"].is_null());
        assert!(value["on"]["push"]["tags"].is_null());
        assert!(generate(&temp, &["--check", "--diff"]).status.success());
        assert!(generate(&temp, &[]).status.success());
        assert_eq!(
            original,
            fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap()
        );
    }
}

#[test]
fn explicit_nix_matrix_tag_patterns_preserve_push_coverage_after_regeneration() {
    let temp = project(
        "[ci.nix_build]\nonly = true\nseparate_events = true\npush_tags = [\"**\", \"!private-*\"]",
    );
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let value = workflow(&temp);
    assert_eq!(value["on"]["push"]["branches"][0], "**");
    assert_eq!(value["on"]["push"]["tags"][0], "**");
    assert_eq!(value["on"]["push"]["tags"][1], "!private-*");
    assert!(value["on"]["push"]["tags-ignore"].is_null());
    assert!(
        value["on"]
            .as_mapping()
            .unwrap()
            .contains_key("pull_request")
    );
    let original = fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap();
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert!(generate(&temp, &[]).status.success());
    assert_eq!(
        original,
        fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap()
    );
    let config: toml_edit::DocumentMut = fs::read_to_string(temp.path().join("simit.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let tags = config["ci"]["nix_build"]["push_tags"].as_array().unwrap();
    assert_eq!(tags.get(0).unwrap().as_str(), Some("**"));
    assert_eq!(tags.get(1).unwrap().as_str(), Some("!private-*"));
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
}

#[test]
fn exact_nix_matrix_keeps_scoped_setup_limits_and_receipts_after_regeneration() {
    let temp = project(
        r#"[ci.nix_build]
only = true
timeout_minutes = 90
max_parallel = 1
max_jobs = 1
cores = 2
kvm = true
capture_results = true
artifact_retention_days = 14
artifact_paths = ["${{ runner.temp }}/receipts/**"]
extra_setup = ["python3 ci/prepare.py"]
post_build = ["python3 ci/retain-receipts.py"]
required_secrets = ["FLAKE_SSH_KEY"]
extra_env = { FLAKE_SSH_KEY = "${{ secrets.FLAKE_SSH_KEY }}" }
"#,
    );
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    assert!(!temp.path().join(".github/workflows/ci.yaml").exists());
    let original = fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap();
    let value = workflow(&temp);
    assert_eq!(value["permissions"]["contents"], "read");
    assert_eq!(
        value["concurrency"]["group"],
        "${{ github.workflow }}-${{ github.event.pull_request.head.repo.full_name || github.repository }}-${{ github.event.pull_request.head.ref || github.ref_name }}"
    );
    let job = &value["jobs"]["build"];
    assert_eq!(job["timeout-minutes"], 90);
    assert_eq!(job["strategy"]["max-parallel"], 1);
    assert_eq!(job["env"]["FLAKE_SSH_KEY"], "${{ secrets.FLAKE_SSH_KEY }}");
    // runner.temp is unavailable in job-level env; the preparation step must
    // export the runtime path through GITHUB_ENV for later steps instead.
    assert!(job["env"]["SIMIT_NIX_BUILD_RESULTS"].is_null());
    let steps = job["steps"].as_sequence().unwrap();
    let scripts = steps
        .iter()
        .filter_map(|step| step["run"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(scripts.contains("/dev/kvm"));
    assert!(scripts.contains("--max-jobs 1 --cores 2"));
    assert!(scripts.contains("--no-update-lock-file"));
    assert!(scripts.contains("--json"));
    assert!(scripts.contains("GITHUB_ENV"));
    assert!(!scripts.contains("nix flake check"));
    assert!(!scripts.contains("${{ secrets."));
    let upload = steps
        .iter()
        .find(|step| step["name"] == "Upload Nix build evidence")
        .unwrap();
    assert_eq!(upload["if"], "always()");
    assert_eq!(upload["with"]["retention-days"], 14);
    assert!(
        upload["with"]["path"]
            .as_str()
            .unwrap()
            .contains("receipts/**")
    );
    assert_eq!(
        upload["uses"],
        simit::render::ci::github_action_ref("actions/upload-artifact", "v7.0.2")
            .split(" #")
            .next()
            .unwrap()
    );
    assert_eq!(
        steps[0]["uses"],
        simit::render::ci::github_action_ref("actions/checkout", "v7.0.1")
            .split(" #")
            .next()
            .unwrap()
    );
    let output = generate(&temp, &["--check", "--diff"]);
    assert!(output.status.success(), "{output:?}");
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        original,
        fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap()
    );
    let audit = simit::registry::audit_ci(temp.path()).unwrap();
    assert_eq!(
        audit.status,
        simit::registry::FeatureStatus::Managed,
        "{audit:?}"
    );
}

#[test]
fn scoped_nix_matrix_isolates_forks_and_coalesces_same_repository_push_and_pr() {
    for options in [
        "[ci.nix_build]\nonly = true",
        "[ci.nix_build]\ncapture_results = true",
        "[ci.nix_build]\nmax_jobs = 1",
        "[ci.nix_build]\nonly = true\npush_tags = [\"**\"]",
    ] {
        let temp = project(options);
        assert!(generate(&temp, &[]).status.success());
        let value = workflow(&temp);
        let group = value["concurrency"]["group"].as_str().unwrap();
        let resolve = |head_repository: Option<&str>,
                       head_branch: Option<&str>,
                       repository: &str,
                       branch: &str,
                       ref_type: &str| {
            let value = group
                .replace("${{ github.workflow }}", "Nix installable builds")
                .replace(
                    "${{ github.event.pull_request.head.repo.full_name || github.repository }}",
                    head_repository.unwrap_or(repository),
                )
                .replace(
                    "${{ github.event.pull_request.head.ref || github.ref_name }}",
                    head_branch.unwrap_or(branch),
                )
                .replace(
                    "${{ github.ref_type == 'tag' && github.ref || format('refs/heads/{0}', github.event.pull_request.head.ref || github.ref_name) }}",
                    &if ref_type == "tag" {
                        format!("refs/tags/{branch}")
                    } else {
                        format!("refs/heads/{}", head_branch.unwrap_or(branch))
                    },
                );
            assert!(
                !value.contains("${{"),
                "unrecognized concurrency input: {value}"
            );
            value
        };
        let push = resolve(None, None, "owner/project", "main", "branch");
        let same_repository_pr = resolve(
            Some("owner/project"),
            Some("main"),
            "owner/project",
            "1/merge",
            "branch",
        );
        let first_fork = resolve(
            Some("alice/project"),
            Some("main"),
            "owner/project",
            "2/merge",
            "branch",
        );
        let second_fork = resolve(
            Some("bob/project"),
            Some("main"),
            "owner/project",
            "3/merge",
            "branch",
        );
        assert_eq!(push, same_repository_pr);
        assert_ne!(first_fork, second_fork);
        assert_ne!(push, first_fork);
        assert_ne!(
            push,
            resolve(None, None, "owner/project", "another-branch", "branch")
        );
        if options.contains("push_tags") {
            let tag = resolve(None, None, "owner/project", "main", "tag");
            assert_ne!(tag, push);
            assert_ne!(tag, same_repository_pr);
            assert_ne!(
                tag,
                resolve(None, None, "owner/project", "refs/tags/main", "branch")
            );
            assert_eq!(tag, resolve(None, None, "owner/project", "main", "tag"));
        }
        assert_eq!(value["concurrency"]["cancel-in-progress"], true);
        assert!(generate(&temp, &["--check", "--diff"]).status.success());
    }
}

#[test]
fn evidence_capture_rejects_effective_forgejo_before_action_pin_lookup_or_writes() {
    for args in [vec![], vec!["--platform", "forgejo"]] {
        let temp = project("[ci.nix_build]\nonly = true\ncapture_results = true");
        let config_path = temp.path().join("simit.toml");
        let config = fs::read_to_string(&config_path)
            .unwrap()
            .replace("platform = \"github\"\n", "");
        fs::write(&config_path, &config).unwrap();
        let output = generate(&temp, &args);
        assert!(!output.status.success(), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("[ci.nix_build] options require GitHub Actions"),
            "{stderr}"
        );
        assert!(!stderr.contains("panicked"));
        assert_eq!(fs::read_to_string(&config_path).unwrap(), config);
        assert!(!temp.path().join(".forgejo/workflows").exists());
        assert!(!temp.path().join(".github/workflows").exists());
    }
}

#[test]
fn rust_flake_can_select_exact_nix_gates_from_its_workspace_root() {
    let temp = project("[ci.nix_build]\nonly = true");
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"native-contract\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    assert!(!temp.path().join(".github/workflows/ci.yaml").exists());
    let original = fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap();
    let output = common::simit()
        .current_dir(temp.path().join("src"))
        .args(["init", "ci", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        original,
        fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap()
    );
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
    for extra in [vec!["--runtime", "cargo"], vec!["--publish-crates=true"]] {
        let output = generate(&temp, &extra);
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(
            original,
            fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap()
        );
    }
}

#[test]
fn diagnostics_run_after_failed_builds_before_upload_without_relaxing_the_gate() {
    let temp = project(
        "[ci.nix_build]\nonly = true\ncapture_results = true\npost_build = [\"echo qualified\"]\npost_build_always = [\"python3 ci/retain-diagnostics.py\"]",
    );
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let value = workflow(&temp);
    let steps = value["jobs"]["build"]["steps"].as_sequence().unwrap();
    let build = steps
        .iter()
        .position(|s| s["name"] == "Build ${{ matrix.installable }}")
        .unwrap();
    let success = steps
        .iter()
        .position(|s| s["name"] == "Retain Nix results 1")
        .unwrap();
    let diagnostics = steps
        .iter()
        .position(|s| s["name"] == "Retain Nix diagnostics 1")
        .unwrap();
    let upload = steps
        .iter()
        .position(|s| s["name"] == "Upload Nix build evidence")
        .unwrap();
    assert!(build < success && success < diagnostics && diagnostics < upload);
    assert!(steps[success]["if"].is_null());
    assert_eq!(steps[diagnostics]["if"], "${{ !cancelled() }}");
    assert_eq!(steps[upload]["if"], "always()");
    assert!(
        steps[build]["run"]
            .as_str()
            .unwrap()
            .contains("exit \"$status\"")
    );
    assert!(steps.iter().all(|s| s["continue-on-error"].is_null()));
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert!(
        fs::read_to_string(temp.path().join("simit.toml"))
            .unwrap()
            .contains("post_build_always")
    );

    let invalid = project("[ci.nix_build]\npost_build_always = [\"echo diagnostics\"]");
    let output = generate(&invalid, &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("capture_results"));
    assert!(
        !invalid
            .path()
            .join(".github/workflows/nix-builds.yaml")
            .exists()
    );
}

#[test]
fn python_flake_can_select_exact_nix_gates_without_language_ci() {
    let temp = project("[ci.nix_build]\nonly = true");
    fs::write(
        temp.path().join("pyproject.toml"),
        "[project]\nname = \"native-contract\"\ndynamic = [\"version\"]\n",
    )
    .unwrap();
    fs::write(temp.path().join("uv.lock"), "version = 1\n").unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    assert!(!temp.path().join(".github/workflows/ci.yaml").exists());
    let original = fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap();
    let output = common::simit()
        .current_dir(temp.path().join("src"))
        .args(["init", "ci", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
    for extra in [vec!["--runtime", "cargo"], vec!["--with-pypi-publish=true"]] {
        let output = generate(&temp, &extra);
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(
            original,
            fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap()
        );
    }
}

#[test]
fn scoped_matrix_credentials_do_not_reach_primary_ci() {
    let temp = project(
        r#"[ci.nix_build]
extra_setup = ["python3 ci/prepare.py"]
required_secrets = ["FLAKE_SSH_KEY"]
extra_env = { FLAKE_SSH_KEY = "${{ secrets.FLAKE_SSH_KEY }}" }
"#,
    );
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let primary = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
    assert!(!primary.contains("FLAKE_SSH_KEY"));
    assert!(!primary.contains("ci/prepare.py"));
    assert_eq!(
        workflow(&temp)["jobs"]["build"]["env"]["FLAKE_SSH_KEY"],
        "${{ secrets.FLAKE_SSH_KEY }}"
    );
}

#[test]
fn nix_only_rejects_effective_language_options_before_changing_workflows() {
    for option in [
        "with_nix_cargo_cache",
        "with_nextest",
        "with_msrv",
        "with_audit",
        "with_deny",
        "with_docs",
        "with_artifacts",
        "with_pypi_publish",
        "publish_crates",
    ] {
        for from_config in [false, true] {
            let temp = project("[ci.nix_build]\nonly = true");
            assert!(generate(&temp, &[]).status.success());
            let path = temp.path().join(".github/workflows/nix-builds.yaml");
            let original = fs::read(&path).unwrap();
            let config = temp.path().join("simit.toml");
            let mut arguments = Vec::new();
            if from_config {
                let content = fs::read_to_string(&config)
                    .unwrap()
                    .replace("[ci]\n", &format!("[ci]\n{option} = true\n"));
                fs::write(&config, content).unwrap();
            } else {
                arguments.push(format!("--{}=true", option.replace('_', "-")));
            }
            let original_config = fs::read(&config).unwrap();
            if from_config {
                let error = simit::registry::audit_ci(temp.path()).unwrap_err();
                assert!(format!("{error:#}").contains(option), "{error:#}");
            }
            for mode in [vec![], vec!["--check", "--diff"]] {
                let mut args = arguments.iter().map(String::as_str).collect::<Vec<_>>();
                args.extend(mode);
                let output = generate(&temp, &args);
                assert!(
                    !output.status.success(),
                    "silently accepted {option} (config={from_config})"
                );
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains(option),
                    "{output:?}"
                );
                assert_eq!(fs::read(&path).unwrap(), original);
                assert_eq!(fs::read(&config).unwrap(), original_config);
                assert!(
                    !temp
                        .path()
                        .join(".github/workflows/release-artifacts.yaml")
                        .exists()
                );
            }
        }
    }
}

#[test]
fn nix_only_persists_explicit_false_overrides_for_language_options() {
    let options = [
        "with_nix_cargo_cache",
        "with_nextest",
        "with_msrv",
        "with_audit",
        "with_deny",
        "with_docs",
        "with_artifacts",
        "with_pypi_publish",
        "publish_crates",
    ];
    let temp = project(&format!(
        "{}\n[ci.nix_build]\nonly = true",
        options
            .iter()
            .map(|option| format!("{option} = true"))
            .collect::<Vec<_>>()
            .join("\n")
    ));
    let arguments = options
        .iter()
        .map(|option| format!("--{}=false", option.replace('_', "-")))
        .collect::<Vec<_>>();
    let args = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    let output = generate(&temp, &args);
    assert!(output.status.success(), "{output:?}");
    let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    for option in options {
        assert!(
            !config.contains(&format!("{option} = true")),
            "did not persist {option} override"
        );
    }
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
}

#[test]
fn rust_workspace_nix_only_rejects_requested_gates_from_member_directories() {
    for option in [
        "with_artifacts",
        "with_audit",
        "with_msrv",
        "publish_crates",
    ] {
        for from_config in [false, true] {
            let temp = project("[ci.nix_build]\nonly = true");
            fs::write(
                temp.path().join("Cargo.toml"),
                "[workspace]\nmembers = [\"member\"]\nresolver = \"2\"\n",
            )
            .unwrap();
            let member = temp.path().join("member");
            fs::create_dir_all(member.join("src")).unwrap();
            fs::write(
                member.join("Cargo.toml"),
                "[package]\nname = \"native-contract\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )
            .unwrap();
            fs::write(member.join("src/main.rs"), "fn main() {}\n").unwrap();
            let output = generate(&temp, &[]);
            assert!(output.status.success(), "{output:?}");
            let workflow_path = temp.path().join(".github/workflows/nix-builds.yaml");
            let original_workflow = fs::read(&workflow_path).unwrap();
            let release_path = temp.path().join(".github/workflows/release-artifacts.yaml");
            fs::write(&release_path, &original_workflow).unwrap();
            let config_path = temp.path().join("simit.toml");
            if from_config {
                let content = fs::read_to_string(&config_path)
                    .unwrap()
                    .replace("[ci]\n", &format!("[ci]\n{option} = true\n"));
                fs::write(&config_path, content).unwrap();
            }
            let original_config = fs::read(&config_path).unwrap();
            let flag = format!("--{}=true", option.replace('_', "-"));
            for check in [false, true] {
                let mut command = common::simit();
                command.current_dir(&member).args(["init", "ci"]);
                if !from_config {
                    command.arg(&flag);
                }
                if check {
                    command.args(["--check", "--diff"]);
                }
                let output = command.output().unwrap();
                assert!(!output.status.success(), "silently accepted {option}");
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains(option),
                    "{output:?}"
                );
                assert_eq!(fs::read(&config_path).unwrap(), original_config);
                assert_eq!(fs::read(&workflow_path).unwrap(), original_workflow);
                assert_eq!(fs::read(&release_path).unwrap(), original_workflow);
            }
            let output = common::simit()
                .current_dir(&member)
                .args(["init", "ci"])
                .arg(format!("--{}=false", option.replace('_', "-")))
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(generate(&temp, &["--check", "--diff"]).status.success());
            assert_eq!(
                simit::registry::audit_ci(temp.path()).unwrap().status,
                simit::registry::FeatureStatus::Managed
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn matrix_secret_preflight_fails_without_exposing_secret_values() {
    let temp = project(
        r#"[ci.nix_build]
only = true
required_secrets = ["FLAKE_SSH_KEY"]
extra_env = { FLAKE_SSH_KEY = "${{ secrets.FLAKE_SSH_KEY }}" }
"#,
    );
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let value = workflow(&temp);
    let steps = value["jobs"]["build"]["steps"].as_sequence().unwrap();
    let script = steps
        .iter()
        .find(|step| step["name"] == "Validate required environment")
        .unwrap()["run"]
        .as_str()
        .unwrap();
    for (key, valid) in [("", false), ("disposable-secret-value", true)] {
        let output = Command::new("bash")
            .args(["-c", script])
            .env("FLAKE_SSH_KEY", key)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), valid, "{output:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("disposable-secret-value"));
    }
}

#[cfg(unix)]
#[test]
fn captured_build_keeps_failure_status_exact_arguments_and_diagnostics() {
    use std::os::unix::fs::PermissionsExt;

    let temp =
        project("[ci.nix_build]\nonly = true\ncapture_results = true\nmax_jobs = 1\ncores = 2");
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let value = workflow(&temp);
    let steps = value["jobs"]["build"]["steps"].as_sequence().unwrap();
    let script = steps
        .iter()
        .find(|step| {
            step["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("Build "))
        })
        .unwrap()["run"]
        .as_str()
        .unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let nix = bin.join("nix");
    fs::write(&nix, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$SIMIT_NIX_BUILD_RESULTS/arguments\"\nprintf '%s\\n' '[{\"drvPath\":\"/nix/store/fixture.drv\",\"outputs\":{\"out\":\"/nix/store/fixture\"}}]'\nprintf '%s\\n' 'fixture build diagnostic' >&2\nexit \"$BUILD_EXIT_CODE\"\n").unwrap();
    fs::set_permissions(&nix, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let installable = ".#checks.client; touch should-not-exist";
    for code in [0, 17] {
        let results = temp.path().join(format!("results-{code}"));
        fs::create_dir(&results).unwrap();
        let output = Command::new("bash")
            .current_dir(temp.path())
            .args(["-c", script])
            .env("PATH", &path)
            .env("INSTALLABLE", installable)
            .env("SIMIT_NIX_BUILD_RESULTS", &results)
            .env("BUILD_EXIT_CODE", code.to_string())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        let arguments = fs::read_to_string(results.join("arguments")).unwrap();
        assert_eq!(arguments.lines().last(), Some(installable));
        assert_eq!(
            fs::read_to_string(results.join("build.log")).unwrap(),
            "fixture build diagnostic\n"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &fs::read(results.join("result.json")).unwrap()
            )
            .unwrap()[0]["outputs"]["out"],
            "/nix/store/fixture"
        );
    }
    assert!(!temp.path().join("should-not-exist").exists());
}

#[test]
fn exact_matrix_replaces_only_previously_managed_primary_workflows() {
    let temp = project("");
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let custom = temp.path().join(".github/workflows/project-owned.yaml");
    fs::write(&custom, "name: Project owned\n").unwrap();
    let config = temp.path().join("simit.toml");
    let content = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("{content}\n[ci.nix_build]\nonly = true\n")).unwrap();
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    assert!(!temp.path().join(".github/workflows/ci.yaml").exists());
    assert_eq!(fs::read_to_string(custom).unwrap(), "name: Project owned\n");
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
}

#[test]
fn direct_nix_matrix_rendering_rejects_invalid_tag_filters() {
    use simit::cli::Platform;
    use simit::config::NixBuildConfig;
    use simit::render::ci::nix_build_matrix_file_with_options;
    use simit::user_config::ResolvedRunner;

    let runner = ResolvedRunner {
        name: None,
        labels: vec!["ubuntu-24.04".into()],
    };
    let render = |patterns: &[&str]| {
        let options = NixBuildConfig {
            push_tags: patterns.iter().map(|pattern| (*pattern).into()).collect(),
            ..NixBuildConfig::default()
        };
        nix_build_matrix_file_with_options(
            Platform::Github,
            &runner,
            &[".#default".into()],
            &[],
            &[],
            &[],
            &options,
        )
    };
    for patterns in [
        vec![""],
        vec!["bad\npattern"],
        vec!["bad\u{2028}pattern"],
        vec!["bad\u{2029}pattern"],
        vec!["bad\u{fffe}pattern"],
        vec!["bad\u{ffff}pattern"],
        vec!["!private-*"],
        vec!["!one", "!two"],
    ] {
        assert!(
            render(&patterns)
                .unwrap_err()
                .to_string()
                .contains("push_tags")
        );
    }
    for patterns in [
        vec![],
        vec!["**", "!private-*"],
        vec![r"\!literal"],
        vec!["发布-α-*"],
    ] {
        assert!(render(&patterns).is_ok(), "{patterns:?}");
    }
}

#[test]
fn matrix_options_reject_invalid_or_ignored_configuration() {
    for (options, expected) in [
        ("[ci.nix_build]\npush_tags = [\"\"]", "push_tags"),
        ("[ci.nix_build]\npush_tags = [\"!private-*\"]", "push_tags"),
        (
            "[ci.nix_build]\npush_tags = [\"!one\", \"!two\"]",
            "push_tags",
        ),
        (
            "[ci.nix_build]\npush_tags = [\"bad\\npattern\"]",
            "push_tags",
        ),
        (
            "[ci.nix_build]\npush_tags = [\"bad\\u2028pattern\"]",
            "push_tags",
        ),
        (
            "[ci.nix_build]\npush_tags = [\"bad\\u2029pattern\"]",
            "push_tags",
        ),
        (
            "[ci.nix_build]\npush_tags = [\"bad\\uFFFEpattern\"]",
            "push_tags",
        ),
        (
            "[ci.nix_build]\npush_tags = [\"bad\\uFFFFpattern\"]",
            "push_tags",
        ),
        ("[ci.nix_build]\ntimeout_minutes = 0", "timeout_minutes"),
        ("[ci.nix_build]\nmax_parallel = 0", "max_parallel"),
        ("[ci.nix_build]\nmax_jobs = 0", "max_jobs"),
        (
            "[ci.nix_build]\nartifact_retention_days = 91",
            "artifact_retention_days",
        ),
        (
            "[ci.nix_build]\nrequired_secrets = [\"BAD-NAME\"]",
            "required_secrets",
        ),
        (
            "[ci.nix_build]\nextra_env = { 'BAD;NAME' = \"value\" }",
            "extra_env",
        ),
        (
            "[ci.nix_build]\nrequired_secrets = [\"FLAKE_SSH_KEY\"]",
            "extra_env",
        ),
        (
            "[ci.nix_build]\npost_build = [\"echo done\"]",
            "capture_results",
        ),
    ] {
        let temp = project(options);
        let output = generate(&temp, &[]);
        assert!(!output.status.success(), "accepted {options}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{output:?}"
        );
        assert!(!temp.path().join(".github").exists());
    }
}

#[test]
fn exact_matrix_rejects_missing_installables_other_backends_and_prebuild() {
    for (replace, replacement, extra, expected) in [
        (
            "nix_builds = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]",
            "nix_builds = []",
            "",
            "nix_builds",
        ),
        (
            "platform = \"github\"",
            "platform = \"forgejo\"",
            "",
            "GitHub Actions",
        ),
        (
            "runtime = \"nix\"",
            "runtime = \"cargo\"",
            "",
            "Nix runtime",
        ),
        (
            "provider = \"actions\"",
            "provider = \"crow\"",
            "",
            "GitHub Actions",
        ),
        (
            "[ci.nix_build]",
            "components = [\"checks\"]\n[ci.nix_build]",
            "",
            "components",
        ),
        (
            "[ci.nix_build]",
            "[ci.nix_build]",
            "\n[prebuild]\nsystem_runners = { x86_64-linux = \"ubuntu-24.04\" }\n",
            "prebuild",
        ),
    ] {
        let temp = project("[ci.nix_build]\nonly = true");
        let config = temp.path().join("simit.toml");
        let content = fs::read_to_string(&config)
            .unwrap()
            .replace(replace, replacement);
        fs::write(config, format!("{content}{extra}")).unwrap();
        let output = generate(&temp, &[]);
        assert!(!output.status.success(), "accepted {replacement}{extra}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{output:?}"
        );
    }
}

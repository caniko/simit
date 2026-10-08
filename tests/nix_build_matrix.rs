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
        "${{ github.workflow }}-${{ github.event.pull_request.head.ref || github.ref_name }}"
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
        simit::render::ci::github_action_ref("actions/upload-artifact", "v7.0.1")
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
fn matrix_options_reject_invalid_or_ignored_configuration() {
    for (options, expected) in [
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

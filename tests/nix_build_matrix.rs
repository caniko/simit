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
    assert!(
        upload["uses"]
            .as_str()
            .unwrap()
            .starts_with("actions/upload-artifact@ea165f8")
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

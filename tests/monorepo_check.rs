#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

use tempfile::TempDir;

#[path = "common/monorepo_caller.rs"]
mod monorepo_caller;

fn fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(
        bin.join("timeout"),
        "#!/bin/sh\nprintf fixture-diagnostics-unavailable >&2\nexit 127\n",
    )
    .unwrap();
    fs::set_permissions(bin.join("timeout"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        r#"
[ci]
platform = "github"
provider = "actions"
runtime = "nix"
[monorepo]
schema_version = 1
[[monorepo.components]]
id = "core"
paths = ["core"]
checks = [
  { id = "pass", run = "printf '%s' \"$GATE_VALUE\"", env = { GATE_VALUE = "gate-owned" } },
  { id = "fail", run = "printf original-failure >&2; exit 7" },
]
"#,
    )
    .unwrap();
    temp
}

fn check(root: &TempDir, id: &str) -> std::process::Output {
    let path = std::env::join_paths(std::iter::once(root.path().join("bin")).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(root.path())
        .args([
            "monorepo",
            "check",
            "--component",
            "core",
            "--check",
            id,
            "--nix-diagnostics",
        ])
        .env("GATE_VALUE", "inherited")
        .env("PATH", path)
        .env("UNRELATED_SECRET", "must-not-be-reported")
        .output()
        .unwrap()
}

#[test]
fn runs_the_declared_gate_and_its_environment() {
    let output = check(&fixture(), "pass");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"gate-owned");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Nix failure diagnostics"));
}

#[test]
fn failure_keeps_original_status_when_diagnostics_are_unavailable() {
    let output = check(&fixture(), "fail");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("original-failure"));
    assert!(stderr.contains("exit status: 7"));
    assert!(stderr.contains("Nix failure diagnostics"));
    assert!(!stderr.contains("must-not-be-reported"));
}

#[test]
fn unknown_gate_fails_before_execution_or_diagnostics() {
    let output = check(&fixture(), "absent");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown check absent"));
    assert!(!stderr.contains("Nix failure diagnostics"));
}

#[test]
#[cfg(target_os = "linux")]
fn successful_diagnostics_cannot_turn_the_failed_gate_green() {
    let root = fixture();
    fs::write(
        root.path().join("bin/timeout"),
        "#!/bin/sh\nprintf 'captured: %s\\n' \"$*\"\n",
    )
    .unwrap();
    let output = check(&root, "fail");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("exit status: 7"));
    assert!(
        stderr.contains("captured: --kill-after=5s 15s sudo -n systemctl show nix-daemon.service")
    );
    assert!(stderr.contains("journalctl --dmesg --since @"));
    assert!(stderr.contains("--lines=150 --no-pager"));
    assert!(!stderr.contains("must-not-be-reported"));
}

#[test]
fn generated_qualification_runs_native_diagnostics_for_every_gate() {
    let root = fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(root.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let yaml: serde_yaml::Value = serde_yaml::from_str(
        &fs::read_to_string(root.path().join(".github/workflows/ci.yaml")).unwrap(),
    )
    .unwrap();
    let steps = yaml["jobs"]["component-core"]["steps"]
        .as_sequence()
        .unwrap();
    // Exercise the installed daemon fix, not the action's moving default.
    for job in ["plan", "component-core"] {
        let installer = yaml["jobs"][job]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| {
                step["uses"]
                    .as_str()
                    .is_some_and(|action| action.starts_with("cachix/install-nix-action@"))
            })
            .unwrap();
        assert_eq!(
            installer["with"]["install_url"].as_str(),
            Some("https://releases.nixos.org/nix/nix-2.35.2/install")
        );
    }
    for id in ["pass", "fail"] {
        let step = steps
            .iter()
            .find(|step| step["name"].as_str() == Some(id))
            .unwrap();
        assert!(step["run"].as_str().unwrap().contains(&format!(
            "simit monorepo check --component core --check {id} --nix-diagnostics"
        )));
        assert_eq!(step["timeout-minutes"].as_u64(), Some(32));
        assert!(step["run"].as_str().unwrap().contains("--deadline"));
    }
    assert_eq!(
        steps
            .iter()
            .find(|step| step["name"].as_str() == Some("pass"))
            .unwrap()["env"]["SIMIT_GATE_VALUE_0"]
            .as_str(),
        Some("gate-owned")
    );
}

#[test]
fn expired_provider_deadline_retains_diagnostics_without_starting_the_gate() {
    let root = environment_fixture("literal");
    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(root.path())
        .args([
            "monorepo",
            "check",
            "--component",
            "core",
            "--check",
            "pass",
            "--deadline",
            "1",
            "--nix-diagnostics",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("timed out"), "{stderr}");
    assert!(stderr.contains("Nix failure diagnostics"), "{stderr}");
    assert!(!root.path().join("executions").exists());
}

#[test]
fn maximum_gate_timeout_reserves_provider_and_job_diagnostic_grace() {
    let root = fixture();
    let config = fs::read_to_string(root.path().join("simit.toml"))
        .unwrap()
        .replace("id = \"fail\",", "id = \"fail\", timeout_minutes = 360,");
    fs::write(root.path().join("simit.toml"), config).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(root.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let yaml: serde_yaml::Value = serde_yaml::from_str(
        &fs::read_to_string(root.path().join(".github/workflows/ci.yaml")).unwrap(),
    )
    .unwrap();
    let job = &yaml["jobs"]["component-core"];
    assert_eq!(job["timeout-minutes"], 360);
    let steps = job["steps"].as_sequence().unwrap();
    assert_eq!(steps[0]["id"], "simit-gate-budget");
    assert!(steps[0]["run"].as_str().unwrap().contains("21480"));
    let gate = steps.iter().find(|step| step["name"] == "fail").unwrap();
    assert_eq!(gate["timeout-minutes"], 360);
    assert!(gate["run"].as_str().unwrap().contains("21480"));
    assert!(
        gate["run"]
            .as_str()
            .unwrap()
            .contains("steps.simit-gate-budget.outputs.deadline")
    );
}

fn environment_fixture(value: &str) -> TempDir {
    let root = fixture();
    let config = fs::read_to_string(root.path().join("simit.toml")).unwrap();
    let config = config.replace("\"gate-owned\"", &toml_edit::Value::from(value).to_string());
    let config = config.replace(
        "printf '%s' \\\"$GATE_VALUE\\\"",
        "printf x >> executions; printf '%s' \\\"$GATE_VALUE\\\" > observed; pwd > working-directory",
    );
    fs::write(root.path().join("simit.toml"), config).unwrap();
    root
}

fn generated_gate(root: &TempDir, id: &str, value: Option<&str>) -> std::process::Output {
    generated_gate_with_deadline(root, id, value, None)
}

fn generated_gate_with_deadline(
    root: &TempDir,
    id: &str,
    value: Option<&str>,
    deadline: Option<u64>,
) -> std::process::Output {
    // Nix provisioning is the external boundary. Execute the unmodified generated
    // caller, forwarding its exact argv to the real candidate Simit executable.
    let bin = root.path().join("bin");
    if !bin.join("nix").exists() {
        fs::write(bin.join("nix"), "#!/bin/sh\nset -eu\ntest \"$1 $2 $3 $4\" = 'develop --print-build-logs .#ci --command'\nshift 4\nexport GATE_VALUE=devShell-override\nexec \"$@\"\n").unwrap();
    }
    if deadline.is_some() {
        let shim = fs::read_to_string(bin.join("nix"))
            .unwrap()
            .replace("exec \"$@\"", "sleep 2\nexec \"$@\"");
        fs::write(bin.join("nix"), shim).unwrap();
    }
    fs::set_permissions(bin.join("nix"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_simit"), bin.join("simit")).unwrap();
    let generated = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(root.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(generated.status.success(), "{generated:?}");
    let yaml: serde_yaml::Value = serde_yaml::from_str(
        &fs::read_to_string(root.path().join(".github/workflows/ci.yaml")).unwrap(),
    )
    .unwrap();
    let step = yaml["jobs"]["component-core"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"].as_str() == Some(id))
        .unwrap();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let mut caller = Command::new("bash");
    let script = deadline.map_or_else(
        || {
            monorepo_caller::script(
                root.path(),
                &yaml,
                "component-core",
                step["run"].as_str().unwrap(),
            )
        },
        |deadline| {
            step["run"].as_str().unwrap().replace(
                "${{ steps.simit-gate-budget.outputs.deadline }}",
                &deadline.to_string(),
            )
        },
    );
    caller
        .current_dir(root.path())
        // GitHub's explicit Bash shell supplies errexit/pipefail, not nounset.
        .args(["-e", "-o", "pipefail", "-c", &script])
        .env("PATH", path)
        .env_remove("GATE_VALUE");
    // Actions supplies the evaluated step environment, including empty values.
    if let Some(environment) = step["env"].as_mapping() {
        for name in environment.keys() {
            let name = name.as_str().unwrap();
            caller.env_remove(name);
            if let Some(value) = value {
                caller.env(name, value);
            }
        }
    }
    if let Some(value) = value {
        caller.env("UNRELATED_SECRET", value);
    }
    caller.output().unwrap()
}

#[test]
fn generated_caller_counts_provisioning_against_the_remaining_job_budget() {
    let root = environment_fixture("literal");
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 1;
    let output = generated_gate_with_deadline(
        &root,
        "pass",
        Some("never-report-this-secret"),
        Some(deadline),
    );
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("timed out"), "{stderr}");
    assert!(stderr.contains("Nix failure diagnostics"), "{stderr}");
    assert!(!stderr.contains("never-report-this-secret"));
    assert!(!root.path().join("executions").exists());
}

#[test]
fn generated_caller_preserves_provider_values_as_data_exactly_once_at_project_root() {
    for (configured, value) in [
        ("${{ secrets.TEST_TOKEN }}", ""),
        ("${{ secrets.TEST_TOKEN }}", "provider-secret"),
        (
            "${{ secrets.TEST_TOKEN }}",
            "'\" $(touch injected); `touch injected` & | $HOME",
        ),
        ("configured-literal", "evaluated-step-value"),
    ] {
        let root = environment_fixture(configured);
        let output = generated_gate(&root, "pass", Some(value));
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            fs::read(root.path().join("observed")).unwrap(),
            value.as_bytes()
        );
        assert_eq!(fs::read(root.path().join("executions")).unwrap(), b"x");
        assert_eq!(
            fs::read_to_string(root.path().join("working-directory"))
                .unwrap()
                .trim(),
            root.path().canonicalize().unwrap().to_str().unwrap()
        );
        assert!(!root.path().join("injected").exists());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn provider_mode_requires_present_values_even_for_configured_literals() {
    for configured in ["literal", "${{ secrets.TEST_TOKEN }}"] {
        let root = environment_fixture(configured);
        let output = generated_gate(&root, "pass", None);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("GATE_VALUE")
                && (stderr.contains("unbound variable")
                    || stderr.contains("missing provider-resolved gate environment variable")),
            "{stderr}"
        );
        assert!(!stderr.contains(configured));
        assert!(!stderr.contains("Nix failure diagnostics"));
        assert!(!root.path().join("executions").exists());
        let direct = Command::new(env!("CARGO_BIN_EXE_simit"))
            .current_dir(root.path())
            .args([
                "monorepo",
                "check",
                "--component",
                "core",
                "--check",
                "pass",
                "--gate-env",
                "provider-resolved",
            ])
            .env_remove("GATE_VALUE")
            .output()
            .unwrap();
        assert!(!direct.status.success());
        let error = String::from_utf8_lossy(&direct.stderr);
        assert!(
            error.contains("missing provider-resolved gate environment variable GATE_VALUE"),
            "{error}"
        );
        assert!(!error.contains(configured));
        assert!(!root.path().join("executions").exists());
    }
}

#[test]
fn local_literals_override_inherited_values_without_shell_interpretation() {
    for value in ["", "'\" $(touch injected); `touch injected` & | $HOME"] {
        let root = environment_fixture(value);
        let output = check(&root, "pass");
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            fs::read(root.path().join("observed")).unwrap(),
            value.as_bytes()
        );
        assert_eq!(fs::read(root.path().join("executions")).unwrap(), b"x");
        assert!(!root.path().join("injected").exists());
    }
}

#[test]
fn local_provider_expressions_fail_before_execution_without_exposing_values() {
    let root = environment_fixture("prefix-${{ secrets.DO_NOT_REPORT }}");
    let output = check(&root, "pass");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("provider expression requires --gate-env provider-resolved"),
        "{stderr}"
    );
    assert!(!stderr.contains("DO_NOT_REPORT"));
    assert!(!stderr.contains("inherited"));
    assert!(!root.path().join("executions").exists());
}

#[test]
fn generated_failures_keep_status_and_do_not_dump_resolved_secrets() {
    let root = environment_fixture("${{ secrets.TEST_TOKEN }}");
    let output = generated_gate(&root, "fail", Some("never-report-this-secret"));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("original-failure"));
    assert!(stderr.contains("exit status: 7"));
    assert!(stderr.contains("Nix failure diagnostics"));
    assert!(!stderr.contains("never-report-this-secret"));
    assert_eq!(stderr.matches("Nix failure diagnostics").count(), 1);
}

#[test]
fn generated_gate_preserves_bash_pipeline_and_unset_variable_failures() {
    for command in [
        "false | true",
        "printf '%s' \"$SIMIT_MISSING_GATE_VARIABLE\"",
    ] {
        let root = environment_fixture("literal");
        let path = root.path().join("simit.toml");
        let config = fs::read_to_string(&path).unwrap();
        let config = config.replace(
            "printf x >> executions; printf '%s' \\\"$GATE_VALUE\\\" > observed; pwd > working-directory",
            &format!("printf x >> executions; {}; touch after-failure", command.replace('"', "\\\"")),
        );
        fs::write(path, config).unwrap();
        let output = generated_gate(&root, "pass", Some("never-report-this-secret"));
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(fs::read(root.path().join("executions")).unwrap(), b"x");
        assert!(!root.path().join("after-failure").exists());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("never-report-this-secret"));
    }
}

#[test]
fn validates_the_complete_graph_before_running_a_gate() {
    let root = environment_fixture("literal");
    let path = root.path().join("simit.toml");
    let config = fs::read_to_string(&path)
        .unwrap()
        .replace("id = \"fail\"", "id = \"pass\"");
    fs::write(path, config).unwrap();
    let output = check(&root, "pass");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("duplicate check"));
    assert!(!root.path().join("executions").exists());
}

#[test]
fn member_invocations_run_from_the_project_root() {
    let root = environment_fixture("literal");
    let member = root.path().join("core/nested");
    fs::create_dir_all(&member).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(member)
        .args([
            "monorepo",
            "check",
            "--component",
            "core",
            "--check",
            "pass",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read(root.path().join("executions")).unwrap(), b"x");
    assert_eq!(
        fs::read_to_string(root.path().join("working-directory"))
            .unwrap()
            .trim(),
        root.path().canonicalize().unwrap().to_str().unwrap()
    );
}

#[test]
fn local_timeout_stops_the_gate_and_descendants_without_retrying() {
    let root = fixture();
    let path = root.path().join("simit.toml");
    let config = fs::read_to_string(&path).unwrap().replace(
        "{ id = \"fail\", run = \"printf original-failure >&2; exit 7\" }",
        "{ id = \"fail\", timeout_minutes = 1, run = \"printf x >> executions; (sleep 65; touch leaked) & wait\" }",
    );
    fs::write(path, config).unwrap();
    let started = std::time::Instant::now();
    // output() also waits for inherited pipes held by the descendant: killing
    // only Bash would still block until sleep finishes and the marker is made.
    let output = check(&root, "fail");
    assert!(!output.status.success(), "{output:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(64));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("timed out (configured limit: 1 minutes)"),
        "{stderr}"
    );
    assert!(stderr.contains("Nix failure diagnostics"));
    assert!(!stderr.contains("must-not-be-reported"));
    assert_eq!(fs::read(root.path().join("executions")).unwrap(), b"x");
    assert!(!root.path().join("leaked").exists());
}

#[test]
fn successful_setup_gate_retains_its_background_service_for_the_next_gate() {
    let root = fixture();
    let path = root.path().join("simit.toml");
    let config = fs::read_to_string(&path).unwrap().replace(
        "printf original-failure >&2; exit 7",
        "(sleep 0.1; touch service-ready; sleep 30) </dev/null >/dev/null 2>&1 & echo $$ > service-pid",
    ).replace("printf '%s' \\\"$GATE_VALUE\\\"", "sleep 0.3; test -f service-ready");
    fs::write(&path, config).unwrap();
    let setup = check(&root, "fail");
    assert!(setup.status.success(), "{setup:?}");
    let pid = fs::read_to_string(root.path().join("service-pid")).unwrap();
    let next = check(&root, "pass");
    let pid = rustix::process::Pid::from_raw(pid.trim().parse().unwrap()).unwrap();
    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    assert!(
        next.status.success(),
        "successful setup lost its service: {next:?}"
    );
}

#[test]
fn cancellation_stops_the_isolated_gate_and_descendants() {
    for signal in [rustix::process::Signal::INT, rustix::process::Signal::TERM] {
        let root = fixture();
        let path = root.path().join("simit.toml");
        let config = fs::read_to_string(&path).unwrap().replace(
            "printf original-failure >&2; exit 7",
            "echo $$ > gate-pid; (sleep 3; touch leaked) & echo ready > ready; wait",
        );
        fs::write(path, config).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_simit"))
            .current_dir(root.path())
            .args([
                "monorepo",
                "check",
                "--component",
                "core",
                "--check",
                "fail",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !root.path().join("ready").exists() {
            assert!(std::time::Instant::now() < deadline, "gate did not start");
            assert!(child.try_wait().unwrap().is_none());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let started = std::time::Instant::now();
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(child.id() as i32).unwrap(),
            signal,
        )
        .unwrap();
        let output = child.wait_with_output().unwrap();
        let elapsed = started.elapsed();
        let gate_pid = fs::read_to_string(root.path().join("gate-pid")).unwrap();
        let _ = rustix::process::kill_process_group(
            rustix::process::Pid::from_raw(gate_pid.trim().parse().unwrap()).unwrap(),
            rustix::process::Signal::KILL,
        );
        assert!(!output.status.success(), "{output:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "descendants held pipes after cancellation: {elapsed:?}"
        );
        assert!(!root.path().join("leaked").exists());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cancelled"),
            "{output:?}"
        );
    }
}

#[test]
fn generated_provisioning_failure_keeps_status_and_collects_diagnostics_once() {
    let root = environment_fixture("${{ secrets.TEST_TOKEN }}");
    fs::write(root.path().join("bin/nix"), "#!/bin/sh\nprintf x >> provisioning-executions\nprintf provisioning-failure >&2\nexit 23\n").unwrap();
    let output = generated_gate(&root, "pass", Some("never-report-this-secret"));
    assert_eq!(output.status.code(), Some(23), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("provisioning-failure"));
    assert!(stderr.contains("Nix failure diagnostics"), "{stderr}");
    assert_eq!(stderr.matches("Nix failure diagnostics").count(), 1);
    assert!(!stderr.contains("never-report-this-secret"));
    assert_eq!(
        fs::read(root.path().join("provisioning-executions")).unwrap(),
        b"x"
    );
    assert!(!root.path().join("executions").exists());
}

#[test]
fn provider_values_survive_bash_special_environment_normalization() {
    for name in ["IFS", "PWD"] {
        let root = environment_fixture("${{ secrets.TEST_TOKEN }}");
        let path = root.path().join("simit.toml");
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace("GATE_VALUE", name),
        )
        .unwrap();
        let value = "provider-'\" $(touch injected); & |";
        let output = generated_gate(&root, "pass", Some(value));
        assert!(output.status.success(), "{name}: {output:?}");
        assert_eq!(
            fs::read(root.path().join("observed")).unwrap(),
            value.as_bytes(),
            "{name}"
        );
        assert_eq!(fs::read(root.path().join("executions")).unwrap(), b"x");
        assert!(!root.path().join("injected").exists());
    }
}

#[test]
fn local_literals_survive_bash_special_environment_normalization() {
    for name in ["IFS", "PWD"] {
        let value = "literal-'\" $(touch injected); & |";
        let root = environment_fixture(value);
        let path = root.path().join("simit.toml");
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace("GATE_VALUE", name),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_simit"))
            .current_dir(root.path())
            .args([
                "monorepo",
                "check",
                "--component",
                "core",
                "--check",
                "pass",
            ])
            .env(name, "unrelated-inherited")
            .output()
            .unwrap();
        assert!(output.status.success(), "{name}: {output:?}");
        assert_eq!(
            fs::read(root.path().join("observed")).unwrap(),
            value.as_bytes(),
            "{name}"
        );
        assert_eq!(fs::read(root.path().join("executions")).unwrap(), b"x");
        assert!(!root.path().join("injected").exists());
    }
}

#[test]
fn stalled_provisioning_expires_before_the_provider_deadline() {
    let root = environment_fixture("literal");
    fs::write(
        root.path().join("bin/nix"),
        "#!/bin/sh\nprintf x >> provisioning-executions\n(sleep 5; touch leaked) & wait\n",
    )
    .unwrap();
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 1;
    let started = std::time::Instant::now();
    let output = generated_gate_with_deadline(
        &root,
        "pass",
        Some("never-report-this-secret"),
        Some(deadline),
    );
    assert!(!output.status.success(), "{output:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "provisioning ignored the reserved deadline"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Nix failure diagnostics"), "{stderr}");
    assert!(!stderr.contains("never-report-this-secret"));
    assert_eq!(
        fs::read(root.path().join("provisioning-executions")).unwrap(),
        b"x"
    );
    assert!(!root.path().join("executions").exists());
    assert!(!root.path().join("leaked").exists());
}

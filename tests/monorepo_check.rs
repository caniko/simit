#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

use tempfile::TempDir;

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
        assert_eq!(step["timeout-minutes"].as_u64(), Some(30));
    }
    assert_eq!(
        steps
            .iter()
            .find(|step| step["name"].as_str() == Some("pass"))
            .unwrap()["env"]["GATE_VALUE"]
            .as_str(),
        Some("gate-owned")
    );
}

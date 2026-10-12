#![cfg(unix)]
//! Run the real CLI at immutable Git/Nix process boundaries.
use serde_json::{Value, json};
use std::{fs, os::unix::fs::symlink, path::Path, process::Command};
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    head: String,
    spec: Value,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("tracked"), "source\n").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["add", "tracked"],
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
                    .current_dir(root.path())
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
                .current_dir(root.path())
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        fs::create_dir(root.path().join("bin")).unwrap();
        symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/group-nix.sh"),
            root.path().join("bin/nix"),
        )
        .unwrap();
        fs::create_dir(root.path().join("results")).unwrap();
        Self {
            root,
            head,
            spec: json!({
                "schema": "simit.nix-group-spec.v1",
                "members": [".#checks.x86_64-linux.client", ".#checks.x86_64-linux.recovery"],
                "plan": [".#checks.x86_64-linux.client", ".#checks.x86_64-linux.recovery"],
                "check_systems": ["x86_64-linux"], "limits": ["--max-jobs", "1", "--cores", "2"],
                "post_build": ["printf '%s' \"$INSTALLABLE\" > \"$SIMIT_NIX_BUILD_RESULTS/hook-member\""],
                "post_build_always": ["printf '%s\\n' \"$INSTALLABLE\" >> \"$SIMIT_NIX_BUILD_RESULTS/hook-diagnostic\""],
                "fresh_installables": []
            }),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_simit"));
        command
            .current_dir(self.root.path())
            .args(["ci", "nix-group", "--spec", "spec.json"])
            .env("GITHUB_SHA", &self.head)
            .env("GITHUB_EVENT_NAME", "push")
            .env("GITHUB_RUN_ID", "123")
            .env("GITHUB_RUN_ATTEMPT", "1")
            .env("SIMIT_NIX_BUILD_RESULTS", self.root.path().join("results"))
            .env("GROUP_CALLS", self.root.path().join("calls"))
            .env(
                "PATH",
                std::env::join_paths(
                    std::iter::once(self.root.path().join("bin"))
                        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                )
                .unwrap(),
            );
        command
    }

    fn run(&self, case: &str) -> std::process::Output {
        fs::write(
            self.root.path().join("spec.json"),
            serde_json::to_vec(&self.spec).unwrap(),
        )
        .unwrap();
        self.command().env("GROUP_CASE", case).output().unwrap()
    }

    fn receipt(&self) -> Value {
        serde_json::from_slice(&fs::read(self.root.path().join("results/receipt.json")).unwrap())
            .unwrap()
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.root.path().join("calls")).unwrap_or_default()
    }
}

#[test]
fn accepted_megabyte_hooks_execute_without_an_oversized_argv() {
    let mut f = Fixture::new();
    let mut script = String::from("#");
    script.push_str(&"x".repeat(1024 * 1024 - 200));
    script.push_str("\nprintf '%s' \"$INSTALLABLE\" > \"$SIMIT_NIX_BUILD_RESULTS/large-hook\"\n");
    f.spec["post_build"] = json!([script]);
    let output = f.run("success");
    assert!(output.status.success(), "{output:?}");
    for (index, member) in f.spec["members"].as_array().unwrap().iter().enumerate() {
        assert_eq!(
            fs::read_to_string(f.root.path().join(format!("results/{index}/large-hook"))).unwrap(),
            member.as_str().unwrap()
        );
    }
    assert_eq!(f.receipt()["success"], true);
}

#[test]
fn complete_receipt_preserves_member_evidence_and_hook_context() {
    let f = Fixture::new();
    let output = f.run("success");
    assert!(output.status.success(), "{output:?}");
    let receipt = f.receipt();
    assert_eq!(receipt["schema"], "simit.nix-group.v1");
    assert_eq!(receipt["head"], f.head);
    assert_eq!(receipt["complete"], true);
    assert_eq!(receipt["success"], true);
    assert_eq!(receipt["results"].as_array().unwrap().len(), 2);
    for (index, member) in f.spec["members"].as_array().unwrap().iter().enumerate() {
        let root = f.root.path().join(format!("results/{index}"));
        assert_eq!(
            fs::read_to_string(root.join("hook-member")).unwrap(),
            member.as_str().unwrap()
        );
        assert_eq!(
            fs::read_to_string(root.join("hook-diagnostic")).unwrap(),
            format!("{}\n", member.as_str().unwrap())
        );
        assert!(root.join("diagnostics-complete").is_file());
        assert_eq!(
            fs::read_to_string(root.join("build.log")).unwrap(),
            "retained builder diagnostic\n"
        );
    }
}

#[test]
fn failed_build_or_malformed_success_keeps_later_members_and_diagnostics() {
    for case in ["fail", "malformed", "spawn-fail"] {
        let f = Fixture::new();
        let output = f.run(case);
        assert!(!output.status.success(), "{case}: {output:?}");
        let receipt = f.receipt();
        assert_eq!(receipt["complete"], true, "{case}");
        assert_eq!(receipt["success"], false);
        assert_eq!(receipt["results"].as_array().unwrap().len(), 2);
        assert!(f.root.path().join("results/1/hook-diagnostic").exists());
        assert!(!f.root.path().join("results/0/hook-member").exists());
        if case == "fail" {
            assert_eq!(receipt["results"][0]["exit_status"], 7);
        }
    }
}

#[test]
fn failed_hooks_fail_group_without_short_circuiting_other_members() {
    for field in ["post_build", "post_build_always"] {
        let mut f = Fixture::new();
        f.spec[field] = json!([
            "exit 19",
            "printf retained > \"$SIMIT_NIX_BUILD_RESULTS/second-hook\""
        ]);
        let output = f.run("success");
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(f.receipt()["success"], false);
        for index in 0..2 {
            assert!(
                f.root
                    .path()
                    .join(format!("results/{index}/second-hook"))
                    .exists()
            );
        }
    }
}

#[test]
fn uncovered_or_empty_inventory_denies_before_realization() {
    for case in ["missing", "empty"] {
        let f = Fixture::new();
        let output = f.run(case);
        assert!(!output.status.success(), "{output:?}");
        assert!(!f.calls().contains("build"));
        assert_eq!(f.receipt()["complete"], false);
    }
}

#[test]
fn wrong_or_dirty_source_denies_before_nix_effects() {
    for dirty in [false, true] {
        let f = Fixture::new();
        fs::write(
            f.root.path().join("spec.json"),
            serde_json::to_vec(&f.spec).unwrap(),
        )
        .unwrap();
        let mut command = f.command();
        if dirty {
            fs::write(f.root.path().join("tracked"), "changed").unwrap();
        } else {
            command.env("GITHUB_SHA", "c".repeat(40));
        }
        let output = command.output().unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(f.calls().is_empty());
        assert!(!f.root.path().join("results/receipt.json").exists());
    }
}

#[test]
fn pull_request_uses_event_head_rather_than_synthetic_merge() {
    let f = Fixture::new();
    fs::write(
        f.root.path().join("spec.json"),
        serde_json::to_vec(&f.spec).unwrap(),
    )
    .unwrap();
    fs::write(
        f.root.path().join("event.json"),
        serde_json::to_vec(&json!({"pull_request":{"head":{"sha":f.head}}})).unwrap(),
    )
    .unwrap();
    let output = f
        .command()
        .env("GITHUB_EVENT_NAME", "pull_request")
        .env("GITHUB_SHA", "c".repeat(40))
        .env("GITHUB_EVENT_PATH", f.root.path().join("event.json"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(f.receipt()["event"], "pull_request");
    assert_eq!(f.receipt()["head"], f.head);
}

#[test]
fn fresh_materialization_never_substitutes_for_current_worker_rebuild() {
    for case in ["success", "fresh-fail"] {
        let mut f = Fixture::new();
        f.spec["fresh_installables"] = json!([".#checks.x86_64-linux.client"]);
        let output = f.run(case);
        assert_eq!(output.status.success(), case == "success", "{output:?}");
        let calls = f.calls();
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.contains("--rebuild"))
                .count(),
            1
        );
        let rebuild = calls
            .lines()
            .find(|line| line.contains("--rebuild"))
            .unwrap();
        assert!(
            rebuild.contains("--option builders  "),
            "fresh rebuild must disable configured remote builders: {rebuild}"
        );
        let row = &f.receipt()["results"][0];
        for name in [
            "build.log",
            "materialization-result.json",
            "fresh-build.log",
            "result.json",
        ] {
            assert!(
                f.root.path().join("results/0").join(name).is_file(),
                "missing transport-compatible fresh evidence {name}"
            );
        }
        assert_eq!(row["fresh_execution"], case == "success");
        if case == "fresh-fail" {
            assert_eq!(row["exit_status"], 9);
        }
    }
}

#[test]
fn invalid_spec_cannot_inject_nix_flags_or_accept_unknown_fields() {
    for (key, value) in [
        ("members", json!(["--impure"])),
        ("members", json!([])),
        (
            "members",
            json!([
                ".#checks.x86_64-linux.client",
                ".#checks.x86_64-linux.client"
            ]),
        ),
        ("limits", json!(["--impure"])),
        ("schema", json!("unknown")),
        ("unrecognized", json!(true)),
    ] {
        let mut f = Fixture::new();
        f.spec[key] = value;
        let output = f.run("success");
        assert!(!output.status.success(), "{key}: {output:?}");
        assert!(f.calls().is_empty());
    }
}

#[test]
fn failed_or_malformed_materialization_never_dispatches_a_fresh_rebuild() {
    for case in ["fail", "malformed"] {
        let mut f = Fixture::new();
        f.spec["fresh_installables"] = json!([".#checks.x86_64-linux.client"]);
        let output = f.run(case);
        assert!(!output.status.success(), "{output:?}");
        assert!(!f.calls().contains("--rebuild"));
        assert_eq!(f.receipt()["results"][0]["fresh_execution"], false);
        assert_eq!(f.receipt()["complete"], true);
        assert!(f.root.path().join("results/1/hook-diagnostic").is_file());
        assert_eq!(
            fs::read(f.root.path().join("results/0/result.json")).unwrap(),
            fs::read(f.root.path().join("results/0/materialization-result.json")).unwrap()
        );
    }
}

#[test]
fn evidence_reuse_and_dangling_receipt_symlinks_fail_without_overwriting() {
    for dangling in [false, true] {
        let f = Fixture::new();
        let receipt = f.root.path().join("results/receipt.json");
        if dangling {
            symlink("missing", &receipt).unwrap();
        } else {
            fs::write(&receipt, "preserved evidence").unwrap();
        }
        let output = f.run("success");
        assert!(!output.status.success(), "{output:?}");
        assert!(f.calls().is_empty());
        if dangling {
            assert_eq!(fs::read_link(receipt).unwrap(), Path::new("missing"));
        } else {
            assert_eq!(fs::read_to_string(receipt).unwrap(), "preserved evidence");
        }
    }
}

#[test]
fn declared_nonlocal_installables_and_zero_cores_remain_supported_as_argv_data() {
    let mut f = Fixture::new();
    f.spec["members"] = json!(["github:owner/project/revision#package; touch injected"]);
    f.spec["plan"] = f.spec["members"].clone();
    f.spec["check_systems"] = json!([]);
    f.spec["limits"] = json!(["--max-jobs", "1", "--cores", "0"]);
    let output = f.run("success");
    assert!(output.status.success(), "{output:?}");
    assert!(
        f.calls().contains(
            "--max-jobs 1 --cores 0 github:owner/project/revision#package; touch injected"
        )
    );
    assert!(!f.root.path().join("injected").exists());
}

#[test]
fn cancellation_between_commands_prevents_all_later_hooks_and_members() {
    use simit::ci_runtime::{
        cancellation::Cancellation,
        process::{CommandRunner, Runner},
    };
    struct BoundaryRunner {
        inner: CommandRunner,
        bin: std::path::PathBuf,
        calls: std::path::PathBuf,
        builds: usize,
    }
    impl Runner for BoundaryRunner {
        fn run(
            &mut self,
            command: &mut Command,
            stdout: &Path,
            stderr: &Path,
            cancellation: &Cancellation,
        ) -> anyhow::Result<i32> {
            let build = command.get_program() == "nix"
                && command.get_args().next() == Some(std::ffi::OsStr::new("build"));
            if command.get_program() == "nix" {
                command.env("GROUP_CALLS", &self.calls).env(
                    "PATH",
                    std::env::join_paths(
                        std::iter::once(self.bin.clone())
                            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                    )?,
                );
            }
            let status = self.inner.run(command, stdout, stderr, cancellation)?;
            if build {
                self.builds += 1;
                cancellation.cancel();
            }
            Ok(status)
        }
    }
    let f = Fixture::new();
    let spec: simit::ci_runtime::group::GroupSpec = serde_json::from_value(f.spec.clone()).unwrap();
    let source = simit::ci_runtime::group::Source {
        head: f.head.clone(),
        event: "push".into(),
        run: "123".into(),
        attempt: 1,
    };
    let mut runner = BoundaryRunner {
        inner: CommandRunner,
        bin: f.root.path().join("bin"),
        calls: f.root.path().join("calls"),
        builds: 0,
    };
    let error = simit::ci_runtime::group::execute_with(
        &spec,
        &source,
        f.root.path(),
        &f.root.path().join("results"),
        &mut runner,
        &Cancellation::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(runner.builds, 1);
    assert_eq!(f.receipt()["complete"], false);
    assert!(!f.root.path().join("results/0/hook-member").exists());
    assert!(!f.root.path().join("results/0/hook-diagnostic").exists());
    assert!(!f.root.path().join("results/1").exists());
}

#[test]
fn cancellation_retains_incomplete_receipt_and_stops_later_members_and_descendants() {
    use std::time::{Duration, Instant};
    let f = Fixture::new();
    fs::remove_file(f.root.path().join("bin/nix")).unwrap();
    symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/group-stall.sh"),
        f.root.path().join("bin/nix"),
    )
    .unwrap();
    let mut spec = f.spec.clone();
    spec["check_systems"] = json!([]);
    fs::write(
        f.root.path().join("spec.json"),
        serde_json::to_vec(&spec).unwrap(),
    )
    .unwrap();
    let marker = f.root.path().join("started");
    let late = f.root.path().join("late-effect");
    let mut child = f
        .command()
        .env("GROUP_STARTED", &marker)
        .env("GROUP_LATE_EFFECT", &late)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "fixture failed to start");
        std::thread::sleep(Duration::from_millis(10));
    }
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(child.id() as i32).unwrap(),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    assert!(!child.wait().unwrap().success());
    assert_eq!(f.receipt()["complete"], false);
    assert!(!f.root.path().join("results/1").exists());
    std::thread::sleep(Duration::from_millis(2200));
    assert!(!late.exists());
}

#[test]
fn signalled_members_and_hooks_preserve_negative_v1_signal_identity() {
    let mut f = Fixture::new();
    f.spec["post_build_always"] = json!(["kill -TERM $$"]);
    let output = f.run("signalled");
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(f.receipt()["results"][0]["exit_status"], -9);
    assert_eq!(
        f.receipt()["results"][0]["post_build_always_statuses"][0],
        -15
    );
    assert_eq!(f.receipt()["complete"], true);
}

#[test]
fn failed_admission_observations_retain_actionable_diagnostics_in_cli_output() {
    let f = Fixture::new();
    let output = f.run("eval-fail");
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("actionable Nix evaluation diagnostic"),
        "{output:?}"
    );
    assert!(!f.calls().contains("build"));
}

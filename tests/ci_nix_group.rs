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
        let mut command = self.environment_command();
        command.args(["--spec", "spec.json"]);
        command
    }

    fn environment_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_simit"));
        command
            .current_dir(self.root.path())
            .args(["ci", "nix-group"])
            .env("GITHUB_SHA", &self.head)
            .env("GITHUB_EVENT_NAME", "push")
            .env("GITHUB_RUN_ID", "123")
            .env("GITHUB_RUN_ATTEMPT", "1")
            .env("SIMIT_NIX_BUILD_RESULTS", self.root.path().join("results"))
            .env("GROUP_CALLS", self.root.path().join("calls"))
            .env_remove("SIMIT_NIX_FRESH")
            .env_remove("SIMIT_NIX_FRESH_INSTALLABLES")
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
fn environment_fresh_keys_preserve_current_worker_rebuilds() {
    for keys in [
        &["SIMIT_NIX_FRESH"][..],
        &["SIMIT_NIX_FRESH_INSTALLABLES"][..],
        &["SIMIT_NIX_FRESH", "SIMIT_NIX_FRESH_INSTALLABLES"][..],
        &[][..],
    ] {
        for case in ["success", "fresh-fail"] {
            let f = Fixture::new();
            let mut command = f.environment_command();
            for (key, field) in [
                ("INSTALLABLES", "members"),
                ("SIMIT_NIX_PLAN", "plan"),
                ("SIMIT_NIX_CHECK_SYSTEMS", "check_systems"),
                ("SIMIT_NIX_LIMITS", "limits"),
                ("SIMIT_NIX_POST_BUILD", "post_build"),
                ("SIMIT_NIX_POST_BUILD_ALWAYS", "post_build_always"),
            ] {
                command.env(key, f.spec[field].to_string());
            }
            for key in keys {
                command.env(
                    key,
                    serde_json::to_string_pretty(&f.spec["members"]).unwrap(),
                );
            }
            let output = command.env("GROUP_CASE", case).output().unwrap();
            let fresh = !keys.is_empty();
            assert_eq!(
                output.status.success(),
                !fresh || case == "success",
                "{keys:?}/{case}: {output:?}"
            );
            let receipt = f.receipt();
            assert_eq!(receipt["complete"], true);
            assert_eq!(receipt["success"], !fresh || case == "success");
            for row in receipt["results"].as_array().unwrap() {
                assert_eq!(
                    row["fresh_execution"],
                    if fresh {
                        json!(case == "success")
                    } else {
                        Value::Null
                    }
                );
                assert_eq!(
                    row["exit_status"],
                    if fresh && case == "fresh-fail" { 9 } else { 0 }
                );
            }
            let calls = f.calls();
            let rebuilds: Vec<_> = calls
                .lines()
                .filter(|line| line.contains("--rebuild"))
                .collect();
            assert_eq!(rebuilds.len(), if fresh { 2 } else { 0 });
            for rebuild in rebuilds {
                assert!(rebuild.contains("--option builders "), "{rebuild}");
            }
        }
    }
}

#[test]
fn conflicting_or_invalid_environment_fresh_keys_reject_before_effects() {
    for (retained, alias) in [
        (Some("[\".#checks.x86_64-linux.client\"]"), Some("[]")),
        (Some("[]"), Some("[\".#checks.x86_64-linux.client\"]")),
        (Some("not-json"), None),
        (None, Some("not-json")),
    ] {
        let f = Fixture::new();
        let mut command = f.environment_command();
        for (key, field) in [
            ("INSTALLABLES", "members"),
            ("SIMIT_NIX_PLAN", "plan"),
            ("SIMIT_NIX_CHECK_SYSTEMS", "check_systems"),
            ("SIMIT_NIX_LIMITS", "limits"),
            ("SIMIT_NIX_POST_BUILD", "post_build"),
            ("SIMIT_NIX_POST_BUILD_ALWAYS", "post_build_always"),
        ] {
            command.env(key, f.spec[field].to_string());
        }
        for (key, value) in [
            ("SIMIT_NIX_FRESH", retained),
            ("SIMIT_NIX_FRESH_INSTALLABLES", alias),
        ] {
            if let Some(value) = value {
                command.env(key, value);
            }
        }
        let output = command.output().unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("SIMIT_NIX_FRESH"),
            "{output:?}"
        );
        assert!(f.calls().is_empty());
        assert_eq!(
            fs::read_dir(f.root.path().join("results")).unwrap().count(),
            0
        );
    }
}

#[test]
fn missing_or_empty_results_input_rejects_before_checkout_or_nix_effects() {
    for input in ["missing", "empty-env", "empty-argument"] {
        let f = Fixture::new();
        fs::write(
            f.root.path().join("spec.json"),
            serde_json::to_vec(&f.spec).unwrap(),
        )
        .unwrap();
        let mut command = f.command();
        command.env("GROUP_CASE", "success");
        match input {
            "missing" => {
                command.env_remove("SIMIT_NIX_BUILD_RESULTS");
            }
            "empty-env" => {
                command.env("SIMIT_NIX_BUILD_RESULTS", "");
            }
            _ => {
                command.args(["--results", ""]);
            }
        }
        let output = command.output().unwrap();
        assert!(!output.status.success(), "{output:?}");
        let error = String::from_utf8_lossy(&output.stderr);
        if input == "empty-argument" {
            assert!(
                error.contains("a value is required for '--results"),
                "{error}"
            );
        } else {
            assert!(
                error.contains("results directory") && error.contains("SIMIT_NIX_BUILD_RESULTS"),
                "{error}"
            );
        }
        assert_eq!(f.calls(), "");
        for name in [
            "receipt.json",
            "installables.json",
            "coverage.json",
            "0",
            "1",
        ] {
            assert!(
                !f.root.path().join(name).exists(),
                "checkout effect: {name}"
            );
            assert!(
                !f.root.path().join("results").join(name).exists(),
                "evidence effect: {name}"
            );
        }
    }
    let f = Fixture::new();
    fs::write(
        f.root.path().join("spec.json"),
        serde_json::to_vec(&f.spec).unwrap(),
    )
    .unwrap();
    let output = f
        .command()
        .env("SIMIT_NIX_BUILD_RESULTS", "")
        .env("GROUP_CASE", "success")
        .arg("--results")
        .arg(f.root.path().join("results"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(f.receipt()["success"].as_bool().unwrap());
}

#[test]
fn serialized_group_spec_budget_is_shared_with_library_admission() {
    let mut f = Fixture::new();
    let mut spec: simit::ci_runtime::group::GroupSpec =
        serde_json::from_value(f.spec.clone()).unwrap();
    let maximum = 16 * 1024 * 1024;
    spec.post_build = vec!["x".repeat(1024 * 1024); 15];
    spec.post_build.push(String::new());
    spec.post_build_always.clear();
    let remaining = maximum - serde_json::to_vec(&spec).unwrap().len();
    assert!(remaining < 1024 * 1024);
    *spec.post_build.last_mut().unwrap() = "x".repeat(remaining);
    assert_eq!(serde_json::to_vec(&spec).unwrap().len(), maximum);
    assert!(
        spec.validate().is_ok(),
        "the exact serialized boundary is valid"
    );
    spec.post_build.last_mut().unwrap().push('x');
    let error = spec.validate().unwrap_err();
    assert!(
        format!("{error:#}").contains("serialized group spec"),
        "{error:#}"
    );

    // Count JSON escaping, not only decoded script bytes. Each individual hook
    // remains below its existing one-MiB bound, but their encoded form does not.
    spec.post_build = vec!["\u{1}".repeat(1024 * 1024); 3];
    assert!(serde_json::to_vec(&spec).unwrap().len() > maximum);
    assert!(spec.validate().is_err());
    f.spec = serde_json::to_value(spec).unwrap();
    let output = f.run("success");
    assert!(!output.status.success());
    assert_eq!(f.calls(), "");
    assert!(!f.root.path().join("results/receipt.json").exists());
}

#[test]
fn accepted_megabyte_hooks_execute_without_an_oversized_argv() {
    use std::os::unix::fs::PermissionsExt;
    for size in [128 * 1024 + 1, 1024 * 1024] {
        let mut f = Fixture::new();
        let body = "\ntest \"$0\" = bash; test \"$#\" = 0; printf '%s' \"$INSTALLABLE\" > \"$SIMIT_NIX_BUILD_RESULTS/large-hook\"\n";
        let script = format!("#{}{body}", "x".repeat(size - body.len() - 1));
        assert_eq!(script.len(), size);
        f.spec["post_build"] = json!([script]);
        f.spec["post_build_always"] = f.spec["post_build"].clone();
        let output = f.run("success");
        assert!(output.status.success(), "{output:?}");
        for (index, member) in f.spec["members"].as_array().unwrap().iter().enumerate() {
            let root = f.root.path().join(format!("results/{index}"));
            assert_eq!(
                fs::read_to_string(root.join("large-hook")).unwrap(),
                member.as_str().unwrap()
            );
            for category in ["post-build", "post-build-always"] {
                let file = root.join(format!("{category}-0.script"));
                assert_eq!(
                    fs::metadata(file).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        assert_eq!(f.receipt()["success"], true);
    }
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
fn stale_numbered_member_evidence_denies_before_any_dispatch_or_write() {
    use simit::ci_runtime::{cancellation::Cancellation, group, process::Runner};

    #[derive(Default)]
    struct NoDispatch(usize);
    impl Runner for NoDispatch {
        fn run(
            &mut self,
            _command: &mut Command,
            _stdout: &Path,
            _stderr: &Path,
            _cancellation: &Cancellation,
        ) -> anyhow::Result<i32> {
            self.0 += 1;
            anyhow::bail!("admission dispatched a command before checking member evidence")
        }
    }
    for name in ["0", "1", "2", "999999999999999999999999", "00"] {
        for kind in ["directory", "file", "symlink"] {
            let f = Fixture::new();
            let root = f.root.path().join("results");
            fs::write(root.join("revision"), "preparation evidence").unwrap();
            let prior = root.join(name);
            match kind {
                "directory" => {
                    fs::create_dir(&prior).unwrap();
                    fs::write(prior.join("fallback-diagnostic-0.script"), "previous hook").unwrap();
                }
                "file" => fs::write(&prior, "previous evidence").unwrap(),
                _ => symlink("missing", &prior).unwrap(),
            }
            let spec: group::GroupSpec = serde_json::from_value(f.spec.clone()).unwrap();
            let source = group::Source {
                head: f.head.clone(),
                event: "push".into(),
                run: "123".into(),
                attempt: 1,
            };
            let mut runner = NoDispatch::default();
            let error = group::execute_with(
                &spec,
                &source,
                f.root.path(),
                &root,
                &mut runner,
                &Cancellation::default(),
            )
            .unwrap_err()
            .to_string();
            assert!(
                error.contains("member evidence") && error.contains(name),
                "{error}"
            );
            assert_eq!(runner.0, 0);
            let output = f.run("success");
            assert!(!output.status.success(), "{name}/{kind}: {output:?}");
            assert!(f.calls().is_empty());
            assert_eq!(
                fs::read_to_string(root.join("revision")).unwrap(),
                "preparation evidence"
            );
            for path in ["receipt.json", "coverage.json", "installables.json"] {
                assert!(!root.join(path).exists(), "unexpected write: {path}");
            }
            assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
            match kind {
                "directory" => assert_eq!(
                    fs::read_to_string(prior.join("fallback-diagnostic-0.script")).unwrap(),
                    "previous hook"
                ),
                "file" => assert_eq!(fs::read_to_string(prior).unwrap(), "previous evidence"),
                _ => assert_eq!(fs::read_link(prior).unwrap(), Path::new("missing")),
            }
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
    use std::{
        io::Write,
        time::{Duration, Instant},
    };
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
    let input_path = f.root.path().join("input");
    assert!(
        Command::new("mkfifo")
            .arg(&input_path)
            .status()
            .unwrap()
            .success()
    );
    let mut child = f
        .command()
        .env("GROUP_STARTED", &marker)
        .env("GROUP_LATE_EFFECT", &late)
        .env("GROUP_INPUT", &input_path)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut input = loop {
        match rustix::fs::open(
            &input_path,
            rustix::fs::OFlags::WRONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        ) {
            Ok(fd) => break fs::File::from(fd),
            Err(rustix::io::Errno::NXIO) => {
                assert!(Instant::now() < deadline, "fixture failed to open input");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("fixture input: {error}"),
        }
    };
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
    // Keep the descendant blocked until cancellation has completed. EPIPE
    // proves all inherited readers exited without racing a timed side effect.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match input.write(b"x") {
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => break,
            Ok(1) => {
                assert!(
                    Instant::now() < deadline,
                    "descendant retained the input pipe"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            result => panic!("fixture input: {result:?}"),
        }
    }
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

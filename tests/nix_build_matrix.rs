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
    // The pinned engine's closure is reused across runs around unchanged provisioning.
    let at = |name: &str| steps.iter().position(|s| s["name"] == name).unwrap();
    let key =
        "simit-engine-v1-${{ vars.SIMIT_CI_ENGINE_REVISION }}-${{ runner.os }}-${{ runner.arch }}";
    assert_eq!(
        steps[at("Restore grouped-build engine")]["with"]["key"],
        key
    );
    assert_eq!(steps[at("Save grouped-build engine")]["with"]["key"], key);
    assert!(at("Restore grouped-build engine") < at("Import grouped-build engine"));
    assert!(at("Import grouped-build engine") < setup);
    assert!(setup < at("Export grouped-build engine"));
    assert!(at("Export grouped-build engine") < at("Save grouped-build engine"));
    assert!(
        steps[at("Import grouped-build engine")]["run"]
            .as_str()
            .unwrap()
            .contains("nix copy --no-check-sigs --all")
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
fn grouped_input_cache_reuses_declared_inputs_without_satisfying_gates() {
    let groups = "[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]";
    let temp = project(&format!(
        "[ci.nix_build]\nonly = true\ncapture_results = true\ncache_installables = [\".#packages.x86_64-linux.kernel\"]\n{groups}"
    ));
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    let check = generate(&temp, &["--check"]);
    assert!(check.status.success(), "{check:?}");
    let value = workflow(&temp);
    let steps = value["jobs"]["build"]["steps"].as_sequence().unwrap();
    let names: Vec<_> = steps.iter().map(|s| s["name"].as_str().unwrap()).collect();
    let at = |name: &str| {
        names
            .iter()
            .position(|candidate| *candidate == name)
            .unwrap_or_else(|| panic!("{name}: {names:?}"))
    };
    for pair in [
        "Resolve cached Nix inputs",
        "Restore cached Nix input 0",
        "Import cached Nix inputs",
        "Build Nix group",
        "Export cached Nix inputs",
        "Save cached Nix input 0",
    ]
    .windows(2)
    {
        assert!(at(pair[0]) < at(pair[1]), "{names:?}");
    }
    let step = |name: &str| &steps[at(name)];
    for (name, action) in [
        ("Restore cached Nix input 0", "restore"),
        ("Save cached Nix input 0", "save"),
    ] {
        assert_eq!(
            step(name)["uses"],
            format!("actions/cache/{action}@55cc8345863c7cc4c66a329aec7e433d2d1c52a9")
        );
        assert_eq!(
            step(name)["with"]["key"],
            "${{ steps.simit-nix-cache.outputs.key-0 }}"
        );
        assert_eq!(
            step(name)["with"]["path"],
            "${{ runner.temp }}/simit-nix-cache/0"
        );
        assert!(step(name)["with"]["restore-keys"].is_null());
    }
    assert_eq!(
        step("Export cached Nix inputs")["if"],
        "${{ !cancelled() }}"
    );
    assert_eq!(
        step("Save cached Nix input 0")["if"],
        "${{ !cancelled() && steps.simit-nix-cache-save.outputs.save-0 == 'true' }}"
    );

    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    for tool in ["nix", "nix-store"] {
        std::os::unix::fs::symlink(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/input-cache-nix.sh"),
            bin.join(tool),
        )
        .unwrap();
    }
    let calls = temp.path().join("calls");
    let run = |name: &str, closure: &str| {
        let script = temp.path().join("step.sh");
        fs::write(&script, step(name)["run"].as_str().unwrap()).unwrap();
        Command::new("bash")
            .arg(&script)
            .env("RUNNER_TEMP", temp.path())
            .env("GITHUB_OUTPUT", temp.path().join("output"))
            .env("CACHE_CALLS", &calls)
            .env("CACHE_CLOSURE", closure)
            .env("SIMIT_NIX_GROUP", "network")
            .env("SIMIT_NIX_CACHE_HIT_0", "true")
            .env(
                "PATH",
                std::env::join_paths(
                    std::iter::once(bin.clone())
                        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                )
                .unwrap(),
            )
            .output()
            .unwrap()
    };
    let resolved = run("Resolve cached Nix inputs", "");
    assert!(resolved.status.success(), "{resolved:?}");
    assert!(
        fs::read_to_string(temp.path().join("output"))
            .unwrap()
            .contains("key-0=simit-nix-input-v1-00000000000000000000000000000003\n")
    );
    // A cached closure containing a gate output would skip that gate's execution.
    let poisoned = run("Import cached Nix inputs", "/nix/store/recovery-out");
    assert!(!poisoned.status.success(), "{poisoned:?}");
    assert!(
        String::from_utf8_lossy(&poisoned.stderr).contains("contains the qualification outputs"),
        "{poisoned:?}"
    );
    assert!(!fs::read_to_string(&calls).unwrap().contains("nix copy"));
    let clean = run("Import cached Nix inputs", "/nix/store/glibc");
    assert!(clean.status.success(), "{clean:?}");
    assert!(fs::read_to_string(&calls).unwrap().contains(&format!(
        "nix copy --no-check-sigs --from file://{}/simit-nix-cache/0 /nix/store/kernel-out /nix/store/kernel-dev\n",
        temp.path().display()
    )));

    for (cached, groups) in [
        (".#checks.x86_64-linux.client", groups),
        (".#checks.x86_64-linux.other", groups),
        (".#packages.x86_64-linux.kernel^out", groups),
        ("github:example/kernel#default", groups),
        (".#packages.x86_64-linux.kernel", ""),
    ] {
        let temp = project(&format!(
            "[ci.nix_build]\nonly = true\ncapture_results = true\ncache_installables = [\"{cached}\"]\n{groups}"
        ));
        let result = generate(&temp, &[]);
        assert!(!result.status.success(), "{cached}: {result:?}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("cache_installables"),
            "{result:?}"
        );
        assert!(!temp.path().join(".github/workflows").exists());
    }
    // A selected output aliases the same derivation as its bare attribute.
    let temp = project(&format!(
        "[ci.nix_build]\nonly = true\ncapture_results = true\ncache_installables = [\".#packages.x86_64-linux.kernel\"]\n{groups}"
    ));
    let config = fs::read_to_string(temp.path().join("simit.toml"))
        .unwrap()
        .replace(
            ".#checks.x86_64-linux.recovery",
            ".#packages.x86_64-linux.kernel^out",
        );
    fs::write(temp.path().join("simit.toml"), config).unwrap();
    let result = generate(&temp, &[]);
    assert!(!result.status.success(), "{result:?}");
    assert!(String::from_utf8_lossy(&result.stderr).contains("cache_installables"));
}

#[cfg(unix)]
#[test]
fn native_exact_matrix_groups_route_homogeneous_members_and_roundtrip() {
    let temp = project(
        r#"[ci.nix_build]
only = true
capture_results = true
max_parallel = 2
max_jobs = 1
cores = 2
kvm = true
extra_setup = ["echo native-setup"]
[ci.nix_build.groups]
arm = [".#checks.aarch64-linux.recovery"]
x86 = [".#checks.x86_64-linux.client"]
[ci.nix_build.system_runners]
"aarch64-linux" = "ubuntu-24.04-arm"
"x86_64-linux" = "ubuntu-24.04"
"#,
    );
    let config_path = temp.path().join("simit.toml");
    let config = fs::read_to_string(&config_path).unwrap().replacen(
        ".#checks.x86_64-linux.recovery",
        ".#checks.aarch64-linux.recovery",
        1,
    );
    fs::write(&config_path, config).unwrap();
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    let value = workflow(&temp);
    let job = &value["jobs"]["build"];
    assert_eq!(job["runs-on"], "${{ matrix.runner }}");
    assert_eq!(job["strategy"]["max-parallel"], 2);
    let rows = job["strategy"]["matrix"]["include"].as_sequence().unwrap();
    assert_eq!(rows.len(), 2);
    for (row, system, runner, member) in [
        (
            &rows[0],
            "aarch64-linux",
            "ubuntu-24.04-arm",
            ".#checks.aarch64-linux.recovery",
        ),
        (
            &rows[1],
            "x86_64-linux",
            "ubuntu-24.04",
            ".#checks.x86_64-linux.client",
        ),
    ] {
        assert_eq!(row["system"], system);
        assert_eq!(row["runner"], runner);
        assert_eq!(
            serde_json::from_str::<Vec<String>>(row["installables"].as_str().unwrap()).unwrap(),
            vec![member]
        );
    }
    let steps = job["steps"].as_sequence().unwrap();
    let position = |name| steps.iter().position(|s| s["name"] == name).unwrap();
    let verify = position("Verify native Nix runner");
    assert!(verify < position("Provision grouped-build Rust engine"));
    assert!(verify < position("Enable hosted KVM"));
    assert!(verify < position("Project setup"));
    assert!(verify < position("Build Nix group"));
    assert_eq!(
        steps[position("Enable hosted KVM")]["if"],
        "${{ matrix.system == 'x86_64-linux' }}"
    );
    let build = &steps[position("Build Nix group")];
    assert!(
        build["run"]
            .as_str()
            .unwrap()
            .contains("\"limits\":[\"--max-jobs\",\"1\",\"--cores\",\"2\"]")
    );
    let matrix_guard = {
        let ungrouped = project(
            "[ci.nix_build]\nonly = true\n[ci.nix_build.system_runners]\n\"x86_64-linux\" = \"ubuntu-24.04\"",
        );
        assert!(generate(&ungrouped, &[]).status.success());
        workflow(&ungrouped)["jobs"]["build"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "Verify native Nix runner")
            .unwrap()["run"]
            .clone()
    };
    assert_eq!(
        steps[verify]["run"], matrix_guard,
        "groups reuse the independently verified hardware guard"
    );
    let persisted = fs::read(&config_path).unwrap();
    let generated = fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap();
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert!(generate(&temp, &[]).status.success());
    assert_eq!(persisted, fs::read(&config_path).unwrap());
    assert_eq!(
        generated,
        fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap()
    );
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
}

#[test]
fn native_exact_matrix_rejects_mixed_groups_and_native_transport_before_writes() {
    for (options, expected) in [
        (
            "[ci.nix_build]\nonly = true\ncapture_results = true\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.aarch64-linux.recovery\"]",
            "homogeneous",
        ),
        (
            "[ci.nix_build]\nonly = true\ncapture_results = true\ncheck_systems = [\"x86_64-linux\", \"aarch64-linux\"]\ndependency_installables = [\".#packages.x86_64-linux.deps\"]\n[ci.nix_build.groups]\nx86 = [\".#checks.x86_64-linux.client\"]\narm = [\".#checks.aarch64-linux.recovery\"]",
            "dependency transport",
        ),
    ] {
        let temp = project(&format!(
            "{options}\n[ci.nix_build.system_runners]\n\"x86_64-linux\" = \"ubuntu-24.04\"\n\"aarch64-linux\" = \"ubuntu-24.04-arm\""
        ));
        let path = temp.path().join("simit.toml");
        let before = fs::read_to_string(&path).unwrap().replacen(
            ".#checks.x86_64-linux.recovery",
            ".#checks.aarch64-linux.recovery",
            1,
        );
        fs::write(&path, &before).unwrap();
        for args in [vec![], vec!["--check", "--diff"]] {
            let result = generate(&temp, &args);
            assert!(!result.status.success(), "{result:?}");
            assert!(
                String::from_utf8_lossy(&result.stderr).contains(expected),
                "{result:?}"
            );
            assert_eq!(before, fs::read_to_string(&path).unwrap());
            assert!(!temp.path().join(".github/workflows").exists());
        }
    }
}

#[test]
fn native_groups_admit_mapped_engine_systems_and_bind_guard_configuration() {
    let options = "[ci.nix_build]\nonly = true\ncapture_results = true\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]\n[ci.nix_build.system_runners]\n\"x86_64-linux\" = \"ubuntu-24.04\"\n[ci.nix_build.extra_env]\nNIX_CONFIG = \"eval-system = aarch64-linux\"";
    let temp = project(options);
    let path = temp.path().join("simit.toml");
    // Engine admission follows mapped native systems, not the unrelated ordinary runner.
    let before = fs::read_to_string(&path)
        .unwrap()
        .replace("runner = \"ubuntu-24.04\"", "runner = \"windows-2022\"");
    fs::write(&path, before).unwrap();
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let value = workflow(&temp);
    let steps = value["jobs"]["build"]["steps"].as_sequence().unwrap();
    let guard = steps
        .iter()
        .position(|s| s["name"] == "Verify native Nix runner")
        .unwrap();
    let engine = steps
        .iter()
        .position(|s| s["name"] == "Provision grouped-build Rust engine")
        .unwrap();
    assert!(guard < engine);
    assert!(
        steps[guard]["env"]["NIX_CONFIG"]
            .as_str()
            .unwrap()
            .contains("eval-system = aarch64-linux")
    );
    assert_eq!(steps[guard]["env"]["BASH_ENV"], "");
    assert_eq!(
        steps[engine]["env"]["NIX_CONFIG"],
        "experimental-features = nix-command flakes"
    );
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    for system in ["powerpc64le-linux", "x86_64-darwin", "i686-linux"] {
        let temp = project(&options.replace("x86_64-linux", system));
        let before = fs::read_to_string(temp.path().join("simit.toml"))
            .unwrap()
            .replace("x86_64-linux", system);
        fs::write(temp.path().join("simit.toml"), &before).unwrap();
        let output = generate(&temp, &[]);
        assert!(!output.status.success(), "{system}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unsupported grouped engine runner"),
            "{output:?}"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("simit.toml")).unwrap(),
            before
        );
        assert!(!temp.path().join(".github").exists());
    }
}

#[test]
fn generated_group_invocation_and_fallback_execute_the_packaged_rust_cli() {
    generated_group_fixture(None);
}

#[cfg(unix)]
fn generated_group_fixture(hook_size: Option<(usize, bool)>) {
    let success = "test -s \"$SIMIT_NIX_BUILD_RESULTS/result.json\" && test -s \"$SIMIT_NIX_BUILD_RESULTS/build.log\" && printf '%s' \"$INSTALLABLE\" > \"$SIMIT_NIX_BUILD_RESULTS/hook-member\"";
    let diagnostic = "printf '%s' \"$INSTALLABLE\" > \"$SIMIT_NIX_BUILD_RESULTS/hook-diagnostic\"";
    let pad = |script: &str, diagnostic: bool| match hook_size {
        Some((size, phase)) if phase == diagnostic => {
            let body = format!("\ntest \"$0\" = bash; test \"$#\" = 0; {script}");
            let hook = format!("#{}{body}", "x".repeat(size - body.len() - 1));
            assert_eq!(hook.len(), size);
            hook
        }
        _ => script.to_owned(),
    };
    let temp = project(&format!(
        "[ci.nix_build]\nonly = true\ncapture_results = true\ncheck_systems = [\"x86_64-linux\"]\nfresh_installables = [\".#checks.x86_64-linux.client\"]\npost_build = [{}]\npost_build_always = [{}]\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]",
        serde_json::to_string(&pad(success, false)).unwrap(),
        serde_json::to_string(&pad(diagnostic, true)).unwrap()
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
    // Each phase exceeds Linux's per-argument cap while the complete generated
    // workflow stays below GitHub's 500 KB limit, including repeated diagnostics.
    for diagnostic in [false, true] {
        generated_group_fixture(Some((140_000, diagnostic)));
    }
}

#[test]
fn grouped_rendered_workflow_limit_covers_specs_groups_and_fallback_repetition() {
    let group = "[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]";
    for (kind, size, groups) in [
        ("post_build", 1024 * 1024, group),
        ("post_build_always", 170_000, group),
        (
            "post_build",
            260_000,
            "[ci.nix_build.groups]\nclient = [\".#checks.x86_64-linux.client\"]\nrecovery = [\".#checks.x86_64-linux.recovery\"]",
        ),
    ] {
        let temp = project(&format!(
            "[ci.nix_build]\nonly = true\ncapture_results = true\n{kind} = [{}]\n{groups}",
            serde_json::to_string(&format!("#{}", "x".repeat(size - 1))).unwrap()
        ));
        let path = temp.path().join("simit.toml");
        let before = fs::read_to_string(&path).unwrap();
        let output = generate(&temp, &[]);
        assert!(!output.status.success(), "oversized {kind}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("500000-byte workflow limit"),
            "{output:?}"
        );
        assert_eq!(fs::read_to_string(path).unwrap(), before);
        assert!(!temp.path().join(".github/workflows").exists());
    }
}

#[test]
fn grouped_rendered_workflow_exact_boundary_preserves_existing_files_on_overflow() {
    let temp = project(
        "[ci.nix_build]\nonly = true\ncapture_results = true\npost_build = [\"#\"]\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]",
    );
    assert!(generate(&temp, &[]).status.success());
    let workflow = temp.path().join(".github/workflows/nix-builds.yaml");
    let padding = 500_000 - fs::read(&workflow).unwrap().len();
    let path = temp.path().join("simit.toml");
    let mut config = fs::read_to_string(&path)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    for extra in [0, 1] {
        let mut hooks = toml_edit::Array::new();
        hooks.push(format!("#{}", "x".repeat(padding + extra)));
        config["ci"]["nix_build"]["post_build"] = toml_edit::value(hooks);
        let before = config.to_string();
        fs::write(&path, &before).unwrap();
        let prior = fs::read(&workflow).unwrap();
        let output = generate(&temp, &[]);
        assert_eq!(output.status.success(), extra == 0, "{output:?}");
        if extra == 0 {
            assert_eq!(fs::read(&workflow).unwrap().len(), 500_000);
        } else {
            assert_eq!(fs::read(&workflow).unwrap(), prior);
            assert_eq!(fs::read_to_string(&path).unwrap(), before);
        }
    }
}

#[test]
fn hook_actions_expressions_reject_before_retaining_expanded_secrets() {
    for kind in ["post_build", "post_build_always"] {
        for script in [
            "printf '%s' '${{ secrets.DIAGNOSTIC_TOKEN }}'",
            "printf '%s' '${{ github.token }}'",
            "# ${{ inputs.command }}",
        ] {
            let temp = project(&format!(
                "[ci.nix_build]\nonly = true\ncapture_results = true\n{kind} = [{}]\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]",
                serde_json::to_string(script).unwrap()
            ));
            let path = temp.path().join("simit.toml");
            let before = fs::read_to_string(&path).unwrap();
            let output = generate(&temp, &[]);
            assert!(!output.status.success(), "{kind}: {output:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("Actions expressions"),
                "{output:?}"
            );
            assert_eq!(fs::read_to_string(path).unwrap(), before);
            assert!(!temp.path().join(".github/workflows").exists());
        }
    }
    let temp = project(
        "[ci.nix_build]\nonly = true\ncapture_results = true\nrequired_secrets = [\"DIAGNOSTIC_TOKEN\"]\npost_build_always = [\"test -n \\\"$DIAGNOSTIC_TOKEN\\\"\"]\n[ci.nix_build.extra_env]\nDIAGNOSTIC_TOKEN = '${{ secrets.DIAGNOSTIC_TOKEN }}'\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]",
    );
    assert!(generate(&temp, &[]).status.success());
    let value = workflow(&temp);
    for step in value["jobs"]["build"]["steps"].as_sequence().unwrap() {
        if let Some(script) = step["run"].as_str() {
            assert!(!script.contains("${{ secrets.DIAGNOSTIC_TOKEN }}"));
        }
    }
}

#[test]
fn oversized_serialized_group_specs_reject_generation_before_writes() {
    let temp = project(
        "[ci.nix_build]\nonly = true\ncapture_results = true\n[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]",
    );
    let path = temp.path().join("simit.toml");
    let mut config = fs::read_to_string(&path)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    let mut hooks = toml_edit::Array::new();
    for _ in 0..9 {
        hooks.push("x".repeat(1024 * 1024));
    }
    config["ci"]["nix_build"]["post_build"] = toml_edit::value(hooks.clone());
    config["ci"]["nix_build"]["post_build_always"] = toml_edit::value(hooks);
    let before = config.to_string();
    fs::write(&path, &before).unwrap();
    let output = generate(&temp, &[]);
    assert!(
        !output.status.success(),
        "oversized generated specs must reject"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("serialized group spec"),
        "{output:?}"
    );
    assert_eq!(fs::read_to_string(path).unwrap(), before);
    assert!(!temp.path().join(".github/workflows").exists());
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
    assert_eq!(
        steps[0]["with"]["ref"],
        "${{ github.event.pull_request.head.sha || github.sha }}"
    );
    assert_eq!(
        steps[0]["with"]["repository"],
        "${{ github.event.pull_request.head.repo.full_name || github.repository }}"
    );
    assert_eq!(steps[0]["with"]["persist-credentials"], false);
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
fn native_exact_matrix_routes_each_gate_once_and_limits_kvm_to_x86_linux() {
    for rust_project in [false, true] {
        let temp = project(
            r#"[ci.nix_build]
only = true
timeout_minutes = 90
max_parallel = 2
max_jobs = 1
cores = 2
kvm = true
capture_results = true

[ci.nix_build.system_runners]
"aarch64-linux" = "ubuntu-24.04-arm"
"x86_64-linux" = "ubuntu-24.04"
"#,
        );
        let config_path = temp.path().join("simit.toml");
        let original_config = fs::read_to_string(&config_path).unwrap().replace(
            ".#checks.x86_64-linux.recovery",
            ".#checks.aarch64-linux.recovery",
        );
        fs::write(&config_path, &original_config).unwrap();
        if rust_project {
            fs::write(
                temp.path().join("Cargo.toml"),
                "[package]\nname = \"native-contract\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )
            .unwrap();
            fs::create_dir(temp.path().join("src")).unwrap();
            fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        }
        let output = generate(&temp, &[]);
        assert!(output.status.success(), "{output:?}");
        assert!(!temp.path().join(".github/workflows/ci.yaml").exists());
        let value = workflow(&temp);
        let job = &value["jobs"]["build"];
        assert_eq!(job["runs-on"], "${{ matrix.runner }}");
        assert_eq!(job["name"], "build (${{ matrix.installable }})");
        assert_eq!(job["timeout-minutes"], 90);
        assert_eq!(job["strategy"]["max-parallel"], 2);
        let matrix = &job["strategy"]["matrix"];
        assert!(matrix["installable"].is_null());
        let rows = matrix["include"].as_sequence().unwrap();
        assert_eq!(rows.len(), 2, "no missing or cross-product gates");
        for (row, installable, system, runner) in [
            (
                &rows[0],
                ".#checks.x86_64-linux.client",
                "x86_64-linux",
                "ubuntu-24.04",
            ),
            (
                &rows[1],
                ".#checks.aarch64-linux.recovery",
                "aarch64-linux",
                "ubuntu-24.04-arm",
            ),
        ] {
            assert_eq!(row["installable"], installable);
            assert_eq!(row["system"], system);
            assert_eq!(row["runner"], runner);
        }
        let steps = job["steps"].as_sequence().unwrap();
        let verify = steps
            .iter()
            .position(|step| step["name"] == "Verify native Nix runner")
            .unwrap();
        let kvm = steps
            .iter()
            .position(|step| step["name"] == "Enable hosted KVM")
            .unwrap();
        let build = steps
            .iter()
            .position(|step| step["name"] == "Build ${{ matrix.installable }}")
            .unwrap();
        assert!(verify < kvm && kvm < build);
        assert_eq!(steps[kvm]["if"], "${{ matrix.system == 'x86_64-linux' }}");
        assert_eq!(
            steps[verify]["env"]["EXPECTED_NIX_SYSTEM"],
            "${{ matrix.system }}"
        );
        assert!(
            steps[verify]["run"]
                .as_str()
                .unwrap()
                .contains("builtins.currentSystem")
        );
        let runner_bin = temp.path().join("runner-bin");
        fs::create_dir(&runner_bin).unwrap();
        let nix = runner_bin.join("nix");
        fs::write(&nix, "#!/bin/sh\nprintf '%s' \"$FAKE_NATIVE_NIX_SYSTEM\"\n").unwrap();
        let uname = runner_bin.join("uname");
        fs::write(&uname, "#!/bin/sh\ncase \"$1\" in -s) printf Linux;; -m) printf '%s' \"${FAKE_NATIVE_NIX_SYSTEM%-linux}\";; *) exit 99;; esac\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&nix, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(&uname, fs::Permissions::from_mode(0o755)).unwrap();
        }
        for actual in ["aarch64-linux", "x86_64-linux"] {
            let output = Command::new("bash")
                .args(["-e", "-c", steps[verify]["run"].as_str().unwrap()])
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        runner_bin.display(),
                        std::env::var("PATH").unwrap_or_default()
                    ),
                )
                .env("EXPECTED_NIX_SYSTEM", "aarch64-linux")
                .env("FAKE_NATIVE_NIX_SYSTEM", actual)
                .output()
                .unwrap();
            assert_eq!(
                output.status.success(),
                actual == "aarch64-linux",
                "{output:?}"
            );
            if actual != "aarch64-linux" {
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("Native Nix runner mismatch")
                );
            }
        }
        assert!(
            steps[build]["run"]
                .as_str()
                .unwrap()
                .contains("--no-update-lock-file --max-jobs 1 --cores 2 --print-build-logs --json")
        );
        assert!(steps.iter().all(|step| step["continue-on-error"].is_null()));
        let generated = fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap();
        let persisted = fs::read(&config_path).unwrap();
        assert!(generate(&temp, &["--check", "--diff"]).status.success());
        assert!(generate(&temp, &[]).status.success());
        assert_eq!(
            generated,
            fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap()
        );
        assert_eq!(persisted, fs::read(&config_path).unwrap());
        assert_eq!(
            simit::registry::audit_ci(temp.path()).unwrap().status,
            simit::registry::FeatureStatus::Managed
        );
    }
}

#[test]
fn native_exact_matrix_rejects_ambiguous_or_unmapped_systems_before_writes() {
    for (installable, mapping, expected) in [
        (
            ".#checks.aarch64-linux.recovery",
            "\"x86_64-linux\" = \"ubuntu-24.04\"",
            "aarch64-linux",
        ),
        (
            ".#recovery",
            "\"x86_64-linux\" = \"ubuntu-24.04\"",
            "system-qualified",
        ),
        (
            ".#checks.\"aarch64-linux\".recovery",
            "\"x86_64-linux\" = \"ubuntu-24.04\"",
            "system-qualified",
        ),
        (
            ".#formatter.aarch64-linux.extra",
            "\"aarch64-linux\" = \"ubuntu-24.04-arm\"\n\"x86_64-linux\" = \"ubuntu-24.04\"",
            "system-qualified",
        ),
        (
            ".#checks.x86_64-linux",
            "\"x86_64-linux\" = \"ubuntu-24.04\"",
            "system-qualified",
        ),
        (
            ".#checks.x86_64-linux.recovery",
            "\"x86_64-linux\" = \"\"",
            "runner",
        ),
    ] {
        let temp = project(&format!(
            "[ci.nix_build]\nonly = true\n[ci.nix_build.system_runners]\n{mapping}"
        ));
        let config_path = temp.path().join("simit.toml");
        let config = fs::read_to_string(&config_path).unwrap().replace(
            ".#checks.x86_64-linux.recovery",
            &installable.replace('"', "\\\""),
        );
        fs::write(&config_path, &config).unwrap();
        for args in [vec![], vec!["--check", "--diff"]] {
            let output = generate(&temp, &args);
            assert!(!output.status.success(), "{output:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains(expected),
                "{output:?}"
            );
            assert_eq!(fs::read_to_string(&config_path).unwrap(), config);
            assert!(!temp.path().join(".github/workflows").exists());
        }
    }
}

#[test]
fn native_exact_matrix_selects_actions_from_mapped_workers() {
    for grouped in [false, true] {
        for (ordinary, mapped, checkout, upload) in [
            ("ubuntu-24.04", "private-worker", "v4.3.1", "v4.6.2"),
            ("private-worker", "ubuntu-24.04", "v7.0.1", "v7.0.2"),
        ] {
            let groups = if grouped {
                "[ci.nix_build.groups]\nnetwork = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]\n"
            } else {
                ""
            };
            let temp = project(&format!(
                "[ci.nix_build]\nonly = true\ncapture_results = true\n{groups}[ci.nix_build.system_runners]\n\"x86_64-linux\" = \"{mapped}\""
            ));
            let path = temp.path().join("simit.toml");
            let config = fs::read_to_string(&path).unwrap().replace(
                "runner = \"ubuntu-24.04\"",
                &format!("runner = \"{ordinary}\""),
            );
            fs::write(path, config).unwrap();
            let output = generate(&temp, &[]);
            assert!(output.status.success(), "{output:?}");
            let value = workflow(&temp);
            let steps = value["jobs"]["build"]["steps"].as_sequence().unwrap();
            for (name, action, version) in [
                ("Checkout", "checkout", checkout),
                ("Upload Nix build evidence", "upload-artifact", upload),
            ] {
                let step = steps.iter().find(|s| s["name"] == name).unwrap();
                let reference =
                    simit::render::ci::github_action_ref(&format!("actions/{action}"), version);
                assert_eq!(
                    step["uses"],
                    reference.split(" #").next().unwrap(),
                    "ordinary={ordinary}, mapped={mapped}, grouped={grouped}"
                );
            }
            if grouped {
                assert!(
                    steps
                        .iter()
                        .any(|s| s["name"] == "Provision grouped-build Rust engine")
                );
                assert!(
                    !steps
                        .iter()
                        .any(|s| s["name"] == "Provision grouped-build Python")
                );
            }
            assert!(generate(&temp, &["--check", "--diff"]).status.success());
        }
    }
}

#[test]
fn native_exact_matrix_accepts_standard_system_derivation_outputs() {
    for grouped in [false, true] {
        for installable in [
            ".#formatter.aarch64-linux",
            ".#formatter.aarch64-linux^out",
            ".#formatter.aarch64-linux^*",
            ".#defaultPackage.aarch64-linux",
            ".#defaultPackage.aarch64-linux^out,dev",
            ".#devShell.aarch64-linux",
            ".#devShell.aarch64-linux^out",
            ".#packages.aarch64-linux.server^out",
        ] {
            let groups = if grouped {
                format!(
                    "[ci.nix_build.groups]\nx86 = [\".#checks.x86_64-linux.client\"]\narm = [\"{installable}\"]\n"
                )
            } else {
                String::new()
            };
            let temp = project(&format!(
                "[ci.nix_build]\nonly = true\ncapture_results = true\n{groups}[ci.nix_build.system_runners]\n\"x86_64-linux\" = \"ubuntu-24.04\"\n\"aarch64-linux\" = \"ubuntu-24.04-arm\""
            ));
            let path = temp.path().join("simit.toml");
            let before = fs::read_to_string(&path)
                .unwrap()
                .replace(".#checks.x86_64-linux.recovery", installable);
            fs::write(&path, before).unwrap();
            let output = generate(&temp, &[]);
            assert!(
                output.status.success(),
                "{installable}, grouped={grouped}: {output:?}"
            );
            let value = workflow(&temp);
            let rows = value["jobs"]["build"]["strategy"]["matrix"]["include"]
                .as_sequence()
                .unwrap();
            let row = rows
                .iter()
                .find(|r| r["system"] == "aarch64-linux")
                .unwrap();
            assert_eq!(row["runner"], "ubuntu-24.04-arm");
            if grouped {
                assert_eq!(
                    serde_json::from_str::<Vec<String>>(row["installables"].as_str().unwrap())
                        .unwrap(),
                    vec![installable]
                );
            } else {
                assert_eq!(row["installable"], installable);
            }
            assert!(generate(&temp, &["--check", "--diff"]).status.success());
            assert_eq!(
                simit::registry::audit_ci(temp.path()).unwrap().status,
                simit::registry::FeatureStatus::Managed
            );
        }
    }
}

#[test]
fn native_exact_matrix_normalizes_unambiguous_hardware_aliases() {
    let temp = project(
        "[ci.nix_build]\nonly = true\n[ci.nix_build.system_runners]\n\"x86_64-linux\" = \"ubuntu-24.04\"",
    );
    assert!(generate(&temp, &[]).status.success());
    let value = workflow(&temp);
    let script = value["jobs"]["build"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Verify native Nix runner")
        .unwrap()["run"]
        .as_str()
        .unwrap();
    let bin = temp.path().join("alias-bin");
    fs::create_dir(&bin).unwrap();
    for (name, text) in [
        (
            "uname",
            "#!/bin/sh\ncase \"$1\" in -m) printf '%s' \"$FAKE_CPU\";; -s) printf Linux;; *) exit 99;; esac\n",
        ),
        ("nix", "#!/bin/sh\nprintf '%s' \"$FAKE_NIX_SYSTEM\"\n"),
    ] {
        let path = bin.join(name);
        fs::write(&path, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    for (cpu, expected) in [
        ("ppc64le", "powerpc64le-linux"),
        ("ppc64", "powerpc64-linux"),
        ("ppcle", "powerpcle-linux"),
        ("ppc", "powerpc-linux"),
        ("amd64", "x86_64-linux"),
        ("arm64", "aarch64-linux"),
        ("i386", "i686-linux"),
    ] {
        let output = Command::new("bash")
            .args(["-c", script])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("FAKE_CPU", cpu)
            .env("EXPECTED_NIX_SYSTEM", expected)
            .env("FAKE_NIX_SYSTEM", expected)
            .output()
            .unwrap();
        assert!(output.status.success(), "{cpu} -> {expected}: {output:?}");
    }
}

#[test]
fn native_exact_matrix_checks_hardware_when_nix_system_is_overridden() {
    let temp = project(
        r#"[ci.nix_build]
only = true
[ci.nix_build.system_runners]
"x86_64-linux" = "ubuntu-24.04"
[ci.nix_build.extra_env]
NIX_CONFIG = "eval-system = aarch64-linux"
"#,
    );
    let generated = generate(&temp, &[]);
    assert!(generated.status.success(), "{generated:?}");
    let value = workflow(&temp);
    let step = value["jobs"]["build"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "Verify native Nix runner")
        .unwrap();
    let script = step["run"].as_str().unwrap();
    let bin = temp.path().join("hardware-bin");
    fs::create_dir(&bin).unwrap();
    for (name, content) in [
        (
            "nix",
            "#!/bin/sh\nprintf '%s' \"${FAKE_NIX_SYSTEM:-aarch64-linux}\"\nexit \"${FAKE_NIX_STATUS:-0}\"\n",
        ),
        (
            "uname",
            "#!/bin/sh\ncase \"$1\" in\n-s) printf '%s' \"$FAKE_KERNEL\"; exit \"${FAKE_KERNEL_STATUS:-0}\";;\n-m) printf '%s' \"$FAKE_CPU\"; exit \"${FAKE_CPU_STATUS:-0}\";;\n*) exit 99;;\nesac\n",
        ),
    ] {
        let path = bin.join(name);
        fs::write(&path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    for (kernel, cpu, accepted) in [
        ("Linux", "aarch64", true),
        ("Linux", "arm64", true),
        ("Linux", "x86_64", false),
        ("Darwin", "arm64", false),
        ("Linux", "unknown", false),
        ("Windows_NT", "arm64", false),
    ] {
        let output = Command::new("bash")
            .args(["-e", "-c", script])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("EXPECTED_NIX_SYSTEM", "aarch64-linux")
            .env(
                "NIX_CONFIG",
                value["jobs"]["build"]["env"]["NIX_CONFIG"]
                    .as_str()
                    .unwrap(),
            )
            .env("FAKE_KERNEL", kernel)
            .env("FAKE_CPU", cpu)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            accepted,
            "{kernel}/{cpu}: {output:?}"
        );
    }
    // Even an observation that emits the expected value must fail closed on
    // command failure; configured Nix identity must also match native hardware.
    for (name, value) in [
        ("FAKE_CPU_STATUS", "17"),
        ("FAKE_KERNEL_STATUS", "18"),
        ("FAKE_NIX_STATUS", "19"),
        ("FAKE_NIX_SYSTEM", "x86_64-linux"),
    ] {
        let output = Command::new("bash")
            .args(["-c", script])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("EXPECTED_NIX_SYSTEM", "aarch64-linux")
            .env("FAKE_KERNEL", "Linux")
            .env("FAKE_CPU", "aarch64")
            .env("NIX_CONFIG", "eval-system = aarch64-linux")
            .env(name, value)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{name}={value}: {output:?}");
    }
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
    let output = generate(&temp, &["--runtime", "cargo"]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        original,
        fs::read(temp.path().join(".github/workflows/nix-builds.yaml")).unwrap()
    );
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
    for option in ["with_artifacts", "with_audit", "with_msrv"] {
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
    fs::write(&nix, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$SIMIT_NIX_BUILD_RESULTS/arguments\"\nprintf '%s\\n' '[{\"drvPath\":\"/nix/store/fixture.drv\",\"outputs\":{\"out\":\"/nix/store/fixture\"}}]'\nprintf '%s\\n' 'fixture build diagnostic' >&2\nfor argument in \"$@\"; do\n  if [ \"$argument\" = --print-build-logs ]; then\n    printf '%s\\n' 'fixture builder phase diagnostic' >&2\n  fi\ndone\nexit \"$BUILD_EXIT_CODE\"\n").unwrap();
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
            "fixture build diagnostic\nfixture builder phase diagnostic\n"
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
fn direct_nix_matrix_rendering_enforces_retained_hook_and_transport_boundaries() {
    use simit::cli::Platform;
    use simit::config::NixBuildConfig;
    use simit::render::ci::nix_build_matrix_file_with_options;
    use simit::user_config::ResolvedRunner;

    let runner = ResolvedRunner {
        name: None,
        labels: vec!["ubuntu-24.04".into()],
    };
    let installables = vec![".#default".to_owned()];
    let mut options = NixBuildConfig {
        capture_results: true,
        groups: [("default".into(), installables.clone())].into(),
        ..NixBuildConfig::default()
    };
    for phase in ["post_build", "post_build_always", "transport"] {
        options.post_build.clear();
        options.post_build_always.clear();
        options.extra_env.clear();
        match phase {
            "post_build" => options.post_build.push("${{ secrets.TOKEN }}".into()),
            "post_build_always" => options.post_build_always.push("${{ github.token }}".into()),
            _ => {
                options
                    .extra_env
                    .insert("SIMIT_DEPENDENCY_MODE".into(), "override".into());
            }
        }
        let error = nix_build_matrix_file_with_options(
            Platform::Github,
            &runner,
            &installables,
            &[],
            &[],
            &[],
            &options,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains(if phase == "transport" {
                "reserved generator variable"
            } else {
                "Actions expressions"
            }),
            "{error}"
        );
    }
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

#[test]
fn explicit_matrix_qualifies_alongside_prebuild_publication() {
    let prebuild = "\n[prebuild]\nnix_builds = [\".#checks.x86_64-linux.client\"]\nsystem_runners = { x86_64-linux = \"ubuntu-24.04\" }\n";
    let temp = project(&format!(
        "[ci.nix_build]\ncapture_results = true\ncheck_systems = [\"x86_64-linux\"]\n[ci.nix_build.groups]\nall = [\".#checks.x86_64-linux.client\", \".#checks.x86_64-linux.recovery\"]{prebuild}"
    ));
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    let matrix = workflow(&temp);
    assert_eq!(
        matrix["jobs"]["build"]["strategy"]["matrix"]["include"][0]["group"],
        "all"
    );
    let published =
        fs::read_to_string(temp.path().join(".github/workflows/prebuild.yaml")).unwrap();
    assert!(published.contains("'.#checks.x86_64-linux.client'"));
    assert!(!published.contains("recovery"), "{published}");
    assert!(generate(&temp, &["--check", "--diff"]).status.success());

    // Declaring the published subset alone opts [ci].nix_builds into their own matrix, and
    // regenerating only the prebuild keeps that matrix.
    let temp = project(prebuild);
    assert!(generate(&temp, &[]).status.success());
    let matrix = temp.path().join(".github/workflows/nix-builds.yaml");
    assert!(matrix.exists());
    let result = generate(&temp, &["--prebuild-only"]);
    assert!(result.status.success(), "{result:?}");
    assert!(matrix.exists());
    let result = generate(&temp, &["--prebuild-only", "--check"]);
    assert!(result.status.success(), "{result:?}");

    // Without an explicit matrix the prebuild still absorbs [ci].nix_builds.
    let temp = project("\n[prebuild]\nsystem_runners = { x86_64-linux = \"ubuntu-24.04\" }\n");
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    assert!(
        !temp
            .path()
            .join(".github/workflows/nix-builds.yaml")
            .exists()
    );
    let published =
        fs::read_to_string(temp.path().join(".github/workflows/prebuild.yaml")).unwrap();
    assert!(published.contains("'.#checks.x86_64-linux.client'"));
    assert!(published.contains("'.#checks.x86_64-linux.recovery'"));
}

#[test]
fn nix_only_workspace_publishes_crates_without_language_ci_or_flake_check() {
    let temp = project("[ci.nix_build]\nonly = true");
    let config = temp.path().join("simit.toml");
    let content = fs::read_to_string(&config)
        .unwrap()
        .replace("[ci]\n", "[ci]\npublish_crates = true\n");
    fs::write(&config, content).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"core\", \"tool\", \"private\"]\nresolver = \"3\"\n",
    )
    .unwrap();
    for (name, extra) in [
        ("native-core", ""),
        ("native-tool", "publish = false\n"),
        ("native-private", "publish = [\"internal\"]\n"),
    ] {
        let member = temp.path().join(name.trim_start_matches("native-"));
        fs::create_dir_all(member.join("src")).unwrap();
        fs::write(
            member.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT\"\n{extra}"
            ),
        )
        .unwrap();
        fs::write(member.join("src/lib.rs"), "").unwrap();
    }

    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let workflows = temp.path().join(".github/workflows");
    let source = fs::read_to_string(workflows.join("publish-crate-native-core.yaml")).unwrap();
    let publisher =
        fs::read_to_string(workflows.join("trusted-publish-crate-native-core.yaml")).unwrap();
    assert!(publisher.contains("Build Crate native-core"), "{publisher}");
    assert!(!source.contains("nix flake check"), "{source}");
    assert!(temp.path().join("keys/maintainers.gpg").is_file());
    assert!(!workflows.join("publish-crate-native-tool.yaml").exists());
    assert!(!workflows.join("publish-crate-native-private.yaml").exists());
    assert!(!workflows.join("ci.yaml").exists());
    assert!(workflows.join("nix-builds.yaml").is_file());

    let check = generate(&temp, &["--check", "--diff"]);
    assert!(check.status.success(), "{check:?}");
    let audit = simit::registry::audit_ci(temp.path()).unwrap();
    assert_eq!(
        audit.status,
        simit::registry::FeatureStatus::Managed,
        "{audit:?}"
    );
}

#[test]
fn nix_only_crate_publication_rejects_unsupported_shapes_without_writing() {
    for (config_extra, args) in [
        ("publish_strategy = \"release-plz\"\n", vec![]),
        ("", vec!["--maintainers-gpg", "keys/custom.gpg"]),
        ("", vec!["--platform", "gitlab"]),
        ("om_ci = true\n", vec![]),
    ] {
        let temp = project("[ci.nix_build]\nonly = true");
        let config = temp.path().join("simit.toml");
        let content = fs::read_to_string(&config).unwrap().replace(
            "[ci]\n",
            &format!("[ci]\npublish_crates = true\n{config_extra}"),
        );
        fs::write(&config, content).unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"native-core\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT\"\n",
        )
        .unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/lib.rs"), "").unwrap();
        let output = generate(&temp, &args);
        assert!(
            !output.status.success(),
            "{config_extra} {args:?}: {output:?}"
        );
        assert!(!temp.path().join(".github/workflows").exists());
    }
}

fn nix_only_publish_workspace(ci_extra: &str, core_dependencies: &str) -> TempDir {
    let temp = project("[ci.nix_build]\nonly = true");
    let config = temp.path().join("simit.toml");
    let content = fs::read_to_string(&config).unwrap().replace(
        "[ci]\n",
        &format!("[ci]\npublish_crates = true\n{ci_extra}"),
    );
    fs::write(&config, content).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"core\", \"private\"]\nresolver = \"3\"\n",
    )
    .unwrap();
    for (dir, manifest) in [
        (
            "core",
            format!(
                "[package]\nname = \"native-core\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT\"\n[dependencies]\n{core_dependencies}"
            ),
        ),
        (
            "private",
            "[package]\nname = \"native-private\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT\"\npublish = [\"internal\"]\n".to_owned(),
        ),
    ] {
        let member = temp.path().join(dir);
        fs::create_dir_all(member.join("src")).unwrap();
        fs::write(member.join("Cargo.toml"), manifest).unwrap();
        fs::write(member.join("src/lib.rs"), "").unwrap();
    }
    temp
}

#[test]
fn nix_only_crate_publication_requires_a_crates_io_dependency_closure() {
    let temp = nix_only_publish_workspace(
        "",
        "native-private = { path = \"../private\", version = \"0.1.0\" }\n",
    );
    let output = generate(&temp, &[]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("not published to crates.io"));
    assert!(!temp.path().join(".github/workflows").exists());
}

#[test]
fn nix_only_crate_publication_preserves_the_feature_policy() {
    let temp = nix_only_publish_workspace("all_features = false\n", "");
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let source = fs::read_to_string(
        temp.path()
            .join(".github/workflows/publish-crate-native-core.yaml"),
    )
    .unwrap();
    assert!(source.contains("cargo test"), "{source}");
    assert!(!source.contains("--all-features"), "{source}");
}

#[test]
fn nix_only_crate_publication_preserves_the_unit_test_scope() {
    let temp = nix_only_publish_workspace("unit_tests_only = true\n", "");
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let source = fs::read_to_string(
        temp.path()
            .join(".github/workflows/publish-crate-native-core.yaml"),
    )
    .unwrap();
    assert!(source.contains(" --lib"), "{source}");
}

#[test]
fn nix_only_publication_audit_rejects_what_generation_rejects() {
    // Start from valid generated Nix-only CI without publication, then enable a
    // publication shape that generation rejects; the audit must agree.
    let gitlab = TempDir::new().unwrap();
    fs::write(
        gitlab.path().join("flake.nix"),
        "{ outputs = { self }: {}; }\n",
    )
    .unwrap();
    fs::write(
        gitlab.path().join("simit.toml"),
        "[ci]\nplatform = \"gitlab\"\nprovider = \"actions\"\nruntime = \"nix\"\nrunner = \"shared\"\n",
    )
    .unwrap();
    let trust_root = nix_only_publish_workspace("", "");
    let config = trust_root.path().join("simit.toml");
    let content = fs::read_to_string(&config)
        .unwrap()
        .replace("publish_crates = true\n", "");
    fs::write(&config, content).unwrap();
    for (temp, release) in [
        (&gitlab, ""),
        (
            &trust_root,
            "\n[release.signing]\ntrust_root = \"keys/custom.gpg\"\n",
        ),
    ] {
        let output = generate(temp, &[]);
        assert!(output.status.success(), "{output:?}");
        assert!(simit::registry::audit_ci(temp.path()).is_ok());
        let config = temp.path().join("simit.toml");
        let invalid = fs::read_to_string(&config).unwrap().replacen(
            "[ci]\n",
            "[ci]\npublish_crates = true\n",
            1,
        ) + release;
        fs::write(&config, invalid).unwrap();
        assert!(simit::registry::audit_ci(temp.path()).is_err(), "{release}");
        assert!(!generate(temp, &["--check"]).status.success(), "{release}");
    }
}

fn dependent_publish_workspace(strategy: &str) -> TempDir {
    let temp = project("[ci.nix_build]\nonly = true");
    let config = temp.path().join("simit.toml");
    let content = fs::read_to_string(&config).unwrap().replace(
        "[ci]\n",
        &format!("[ci]\npublish_crates = true\npublish_strategy = \"{strategy}\"\n"),
    );
    fs::write(&config, content).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"base\", \"top\"]\nresolver = \"3\"\n",
    )
    .unwrap();
    for (dir, name, dependencies) in [
        ("base", "native-base", ""),
        (
            "top",
            "native-top",
            "native-base = { path = \"../base\", version = \"0.1.0\" }\n",
        ),
    ] {
        let member = temp.path().join(dir);
        fs::create_dir_all(member.join("src")).unwrap();
        fs::write(
            member.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT\"\n[dependencies]\n{dependencies}"
            ),
        )
        .unwrap();
        fs::write(member.join("src/lib.rs"), "").unwrap();
    }
    temp
}

#[test]
fn nix_only_members_publication_rejects_dependent_members() {
    let temp = dependent_publish_workspace("members");
    let output = generate(&temp, &[]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("coordinated"));
    assert!(!temp.path().join(".github/workflows").exists());
}

#[test]
fn nix_only_coordinated_publication_orders_dependent_members() {
    let temp = dependent_publish_workspace("coordinated");
    let output = generate(&temp, &[]);
    assert!(output.status.success(), "{output:?}");
    let workflows = temp.path().join(".github/workflows");
    assert!(workflows.join("publish-workspace.yaml").is_file());
    assert!(workflows.join("trusted-publish-workspace.yaml").is_file());
    assert!(!workflows.join("publish-crate-native-base.yaml").exists());
    assert!(!workflows.join("ci.yaml").exists());
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
}

#[test]
fn published_subset_adds_no_matrix_outside_github_primary_ci() {
    let prebuild = "\n[prebuild]\nnix_builds = [\".#checks.x86_64-linux.client\"]\nsystem_runners = { x86_64-linux = \"ubuntu-24.04\" }\n";
    for platform in ["gitlab", "forgejo"] {
        let temp = project(prebuild);
        let config = temp.path().join("simit.toml");
        let content = fs::read_to_string(&config).unwrap().replace(
            "platform = \"github\"",
            &format!("platform = \"{platform}\""),
        );
        fs::write(&config, content).unwrap();
        let result = generate(&temp, &[]);
        assert!(result.status.success(), "{platform}: {result:?}");
        assert!(
            !temp
                .path()
                .join(".github/workflows/nix-builds.yaml")
                .exists()
        );
    }

    // A GitHub matrix left behind after moving primary CI to Forgejo is obsolete.
    let temp = project(prebuild);
    assert!(generate(&temp, &[]).status.success());
    let stale = temp.path().join(".github/workflows/nix-builds.yaml");
    assert!(stale.exists());
    let config = temp.path().join("simit.toml");
    let content = fs::read_to_string(&config)
        .unwrap()
        .replace("platform = \"github\"", "platform = \"forgejo\"");
    fs::write(&config, content).unwrap();
    let result = generate(&temp, &["--prebuild-only"]);
    assert!(result.status.success(), "{result:?}");
    assert!(!stale.exists());
}

#[test]
fn gitlab_check_reports_stale_github_matrix_and_audits_the_prebuild() {
    let temp = project(
        "\n[prebuild]\nnix_builds = [\".#checks.x86_64-linux.client\"]\nsystem_runners = { x86_64-linux = \"ubuntu-24.04\" }\n",
    );
    assert!(generate(&temp, &[]).status.success());
    let stale = temp.path().join(".github/workflows/nix-builds.yaml");
    assert!(stale.exists());
    let config = temp.path().join("simit.toml");
    let content = fs::read_to_string(&config)
        .unwrap()
        .replace("platform = \"github\"", "platform = \"gitlab\"");
    fs::write(&config, content).unwrap();

    // --check must report what a write would remove, not just compare expected files.
    assert!(generate(&temp, &[]).status.success());
    fs::write(
        &stale,
        "# Generated by simit. Manual edits will be reported as ci=drift.\n",
    )
    .unwrap();
    let result = generate(&temp, &["--check"]);
    assert!(!result.status.success(), "{result:?}");
    assert!(String::from_utf8_lossy(&result.stderr).contains("nix-builds.yaml"));
    assert!(generate(&temp, &[]).status.success());
    assert!(!stale.exists());
    assert!(generate(&temp, &["--check"]).status.success());

    // The GitHub prebuild generated next to .gitlab-ci.yml is expected, not drift.
    assert!(temp.path().join(".github/workflows/prebuild.yaml").exists());
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
}

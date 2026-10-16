use std::{fs, path::Path};

use tempfile::TempDir;

mod common;

fn project(root: &Path, name: &str) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[features]\ndefault = []\nextra = []\n"),
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn example() {}\n").unwrap();
}

fn generate(root: &Path, check: bool) {
    let mut command = common::simit();
    command
        .current_dir(root)
        .env("SIMIT_NO_REGISTRY", "1")
        .args(["init", "ci"]);
    if check {
        command.args(["--check", "--diff"]);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if check {
        let audit = simit::registry::audit_ci(root).unwrap();
        assert_eq!(
            audit.status,
            simit::registry::FeatureStatus::Managed,
            "{audit:?}"
        );
    }
}

#[test]
fn rust_component_selection_is_consistent_across_providers_and_layouts() {
    for (platform, provider, path, runners) in [
        ("github", "actions", ".github/workflows/ci.yaml", ""),
        (
            "forgejo",
            "actions",
            ".forgejo/workflows/ci.yaml",
            "step_runners = { nix-check = \"other\" }\n",
        ),
        ("forgejo", "crow", ".crow/build.yaml", ""),
    ] {
        for (components, evaluation, builds) in [
            ("[\"flake-evaluation\"]", 1, 0),
            ("[\"checks\"]", 0, 1),
            ("[\"flake-evaluation\", \"checks\"]", 0, 1),
            ("[\"flake-wiring\"]", 0, 0),
            ("[]", 0, 1),
        ] {
            let temp = TempDir::new().unwrap();
            project(temp.path(), "demo");
            fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
            fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"{platform}\"\nprovider = \"{provider}\"\nruntime = \"nix\"\nrunner = \"atlas\"\ncomponents = {components}\n{runners}")).unwrap();
            generate(temp.path(), false);
            let workflow = fs::read_to_string(temp.path().join(path)).unwrap();
            assert_eq!(
                workflow.matches("nix flake check --no-build").count(),
                evaluation,
                "{platform}/{provider} {components}: {workflow}"
            );
            let full = workflow
                .lines()
                .filter(|line| line.contains("nix flake check") && !line.contains("--no-build"))
                .count();
            assert_eq!(
                full, builds,
                "{platform}/{provider} {components}: {workflow}"
            );
            assert!(workflow.contains("cargo test"));
            assert_eq!(
                workflow.contains("init flake --check --diff"),
                components.contains("flake-wiring")
            );
            generate(temp.path(), true);
        }
    }
}

#[test]
fn member_workflows_run_repository_gates_once() {
    for (platform, runtime, extra) in [
        ("github", "nix", ""),
        ("github", "cargo", ""),
        ("github", "nix", "independent_lanes = true\n"),
        (
            "forgejo",
            "nix",
            "step_runners = { nix-check = \"other\" }\n",
        ),
        (
            "forgejo",
            "cargo",
            "step_runners = { cargo-test = \"other\" }\n",
        ),
    ] {
        let temp = TempDir::new().unwrap();
        for name in ["first", "second"] {
            project(&temp.path().join(name), name);
        }
        fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"first\", \"second\"]\nresolver = \"3\"\n",
        )
        .unwrap();
        fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"{platform}\"\nruntime = \"{runtime}\"\nworkspace = true\nworkspace_strategy = \"members\"\nwith_audit = true\nwith_deny = true\nwith_docs = true\n{extra}required_gates = [{{ id = \"shared\", run = \"nix run .#shared-contract\" }}]\n")).unwrap();
        generate(temp.path(), false);
        let workflows = ["first", "second"].map(|name| {
            fs::read_to_string(
                temp.path()
                    .join(format!(".{platform}/workflows/ci-{name}.yaml")),
            )
            .unwrap()
        });
        let joined = workflows.join("\n");
        assert_eq!(
            joined.lines().filter(|line| line.contains("run: nix flake check") && !line.contains("--no-build")).count(),
            usize::from(runtime == "nix"),
            "{platform}/{runtime} {extra}: {joined}"
        );
        assert_eq!(joined.matches("name: Format check").count(), 1);
        assert_eq!(joined.matches("run: nix run .#shared-contract").count(), 1);
        assert_eq!(
            joined.matches("cargo audit --").count(),
            1,
            "{platform}/{runtime} {extra}: {joined}"
        );
        assert_eq!(
            joined.matches("cargo deny check").count(),
            1,
            "{platform}/{runtime} {extra}: {joined}"
        );
        for (name, workflow) in ["first", "second"].into_iter().zip(workflows) {
            assert!(workflow.contains(&format!("cargo test -p {name}")));
            assert!(workflow.contains(&format!("cargo clippy -p {name}")));
            assert!(workflow.contains(&format!("cargo doc -p {name}")));
        }
        generate(temp.path(), true);
    }
}

#[test]
fn actions_nextest_retains_doctests_without_repeating_the_test_harness() {
    let temp = TempDir::new().unwrap();
    project(temp.path(), "demo");
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nplatform = \"github\"\nruntime = \"cargo\"\nwith_nextest = true\n",
    )
    .unwrap();
    generate(temp.path(), false);
    let workflow = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
    for features in ["--all-features", "--no-default-features"] {
        assert!(
            workflow
                .lines()
                .any(|line| line.contains("cargo nextest run") && line.contains(features))
        );
        assert!(
            workflow
                .lines()
                .any(|line| line.contains("cargo test --doc") && line.contains(features))
        );
    }
    assert!(
        !workflow
            .lines()
            .any(|line| line.contains("cargo test") && !line.contains("--doc"))
    );
    generate(temp.path(), true);
}

#[test]
fn split_runner_omnix_replacement_has_no_generic_test_or_clippy_duplicate() {
    let temp = TempDir::new().unwrap();
    project(temp.path(), "demo");
    fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
    fs::write(temp.path().join("simit.toml"), "[ci]\nplatform = \"forgejo\"\nruntime = \"nix\"\nrunner = \"atlas\"\nstep_runners = { nix-check = \"other\" }\nom_ci = true\n").unwrap();
    generate(temp.path(), false);
    let workflow = fs::read_to_string(temp.path().join(".forgejo/workflows/ci.yaml")).unwrap();
    assert!(workflow.contains("ci run"));
    assert!(!workflow.contains("cargo test"));
    assert!(!workflow.contains("cargo clippy"));
    generate(temp.path(), true);
}

#[test]
fn crow_nextest_replaces_tests_and_retains_doctests_and_feature_modes() {
    let temp = TempDir::new().unwrap();
    project(temp.path(), "demo");
    fs::write(temp.path().join("simit.toml"), "[ci]\nplatform = \"forgejo\"\nprovider = \"crow\"\nruntime = \"cargo\"\nrunner = \"atlas\"\nwith_nextest = true\n").unwrap();
    generate(temp.path(), false);
    let workflow = fs::read_to_string(temp.path().join(".crow/build.yaml")).unwrap();
    assert!(workflow.contains("cargo nextest run --all-features"));
    assert!(workflow.contains("cargo nextest run --no-default-features"));
    assert!(workflow.contains("cargo test --doc --all-features"));
    assert!(workflow.contains("cargo test --doc --no-default-features"));
    assert!(
        !workflow
            .lines()
            .any(|line| line.contains("cargo test") && !line.contains("--doc"))
    );
    generate(temp.path(), true);
}

#[test]
fn crow_omnix_replacement_has_no_generic_test_or_clippy_duplicate() {
    let temp = TempDir::new().unwrap();
    project(temp.path(), "demo");
    fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
    fs::write(temp.path().join("simit.toml"), "[ci]\nplatform = \"forgejo\"\nprovider = \"crow\"\nruntime = \"nix\"\nrunner = \"atlas\"\nom_ci = true\n").unwrap();
    generate(temp.path(), false);
    let workflow = fs::read_to_string(temp.path().join(".crow/build.yaml")).unwrap();
    assert!(workflow.contains("ci run"));
    assert!(!workflow.contains("cargo test"));
    assert!(!workflow.contains("cargo clippy"));
    generate(temp.path(), true);
}

#[test]
fn ordinary_github_jobs_admit_prs_and_default_branch_pushes_without_cross_event_cancellation() {
    let temp = TempDir::new().unwrap();
    project(temp.path(), "demo");
    fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
    fs::write(temp.path().join("simit.toml"), "[ci]\nplatform = \"github\"\nruntime = \"nix\"\npush_branches = [\"trunk\"]\nnix_builds = [\".#checks.x86_64-linux.contract\"]\nrequired_gates = [{ id = \"shared\", run = \"nix run .#shared-contract\" }]\n[ci.nix_build]\npush_branches = [\"trunk\"]\n").unwrap();
    generate(temp.path(), false);
    for path in ["ci.yaml", "nix-builds.yaml"] {
        let workflow =
            fs::read_to_string(temp.path().join(".github/workflows").join(path)).unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
        for (_, job) in yaml["jobs"].as_mapping().unwrap() {
            assert!(job["if"].is_null());
        }
        assert_eq!(
            yaml["on"]["push"]["branches"],
            serde_yaml::to_value(["trunk"]).unwrap()
        );
        assert!(
            yaml["on"]
                .as_mapping()
                .unwrap()
                .contains_key("pull_request")
        );
        assert!(
            yaml["concurrency"]["group"]
                .as_str()
                .unwrap()
                .contains("github.event_name")
        );
    }
    generate(temp.path(), true);
}

#[test]
fn explicitly_separate_github_events_keep_both_producers() {
    let temp = TempDir::new().unwrap();
    project(temp.path(), "demo");
    fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nplatform = \"github\"\nruntime = \"nix\"\nseparate_events = true\n",
    )
    .unwrap();
    generate(temp.path(), false);
    let workflow = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    assert!(yaml["jobs"]["test"]["if"].is_null());
    generate(temp.path(), true);
}

#[test]
fn nextest_does_not_add_invalid_binary_or_disabled_library_doctests() {
    for (platform, provider, path) in [
        ("github", "actions", ".github/workflows/ci.yaml"),
        ("forgejo", "crow", ".crow/build.yaml"),
    ] {
        for kind in ["binary", "disabled", "unit-only"] {
            let temp = TempDir::new().unwrap();
            project(temp.path(), "demo");
            if kind == "binary" {
                fs::remove_file(temp.path().join("src/lib.rs")).unwrap();
                fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
            } else if kind == "disabled" {
                let manifest = temp.path().join("Cargo.toml");
                fs::write(
                    &manifest,
                    format!(
                        "{}\n[lib]\ndoctest = false\n",
                        fs::read_to_string(&manifest).unwrap()
                    ),
                )
                .unwrap();
            }
            fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"{platform}\"\nprovider = \"{provider}\"\nruntime = \"cargo\"\nrunner = \"atlas\"\nwith_nextest = true\nunit_tests_only = {}\n", kind == "unit-only")).unwrap();
            generate(temp.path(), false);
            let workflow = fs::read_to_string(temp.path().join(path)).unwrap();
            assert!(workflow.contains("cargo nextest run"));
            assert!(!workflow.contains("cargo test --doc"), "{kind}: {workflow}");
            generate(temp.path(), true);
        }
    }
}

#[test]
fn aggregate_nextest_includes_library_doctests_when_the_first_member_is_a_binary() {
    let temp = TempDir::new().unwrap();
    for name in ["aaa-bin", "zzz-lib"] {
        project(&temp.path().join(name), name);
    }
    fs::remove_file(temp.path().join("aaa-bin/src/lib.rs")).unwrap();
    fs::write(temp.path().join("aaa-bin/src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"aaa-bin\", \"zzz-lib\"]\nresolver = \"3\"\n",
    )
    .unwrap();
    fs::write(temp.path().join("simit.toml"), "[ci]\nplatform = \"github\"\nruntime = \"cargo\"\nworkspace = true\nworkspace_strategy = \"aggregate\"\nwith_nextest = true\n").unwrap();
    generate(temp.path(), false);
    let workflow = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
    assert!(workflow.contains("cargo test --doc --workspace --all-features"));
    generate(temp.path(), true);
}

#[test]
fn branch_filters_reject_empty_negative_only_and_multiline_patterns_before_writes() {
    for pattern in ["", "!trunk", "trunk\nother", "trunk\u{2028}other"] {
        for matrix in [false, true] {
            let temp = TempDir::new().unwrap();
            project(temp.path(), "demo");
            fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
            let table = if matrix {
                "nix_builds = [\".#default\"]\n[ci.nix_build]\n"
            } else {
                ""
            };
            let literal = toml_edit::Value::from(pattern).to_string();
            fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"github\"\nruntime = \"nix\"\n{table}push_branches = [{literal}]\n")).unwrap();
            let output = common::simit()
                .current_dir(temp.path())
                .args(["init", "ci"])
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("push_branches"));
            assert!(!temp.path().join(".github/workflows").exists());
        }
    }
}

#[test]
fn flake_and_python_primary_filters_preserve_pr_qualification_and_scope() {
    for python in [false, true] {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
        if python {
            fs::write(
                temp.path().join("pyproject.toml"),
                "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n[tool.uv]\n",
            )
            .unwrap();
            fs::write(temp.path().join("uv.lock"), "version = 1\n").unwrap();
        }
        fs::write(temp.path().join("simit.toml"), "[ci]\nplatform = \"github\"\nruntime = \"nix\"\npush_branches = [\"trunk\"]\ncomponents = [\"flake-evaluation\", \"checks\"]\n").unwrap();
        generate(temp.path(), false);
        let workflow = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
        assert_eq!(
            yaml["on"]["push"]["branches"],
            serde_yaml::to_value(["trunk"]).unwrap()
        );
        assert!(
            yaml["on"]
                .as_mapping()
                .unwrap()
                .contains_key("pull_request")
        );
        if python {
            assert!(workflow.contains("nix flake check --no-build"));
            assert!(workflow.contains("nix build .#checks.x86_64-linux.offline-tests"));
        } else {
            assert!(!workflow.contains("nix flake check --no-build"));
            assert_eq!(workflow.matches("nix flake check").count(), 1);
        }
        generate(temp.path(), true);
    }
}

#[test]
fn project_owned_workspace_command_has_one_ci_owner() {
    let temp = TempDir::new().unwrap();
    for name in ["first", "second"] {
        project(&temp.path().join(name), name);
    }
    fs::write(
        temp.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"first\", \"second\"]\nresolver = \"3\"\n",
    )
    .unwrap();
    fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
    fs::write(temp.path().join("simit.toml"), "[ci]\nplatform = \"github\"\nruntime = \"nix\"\nworkspace = true\ncheck_command = \"cargo xtask ci\"\n").unwrap();
    generate(temp.path(), false);
    let workflow = fs::read_to_string(temp.path().join(".github/workflows/ci-first.yaml")).unwrap();
    assert_eq!(workflow.matches("cargo xtask ci").count(), 1);
    assert!(
        !temp
            .path()
            .join(".github/workflows/ci-second.yaml")
            .exists()
    );
    assert!(!workflow.contains("cargo test"));
    generate(temp.path(), true);
}

#[test]
fn omnix_workspace_replacement_runs_once_and_preserves_member_documentation() {
    for (provider, extra, directory, prefix) in [
        ("actions", "", ".forgejo/workflows", "ci"),
        (
            "actions",
            "step_runners = { nix-check = \"other\" }\n",
            ".forgejo/workflows",
            "ci",
        ),
        ("crow", "", ".crow", "build"),
    ] {
        let temp = TempDir::new().unwrap();
        for name in ["first", "second"] {
            project(&temp.path().join(name), name);
        }
        fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"first\", \"second\"]\nresolver = \"3\"\n",
        )
        .unwrap();
        fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"forgejo\"\nprovider = \"{provider}\"\nruntime = \"nix\"\nrunner = \"atlas\"\nworkspace = true\nom_ci = true\nwith_docs = true\n{extra}")).unwrap();
        generate(temp.path(), false);
        let mut joined = String::new();
        for name in ["first", "second"] {
            let workflow = fs::read_to_string(
                temp.path()
                    .join(format!("{directory}/{prefix}-{name}.yaml")),
            )
            .unwrap();
            assert!(
                workflow.contains(&format!("cargo doc -p {name}")),
                "{workflow}"
            );
            joined.push_str(&workflow);
        }
        assert_eq!(joined.matches(" ci run").count(), 1, "{joined}");
        assert!(!joined.contains("cargo test"));
        assert!(!joined.contains("cargo clippy"));
        generate(temp.path(), true);
    }
}

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
        .env("XDG_CONFIG_HOME", root.join(".config"))
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
fn independent_lanes_respect_flake_component_selection() {
    for (components, evaluation, realization) in [
        ("[\"flake-wiring\"]", false, false),
        ("[\"flake-evaluation\"]", true, false),
        ("[\"checks\"]", true, true),
        ("[]", true, true),
    ] {
        let temp = TempDir::new().unwrap();
        project(temp.path(), "demo");
        fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"github\"\nruntime = \"nix\"\nindependent_lanes = true\ncomponents = {components}\n")).unwrap();
        generate(temp.path(), false);
        let workflow = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
        assert_eq!(
            !yaml["jobs"]["evaluation"].is_null(),
            evaluation,
            "{components}: {workflow}"
        );
        assert_eq!(!yaml["jobs"]["realization"].is_null(), realization);
        assert_eq!(
            workflow.contains("init flake --check --diff"),
            components.contains("flake-wiring")
        );
        generate(temp.path(), true);
    }
}

#[test]
fn omnix_replacement_preserves_selected_flake_wiring_across_layouts() {
    for extra in ["", "step_runners = { nix-check = \"other\" }\n"] {
        let temp = TempDir::new().unwrap();
        project(temp.path(), "demo");
        fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"forgejo\"\nruntime = \"nix\"\nrunner = \"atlas\"\nom_ci = true\ncomponents = [\"flake-wiring\"]\n{extra}")).unwrap();
        generate(temp.path(), false);
        let workflow = fs::read_to_string(temp.path().join(".forgejo/workflows/ci.yaml")).unwrap();
        assert_eq!(
            workflow.matches("init flake --check --diff").count(),
            1,
            "{extra}: {workflow}"
        );
        assert!(workflow.contains(" ci run"));
        assert!(!workflow.contains("nix flake check"));
        generate(temp.path(), true);
    }
}

#[test]
fn split_quality_tools_are_installed_and_run_in_their_selected_job() {
    for (runtime, extra) in [("cargo", ""), ("nix", "om_ci = true\n")] {
        let temp = TempDir::new().unwrap();
        project(temp.path(), "demo");
        fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"forgejo\"\nruntime = \"{runtime}\"\nrunner = \"atlas\"\nwith_audit = true\nwith_deny = true\nwith_docs = true\n{extra}step_runners = {{ quality-tools = \"quality\", cargo-doc = \"docs\" }}\n")).unwrap();
        generate(temp.path(), false);
        let workflow = fs::read_to_string(temp.path().join(".forgejo/workflows/ci.yaml")).unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
        let quality = serde_yaml::to_string(&yaml["jobs"]["quality"]).unwrap();
        let docs = serde_yaml::to_string(&yaml["jobs"]["docs"]).unwrap();
        for command in [
            if runtime == "cargo" {
                "cargo install cargo-audit"
            } else {
                "cargo-audit --version"
            },
            "cargo audit --",
            if runtime == "cargo" {
                "cargo install cargo-deny"
            } else {
                "cargo-deny --version"
            },
            "cargo deny check",
        ] {
            assert!(quality.contains(command), "{runtime}: {quality}");
            assert!(!docs.contains(command), "{runtime}: {docs}");
        }
        assert!(docs.contains("cargo doc"));
        generate(temp.path(), true);
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
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"{platform}\"\nruntime = \"{runtime}\"\nrunner = \"atlas\"\nworkspace = true\nworkspace_strategy = \"members\"\nwith_audit = true\nwith_deny = true\nwith_docs = true\n{extra}required_gates = [{{ id = \"shared\", run = \"nix run .#shared-contract\" }}]\n")).unwrap();
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
fn branch_filters_cannot_silently_retire_explicit_dual_event_producers() {
    for matrix in [false, true] {
        let temp = TempDir::new().unwrap();
        project(temp.path(), "demo");
        fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
        let table = if matrix {
            "nix_builds = [\".#default\"]\n[ci.nix_build]\n"
        } else {
            ""
        };
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"github\"\nruntime = \"nix\"\n{table}separate_events = true\npush_branches = [\"trunk\"]\n")).unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("push_branches") && error.contains("separate_events"),
            "{error}"
        );
        assert!(!temp.path().join(".github/workflows").exists());
    }
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
        temp.path().join("aaa-bin/Cargo.toml"),
        "[package]\nname = \"aaa-bin\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"aaa-bin\", \"zzz-lib\"]\nresolver = \"3\"\n",
    )
    .unwrap();
    fs::write(temp.path().join("simit.toml"), "[ci]\nplatform = \"github\"\nruntime = \"cargo\"\nworkspace = true\nworkspace_strategy = \"aggregate\"\nwith_nextest = true\n").unwrap();
    generate(temp.path(), false);
    let workflow = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
    assert!(workflow.contains("cargo test --doc --workspace --all-features"));
    for command in ["cargo nextest run", "cargo test --doc", "cargo clippy"] {
        assert!(
            workflow.lines().any(|line| line.contains(command)
                && line.contains("--workspace")
                && line.contains("--no-default-features")),
            "{workflow}"
        );
    }
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
        if !python {
            assert_eq!(
                yaml["on"]["push"]["tags"],
                serde_yaml::to_value(["**"]).unwrap()
            );
        }
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

#[test]
fn crow_preserves_each_members_msrv_including_omnix_replacement() {
    for (runtime, omnix) in [("cargo", false), ("nix", false), ("nix", true)] {
        let temp = TempDir::new().unwrap();
        for (name, version) in [("first", "1.88"), ("second", "1.89")] {
            let root = temp.path().join(name);
            project(&root, name);
            let manifest = root.join("Cargo.toml");
            fs::write(
                &manifest,
                fs::read_to_string(&manifest).unwrap().replace(
                    "edition = \"2024\"",
                    &format!("edition = \"2024\"\nrust-version = \"{version}\""),
                ),
            )
            .unwrap();
        }
        fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"first\", \"second\"]\nresolver = \"3\"\n",
        )
        .unwrap();
        fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"forgejo\"\nprovider = \"crow\"\nruntime = \"{runtime}\"\nrunner = \"atlas\"\nworkspace = true\nom_ci = {omnix}\nwith_msrv = true\n")).unwrap();
        generate(temp.path(), false);
        for (name, version) in [("first", "1.88"), ("second", "1.89")] {
            let workflow =
                fs::read_to_string(temp.path().join(format!(".crow/build-{name}.yaml"))).unwrap();
            let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
            let step = yaml["steps"]
                .as_sequence()
                .unwrap()
                .iter()
                .find(|step| step["name"] == "cargo-msrv")
                .expect("every member must retain its MSRV check");
            let command = serde_yaml::to_string(&step["commands"]).unwrap();
            assert!(command.contains(&format!("-p {name}")), "{command}");
            assert!(
                command.contains("--all-targets --all-features"),
                "{command}"
            );
            if runtime == "cargo" {
                assert!(
                    command.contains(&format!("rustup toolchain install {version}")),
                    "{command}"
                );
                assert!(
                    command.contains(&format!("cargo +{version} check")),
                    "{command}"
                );
            } else {
                assert!(
                    command.contains("nix develop .#msrv -c cargo check"),
                    "{command}"
                );
            }
        }
        generate(temp.path(), true);
    }
}

#[test]
fn unit_only_tests_select_valid_targets_across_providers_and_harnesses() {
    for (platform, provider, path) in [
        ("github", "actions", ".github/workflows/ci.yaml"),
        ("forgejo", "actions", ".forgejo/workflows/ci.yaml"),
        ("forgejo", "crow", ".crow/build.yaml"),
    ] {
        for nextest in [false, true] {
            for kind in ["binary", "library", "disabled-doctests"] {
                let temp = TempDir::new().unwrap();
                project(temp.path(), "demo");
                if kind == "binary" {
                    fs::remove_file(temp.path().join("src/lib.rs")).unwrap();
                    fs::write(
                        temp.path().join("src/main.rs"),
                        "fn main() {}\n#[test]\nfn binary_unit() { assert_eq!(2 + 2, 4); }\n",
                    )
                    .unwrap();
                } else if kind == "disabled-doctests" {
                    let manifest = temp.path().join("Cargo.toml");
                    fs::write(
                        &manifest,
                        fs::read_to_string(&manifest).unwrap() + "\n[lib]\ndoctest = false\n",
                    )
                    .unwrap();
                }
                fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform = \"{platform}\"\nprovider = \"{provider}\"\nruntime = \"cargo\"\nrunner = \"atlas\"\nwith_nextest = {nextest}\nunit_tests_only = true\n")).unwrap();
                generate(temp.path(), false);
                generate(temp.path(), true);
                let workflow = fs::read_to_string(temp.path().join(path)).unwrap();
                let commands: Vec<_> = workflow
                    .lines()
                    .filter(|line| {
                        line.contains("cargo test") || line.contains("cargo nextest run")
                    })
                    .collect();
                assert_eq!(commands.len(), 2, "{workflow}");
                for command in commands {
                    let expected = if kind == "binary" {
                        " --bins"
                    } else {
                        " --lib"
                    };
                    assert!(
                        command.contains(expected),
                        "{platform}/{provider}/{kind}/{nextest}: {command}"
                    );
                    assert!(!command.contains(" --doc"), "{command}");
                }
                if platform == "github" && kind == "binary" && !nextest {
                    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
                    let command = yaml["jobs"]["test"]["steps"]
                        .as_sequence()
                        .unwrap()
                        .iter()
                        .filter_map(|step| step["run"].as_str())
                        .find(|run| run.starts_with("cargo test") && run.contains("--all-features"))
                        .unwrap();
                    let output = std::process::Command::new("cargo")
                        .current_dir(temp.path())
                        .args(command.split_whitespace().skip(1))
                        .output()
                        .unwrap();
                    assert!(output.status.success(), "{output:?}");
                    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
                }
                if platform == "github" {
                    fs::write(
                        temp.path().join("simit.toml"),
                        "[ci]\nplatform = \"github\"\n",
                    )
                    .unwrap();
                    generate(temp.path(), true);
                }
            }
        }
    }
}

#[test]
fn aggregate_unit_test_selection_uses_all_selected_members() {
    for second_has_library in [false, true] {
        let temp = TempDir::new().unwrap();
        for name in ["first", "second"] {
            let root = temp.path().join(name);
            project(&root, name);
            if name == "first" || !second_has_library {
                fs::remove_file(root.join("src/lib.rs")).unwrap();
                fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
            } else {
                let manifest = root.join("Cargo.toml");
                fs::write(
                    &manifest,
                    fs::read_to_string(&manifest).unwrap() + "\n[lib]\ndoctest = false\n",
                )
                .unwrap();
            }
        }
        fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"first\", \"second\"]\nresolver = \"3\"\n",
        )
        .unwrap();
        fs::write(temp.path().join("simit.toml"), "[ci]\nplatform = \"github\"\nruntime = \"cargo\"\nworkspace = true\nworkspace_strategy = \"aggregate\"\nunit_tests_only = true\nwith_nextest = true\n").unwrap();
        generate(temp.path(), false);
        generate(temp.path(), true);
        let workflow = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
        let target = if second_has_library {
            "--lib"
        } else {
            "--bins"
        };
        assert!(
            workflow.contains(&format!("cargo nextest run --workspace {target}")),
            "{workflow}"
        );
    }
}

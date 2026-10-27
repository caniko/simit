use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn init_package(with_flake: bool) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "demo"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "MIT"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

    if with_flake {
        fs::write(root.join("flake.nix"), "{}\n").unwrap();
    }

    temp
}

#[test]
fn primary_plan_is_versioned_read_only_and_preserves_other_owners() {
    let temp = init_package(false);
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nplatform = 'github'\nprovider = 'actions'\nruntime = 'cargo'\n",
    )
    .unwrap();
    fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
    let publisher = temp.path().join(".github/workflows/publish-crate.yaml");
    fs::write(&publisher, "project-owned publisher\n").unwrap();
    let before = fs::read(temp.path().join("simit.toml")).unwrap();
    let output = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--primary-only", "--plan-json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["schema"], "simit.ci-update-plan.v1");
    assert!(
        plan["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["relative_path"] == ".github/workflows/ci.yaml")
    );
    assert!(!temp.path().join(".github/workflows/ci.yaml").exists());
    assert_eq!(fs::read(temp.path().join("simit.toml")).unwrap(), before);
    assert_eq!(
        fs::read_to_string(publisher).unwrap(),
        "project-owned publisher\n"
    );
}

#[test]
fn full_plan_patches_changed_project_policy_without_resolving_trust() {
    let temp = init_package(false);
    let root = temp.path();
    // An unexportable signing key makes any trust-root resolution fail.
    fs::write(
        root.join("simit.toml"),
        "[ci]\nplatform = 'github'\nprovider = 'actions'\nruntime = 'cargo'\nwith_deny = true\nwith_artifacts = true\n[release.signing]\nkey = '0000000000000000000000000000000000000000'\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        simit()
            .current_dir(root)
            .env_remove("SIMIT_MAINTAINERS_GPG")
            .env("GNUPGHOME", root.join("no-keys"))
            .args(args)
            .output()
            .unwrap()
    };
    assert!(!run(&["init", "ci"]).status.success());
    // Publishing workflows fetch the committed trust root, so a plan needs one.
    let missing = run(&["init", "ci", "--plan-json"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("committed release trust root"));
    fs::create_dir(root.join("keys")).unwrap();
    fs::copy(
        common::maintainer_key_path(),
        root.join("keys/maintainers.gpg"),
    )
    .unwrap();
    // An untracked trust root is not what the publishing workflows fetch.
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "--quiet"]);
    let untracked = run(&["init", "ci", "--plan-json"]);
    assert!(!untracked.status.success());
    assert!(String::from_utf8_lossy(&untracked.stderr).contains("committed release trust root"));
    git(&["add", "keys/maintainers.gpg"]);
    let tree = git(&["write-tree"]);
    let commit = git(&["commit-tree", "-m", "trust root", &tree]);
    git(&["update-ref", "HEAD", &commit]);
    let before = fs::read(root.join("simit.toml")).unwrap();
    let plan = || {
        let output = run(&["init", "ci", "--plan-json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let paths = |plan: &serde_json::Value, key: &str| {
        plan[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|file| file["relative_path"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let first = plan();
    assert_eq!(first["schema"], "simit.ci-update-plan.v2");
    let files = paths(&first, "files");
    assert!(files.contains(&".github/workflows/publish-crate.yaml".to_owned()));
    assert!(!files.iter().any(|path| path.starts_with("keys/")));
    assert_eq!(paths(&first, "patches"), ["deny.toml", "simit.toml"]);
    assert_eq!(fs::read(root.join("simit.toml")).unwrap(), before);
    assert!(!root.join("deny.toml").exists() && !root.join(".github").exists());

    for file in first["files"]
        .as_array()
        .unwrap()
        .iter()
        .chain(first["patches"].as_array().unwrap())
    {
        let path = root.join(file["relative_path"].as_str().unwrap());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, file["content"].as_str().unwrap()).unwrap();
    }
    let second = plan();
    assert_eq!(second["schema"], "simit.ci-update-plan.v1");
    assert!(second.get("patches").is_none());
    assert_eq!(second["files"], first["files"]);
}

#[test]
fn full_plan_rejects_flake_config_it_cannot_persist() {
    let temp = init_package(false);
    let root = temp.path();
    fs::write(
        root.join("flake.nix"),
        "{ outputs = _: { simitConfig = {}; }; }\n",
    )
    .unwrap();
    // A fake evaluator keeps the test independent of a Nix installation.
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(
        bin.join("nix"),
        "#!/bin/sh\nprintf '%s\\n' '{\"ci\":{\"platform\":\"github\",\"provider\":\"actions\",\"runtime\":\"cargo\"}}'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(bin.join("nix"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let plan = |args: &[&str]| {
        simit()
            .current_dir(root)
            .env("PATH", &path)
            .args(args)
            .output()
            .unwrap()
    };
    let primary = plan(&["init", "ci", "--primary-only", "--plan-json"]);
    assert!(
        primary.status.success(),
        "{}",
        String::from_utf8_lossy(&primary.stderr)
    );
    let full = plan(&["init", "ci", "--plan-json"]);
    assert!(!full.status.success());
    assert!(
        String::from_utf8_lossy(&full.stderr).contains("flake outputs.simitConfig"),
        "{}",
        String::from_utf8_lossy(&full.stderr)
    );
    assert!(!root.join("simit.toml").exists());
}

#[test]
fn nix_format_jobs_are_uncached_across_providers_and_job_layouts() {
    for (platform, provider, path, step_runners) in [
        ("github", "actions", ".github/workflows/ci.yaml", ""),
        (
            "forgejo",
            "actions",
            ".forgejo/workflows/ci.yaml",
            "step_runners = { cargo-fmt = \"atlas-nix-trusted\" }",
        ),
        ("forgejo", "crow", ".crow/build.yaml", ""),
    ] {
        let temp = init_package(true);
        fs::write(
            temp.path().join("simit.toml"),
            format!("[ci]\nplatform = \"{platform}\"\nprovider = \"{provider}\"\nruntime = \"nix\"\n{step_runners}\n"),
        )
        .unwrap();
        let output = simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let workflow = read(&temp.path().join(path));
        assert_yaml_parses(&workflow);
        let invocation = if provider == "crow" {
            "exec \"$$formatter_exe\" --ci"
        } else {
            "exec \"$formatter_exe\" --ci"
        };
        assert!(
            workflow.contains("formatter_program") && workflow.contains(invocation),
            "{platform}/{provider}: {workflow}"
        );
        assert!(!workflow.contains("treefmt --fail-on-change"));
    }
}

#[test]
fn private_github_runner_generation_preserves_the_compatible_checkout_baseline() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nplatform = 'github'\nprovider = 'actions'\nruntime = 'nix'\nrunner = 'private-legacy-linux'\nnix_builds = ['.#default']\n[ci.nix_build]\ncapture_results = true\n",
    )
    .unwrap();
    let result = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--publish-crates",
            "--publish-strategy",
            "members",
        ])
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    for path in [
        ".github/workflows/ci.yaml",
        ".github/workflows/publish-crate.yaml",
        ".github/workflows/trusted-publish-crate.yaml",
        ".github/workflows/nix-builds.yaml",
    ] {
        let workflow = read(&temp.path().join(path));
        assert_yaml_parses(&workflow);
        assert!(
            workflow.contains(if path.contains("publish-crate") {
                // Candidate builds must not inherit credentials mounted on a
                // private CI/release runner, including with the Nix runtime.
                "runs-on: ubuntu-24.04"
            } else {
                "runs-on: private-legacy-linux"
            }),
            "{path}: {workflow}"
        );
        // Private labels do not attest the Node24 runner minimum. Strict
        // publication jobs use their own known hosted capacity.
        assert!(
            workflow.contains(&simit::render::ci::github_action_ref(
                "actions/checkout",
                if path.contains("publish-crate") {
                    "v7.0.1"
                } else {
                    "v4.3.1"
                }
            )),
            "{path}: {workflow}"
        );
        assert!(
            path.contains("publish-crate")
                || !workflow.contains("actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1"),
            "{path}: {workflow}"
        );
        if path.ends_with("nix-builds.yaml") {
            assert!(workflow.contains("Upload Nix build evidence"));
            assert!(
                workflow.contains(&simit::render::ci::github_action_ref(
                    "actions/upload-artifact",
                    "v4.6.2"
                )),
                "{path}: {workflow}"
            );
            assert!(
                !workflow.contains(&simit::render::ci::github_action_ref(
                    "actions/upload-artifact",
                    "v7.0.1"
                )),
                "{path}: {workflow}"
            );
        }
    }
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--publish-crates", "--check", "--diff"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn native_runner_matrices_keep_unknown_workers_on_compatible_actions() {
    for (arm_runner, checkout, upload) in [
        ("ubuntu-24.04-arm", "v7.0.1", "v7.0.2"),
        ("private-arm-worker", "v4.3.1", "v4.6.2"),
    ] {
        let temp = init_flake_only();
        fs::write(
            temp.path().join("simit.toml"),
            format!(
                r#"[ci]
platform = "github"
provider = "actions"
runtime = "nix"
nix_builds = [".#default"]
[ci.nix_system_runners]
x86_64-linux = "ubuntu-24.04"
aarch64-linux = "{arm_runner}"
[prebuild]
release_archives = true
[release.artifacts]
prebuild_binaries = true
"#
            ),
        )
        .unwrap();
        let output = simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        for path in [
            ".github/workflows/ci.yaml",
            ".github/workflows/prebuild.yaml",
        ] {
            let workflow = read(&temp.path().join(path));
            assert_yaml_parses(&workflow);
            assert!(
                workflow.contains(&simit::render::ci::github_action_ref(
                    "actions/checkout",
                    checkout
                )),
                "{path}: {workflow}"
            );
            if path.ends_with("prebuild.yaml") {
                assert!(
                    workflow.contains(&simit::render::ci::github_action_ref(
                        "actions/upload-artifact",
                        upload
                    )),
                    "{path}: {workflow}"
                );
                assert!(workflow.contains("ref: ${{ github.sha }}"));
                assert!(workflow.contains("persist-credentials: false"));
            }
        }
        assert!(
            simit()
                .current_dir(temp.path())
                .args(["init", "ci", "--check", "--diff"])
                .status()
                .unwrap()
                .success()
        );
    }
}

#[test]
fn github_required_gates_have_job_scoped_permissions() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
provider = "actions"
runtime = "nix"
required_gates = [
    { id = "integration", run = "nix run .#integration" },
    { id = "module-smoke", run = "nix build .#module-smoke" },
]
"#,
    )
    .unwrap();
    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .status()
        .unwrap();
    assert!(status.success());
    let workflow = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    for id in ["gate-integration", "gate-module-smoke"] {
        let job = &yaml["jobs"][id];
        assert_eq!(job["permissions"]["contents"].as_str(), Some("read"));
        assert!(job["steps"].as_sequence().is_some());
    }
}

#[test]
fn required_gate_evidence_survives_failure_and_round_trips_through_publishers() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
provider = "actions"
runtime = "cargo"
publish_crates = true
[[ci.required_gates]]
id = "browser"
run = "cargo test"
timeout_minutes = 60
[ci.required_gates.artifacts]
name = "browser-evidence"
paths = ["target/evidence/", "!target/evidence/private/**", ".playwright/"]
retention_days = 14
"#,
    )
    .unwrap();
    for check in [false, true] {
        let mut command = simit();
        command.current_dir(temp.path()).args(["init", "ci"]);
        if check {
            command.args(["--check", "--diff"]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for (file, prefix, suffix) in [
        ("ci.yaml", "", ""),
        ("publish-crate.yaml", "", ".source"),
        (
            "trusted-publish-crate.yaml",
            ".simit-release-source/",
            ".demo",
        ),
    ] {
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&read(&temp.path().join(".github/workflows").join(file))).unwrap();
        let steps = yaml["jobs"]
            .as_mapping()
            .unwrap()
            .values()
            .filter_map(|job| job["steps"].as_sequence())
            .find(|steps| {
                steps
                    .iter()
                    .any(|step| step["name"] == "Required gate browser")
            })
            .unwrap();
        let gate = steps
            .iter()
            .position(|step| step["name"] == "Required gate browser")
            .unwrap();
        let upload = &steps[gate + 1];
        assert_eq!(steps[gate]["id"], "simit-required-gate-browser");
        assert_eq!(steps[gate]["timeout-minutes"], 60);
        assert_eq!(
            upload["if"],
            "always() && steps.simit-required-gate-browser.outcome != '' && steps.simit-required-gate-browser.outcome != 'skipped'"
        );
        assert_eq!(upload["timeout-minutes"], 5);
        assert_eq!(upload["with"]["include-hidden-files"], true);
        assert!(
            upload["uses"]
                .as_str()
                .unwrap()
                .starts_with("actions/upload-artifact@")
        );
        assert_eq!(upload["with"]["name"], format!("browser-evidence{suffix}"));
        assert_eq!(
            upload["with"]["path"],
            format!(
                "{prefix}target/evidence/\n!{prefix}target/evidence/private/**\n{prefix}.playwright/\n"
            )
        );
        assert_eq!(upload["with"]["retention-days"], 14);
        assert!(steps[gate]["continue-on-error"].is_null());
    }
    let ci: serde_yaml::Value =
        serde_yaml::from_str(&read(&temp.path().join(".github/workflows/ci.yaml"))).unwrap();
    assert_eq!(ci["jobs"]["gate-browser"]["timeout-minutes"], 75);
    let config: toml_edit::DocumentMut = read(&temp.path().join("simit.toml")).parse().unwrap();
    assert_eq!(
        config["ci"]["required_gates"][0]["artifacts"]["retention_days"].as_integer(),
        Some(14)
    );
}

#[test]
fn required_gate_source_evidence_names_do_not_alias_crate_archives() {
    for runtime in ["cargo", "nix"] {
        let temp = init_package(true);
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform='github'\nruntime='{runtime}'\npublish_crates=true\n[[ci.required_gates]]\nid='browser'\nrun='cargo test'\n[ci.required_gates.artifacts]\nname='simit-crate-demo-1'\npaths=['target/evidence/']\n")).unwrap();
        let output = simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let workflow: serde_yaml::Value = serde_yaml::from_str(&read(
            &temp.path().join(".github/workflows/publish-crate.yaml"),
        ))
        .unwrap();
        let uploads = workflow["jobs"]["publish"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|step| step["with"]["name"].as_str())
            .collect::<Vec<_>>();
        assert!(uploads.contains(&"simit-crate-demo-${{ github.run_attempt }}"));
        assert!(
            uploads.contains(&"simit-crate-demo-1.source"),
            "{uploads:?}"
        );
        assert!(!uploads.contains(&"simit-crate-demo-1"));
    }
}

#[test]
fn trusted_gate_artifact_paths_preserve_runner_absolute_syntax() {
    let temp = init_package(true);
    let paths = [
        "target/evidence/",
        "!target/evidence/private/**",
        "${{ runner.temp }}/evidence",
        "!${{ runner.temp }}/private/**",
        "${{ github.workspace }}/evidence",
        "target/${{ format('env-{0}', vars.env) }}/evidence",
        "/var/evidence/",
        "~/evidence/",
        r"C:\evidence\",
        "C:/evidence/",
        r"\\server\evidence\",
        r"C:relative\evidence",
    ];
    let paths = toml_edit::Value::Array(paths.into_iter().collect());
    fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform='github'\nruntime='cargo'\npublish_crates=true\n[[ci.required_gates]]\nid='browser'\nrun='cargo test'\n[ci.required_gates.artifacts]\nname='evidence'\npaths={paths}\n")).unwrap();
    let output = simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let workflow: serde_yaml::Value = serde_yaml::from_str(&read(
        &temp
            .path()
            .join(".github/workflows/trusted-publish-crate.yaml"),
    ))
    .unwrap();
    let steps = workflow["jobs"]["verify-demo"]["steps"]
        .as_sequence()
        .unwrap();
    let upload = steps
        .iter()
        .find(|step| step["name"] == "Retain required gate evidence")
        .unwrap();
    assert_eq!(upload["with"]["include-hidden-files"], true);
    assert_eq!(
        upload["with"]["path"],
        concat!(
            ".simit-release-source/target/evidence/\n",
            "!.simit-release-source/target/evidence/private/**\n",
            "${{ runner.temp }}/evidence\n!${{ runner.temp }}/private/**\n",
            "${{ github.workspace }}/evidence\n",
            ".simit-release-source/target/${{ format('env-{0}', vars.env) }}/evidence\n",
            "/var/evidence/\n~/evidence/\n",
            "C:\\evidence\\\nC:/evidence/\n\\\\server\\evidence\\\n",
            ".simit-release-source/C:relative\\evidence\n",
        )
    );
}

#[test]
fn required_gate_artifacts_reject_invalid_or_unsupported_configuration() {
    for (platform, artifacts) in [
        ("forgejo", "name='evidence'\npaths=['target/evidence']"),
        ("github", "name=''\npaths=['target/evidence']"),
        ("github", "name=' evidence'\npaths=['target/evidence']"),
        ("github", "name='evidence '\npaths=['target/evidence']"),
        (
            "github",
            "name='\u{feff}evidence'\npaths=['target/evidence']",
        ),
        ("github", "name='evidence'\npaths=[' target/evidence']"),
        ("github", "name='evidence'\npaths=['target/evidence ']"),
        (
            "github",
            "name='evidence'\npaths=['\u{feff}target/evidence']",
        ),
        ("github", "name='evidence'\npaths=[]"),
        (
            "github",
            "name='evidence'\npaths=[\"target/evidence\\nother\"]",
        ),
        (
            "github",
            "name='evidence'\npaths=['target/evidence']\nretention_days=0",
        ),
        (
            "github",
            "name='evidence'\npaths=['target/evidence']\nretention_days=91",
        ),
    ] {
        let temp = init_package(true);
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform='{platform}'\nprovider='actions'\n[[ci.required_gates]]\nid='browser'\nrun='cargo test'\n[ci.required_gates.artifacts]\n{artifacts}\n")).unwrap();
        let output = simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("artifact"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!temp.path().join(".github/workflows/ci.yaml").exists());
    }
}

#[test]
fn required_gate_artifacts_reject_step_environment_expressions() {
    for expression in [
        "${{ env.EVIDENCE_DIR }}",
        "${{ ENV['EVIDENCE_DIR'] }}",
        "${{ toJSON(env) }}",
        "${{ format('}}{0}', env.EVIDENCE_DIR) }}",
        "${{ format('${{}}{0}', env.EVIDENCE_DIR) }}",
    ] {
        for field in ["name", "paths"] {
            let temp = init_package(true);
            let (name, path) = if field == "name" {
                (expression, "target/evidence/")
            } else {
                ("evidence", expression)
            };
            fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform='github'\nruntime='cargo'\n[[ci.required_gates]]\nid='browser'\nrun='false'\nenv={{EVIDENCE_DIR='target/evidence/'}}\n[ci.required_gates.artifacts]\nname={}\npaths=[{}]\n", toml_edit::Value::from(name), toml_edit::Value::from(path))).unwrap();
            let output = simit()
                .current_dir(temp.path())
                .args(["init", "ci"])
                .output()
                .unwrap();
            assert!(
                !output.status.success(),
                "{field}: {expression}: {output:?}"
            );
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("artifact expressions must not reference env"),
                "{output:?}"
            );
            assert!(!temp.path().join(".github/workflows/ci.yaml").exists());
        }
    }
}

#[test]
fn required_gate_artifacts_use_the_effective_cli_backend() {
    for configured in ["", "platform='forgejo'\nprovider='crow'\n"] {
        let temp = init_package(true);
        fs::write(temp.path().join("simit.toml"), format!("[ci]\n{configured}runtime='cargo'\nrunner='ubuntu-24.04'\n[[ci.required_gates]]\nid='browser'\nrun='false'\n[ci.required_gates.artifacts]\nname='evidence'\npaths=['target/evidence']\n")).unwrap();
        let output = simit()
            .current_dir(temp.path())
            .args([
                "init",
                "ci",
                "--platform",
                "github",
                "--ci-provider",
                "actions",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(temp.path().join(".github/workflows/ci.yaml").exists());
    }
}

#[test]
fn required_gate_artifact_upload_matches_the_ci_runner() {
    for (runner, version) in [("ubuntu-24.04", "v7.0.1"), ("private-linux", "v4.6.2")] {
        let temp = init_package(true);
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform='github'\nruntime='cargo'\nrunner='{runner}'\n[[ci.required_gates]]\nid='browser'\nrun='false'\n[ci.required_gates.artifacts]\nname='evidence'\npaths=['target/evidence']\n")).unwrap();
        let output = simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let workflow = read(&temp.path().join(".github/workflows/ci.yaml"));
        assert!(
            workflow.contains(&simit::render::ci::github_action_ref(
                "actions/upload-artifact",
                version
            )),
            "{workflow}"
        );
    }
}

#[test]
fn required_gate_artifact_timeouts_reserve_setup_and_upload_budget() {
    for (timeout, accepted) in [(345, true), (346, false), (360, false)] {
        let temp = init_package(true);
        fs::write(temp.path().join("simit.toml"), format!("[ci]\nplatform='github'\nruntime='cargo'\nrunner='ubuntu-24.04'\n[[ci.required_gates]]\nid='browser'\nrun='false'\ntimeout_minutes={timeout}\n[ci.required_gates.artifacts]\nname='evidence'\npaths=['target/evidence']\n")).unwrap();
        let output = simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert_eq!(output.status.success(), accepted, "{timeout}: {output:?}");
        if accepted {
            let yaml: serde_yaml::Value =
                serde_yaml::from_str(&read(&temp.path().join(".github/workflows/ci.yaml")))
                    .unwrap();
            assert_eq!(yaml["jobs"]["gate-browser"]["timeout-minutes"], 360);
        }
    }
}

fn init_flake_only() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("flake.nix"),
        "{ outputs = { self }: {}; }\n",
    )
    .unwrap();
    temp
}

#[test]
fn rust_provider_migrations_reconcile_all_generated_ci_outputs() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nprovider = \"crow\"\nplatform = \"forgejo\"\nruntime = \"nix\"\nrunner = \"atlas-nix-trusted\"\n",
    )
    .unwrap();
    assert!(
        simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(["init", "ci"])
            .status()
            .unwrap()
            .success()
    );
    for name in ["pages.yaml", "publish-crate.yaml", "release.yml"] {
        fs::write(
            temp.path().join(".crow").join(name),
            format!(
                "{}\nname: retained\n",
                simit::render::ci::GENERATED_WORKFLOW_MARKER
            ),
        )
        .unwrap();
    }
    fs::write(
        temp.path().join(".crow/build-operator.yaml"),
        "name: handwritten\n",
    )
    .unwrap();

    for args in [
        vec![
            "init",
            "ci",
            "--platform",
            "github",
            "--ci-provider",
            "actions",
            "--runner",
            "ubuntu-24.04",
        ],
        vec![
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--ci-provider",
            "crow",
            "--runner",
            "atlas-nix-trusted",
        ],
    ] {
        let output = simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(&args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let checked = simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(&args)
            .arg("--check")
            .output()
            .unwrap();
        assert!(
            checked.status.success(),
            "{}",
            String::from_utf8_lossy(&checked.stderr)
        );
        assert_eq!(
            simit::registry::audit_ci(temp.path()).unwrap().status,
            simit::registry::FeatureStatus::ManagedExtra
        );
    }
    assert!(temp.path().join(".crow/release.yml").exists());
    assert!(temp.path().join(".crow/build-operator.yaml").exists());
    assert!(!temp.path().join(".github/workflows/ci.yaml").exists());
}

#[test]
fn python_default_ci_builds_treefmt_check() {
    let temp = init_python_project();
    fs::remove_file(temp.path().join("simit.toml")).unwrap();
    fs::remove_file(temp.path().join("flake.nix")).unwrap();
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "flake"])
            .status()
            .unwrap()
            .success()
    );
    let output = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
    assert!(workflow.contains("checks.x86_64-linux.formatting"));
    assert!(!workflow.contains("checks.x86_64-linux.uv-format"));
    let output = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--ci-provider",
            "crow",
            "--runtime",
            "nix",
            "--runner",
            "atlas-nix-trusted",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = fs::read_to_string(temp.path().join(".crow/ci.yaml")).unwrap();
    assert!(workflow.contains("checks.x86_64-linux.formatting"));
    assert!(!workflow.contains("checks.x86_64-linux.uv-format"));
}

fn init_python_project() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::write(
        root.join("pyproject.toml"),
        r#"[project]
name = "py-demo"
version = "0.1.0"

[project.scripts]
py-demo = "py_demo:main"

[project.optional-dependencies]
cpu = ["pytest"]

[dependency-groups]
dev = ["mypy", "pytest", "ruff"]
"#,
    )
    .unwrap();
    fs::write(root.join("uv.lock"), "").unwrap();
    fs::write(
        root.join("flake.nix"),
        r#"{
  inputs.py-harbor.url = "git+https://github.com/caniko/harbor-py.git";
  outputs = { self, py-harbor, ... }: {
    checks.x86_64-linux.offline-tests = {};
    checks.x86_64-linux.typecheck = {};
  };
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("simit.toml"),
        r#"[flake]
scope = "full"
mode = "custom"
backend = "py-harbor"

[flake.expected_outputs]
checks = ["offline-tests", "typecheck"]

[ci]
runtime = "nix"
"#,
    )
    .unwrap();
    temp
}

fn init_workspace_fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    copy_dir(Path::new("tests/fixtures/workspace-ci"), temp.path());
    temp
}

fn init_diverging_workspace_fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    copy_dir(
        Path::new("tests/fixtures/workspace-ci-diverging"),
        temp.path(),
    );
    temp
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).unwrap();
        }
    }
}

fn write_chocolatey_config(root: &Path) {
    fs::write(
        root.join("simit.toml"),
        r#"[chocolatey]
authors = "Example Maintainers"
description = "demo binary"
project_url = "https://example.com/demo"
download_repo = "foo/demo"
"#,
    )
    .unwrap();
}

fn write_scoop_config(root: &Path) {
    fs::write(
        root.join("simit.toml"),
        r#"[scoop]
bucket_url = "https://example.com/scoop-demo.git"
description = "demo binary"
homepage = "https://example.com/demo"
license = "MIT"
download_repo = "foo/demo"
"#,
    )
    .unwrap();
}

fn write_windows_packager_config(root: &Path) {
    fs::write(
        root.join("simit.toml"),
        r#"[chocolatey]
authors = "Example Maintainers"
description = "demo binary"
project_url = "https://example.com/demo"
download_repo = "foo/demo"

[scoop]
bucket_url = "https://example.com/scoop-demo.git"
description = "demo binary"
homepage = "https://example.com/demo"
license = "MIT"
download_repo = "foo/demo"
"#,
    )
    .unwrap();
}

fn write_release_smoke_config(root: &Path) {
    fs::write(
        root.join("simit.toml"),
        r#"[release.smoke]
command = "nix run .#release-smoke --"
"#,
    )
    .unwrap();
}

fn write_ci_customization_config(root: &Path) {
    fs::write(
        root.join("simit.toml"),
        r#"[ci]
extra_setup = [
  "apt-get update && apt-get install -y --no-install-recommends postgresql-client"
]
extra_env = { SKILLNET_TEST_PG_URL = "${{ secrets.SKILLNET_TEST_PG_URL }}" }
required_secrets = ["SKILLNET_TEST_PG_URL"]
required_env = ["VSCE_PAT_FILE", "OVSX_PAT_FILE"]
"#,
    )
    .unwrap();
}

fn write_user_runner_config(root: &Path) {
    let config_dir = root.join(".xdg/simit");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true

[ci.runners.windows_atlas]
platform = "forgejo"
labels = ["windows-atlas"]
os = "windows"
arch = "x86_64"
runtimes = ["cargo"]

[ci.defaults.forgejo]
cargo = "atlas"
nix = "atlas"
release = "atlas"
windows = "windows_atlas"
"#,
    )
    .unwrap();
}

fn write_user_runner_config_with_omnix_ref(root: &Path) {
    let config_dir = root.join(".xdg/simit");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true

[ci.runners.windows_atlas]
platform = "forgejo"
labels = ["windows-atlas"]
os = "windows"
arch = "x86_64"
runtimes = ["cargo"]

[ci.defaults.forgejo]
cargo = "atlas"
nix = "atlas"
release = "atlas"
windows = "windows_atlas"

[ci.tools.omnix]
ref = "github:user/pin/v3"
"#,
    )
    .unwrap();
}

fn simit_with_user_config(root: &Path) -> Command {
    write_user_runner_config(root);
    let mut command = simit();
    command.env("XDG_CONFIG_HOME", root.join(".xdg"));
    command
}

fn simit_with_multilabel_user_config(root: &Path) -> Command {
    let config_dir = root.join(".xdg/simit");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["self-hosted", "atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true

[ci.defaults.forgejo]
cargo = "atlas"
nix = "atlas"
release = "atlas"
"#,
    )
    .unwrap();
    let mut command = simit();
    command.env("XDG_CONFIG_HOME", root.join(".xdg"));
    command
}

fn simit_with_split_runtime_user_config(root: &Path) -> Command {
    let config_dir = root.join(".xdg/simit");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo"]
trusted = true

[ci.runners.atlas_nix]
platform = "forgejo"
labels = ["atlas-nix-trusted"]
os = "linux"
arch = "x86_64"
runtimes = ["nix"]
trusted = true

[ci.defaults.forgejo]
cargo = "atlas"
nix = "atlas_nix"
release = "atlas"
"#,
    )
    .unwrap();
    let mut command = simit();
    command.env("XDG_CONFIG_HOME", root.join(".xdg"));
    command
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

fn assert_yaml_parses(text: &str) {
    serde_yaml::from_str::<serde_yaml::Value>(text).unwrap();
}

fn assert_all_branch_push_trigger(workflow: &str) {
    assert!(workflow.contains("branches: [\"**\"]"));
    assert!(workflow.contains("tags-ignore: [\"**\"]"));
    assert!(!workflow.contains("branches: [trunk]"));
}

fn assert_homebrew_run_block_indentation(workflow: &str) {
    let mut in_homebrew_step = false;
    let mut in_run_block = false;
    let mut saw_script_line = false;

    for line in workflow.lines() {
        if line == "      - name: Publish Homebrew tap" {
            in_homebrew_step = true;
            continue;
        }
        if in_homebrew_step && line == "        run: |" {
            in_run_block = true;
            continue;
        }
        if in_run_block && line.starts_with("      - name: ") {
            break;
        }
        if in_run_block && !line.is_empty() {
            assert!(
                line.starts_with("          "),
                "Homebrew run block line is under-indented: {line:?}"
            );
            saw_script_line = true;
        }
    }

    assert!(saw_script_line, "Homebrew run block was not found");
}

fn assert_release_integrity_steps(workflow: &str) {
    assert!(workflow.contains("keys/maintainers.gpg"));
    assert!(workflow.contains("git verify-tag \"$tag\""));
    assert!(workflow.contains("keys/minisign.pub"));
    assert!(workflow.contains("MINISIGN_SECRET_KEY: ${{ secrets.MINISIGN_SECRET_KEY }}"));
    assert!(workflow.contains("MINISIGN_PASSWORD: ${{ secrets.MINISIGN_PASSWORD }}"));
    assert!(workflow.contains("COSIGN_PRIVATE_KEY: ${{ secrets.COSIGN_PRIVATE_KEY }}"));
    assert!(workflow.contains("release/SHA256SUMS.txt.minisig"));
    assert!(workflow.contains("jq -er '.value' > \"$oidc_token\""));
    assert!(
        workflow.contains("cosign sign-blob --yes --identity-token \"$(cat \"$oidc_token\")\"")
    );
    assert!(
        workflow.contains("cosign attest-blob --yes --identity-token \"$(cat \"$oidc_token\")\"")
    );
    assert!(workflow.contains("--type slsaprovenance1"));
    assert!(!workflow.contains("--output-attestation"));
    assert!(workflow.contains("--bundle \"${file}.intoto.bundle\""));
    assert!(
        workflow
            .contains("::error::keyless Sigstore failed and COSIGN_PRIVATE_KEY is unset for $file")
    );
    assert!(workflow.contains("COSIGN_PRIVATE_KEY fallback"));
}

fn assert_maintainer_key_written(root: &Path) {
    let key = read(&root.join("keys/maintainers.gpg"));
    assert!(key.contains("PGP PUBLIC KEY BLOCK") || !key.is_empty());
}

#[test]
fn generates_forgejo_nix_workflows() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: atlas"));
    assert!(ci.contains("NIX_CONFIG: \"experimental-features = nix-command flakes\""));
    assert!(ci.contains("XDG_CACHE_HOME: \"/tmp/.cache\""));
    assert!(ci.contains("CARGO_HOME: \"/tmp/.cargo\""));
    assert!(ci.contains("run: echo \"$CARGO_HOME/bin\" >> \"$GITHUB_PATH\""));
    assert!(ci.contains("group: ${{ github.workflow_ref }}-${{ github.ref }}"));
    assert!(ci.contains("uses: https://code.forgejo.org/actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1"));
    assert!(!ci.contains("pull_request:"));
    assert!(!ci.contains("uses: https://github.com/cachix/install-nix-action@v31"));
    assert!(!ci.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(!ci.contains("path: ~/.cargo/bin"));
    assert!(!ci.contains("command -v cargo-nextest"));
    assert!(ci.contains("run: nix flake check"));
    assert!(ci.contains(
        "run: nix develop -c cargo clippy --all-targets --all-features -- --deny warnings"
    ));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: atlas"));
    assert!(publish.contains("NIX_CONFIG: \"experimental-features = nix-command flakes\""));
    assert!(publish.contains("XDG_CACHE_HOME: \"/tmp/.cache\""));
    assert!(publish.contains("CARGO_HOME: \"/tmp/.cargo\""));
    assert!(publish.contains("run: echo \"$CARGO_HOME/bin\" >> \"$GITHUB_PATH\""));
    assert!(!publish.contains("uses: https://github.com/cachix/install-nix-action@v31"));
    assert!(!publish.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(!publish.contains("path: ~/.cargo/bin"));
    assert!(!publish.contains("command -v cargo-nextest"));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(!publish.contains("  push:\n    tags:\n"));
    assert!(publish.contains("  workflow_dispatch:\n"));
    assert!(publish.contains("grep -Eq '^[0-9]+\\.[0-9]+\\.[0-9]+$'"));
    assert!(publish.contains("keys/maintainers.gpg"));
    assert!(publish.contains("git verify-tag \"$tag\""));
    assert!(publish.contains(
        r#"nix develop -c cargo pkgid -p demo | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#
    ));
    assert!(!publish.contains("inputs:\n"));
    assert!(publish.contains("CRATES_IO_API_TOKEN: ${{ secrets.CRATES_IO_API_TOKEN }}"));
    assert!(publish.contains("CRATES_IO_API_TOKEN is required"));
    assert!(publish.contains("export CARGO_REGISTRY_TOKEN="));
    assert!(publish.contains("https://crates.io/api/v1/crates/${crate_name}/${version}"));
    assert!(publish.contains("checksum matches; resuming (already published)"));
    assert!(publish.contains("invalid, mismatched or yanked registry version"));
    assert!(!publish.contains("cargo login"));
    assert_maintainer_key_written(temp.path());
}

#[test]
fn github_ci_generates_declared_nix_installable_matrix() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
provider = "actions"
runtime = "nix"
nix_builds = [".#oci-api", ".#oci-etl"]
extra_setup = ["echo prepare-runner"]
"#,
    )
    .unwrap();
    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--ci-provider",
            "actions",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/nix-builds.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("permissions:\n  contents: read"));
    assert!(
        workflow
            .contains("group: ${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.head_ref || github.ref_name }}")
    );
    assert!(workflow.contains("runs-on: ubuntu-latest"));
    assert!(workflow.contains("fail-fast: false"));
    assert!(workflow.contains("max-parallel: 2"));
    assert!(workflow.contains("- \".#oci-api\""));
    assert!(workflow.contains("- \".#oci-etl\""));
    assert!(workflow.contains("run: nix build --no-link \"$INSTALLABLE\""));
    assert!(workflow.contains("run: echo prepare-runner"));
    assert!(!workflow.contains("secrets."));
}

#[test]
fn github_nix_only_uses_native_system_runner_matrix() {
    let temp = init_flake_only();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
provider = "actions"
platform = "github"
runtime = "nix"

[ci.nix_system_runners]
"aarch64-darwin" = "macos-15"
"aarch64-linux" = "ubuntu-24.04-arm"
"x86_64-linux" = "ubuntu-24.04"
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_yaml_parses(&workflow);
    let parsed: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    assert_eq!(parsed["permissions"]["contents"], "read");
    let group = parsed["concurrency"]["group"].as_str().unwrap();
    let resolve = |head_repository: Option<&str>,
                   head_branch: Option<&str>,
                   repository: &str,
                   branch: &str| {
        let resolved = group
            .replace("${{ github.workflow }}", "Nix flake check")
            .replace(
                "${{ github.event.pull_request.head.repo.full_name || github.repository }}",
                head_repository.unwrap_or(repository),
            )
            .replace(
                "${{ github.event.pull_request.head.ref || github.ref_name }}",
                head_branch.unwrap_or(branch),
            );
        assert!(!resolved.contains("${{"), "unrecognized group: {resolved}");
        resolved
    };
    let push = resolve(None, None, "owner/project", "feature");
    let same_repository_pr = resolve(
        Some("owner/project"),
        Some("feature"),
        "owner/project",
        "1/merge",
    );
    let first_fork = resolve(
        Some("alice/project"),
        Some("feature"),
        "owner/project",
        "2/merge",
    );
    let second_fork = resolve(
        Some("bob/project"),
        Some("feature"),
        "owner/project",
        "3/merge",
    );
    assert_ne!(
        first_fork, second_fork,
        "unrelated forks must not cancel each other"
    );
    assert_ne!(
        push, first_fork,
        "fork PRs must not cancel repository pushes"
    );
    assert_eq!(push, same_repository_pr);
    assert_ne!(push, resolve(None, None, "owner/project", "other-feature"));
    assert_eq!(parsed["concurrency"]["cancel-in-progress"], true);
    assert_eq!(
        parsed["concurrency"]["group"],
        "${{ github.workflow }}-${{ github.event.pull_request.head.repo.full_name || github.repository }}-${{ github.event.pull_request.head.ref || github.ref_name }}"
    );
    assert_eq!(parsed["jobs"]["flake-check"]["strategy"]["max-parallel"], 2);
    assert_eq!(parsed["jobs"]["flake-check"]["timeout-minutes"], 60);
    assert!(workflow.contains("- system: aarch64-darwin\n            runner: macos-15"));
    assert!(workflow.contains("- system: aarch64-linux\n            runner: ubuntu-24.04-arm"));
    assert!(workflow.contains("- system: x86_64-linux\n            runner: ubuntu-24.04"));
    assert!(workflow.contains("runs-on: ${{ matrix.runner }}"));
    assert!(workflow.contains(
        "nix eval --impure --raw --expr builtins.currentSystem)\" = \"${{ matrix.system }}\""
    ));
    assert!(
        workflow
            .contains("nix flake check --no-update-lock-file --max-jobs 1 --cores 2 --system \"${{ matrix.system }}\"")
    );
    assert!(!workflow.contains("--all-systems"));

    let check = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--check",
            "--diff",
        ])
        .status()
        .unwrap();
    assert!(check.success());
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        workflow,
        read(&temp.path().join(".github/workflows/ci.yaml"))
    );
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
}

#[test]
fn github_prebuild_push_branches_filter_only_branch_pushes() {
    let temp = init_flake_only();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[prebuild]
push_branches = ["trunk"]

[prebuild.system_runners]
"x86_64-linux" = "ubuntu-24.04"

[ci]
provider = "actions"
platform = "github"
runtime = "nix"
nix_builds = [".#server"]
"#,
    )
    .unwrap();
    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());
    let workflow = read(&temp.path().join(".github/workflows/prebuild.yaml"));
    assert_yaml_parses(&workflow);
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let on = &yaml["on"];
    assert_eq!(on["push"]["branches"][0].as_str(), Some("trunk"));
    assert_eq!(on["push"]["branches"].as_sequence().unwrap().len(), 1);
    assert_eq!(on["push"]["tags-ignore"][0].as_str(), Some("**"));
    assert!(on.get("pull_request").is_some());
    // No release target or release archives: nothing calls a release entry.
    assert!(on.get("workflow_call").is_none());
}

#[test]
fn github_prebuild_without_release_omits_reusable_release_entry() {
    let attic = r#"
[release.attic]
cache = "demo"
url = "https://attic.example"
token_name = "demo"
token_secret = "ATTIC_TOKEN"
"#;
    let base = format!(
        r#"[prebuild]
publish_attic = true
attic_app = ".#push-flake-inputs"
nix_builds = [".#server"]

[prebuild.system_runners]
"x86_64-linux" = "ubuntu-24.04"

[ci]
provider = "crow"
platform = "forgejo"
runtime = "nix"
{attic}"#
    );
    let render = |config: &str| {
        let temp = init_flake_only();
        fs::write(temp.path().join("simit.toml"), config).unwrap();
        let status = simit()
            .current_dir(temp.path())
            .args([
                "init",
                "ci",
                "--prebuild-only",
                "--platform",
                "github",
                "--ci-provider",
                "actions",
                "--runtime",
                "nix",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        read(&temp.path().join(".github/workflows/prebuild.yaml"))
    };

    // Publish-only: no caller, so no changelog/keyring gate and no reusable
    // secret indirection; trunk pushes read the repository secret directly.
    let workflow = render(&base);
    assert_yaml_parses(&workflow);
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    assert!(yaml["on"].get("workflow_call").is_none());
    assert!(!workflow.contains("inputs.release"), "{workflow}");
    assert!(!workflow.contains("Validate tag"));
    assert!(!workflow.contains("CHANGELOG.md"));
    assert!(!workflow.contains("maintainers.gpg"));
    assert!(workflow.contains("ATTIC_TOKEN: ${{ secrets.ATTIC_TOKEN }}"));
    assert!(workflow.contains("nix run '.#push-flake-inputs'"));

    // A GitHub release target calls the entry with `attic_token`; direct
    // push runs still fall back to the repository secret.
    let workflow = render(&format!(
        "{base}\n[release.github]\nrepo = \"example/demo\"\n"
    ));
    assert_yaml_parses(&workflow);
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    assert!(yaml["on"]["workflow_call"]["inputs"]["release"].is_mapping());
    assert!(workflow.contains("Validate tag"));
    assert!(workflow.contains("ATTIC_TOKEN: ${{ secrets.attic_token || secrets.ATTIC_TOKEN }}"));
}

#[test]
fn github_prebuild_is_reusable_publishes_attic_and_tracks_drift() {
    let temp = init_flake_only();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[prebuild]
release_archives = true
publish_attic = true
attic_app = ".#push-flake-inputs"

[prebuild.system_runners]
"aarch64-linux" = "ubuntu-24.04-arm"
"x86_64-linux" = "ubuntu-24.04"

[ci]
provider = "actions"
platform = "github"
runtime = "nix"
nix_builds = [".#server", ".#worker"]

[release.artifacts]
prebuild_binaries = true
substituters = ["https://cache.example"]
trusted_public_keys = ["cache.example:abc"]

[release.attic]
cache = "demo"
url = "https://attic.example"
token_name = "demo"
token_secret = "ATTIC_TOKEN"
"#,
    )
    .unwrap();
    fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
    fs::write(
        temp.path().join(".github/workflows/nix-builds.yaml"),
        format!(
            "{}\nname: obsolete\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        ),
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        !temp
            .path()
            .join(".github/workflows/nix-builds.yaml")
            .exists()
    );

    let workflow = read(&temp.path().join(".github/workflows/prebuild.yaml"));
    assert_yaml_parses(&workflow);
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    // A push and its PR must each retain their required prebuild check.
    assert_eq!(
        yaml["concurrency"]["group"].as_str(),
        Some(
            "${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.event.pull_request.head.repo.full_name || github.repository }}-${{ github.head_ref || github.ref_name }}-prebuild"
        )
    );
    assert_eq!(
        yaml["concurrency"]["cancel-in-progress"].as_str(),
        Some(
            "${{ !inputs.release && (github.event_name == 'push' || github.event_name == 'pull_request') }}"
        )
    );
    assert!(workflow.contains("workflow_call:\n    inputs:\n      release:"));
    assert!(workflow.contains("secrets:\n      attic_token:\n        required: true"));
    assert!(workflow.contains("runs-on: ${{ matrix.runner }}"));
    assert!(workflow.contains("timeout-minutes: 180"));
    assert!(workflow.contains("--print-build-logs --max-jobs 1 --cores 2"));
    assert!(workflow.contains("nix build '.#server' --out-link '.simit-prebuild/ci-0'"));
    assert!(
        workflow.contains("nix build '.#release-bundle' --out-link '.simit-prebuild/release-0'")
    );
    assert!(workflow.contains("nix run '.#push-flake-inputs'"));
    assert!(workflow.contains("HARBOR_ATTIC_MANIFEST: attic-paths.txt"));
    assert!(workflow.contains("github.event.repository.default_branch"));
    assert!(workflow.contains("name: flake-inputs-${{ matrix.system }}"));
    assert!(!workflow.contains("attic push"));
    assert!(!workflow.contains("default-server"));
    assert!(!workflow.contains("attic login"));

    let audit = simit::registry::audit_ci(temp.path()).unwrap();
    assert_eq!(audit.status, simit::registry::FeatureStatus::Managed);
    fs::write(
        temp.path().join(".github/workflows/prebuild.yaml"),
        format!("{workflow}\n# drift\n"),
    )
    .unwrap();
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Drift
    );
}

#[test]
fn github_prebuild_is_additive_to_forgejo_crow_ci() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[prebuild]
publish_attic = true
attic_app = ".#push-flake-inputs"

[prebuild.system_runners]
"x86_64-linux" = "ubuntu-24.04"

[ci]
provider = "crow"
platform = "forgejo"
runtime = "nix"
runner = "atlas-nix-trusted"

[release.attic]
cache = "demo"
url = "https://attic.example"
token_name = "demo"
token_secret = "ATTIC_TOKEN"
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(temp.path().join(".crow/build.yaml").is_file());

    let prebuild = read(&temp.path().join(".github/workflows/prebuild.yaml"));
    assert_yaml_parses(&prebuild);
    assert!(prebuild.contains("runs-on: ${{ matrix.runner }}"));
    assert!(prebuild.contains("nix run '.#push-flake-inputs'"));
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
}

#[test]
fn crow_prebuild_publishes_attic_on_default_branch_pushes() {
    let config = r#"[prebuild]
provider = "crow"
publish_attic = true
attic_app = ".#push-flake-inputs"
nix_builds = [".#packages.x86_64-linux.server"]

[ci]
provider = "actions"
platform = "github"
runtime = "nix"

[ci.crow]
nix_image = "nixos/nix:2.34.8"
labels = { group = "infra", platform = "linux/amd64" }
skip_clone = true

[release.attic]
cache = "demo"
url = "https://attic.example"
token_name = "demo"
token_secret = "DEMO_ATTIC_TOKEN"
"#;
    let temp = init_flake_only();
    fs::write(temp.path().join("simit.toml"), config).unwrap();
    let prebuild_only = |check: bool| {
        let mut args = vec!["init", "ci", "--prebuild-only"];
        if check {
            args.push("--check");
        }
        simit()
            .current_dir(temp.path())
            .args(args)
            .status()
            .unwrap()
            .success()
    };
    assert!(prebuild_only(false));
    assert!(!temp.path().join(".github/workflows/prebuild.yaml").exists());

    let workflow = read(&temp.path().join(".crow/prebuild.yaml"));
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    // Publication is pinned to pushes of the default branch; PRs never see the secret.
    assert_eq!(yaml["when"].as_sequence().unwrap().len(), 1);
    assert_eq!(yaml["when"][0]["event"].as_str(), Some("push"));
    assert_eq!(
        yaml["when"][0]["branch"].as_str(),
        Some("${CI_REPO_DEFAULT_BRANCH}")
    );
    assert_eq!(yaml["labels"]["group"].as_str(), Some("infra"));
    // The prebuild builds the checked-out flake, so it keeps the clone.
    assert!(yaml.get("skip_clone").is_none());
    let step = &yaml["steps"][0];
    assert_eq!(step["image"].as_str(), Some("nixos/nix:2.34.8"));
    // The push app reads ATTIC_TOKEN whatever the Crow secret is named.
    assert_eq!(
        step["environment"]["ATTIC_TOKEN"]["from_secret"].as_str(),
        Some("DEMO_ATTIC_TOKEN")
    );
    let commands = step["commands"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|command| command.as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        commands,
        [
            "mkdir -p .simit-prebuild",
            "nix build '.#packages.x86_64-linux.server' --out-link '.simit-prebuild/ci-0' --print-build-logs",
            "test -n \"$${ATTIC_TOKEN:-}\"",
            "nix run '.#push-flake-inputs'",
        ]
    );
    assert!(prebuild_only(true));

    // The full GitHub-primary scope keeps the Crow prebuild, and the registry expects it.
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--runner", "ubuntu-24.04"])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(read(&temp.path().join(".crow/prebuild.yaml")), workflow);
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );

    // GitHub runner labels and branch filters have no Crow meaning.
    let runners = "\n[prebuild.system_runners]\n\"x86_64-linux\" = \"ubuntu-24.04\"\n";
    fs::write(temp.path().join("simit.toml"), format!("{config}{runners}")).unwrap();
    assert!(!prebuild_only(true));

    // Moving back to the GitHub prebuild retires the Crow pipeline and its secret use.
    let github = config.replace("provider = \"crow\"\n", "");
    fs::write(temp.path().join("simit.toml"), format!("{github}{runners}")).unwrap();
    assert!(!prebuild_only(true));
    assert!(prebuild_only(false));
    assert!(!temp.path().join(".crow/prebuild.yaml").exists());
    assert!(
        temp.path()
            .join(".github/workflows/prebuild.yaml")
            .is_file()
    );
}

#[test]
fn crow_prebuild_qualify_runs_secret_free_on_pull_requests() {
    let config = r#"[prebuild]
provider = "crow"
publish_attic = true
attic_app = ".#push-flake-inputs"
nix_builds = [".#packages.x86_64-linux.server"]
qualify = [".#checks.x86_64-linux.clippy"]

[ci]
provider = "actions"
platform = "github"
runtime = "nix"

[ci.crow]
nix_image = "nixos/nix:2.34.8"
labels = { group = "infra", platform = "linux/amd64" }

[release.attic]
cache = "demo"
url = "https://attic.example"
token_name = "demo"
token_secret = "ATTIC_TOKEN"
"#;
    let temp = init_flake_only();
    fs::write(temp.path().join("simit.toml"), config).unwrap();
    let prebuild_only = |check: bool| {
        let mut args = vec!["init", "ci", "--prebuild-only"];
        if check {
            args.push("--check");
        }
        simit()
            .current_dir(temp.path())
            .args(args)
            .status()
            .unwrap()
            .success()
    };
    assert!(prebuild_only(false));
    assert!(temp.path().join(".crow/prebuild.yaml").is_file());

    // One workflow per command, so each gets the repository timeout.
    let qualify = |job: &str| {
        temp.path()
            .join(format!(".crow/prebuild-qualify-{job}.yaml"))
    };
    let expected = [
        ("flake-check", "nix flake check --no-build"),
        (
            "clippy",
            "nix build '.#checks.x86_64-linux.clippy' --no-link --print-build-logs",
        ),
        (
            "server",
            "nix build '.#packages.x86_64-linux.server' --no-link --print-build-logs",
        ),
    ];
    for (job, command) in expected {
        let workflow = read(&qualify(job));
        let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
        assert_eq!(
            yaml["name"].as_str(),
            Some(format!("qualify-{job}").as_str())
        );
        assert_eq!(yaml["when"][0]["event"].as_str(), Some("pull_request"));
        assert_eq!(yaml["when"][1]["event"].as_str(), Some("push"));
        assert_eq!(
            yaml["when"][1]["branch"].as_str(),
            Some("${CI_REPO_DEFAULT_BRANCH}")
        );
        assert_eq!(yaml["labels"]["group"].as_str(), Some("infra"));
        // Pull requests run these workflows, so they never receive a secret.
        assert!(!workflow.contains("from_secret"));
        assert_eq!(yaml["steps"].as_sequence().unwrap().len(), 1);
        assert_eq!(yaml["steps"][0]["commands"][0].as_str(), Some(command));
        assert_eq!(yaml["steps"][0]["commands"].as_sequence().unwrap().len(), 1);
    }
    let workflow = read(&qualify("clippy"));
    assert!(prebuild_only(true));

    // The full GitHub-primary scope keeps the workflows, and the registry expects them.
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--runner", "ubuntu-24.04"])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(read(&qualify("clippy")), workflow);
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );

    // Dropping qualify retires the generated workflows in both scopes, and
    // the earlier single-file pipeline as well.
    let without = config.replace("qualify = [\".#checks.x86_64-linux.clippy\"]\n", "");
    fs::write(temp.path().join("simit.toml"), &without).unwrap();
    assert!(!prebuild_only(true));
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--runner", "ubuntu-24.04"])
            .status()
            .unwrap()
            .success()
    );
    assert!(!qualify("clippy").exists());
    fs::write(qualify("clippy"), &workflow).unwrap();
    fs::write(temp.path().join(".crow/qualify.yaml"), &workflow).unwrap();
    assert!(prebuild_only(false));
    assert!(!qualify("clippy").exists());
    assert!(!temp.path().join(".crow/qualify.yaml").exists());

    // The GitHub prebuild qualifies PRs itself, so qualify is Crow-only.
    let github = config.replace("provider = \"crow\"\n", "")
        + "\n[prebuild.system_runners]\n\"x86_64-linux\" = \"ubuntu-24.04\"\n";
    fs::write(temp.path().join("simit.toml"), github).unwrap();
    assert!(!prebuild_only(false));
}

#[test]
fn crow_prebuild_rejects_unqualified_crow_ci_nix_builds() {
    let temp = init_package(true);
    let config = r#"[prebuild]
provider = "crow"
publish_attic = true
attic_app = ".#push-flake-inputs"
nix_builds = [".#server"]

[release.attic]
cache = "demo"
url = "https://attic.example"
token_name = "demo"
token_secret = "ATTIC_TOKEN"
"#;
    let crow_ci = |config: &str| {
        fs::write(temp.path().join("simit.toml"), config).unwrap();
        simit()
            .current_dir(temp.path())
            .args([
                "init",
                "ci",
                "--ci-provider",
                "crow",
                "--platform",
                "forgejo",
                "--runtime",
                "nix",
                "--runner",
                "crow-agent",
            ])
            .output()
            .unwrap()
    };
    assert!(crow_ci(config).status.success());
    assert!(temp.path().join(".crow/prebuild.yaml").is_file());

    // A CLI-selected Crow primary never builds [ci].nix_builds on PRs either.
    let output = crow_ci(&format!("{config}\n[ci]\nnix_builds = [\".#checks\"]\n"));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not qualify [ci].nix_builds"));
}

#[test]
fn crow_python_ci_keeps_the_crow_prebuild() {
    let temp = init_python_project();
    let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap_or_default();
    fs::write(
        temp.path().join("simit.toml"),
        format!(
            "{config}\n[prebuild]\nprovider = \"crow\"\npublish_attic = true\nattic_app = \".#push-flake-inputs\"\nnix_builds = [\".#default\"]\n\n[release.attic]\ncache = \"demo\"\nurl = \"https://attic.example\"\ntoken_name = \"demo\"\ntoken_secret = \"ATTIC_TOKEN\"\n"
        ),
    )
    .unwrap();
    let output = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--ci-provider",
            "crow",
            "--runtime",
            "nix",
            "--runner",
            "atlas-nix-trusted",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_crow_push_only_maintained_branches(&temp.path().join(".crow/ci.yaml"));
    assert!(temp.path().join(".crow/prebuild.yaml").is_file());
}

#[test]
fn crow_ci_rejects_non_forgejo_platforms() {
    let temp = init_package(true);

    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--ci-provider",
            "crow",
            "--platform",
            "github",
            "--runner",
            "crow-agent",
        ])
        .status()
        .unwrap();
    assert!(!status.success());
    assert!(!temp.path().join(".crow/build.yaml").exists());
    assert!(!temp.path().join(".github/workflows/ci.yaml").exists());

    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--ci-provider",
            "crow",
            "--platform",
            "gitlab",
            "--runner",
            "crow-agent",
        ])
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn crow_ci_accepts_default_forgejo_platform() {
    let temp = init_package(true);

    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--ci-provider",
            "crow",
            "--runner",
            "crow-agent",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert_crow_push_only_maintained_branches(&temp.path().join(".crow/build.yaml"));
}

// Branch pushes besides the default branch and updater refs duplicate PR runs.
fn assert_crow_push_only_maintained_branches(path: &Path) {
    let yaml: serde_yaml::Value = serde_yaml::from_str(&read(path)).unwrap();
    let when = yaml["when"].as_sequence().unwrap();
    assert_eq!(when.len(), 2);
    assert_eq!(when[0]["event"].as_str(), Some("push"));
    assert_eq!(
        when[0]["branch"],
        serde_yaml::from_str::<serde_yaml::Value>(
            r#"["${CI_REPO_DEFAULT_BRANCH}", "simit/update/**"]"#
        )
        .unwrap()
    );
    assert_eq!(when[1]["event"].as_str(), Some("pull_request"));
    assert!(when[1]["branch"].is_null());
}

#[test]
fn crow_ci_can_delegate_project_checks_to_a_single_command() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
provider = "crow"
platform = "forgejo"
runner = "codefloe-global"
runtime = "nix"

[ci.crow]
check_command = "nix develop -c cargo run --locked -p demo-ci -- ci"
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .status()
        .unwrap();
    assert!(status.success());
    let workflow = fs::read_to_string(temp.path().join(".crow/build.yaml")).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let steps = yaml["steps"].as_sequence().unwrap();
    assert!(steps.iter().any(|step| {
        step["name"] == "project-check"
            && step["commands"][0] == "nix develop -c cargo run --locked -p demo-ci -- ci"
    }));
    assert!(!workflow.contains("cargo test"));
    assert!(!workflow.contains("cargo package"));
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--check"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn crow_check_push_secret_never_reaches_pull_requests() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
provider = "crow"
platform = "forgejo"
runtime = "nix"

[ci.crow]
check_command = "nix develop -c cargo run -p demo-ci -- ci"
check_push_secret = "ATTIC_TOKEN"
"#,
    )
    .unwrap();
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .status()
            .unwrap()
            .success()
    );
    let workflow = fs::read_to_string(temp.path().join(".crow/build.yaml")).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let steps = yaml["steps"].as_sequence().unwrap();
    assert_eq!(
        steps.len(),
        3,
        "Nix setup and both event-specific full gates"
    );
    let push = steps
        .iter()
        .find(|step| step["name"] == "project-check-push")
        .unwrap();
    let pr = steps
        .iter()
        .find(|step| step["name"] == "project-check-pr")
        .unwrap();
    assert_eq!(
        push["commands"], pr["commands"],
        "no quality gate may be dropped on PRs"
    );
    assert_eq!(
        push["environment"]["ATTIC_TOKEN"]["from_secret"],
        "ATTIC_TOKEN"
    );
    assert_eq!(push["when"][0]["event"], "push");
    assert_eq!(pr["when"][0]["event"], "pull_request");
    assert!(pr["environment"].get("ATTIC_TOKEN").is_none());
    assert!(steps[0]["environment"].get("ATTIC_TOKEN").is_none());
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--check"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn crow_check_push_secret_requires_a_gate_and_cannot_also_be_global() {
    let temp = init_package(true);
    let config = temp.path().join("simit.toml");
    fs::write(
        &config,
        "[ci]\nprovider = \"crow\"\n[ci.crow]\ncheck_push_secret = \"ATTIC_TOKEN\"\n",
    )
    .unwrap();
    let output = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--check"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires check_command"));

    fs::write(&config, "[ci]\nprovider = \"crow\"\nrequired_secrets = [\"ATTIC_TOKEN\"]\n[ci.crow]\ncheck_command = \"cargo test\"\ncheck_push_secret = \"ATTIC_TOKEN\"\n").unwrap();
    let output = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--check"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("must not also be a global required_secret")
    );
}

#[test]
fn infer_project_ci_target_prefers_config_over_marked_workflows() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nplatform = \"github\"\n",
    )
    .unwrap();
    fs::create_dir_all(temp.path().join(".crow")).unwrap();

    let target = simit::registry::infer_project_ci_target(temp.path()).unwrap();
    assert_eq!(
        target,
        Some(simit::ci_resolution::CiBackend::Actions {
            platform: simit::cli::Platform::Github
        })
    );
}

#[test]
fn infer_project_ci_target_falls_back_to_marked_workflows() {
    let temp = init_package(true);
    fs::create_dir_all(temp.path().join(".crow")).unwrap();
    fs::write(
        temp.path().join(".crow/build.yaml"),
        format!(
            "{}\nname: build\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        ),
    )
    .unwrap();

    let target = simit::registry::infer_project_ci_target(temp.path()).unwrap();
    assert_eq!(target, Some(simit::ci_resolution::CiBackend::Crow));
}

#[test]
fn infer_project_ci_target_returns_none_for_unmarked_workspace() {
    let temp = init_package(true);
    fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
    fs::write(temp.path().join(".github/workflows/ci.yaml"), "name: ci\n").unwrap();

    assert_eq!(
        simit::registry::infer_project_ci_target(temp.path()).unwrap(),
        None
    );
}

#[test]
fn github_nix_only_keeps_single_runner_when_no_system_map_is_configured() {
    let temp = init_flake_only();

    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--runner",
            "ubuntu-24.04",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("runs-on: ubuntu-24.04"));
    assert!(workflow.contains("run: nix flake check\n"));
    assert!(!workflow.contains("matrix:"));
}

#[test]
fn github_nix_only_can_select_flake_evaluation_without_building_checks() {
    let temp = init_flake_only();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
runtime = "nix"
runner = "ubuntu-24.04"
components = ["flake-evaluation"]
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("run: nix flake check --no-build\n"));
    assert!(!workflow.contains("run: nix flake check\n"));
    assert!(!workflow.contains("Build flake checks"));
}

#[test]
fn github_nix_only_rejects_invalid_native_runner_maps() {
    let temp = init_flake_only();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
runtime = "nix"
runner = "ubuntu-24.04"

[ci.nix_system_runners]
"aarch64-linux" = ""
"#,
    )
    .unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[ci].runner cannot be combined with [ci].nix_system_runners"));
}

#[test]
fn generic_rust_ci_does_not_generate_publish_workflow_by_default() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_yaml_parses(&ci);
    assert!(temp.path().join(".forgejo/workflows/ci.yaml").exists());
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/publish-crate.yaml")
            .exists()
    );
    assert!(!temp.path().join("keys/maintainers.gpg").exists());
}

#[test]
fn package_metadata_nix_builds_validate_and_render() {
    let temp = init_package(true);
    let manifest = temp.path().join("Cargo.toml");
    let base = read(&manifest);
    fs::write(
        &manifest,
        format!(
            "{base}\n[package.metadata.simit.ci]\nplatform = \"github\"\nprovider = \"actions\"\nruntime = \"nix\"\nnix_builds = [\"\"]\n"
        ),
    )
    .unwrap();
    let invalid = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("[ci].nix_builds"));

    fs::write(
        &manifest,
        format!(
            "{base}\n[package.metadata.simit.ci]\nplatform = \"github\"\nprovider = \"actions\"\nruntime = \"nix\"\nnix_builds = [\".#oci-api\", \".#oci-etl\"]\nextra_setup = [\"echo prepare-runner\"]\n"
        ),
    )
    .unwrap();
    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/nix-builds.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("permissions:\n  contents: read"));
    assert!(
        workflow
            .contains("group: ${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.head_ref || github.ref_name }}")
    );
    assert!(workflow.contains("- \".#oci-api\""));
    assert!(workflow.contains("- \".#oci-etl\""));
    assert!(workflow.contains("run: echo prepare-runner"));
    assert!(workflow.contains("runs-on: ubuntu-latest"));
    assert!(workflow.contains("fail-fast: false"));
    assert!(workflow.contains("max-parallel: 2"));
    assert!(workflow.contains("run: nix build --no-link \"$INSTALLABLE\""));
    assert!(!workflow.contains("secrets."));
}

#[test]
fn generic_flake_integrated_rust_ci_auto_selects_nix_without_publish() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("flake.nix"),
        r#"{
  outputs = { self }: {
    packages.x86_64-linux.default = {};
    apps.x86_64-linux.default = { type = "app"; program = "/bin/demo"; };
    checks.x86_64-linux.demo = {};
    devShells.x86_64-linux.default = {};
    nixosModules.default = {};
    homeModules.default = {};
    lib = {};
  };
}
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--with-audit",
            "--with-deny",
            "--with-docs",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_yaml_parses(&ci);
    assert!(ci.contains("NIX_CONFIG: \"experimental-features = nix-command flakes\""));
    assert!(ci.contains("run: nix flake check"));
    assert!(ci.contains("nix develop -c cargo test"));
    assert!(ci.contains("nix develop"));
    assert!(ci.contains("cargo doc"));
    assert!(ci.contains("--no-deps --all-features"));
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/publish-crate.yaml")
            .exists()
    );
    assert!(!temp.path().join("keys/maintainers.gpg").exists());

    let simit_toml = read(&temp.path().join("simit.toml"));
    assert!(simit_toml.contains("runtime = \"nix\""));
    assert!(simit_toml.contains("with_audit = true"));
    assert!(simit_toml.contains("with_deny = true"));
    assert!(simit_toml.contains("with_docs = true"));
    assert!(!simit_toml.contains("publish_crates = true"));
}

#[test]
fn forgejo_nix_can_generate_codeberg_pages_workflow() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-codeberg-pages",
            "--pages-repo",
            "caniko/plinth",
            "--pages-canonical-domain",
            "plinth.tartanoglu.com",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let pages = read(&temp.path().join(".forgejo/workflows/pages.yaml"));
    assert_yaml_parses(&pages);
    assert!(pages.contains("# Generated by simit."));
    assert!(pages.contains("name: pages"));
    assert!(pages.contains("      - trunk"));
    assert!(pages.contains("group: ${{ codeberg.workflow }}-${{ codeberg.ref }}"));
    assert!(pages.contains("runs-on: atlas"));
    assert!(pages.contains("      - name: Validate Pages domain"));
    assert!(pages.contains("nix build .#site --out-link result-pages-site"));
    assert!(!pages.contains("--no-link"));
    assert!(pages.contains("grep -qx plinth.tartanoglu.com result-pages-site/.domains"));
    assert!(pages.contains("CODEBERG_TOKEN: ${{ secrets.codeberg_token }}"));
    assert!(pages.contains("test -n \"$CODEBERG_TOKEN\""));
    assert!(pages.contains("git config user.name \"forgejo-actions\""));
    assert!(pages.contains(
        "git remote add pages-origin \"https://caniko:${CODEBERG_TOKEN}@codeberg.org/caniko/plinth.git\""
    ));
    assert!(pages.contains("DEPLOY_REMOTE=pages-origin nix run .#deploy-pages"));

    let simit_toml = read(&temp.path().join("simit.toml"));
    assert!(simit_toml.contains("[ci.pages]"));
    assert!(simit_toml.contains("repo = \"caniko/plinth\""));
    assert!(simit_toml.contains("canonical_domain = \"plinth.tartanoglu.com\""));

    let check = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn github_nix_can_generate_github_pages_workflow() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nextra_setup = [\"echo prepare-pages-cache\"]\n",
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--runner",
            "ubuntu-latest",
            "--with-codeberg-pages",
            "--pages-repo",
            "caniko/plinth",
            "--pages-canonical-domain",
            "plinth.tartanoglu.com",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let pages = read(&temp.path().join(".github/workflows/pages.yaml"));
    assert_yaml_parses(&pages);
    let value: serde_yaml::Value = serde_yaml::from_str(&pages).unwrap();
    assert_eq!(value["concurrency"]["group"], "github-pages");
    assert_eq!(value["concurrency"]["cancel-in-progress"], false);
    assert!(value["on"]["workflow_dispatch"].is_null());
    assert_eq!(value["on"]["push"]["branches"][0], "trunk");
    let ci: serde_yaml::Value =
        serde_yaml::from_str(&read(&temp.path().join(".github/workflows/ci.yaml"))).unwrap();
    assert_ne!(ci["concurrency"]["group"], value["concurrency"]["group"]);
    assert_eq!(ci["concurrency"]["cancel-in-progress"], true);
    assert!(pages.contains("permissions:\n  contents: read\n  pages: write\n  id-token: write"));
    assert!(pages.contains("uses: actions/checkout@"));
    assert!(pages.contains("uses: actions/configure-pages@"));
    assert!(pages.contains("uses: actions/upload-pages-artifact@"));
    assert!(pages.contains("uses: actions/deploy-pages@"));
    assert!(pages.contains("environment:\n      name: github-pages\n      url: ${{ steps.deployment.outputs.page_url }}"));
    assert!(pages.contains("nix build .#site --out-link result-pages-site"));
    assert!(!pages.contains("--no-link"));
    assert!(pages.contains("grep -qx plinth.tartanoglu.com result-pages-site/.domains"));
    assert!(!pages.contains("CODEBERG_TOKEN"));
    assert!(!pages.contains("codeberg.workflow"));
    let setup = pages.find("run: echo prepare-pages-cache").unwrap();
    assert!(pages.find("- name: Install Nix").unwrap() < setup);
    assert!(setup < pages.find("- name: Build Pages site").unwrap());
    let check = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--pages-only",
            "--platform",
            "github",
            "--check",
            "--diff",
        ])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn pages_only_preserves_primary_crow_ci() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
provider = "crow"
platform = "forgejo"
runtime = "nix"
runner = "atlas-nix-trusted"
"#,
    )
    .unwrap();
    fs::create_dir_all(temp.path().join(".crow")).unwrap();
    fs::write(
        temp.path().join(".crow/ci.yaml"),
        "existing crow workflow\n",
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--pages-only",
            "--platform",
            "github",
            "--with-pages",
            "--pages-repo",
            "caniko/plinth",
            "--pages-canonical-domain",
            "plinth.tartanoglu.com",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let pages = read(&temp.path().join(".github/workflows/pages.yaml"));
    assert_yaml_parses(&pages);
    assert!(pages.contains("uses: actions/deploy-pages@"));
    assert_eq!(
        read(&temp.path().join(".crow/ci.yaml")),
        "existing crow workflow\n"
    );

    let simit_toml = read(&temp.path().join("simit.toml"));
    assert!(simit_toml.contains("provider = \"crow\""));
    assert!(simit_toml.contains("platform = \"forgejo\""));
    assert!(simit_toml.contains("[ci.pages]"));

    let check = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--pages-only",
            "--platform",
            "github",
            "--check",
        ])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn pages_only_check_infers_managed_workflow_from_cargo_workspace() {
    let temp = init_package(true);
    let manifest = read(&temp.path().join("Cargo.toml"));
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            "{manifest}\n[package.metadata.simit.ci]\nplatform = \"forgejo\"\nruntime = \"nix\"\n"
        ),
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path().join("src"))
        .args([
            "init",
            "ci",
            "--pages-only",
            "--platform",
            "forgejo",
            "--runner",
            "atlas",
            "--with-pages",
            "--pages-repo",
            "caniko/demo",
            "--pages-canonical-domain",
            "demo.example.com",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!temp.path().join("simit.toml").exists());

    let check = simit()
        .current_dir(temp.path().join("src"))
        .args([
            "init",
            "ci",
            "--pages-only",
            "--platform",
            "forgejo",
            "--check",
        ])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn github_generation_drops_persisted_forgejo_runner() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
provider = "actions"
platform = "forgejo"
runtime = "nix"
runner = "codefloe-global"
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: ubuntu-latest"));
    assert!(!ci.contains("codefloe-global"));
}

#[test]
fn persisted_codeberg_pages_overrides_survive_regeneration() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-codeberg-pages",
            "--pages-repo",
            "caniko/rs-harbor",
            "--pages-canonical-domain",
            "rs-harbor.tartanoglu.com",
            "--pages-site-output",
            "./site#site",
            "--pages-deploy-app",
            "./site#deploy-pages",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let simit_toml = read(&temp.path().join("simit.toml"));
    assert!(simit_toml.contains("site_output = \"./site#site\""));
    assert!(simit_toml.contains("deploy_app = \"./site#deploy-pages\""));

    let check = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check.success());

    let pages = read(&temp.path().join(".forgejo/workflows/pages.yaml"));
    assert!(pages.contains("nix build ./site#site --out-link result-pages-site"));
    assert!(pages.contains("DEPLOY_REMOTE=pages-origin nix run ./site#deploy-pages"));
}

#[test]
fn forgejo_nix_can_generate_vscode_publish_workflow_with_file_env_pats() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
runtime = "nix"
runner = "atlas-nix-trusted"

[vscode]
extension_dir = "pkl-lsp-vscode"
runner = "atlas-nix-trusted"
codeberg_repo = "caniko/pkl-lsp"
codeberg_token_secret = "codeberg_token"
pat_source = "file-env"
vsce_pat_file_env = "VSCE_PAT_FILE"
ovsx_pat_file_env = "OVSX_PAT_FILE"
cargo_package = "pkl-lsp-server"
prepublish_commands = ["nix flake check --no-build"]
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-vscode",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-vscode-extension.yaml"),
    );
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("# Generated by simit."));
    assert!(workflow.contains("name: Publish VS Code Extension"));
    let parsed: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let steps = parsed["jobs"]["publish"]["steps"].as_sequence().unwrap();
    let verification = steps
        .iter()
        .position(|step| {
            step["name"].as_str() == Some("Validate release tag and extension version")
        })
        .unwrap();
    let credentials = steps
        .iter()
        .position(|step| step["name"].as_str() == Some("Validate publish credentials"))
        .unwrap();
    assert!(verification < credentials);
    assert!(steps[..=verification].iter().all(|step| {
        !serde_yaml::to_string(&step["env"])
            .unwrap()
            .contains("secrets.")
    }));
    assert!(workflow.contains("tags: [\"[0-9]*.[0-9]*.[0-9]*\"]"));
    assert!(workflow.contains("runs-on: atlas-nix-trusted"));
    assert!(workflow.contains("CODEBERG_TOKEN: ${{ secrets.codeberg_token }}"));
    assert!(workflow.contains("file_env=VSCE_PAT_FILE"));
    assert!(workflow.contains("file_env=OVSX_PAT_FILE"));
    assert!(workflow.contains(
        "VSCE_PUBLISHER=$(nix shell nixpkgs#jq -c jq -r '.publisher' pkl-lsp-vscode/package.json)"
    ));
    assert!(workflow.contains(
        "nix develop -c npx --yes @vscode/vsce verify-pat \"$VSCE_PUBLISHER\" --pat \"$VSCE_PUBLISH_PAT\""
    ));
    assert!(workflow.contains("https://aka.ms/vsm-create-publisher"));
    assert!(!workflow.contains("secrets.VSCE_PAT"));
    assert!(!workflow.contains("secrets.OVSX_PAT"));
    assert!(!workflow.contains("skipping VS Code"));
    assert!(!workflow.contains("skipping Open VSX"));
    assert!(workflow.contains("git verify-tag \"$GITHUB_REF_NAME\""));
    assert!(workflow.contains(
        "cargo_metadata=$(nix shell nixpkgs#cargo -c cargo metadata --no-deps --format-version 1)"
    ));
    assert!(workflow.contains("nix shell nixpkgs#jq -c jq -r --arg name pkl-lsp-server"));
    assert!(workflow.contains(
        "extension_version=$(nix shell nixpkgs#jq -c jq -r '.version' pkl-lsp-vscode/package.json)"
    ));
    assert!(workflow.contains("nix shell nixpkgs#jq -c jq -r '.id // empty'"));
    assert!(workflow.contains(
        "existing_ids=$(printf '%s' \"$release_json\" | nix shell nixpkgs#jq -c jq -r --arg name \"$name\" '.assets[]? | select(.name == $name) | .id')"
    ));
    assert!(workflow.contains(
        "curl --fail --silent --show-error --request DELETE --header \"$auth_header\" \"$api/repos/$repo/releases/$release_id/assets/$asset_id\" >/dev/null"
    ));
    assert!(workflow.contains("nix flake check --no-build"));
    assert!(workflow.contains(
        "nix develop -c npx --yes @vscode/vsce publish --packagePath release/*.vsix --pat \"$PUBLISH_PAT\" --skip-duplicate"
    ));
    assert!(workflow.contains("Verify VS Code Marketplace visibility"));
    assert!(workflow.contains(
        "nix develop -c npm --prefix \"$diag_dir\" install --silent azure-devops-node-api@15.1.2 >/dev/null"
    ));
    assert!(workflow.contains(
        "Marketplace extension is missing Public flag; setting it for ${publisher}.${extension}"
    ));
    assert!(
        workflow
            .contains("https://marketplace.visualstudio.com/_apis/public/gallery/extensionquery")
    );
    assert!(
        workflow.contains("$EXTENSION_ID is visible in the public VS Code Marketplace Gallery API")
    );
    assert!(workflow.contains("Verify VS Code Marketplace signatures"));
    assert!(workflow.contains("Microsoft.VisualStudio.Services.VSIXPackage"));
    assert!(workflow.contains("Microsoft.VisualStudio.Services.VsixSignature"));
    assert!(workflow.contains(".signature.manifest"));
    assert!(workflow.contains(".signature.p7s"));
    assert!(workflow.contains("nix shell nixpkgs#unzip -c unzip -q \"$signature_zip\""));
    assert!(workflow.contains(
        "nix develop -c npx --yes @vscode/vsce@latest verify-signature --packagePath \"$package_path\" --manifestPath \"$manifest_path\" --signaturePath \"$signature_path\""
    ));
    assert!(workflow.contains(
        "OVSX_NAMESPACE=$(nix shell nixpkgs#jq -c jq -r '.publisher' pkl-lsp-vscode/package.json)"
    ));
    assert!(workflow.contains(
        "nix develop -c npx --yes ovsx create-namespace \"$OVSX_NAMESPACE\" --pat \"$PUBLISH_PAT\" || true"
    ));
    assert!(workflow.contains(
        "nix develop -c npx --yes ovsx publish --packagePath release/*.vsix --pat \"$PUBLISH_PAT\" --skip-duplicate"
    ));
    #[cfg(unix)]
    verify_editor_publish_package_arguments(temp.path(), &parsed);

    let check = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check.success());
}

#[cfg(unix)]
#[test]
fn vscode_publishers_use_configured_release_tag_prefix() {
    for platform in ["github", "forgejo"] {
        for prefix in ["", "v"] {
            let temp = init_package(true);
            fs::write(temp.path().join("simit.toml"), format!(
                "[ci]\nruntime='nix'\n[release]\ntag_prefix='{prefix}'\n[vscode]\ncodeberg_repo='example/extension'\n"
            )).unwrap();
            let output = simit_with_user_config(temp.path())
                .current_dir(temp.path())
                .args([
                    "init",
                    "ci",
                    "--platform",
                    platform,
                    "--runtime",
                    "nix",
                    "--with-vscode",
                ])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let directory = if platform == "github" {
                ".github"
            } else {
                ".forgejo"
            };
            let output = simit_with_user_config(temp.path())
                .current_dir(temp.path())
                .args(["init", "ci", "--check"])
                .output()
                .unwrap();
            assert!(output.status.success(), "{platform}/{prefix}: {output:?}");
            let workflow = read(&temp.path().join(format!(
                "{directory}/workflows/publish-vscode-extension.yaml"
            )));
            let doc: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
            assert_eq!(
                doc["on"]["push"]["tags"][0],
                format!("{prefix}[0-9]*.[0-9]*.[0-9]*")
            );
            let script = doc["jobs"]["publish"]["steps"]
                .as_sequence()
                .unwrap()
                .iter()
                .find(|step| step["name"] == "Validate release tag and extension version")
                .unwrap()["run"]
                .as_str()
                .unwrap();
            let version_script = script.split("git_fetch()").next().unwrap();
            for tag in ["1.2.3", "v1.2.3", "vv1.2.3", "branch"] {
                let output = Command::new("bash")
                    .args([
                        "-c",
                        &format!("{version_script}\ntest \"$VERSION\" = 1.2.3"),
                    ])
                    .env("GITHUB_REF_NAME", tag)
                    .output()
                    .unwrap();
                assert_eq!(
                    output.status.success(),
                    tag == format!("{prefix}1.2.3"),
                    "{platform}/{prefix}/{tag}: {output:?}"
                );
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn vscode_signed_tag_names_reject_component_aliases_before_credentials() {
    let key = TempDir::new().unwrap();
    let output = Command::new("gpg")
        .env("GNUPGHOME", key.path())
        .args([
            "--batch",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "",
            "--quick-generate-key",
            "Extension fixture <extension@example.invalid>",
            "ed25519",
            "sign",
            "0",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    for platform in ["github", "forgejo"] {
        for prefix in ["", "v"] {
            let temp = init_package(true);
            let root = temp.path();
            fs::write(root.join("simit.toml"), format!("[ci]\nruntime='nix'\n[release]\ntag_prefix='{prefix}'\n[vscode]\ncodeberg_repo='example/extension'\n")).unwrap();
            let output = simit_with_user_config(root)
                .current_dir(root)
                .args([
                    "init",
                    "ci",
                    "--platform",
                    platform,
                    "--runtime",
                    "nix",
                    "--with-vscode",
                ])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let dir = if platform == "github" {
                ".github"
            } else {
                ".forgejo"
            };
            let doc: serde_yaml::Value = serde_yaml::from_str(&read(
                &root.join(format!("{dir}/workflows/publish-vscode-extension.yaml")),
            ))
            .unwrap();
            let steps = doc["jobs"]["publish"]["steps"].as_sequence().unwrap();
            let validation = steps
                .iter()
                .position(|step| step["name"] == "Validate release tag and extension version")
                .unwrap();
            let credentials = steps
                .iter()
                .position(|step| step["name"] == "Validate publish credentials")
                .unwrap();
            assert!(validation < credentials);
            let script = steps[validation]["run"].as_str().unwrap();
            let (_, signature_boundary) = script.split_once("git verify-tag").unwrap();
            let signature_boundary = signature_boundary
                .split("extension_version=")
                .next()
                .unwrap();
            let git = |args: &[&str]| {
                let output = Command::new("git")
                    .current_dir(root)
                    .env("GNUPGHOME", key.path())
                    .args([
                        "-c",
                        "user.name=Extension fixture",
                        "-c",
                        "user.email=extension@example.invalid",
                        "-c",
                        "user.signingkey=extension@example.invalid",
                    ])
                    .args(args)
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{output:?}");
                output
            };
            git(&["init"]);
            git(&["add", "src", "Cargo.toml", "flake.nix", "simit.toml"]);
            git(&["commit", "-m", "fixture"]);
            git(&["tag", "-s", "component-a/v0.1.0", "-m", "component release"]);
            let object =
                String::from_utf8(git(&["rev-parse", "refs/tags/component-a/v0.1.0"]).stdout)
                    .unwrap();
            let tag = format!("{prefix}0.1.0");
            git(&["update-ref", &format!("refs/tags/{tag}"), object.trim()]);
            git(&["verify-tag", &format!("refs/tags/{tag}")]);
            let validate = || {
                Command::new("bash")
                    .current_dir(root)
                    .env("GNUPGHOME", key.path())
                    .env("GITHUB_REF_NAME", &tag)
                    .args([
                        "-euo",
                        "pipefail",
                        "-c",
                        &format!("git verify-tag{signature_boundary}\nprintf validated > admitted"),
                    ])
                    .output()
                    .unwrap()
            };
            let output = validate();
            assert!(!output.status.success(), "{platform}/{prefix}: {output:?}");
            assert!(!root.join("admitted").exists());
            git(&["tag", "-s", "-f", &tag, "-m", "correct release"]);
            let output = validate();
            assert!(output.status.success(), "{output:?}");
            assert!(root.join("admitted").exists());
        }
    }
}

#[test]
fn forgejo_nix_can_generate_jetbrains_publish_workflow() {
    let temp = init_package(true);
    let plugin_xml = temp
        .path()
        .join("example-jetbrains/src/main/resources/META-INF");
    fs::create_dir_all(&plugin_xml).unwrap();
    fs::write(
        plugin_xml.join("plugin.xml"),
        "<idea-plugin><id>com.example.demo</id></idea-plugin>\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
runtime = "nix"
runner = "atlas-nix-trusted"

[jetbrains]
        plugin_dir = "example-jetbrains"
plugin_xml_id = "com.example.demo"
package_installable = ".#jetbrains-plugin"
runner = "atlas-nix-trusted"
credential_source = "file-env"
prepublish_commands = ["nix flake check --no-build"]
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-jetbrains",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-jetbrains-plugin.yaml"),
    );
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("name: Publish JetBrains Plugin"));
    assert!(workflow.contains("runs-on: atlas-nix-trusted"));
    assert!(workflow.contains("INSTALLABLE: .#jetbrains-plugin"));
    assert!(workflow.contains("tags: [\"[0-9]*.[0-9]*.[0-9]*\"]"));
    assert!(workflow.contains("  workflow_dispatch:\n    inputs:\n      version:"));
    assert!(workflow.contains("VERSION=\"$DISPATCH_VERSION\""));
    assert!(workflow.contains("VERSION=\"$TAG_LABEL\""));
    assert!(workflow.contains("<id>$PLUGIN_XML_ID</id>"));
    assert!(workflow.contains("CERTIFICATE_CHAIN_FILE=\"$RUNNER_TEMP/certificate-chain.pem\""));
    assert!(workflow.contains(
        "nix shell nixpkgs#gradle_9 nixpkgs#jdk21 -c gradle --no-daemon -x buildPlugin signPlugin verifyPluginSignature"
    ));
    assert!(workflow.contains("-F \"xmlId=$PLUGIN_XML_ID\""));
    assert!(
        workflow
            .contains("-F \"file=@$RUNNER_TEMP/jetbrains-plugin-signed.zip;type=application/zip\"")
    );
    assert!(workflow.contains("name: example-jetbrains-${{ github.ref_name }}"));
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let upload = yaml["jobs"]["publish"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "Publish to JetBrains Marketplace")
        .unwrap();
    assert_eq!(upload["env"]["CHANNEL"].as_str(), Some(""));
    assert!(workflow.contains("jetbrains-plugin-archive-name"));
    assert!(!workflow.contains("pluginId="));
    assert!(!workflow.contains("pkl-lsp-jetbrains"));
    assert!(workflow.contains("upload-artifact"));
    assert!(!workflow.contains("secrets.JETBRAINS_MARKETPLACE_TOKEN"));
    #[cfg(unix)]
    verify_jetbrains_signed_archive_selection(temp.path(), &yaml);
}

#[cfg(unix)]
#[test]
fn jetbrains_publishers_enforce_configured_release_prefix() {
    for platform in ["github", "forgejo"] {
        for prefix in ["", "v"] {
            let temp = init_package(true);
            let root = temp.path();
            fs::create_dir_all(root.join("plugin/src/main/resources/META-INF")).unwrap();
            fs::write(
                root.join("plugin/src/main/resources/META-INF/plugin.xml"),
                "<idea-plugin><id>com.example.demo</id></idea-plugin>",
            )
            .unwrap();
            fs::write(
                root.join("plugin/gradle.properties"),
                "pluginVersion=0.1.0\n",
            )
            .unwrap();
            fs::write(root.join("simit.toml"), format!("[ci]\nruntime='nix'\n[release]\ntag_prefix='{prefix}'\n[jetbrains]\nplugin_dir='plugin'\nplugin_xml_id='com.example.demo'\npackage_installable='.#plugin'\n")).unwrap();
            let output = simit_with_user_config(root)
                .current_dir(root)
                .args([
                    "init",
                    "ci",
                    "--platform",
                    platform,
                    "--runtime",
                    "nix",
                    "--with-jetbrains",
                ])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let dir = if platform == "github" {
                ".github"
            } else {
                ".forgejo"
            };
            let workflow: serde_yaml::Value = serde_yaml::from_str(&read(
                &root.join(format!("{dir}/workflows/publish-jetbrains-plugin.yaml")),
            ))
            .unwrap();
            assert_eq!(
                workflow["on"]["push"]["tags"],
                serde_yaml::to_value(vec![format!("{prefix}[0-9]*.[0-9]*.[0-9]*")]).unwrap()
            );
            let steps = workflow["jobs"]["publish"]["steps"].as_sequence().unwrap();
            let script = steps
                .iter()
                .find(|s| s["name"] == "Validate plugin version and metadata")
                .unwrap()["run"]
                .as_str()
                .unwrap();
            for (label, dispatch, success) in [
                (format!("{prefix}0.1.0"), "", true),
                (
                    if prefix.is_empty() { "v0.1.0" } else { "0.1.0" }.to_owned(),
                    "",
                    false,
                ),
                ("trunk".to_owned(), "0.1.0", true),
            ] {
                let output = Command::new("bash")
                    .current_dir(root)
                    .env("TAG_LABEL", &label)
                    .env("DISPATCH_VERSION", dispatch)
                    .env("PLUGIN_DIR", "plugin")
                    .env("PLUGIN_XML_ID", "com.example.demo")
                    .env("GITHUB_ENV", root.join("environment"))
                    .args(["-c", script])
                    .output()
                    .unwrap();
                assert_eq!(
                    output.status.success(),
                    success,
                    "{platform}/{prefix}/{label}: {output:?}"
                );
            }
        }
    }
}

#[cfg(unix)]
fn verify_editor_publish_package_arguments(root: &Path, yaml: &serde_yaml::Value) {
    use std::os::unix::fs::PermissionsExt;

    let bin = root.join("publisher-bin");
    fs::create_dir_all(&bin).unwrap();
    let nix = bin.join("nix");
    fs::write(
        &nix,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$PUBLISH_ARGUMENTS\"\n",
    )
    .unwrap();
    fs::set_permissions(&nix, fs::Permissions::from_mode(0o755)).unwrap();
    let release = root.join("release");
    fs::create_dir(&release).unwrap();
    let names = [
        "universal",
        "linux-x64",
        "linux-arm64",
        "darwin-x64",
        "darwin-arm64",
        "win32-x64",
    ];
    for name in names {
        fs::write(release.join(format!("plugin-{name}.vsix")), "fixture").unwrap();
    }
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    for publisher in ["@vscode/vsce", "ovsx"] {
        let script = yaml["jobs"]["publish"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|step| step["run"].as_str())
            .flat_map(str::lines)
            .find(|line| line.contains(&format!("{publisher} publish ")))
            .unwrap();
        let arguments = root.join("publisher-arguments");
        let result = Command::new("bash")
            .args(["-c", script])
            .current_dir(root)
            .env("PATH", &path)
            .env("PUBLISH_PAT", "fixture-not-a-token")
            .env("PUBLISH_ARGUMENTS", &arguments)
            .output()
            .unwrap();
        assert!(result.status.success(), "{publisher}: {result:?}");
        let arguments = fs::read_to_string(arguments).unwrap();
        let args = arguments.lines().collect::<Vec<_>>();
        let start = args.iter().position(|arg| *arg == "--packagePath").unwrap() + 1;
        let end = args.iter().position(|arg| *arg == "--pat").unwrap();
        assert_eq!(end - start, 6, "{publisher}: {args:?}");
        for name in names {
            assert!(args[start..end].contains(&format!("release/plugin-{name}.vsix").as_str()));
        }
    }
}

#[cfg(unix)]
fn verify_jetbrains_signed_archive_selection(root: &Path, yaml: &serde_yaml::Value) {
    use std::os::unix::fs::PermissionsExt;

    let script = yaml["jobs"]["publish"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "Sign plugin")
        .unwrap()["run"]
        .as_str()
        .unwrap()
        .replace(
            "${{ github.event.inputs.version || github.ref_name }}",
            "1.0.0",
        );
    assert!(script.contains("verify.inputArchiveFile.set(sign.signedArchiveFile)"));
    assert!(script.contains("sign.signedArchiveFile.get().asFile"));
    let bin = root.join("signer-bin");
    fs::create_dir_all(&bin).unwrap();
    let nix = bin.join("nix");
    fs::write(&nix, r#"#!/bin/sh
set -eu
case "$*" in
  *'--init-script '*) ;;
  *) exit 1;;
esac
case "$SIGNED_ARCHIVE_CASE" in
  default) signed="$PWD/build/distributions/pkl-lsp-1.0.0-signed.zip";;
  override) signed="$PWD/custom outputs/verified.zip";;
  missing) signed="$PWD/missing.zip";;
  ambiguous) printf 'first.zip\nsecond.zip\n' > "$RUNNER_TEMP/jetbrains-plugin-signed-path"; exit 0;;
  unsigned) signed="$PWD/build/distributions/pkl-lsp-1.0.0.zip";;
esac
if [ "$SIGNED_ARCHIVE_CASE" != missing ] && [ "$SIGNED_ARCHIVE_CASE" != unsigned ]; then
  mkdir -p "$(dirname "$signed")"
  printf 'verified signed fixture' > "$signed"
fi
printf '%s\n' "$signed" > "$RUNNER_TEMP/jetbrains-plugin-signed-path"
"#).unwrap();
    fs::set_permissions(&nix, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    for (case, accepted) in [
        ("default", true),
        ("override", true),
        ("missing", false),
        ("ambiguous", false),
        ("unsigned", false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("jetbrains-plugin-unsigned.zip"),
            "unsigned fixture",
        )
        .unwrap();
        fs::write(
            temp.path().join("jetbrains-plugin-archive-name"),
            "pkl-lsp-1.0.0.zip\n",
        )
        .unwrap();
        fs::write(temp.path().join("private-key-password"), "fixture-password").unwrap();
        let plugin = root.join(format!("signing-{case}"));
        fs::create_dir_all(&plugin).unwrap();
        let result = Command::new("bash")
            .args(["-c", &script])
            .current_dir(root)
            .env("PATH", &path)
            .env("PLUGIN_DIR", plugin)
            .env("RUNNER_TEMP", temp.path())
            .env("SIGNED_ARCHIVE_CASE", case)
            .output()
            .unwrap();
        assert_eq!(result.status.success(), accepted, "{case}: {result:?}");
        let output = temp.path().join("jetbrains-plugin-signed.zip");
        assert_eq!(output.exists(), accepted);
        if accepted {
            assert_eq!(
                fs::read_to_string(output).unwrap(),
                "verified signed fixture"
            );
        }
    }
}

#[test]
fn forgejo_nix_runtime_uses_devshell_quality_tools_without_cargo_install() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-audit",
            "--with-deny",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("      - name: Check cargo-audit tool\n"));
    assert!(ci.contains("run: nix develop -c cargo-audit --version"));
    assert!(ci.contains("      - name: Check cargo-deny tool\n"));
    assert!(ci.contains("run: nix develop -c cargo-deny --version"));
    assert!(!ci.contains("nix develop -c cargo install cargo-audit"));
    assert!(!ci.contains("nix develop -c cargo install cargo-deny"));
}

#[test]
fn forgejo_nix_with_om_ci_replace_emits_om_ci_step() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-om-ci",
            "--with-docs",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("OMNIX_REF:"));
    assert!(ci.contains("nix run \"$OMNIX_REF\" -- ci run"));
    assert!(ci.contains("run: nix develop .#docs -c cargo doc --no-deps --all-features"));
    assert!(!ci.contains("run: nix develop -c cargo doc --no-deps --all-features"));
    assert!(!ci.ends_with("\n\n"));
    assert!(!ci.contains("run: nix flake check"));
    assert!(!ci.contains("nix develop -c cargo test"));
    assert!(!ci.contains("nix develop -c cargo clippy"));
    assert!(!ci.contains("nix develop -c cargo package"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("OMNIX_REF:"));
    assert!(publish.contains("nix run \"$OMNIX_REF\" -- ci run"));
    assert!(publish.contains(
        r#"nix develop -c cargo pkgid -p demo | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#
    ));
    assert!(publish.contains("nix develop -c cargo publish --dry-run"));
    assert!(!publish.contains("run: nix flake check"));
    assert!(!publish.contains("nix develop -c cargo test"));
    assert!(!publish.contains("nix develop -c cargo clippy"));
}

#[test]
fn forgejo_nix_with_om_ci_augment_keeps_legacy_steps() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--om-ci-augment",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("OMNIX_REF:"));
    assert!(ci.contains("nix run \"$OMNIX_REF\" -- ci run"));
    assert!(ci.contains("run: nix flake check"));
    assert!(ci.contains("nix develop -c cargo test"));
    assert!(
        ci.contains("nix develop -c cargo clippy --all-targets --all-features -- --deny warnings")
    );
}

#[test]
fn forgejo_nix_msrv_uses_and_verifies_the_msrv_devshell_toolchain() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-msrv",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("nix develop .#msrv -c rustc --version"));
    assert!(ci.contains("= '1.85.0'"));
    assert!(ci.contains("nix develop .#msrv -c cargo check --all-targets --all-features"));
    assert!(!ci.contains("cargo +1.85 check"));
}

#[test]
fn github_nix_with_om_ci_replace_keeps_install_nix_action() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--with-om-ci",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains(&format!(
        "uses: {}",
        simit::render::ci::github_action_ref("cachix/install-nix-action", "v31.11.1")
    )));
    assert!(ci.contains("OMNIX_REF:"));
    assert!(ci.contains("nix run \"$OMNIX_REF\" -- ci run"));
    assert!(!ci.contains("run: nix flake check"));
}

#[test]
fn with_om_ci_without_runtime_nix_fails() {
    let temp = init_package(false);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--with-om-ci", "--platform", "forgejo"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--runtime nix"));
}

#[test]
fn omnix_ref_without_om_ci_warns_and_does_not_change_output() {
    let with_ref = init_package(true);
    let without_ref = init_package(true);

    let output = simit_with_user_config(with_ref.path())
        .current_dir(with_ref.path())
        .args([
            "init",
            "ci",
            "--omnix-ref",
            "github:x/y/z",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("ignored"));

    let status = simit_with_user_config(without_ref.path())
        .current_dir(without_ref.path())
        .args(["init", "ci", "--platform", "forgejo", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());

    assert_eq!(
        read(&with_ref.path().join(".forgejo/workflows/ci.yaml")),
        read(&without_ref.path().join(".forgejo/workflows/ci.yaml"))
    );
}

#[test]
fn project_config_om_ci_matches_cli_flag() {
    let from_config = init_package(true);
    fs::write(
        from_config.path().join("simit.toml"),
        "[ci]\nom_ci = true\n",
    )
    .unwrap();

    let config_status = simit_with_user_config(from_config.path())
        .current_dir(from_config.path())
        .args(["init", "ci", "--platform", "forgejo", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(config_status.success());

    let from_cli = init_package(true);
    let cli_status = simit_with_user_config(from_cli.path())
        .current_dir(from_cli.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-om-ci",
        ])
        .status()
        .unwrap();
    assert!(cli_status.success());

    assert_eq!(
        read(&from_config.path().join(".forgejo/workflows/ci.yaml")),
        read(&from_cli.path().join(".forgejo/workflows/ci.yaml"))
    );
}

#[test]
fn cli_omnix_ref_beats_project_config() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
om_ci = true
omnix_ref = "github:project/pin/v1"
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--omnix-ref",
            "github:cli/pin/v2",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("github:cli/pin/v2"));
    assert!(!ci.contains("github:project/pin/v1"));
}

#[test]
fn user_config_omnix_ref_used_when_no_override() {
    let temp = init_package(true);
    write_user_runner_config_with_omnix_ref(temp.path());

    let mut command = simit();
    let status = command
        .env("XDG_CONFIG_HOME", temp.path().join(".xdg"))
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-om-ci",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("github:user/pin/v3"));
}

#[test]
fn generates_github_plain_cargo_workflows() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-nextest",
            "--publish-crates",
            "--publish-strategy",
            "members",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: ubuntu-latest"));
    assert!(ci.contains(
        "uses: dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30 # stable"
    ));
    assert!(ci.contains("toolchain: stable"));
    assert!(ci.contains("uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0"));
    assert!(!ci.contains("https://code.forgejo.org/actions/cache@v4"));
    assert!(
        ci.contains("uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2.9.2")
    );
    assert!(ci.contains("path: ~/.cargo/bin"));
    assert!(ci.contains("hashFiles('.github/workflows/*.yaml')"));
    assert!(ci.contains("cargo-bin-${{ runner.os }}-${{ runner.arch }}-"));
    assert!(ci.contains(
        "save-if: ${{ github.ref == format('refs/heads/{0}', github.event.repository.default_branch) }}"
    ));
    assert!(!ci.contains("hashFiles('.forgejo/workflows/ci.yaml', '.github/workflows/ci.yaml')"));
    assert!(ci.contains(
        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version 0.9.100"
    ));
    assert!(!ci.contains("&>/dev/null"));
    assert!(ci.contains("run: cargo nextest run --all-features"));
    assert!(ci.contains("run: cargo package --allow-dirty --list"));

    let publish = read(&temp.path().join(".github/workflows/publish-crate.yaml"));
    assert!(publish.contains("group: ${{ github.workflow_ref }}-${{ github.ref }}"));
    assert!(publish.contains("cancel-in-progress: false"));
    assert!(publish.contains("on:\n  push:\n    tags:\n      - \"[0-9]*\"\n"));
    assert!(!publish.contains("workflow_dispatch"));
    assert!(
        publish.contains("uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0")
    );
    assert!(!publish.contains("https://code.forgejo.org/actions/cache@v4"));
    assert!(
        publish.contains(
            "uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2.9.2"
        )
    );
    assert!(publish.contains("path: ~/.cargo/bin"));
    assert!(publish.contains("hashFiles('.github/workflows/*.yaml')"));
    assert!(publish.contains("cargo-bin-${{ runner.os }}-${{ runner.arch }}-"));
    assert!(publish.contains(
        "save-if: ${{ github.ref == format('refs/heads/{0}', github.event.repository.default_branch) }}"
    ));
    assert!(
        !publish.contains("hashFiles('.forgejo/workflows/ci.yaml', '.github/workflows/ci.yaml')")
    );
    assert!(publish.contains(
        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version 0.9.100"
    ));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(publish.contains("run: cargo publish --dry-run"));
    assert!(
        publish.contains(r#"cargo pkgid -p demo | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#)
    );
    assert!(!publish.contains("CARGO_REGISTRY_TOKEN"));
    assert!(!publish.contains("CRATES_IO_API_TOKEN"));
    let trusted = read(
        &temp
            .path()
            .join(".github/workflows/trusted-publish-crate.yaml"),
    );
    assert!(trusted.contains("workflow_run:"));
    assert!(trusted.contains("--publisher-stage publish"));
    assert!(trusted.contains(
        "already-published crates with matching checksums exit 0 after exact registry resolution"
    ));
    assert!(!publish.contains("cargo login"));
    // Regression: the publish workflow must not infer the release version from
    // the first package in workspace-wide cargo metadata.
    assert!(
        !publish.contains("cargo metadata --no-deps --format-version 1"),
        "publish workflow must use a package-scoped version extractor"
    );
}

#[test]
fn github_ignores_forgejo_step_runner_labels() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
provider = "actions"
platform = "github"
runtime = "nix"
runner = "ubuntu-24.04"
step_runners = { nix-check = "atlas-nix-trusted", cargo-test = "codeberg-medium" }
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--ci-provider",
            "actions",
            "--platform",
            "github",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: ubuntu-24.04"));
    assert!(!ci.contains("atlas-nix-trusted"));
    assert!(!ci.contains("codeberg-medium"));
}

#[test]
fn generated_workflows_include_project_ci_setup_and_env() {
    let temp = init_package(true);
    write_ci_customization_config(temp.path());

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-artifacts",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    for path in [
        ".forgejo/workflows/ci.yaml",
        ".forgejo/workflows/publish-crate.yaml",
        ".forgejo/workflows/release-artifacts.yaml",
    ] {
        let workflow = read(&temp.path().join(path));
        assert_yaml_parses(&workflow);
        assert!(workflow.contains("# Project-required secrets:\n# - SKILLNET_TEST_PG_URL"));
        assert!(workflow.contains(
            "    env:\n      NIX_CONFIG: \"experimental-features = nix-command flakes\"\n      XDG_CACHE_HOME: \"/tmp/.cache\"\n      CARGO_HOME: \"/tmp/.cargo\"\n      SKILLNET_TEST_PG_URL: \"${{ secrets.SKILLNET_TEST_PG_URL }}\""
        ));
        assert!(workflow.contains("      - name: Validate required environment"));
        assert!(workflow.contains(
            "if [ -z \"${VSCE_PAT_FILE:-}\" ]; then echo 'VSCE_PAT_FILE is required by simit project configuration.' >&2; exit 1; fi"
        ));
        assert!(workflow.contains(
            "if [ ! -r \"${VSCE_PAT_FILE}\" ] || [ ! -s \"${VSCE_PAT_FILE}\" ]; then echo 'VSCE_PAT_FILE must point to a readable, non-empty file.' >&2; exit 1; fi"
        ));
        assert!(workflow.contains(
            "if [ -z \"${OVSX_PAT_FILE:-}\" ]; then echo 'OVSX_PAT_FILE is required by simit project configuration.' >&2; exit 1; fi"
        ));
        assert!(workflow.contains(
            "if [ ! -r \"${OVSX_PAT_FILE}\" ] || [ ! -s \"${OVSX_PAT_FILE}\" ]; then echo 'OVSX_PAT_FILE must point to a readable, non-empty file.' >&2; exit 1; fi"
        ));
        assert!(workflow.contains("      - name: Project setup\n        run: apt-get update && apt-get install -y --no-install-recommends postgresql-client"));
    }
}

#[test]
fn forgejo_auto_runtime_uses_rust_container_even_when_flake_exists() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--with-nextest",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: atlas"));
    assert!(ci.contains("cancel-in-progress: true"));
    assert!(ci.contains("container: rust:1.85-bookworm"));
    assert!(!ci.contains("SCCACHE_REDIS_ENDPOINT:"));
    assert!(!ci.contains("SCCACHE_REDIS_KEY_PREFIX:"));
    assert!(ci.contains("name: Verify compiler cache"));
    assert!(ci.contains("/usr/local/bin/sccache --zero-stats"));
    assert!(ci.contains("name: Record compiler cache stats"));
    assert!(ci.contains("uses: https://code.forgejo.org/actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1"));
    assert!(ci.contains("uses: https://code.forgejo.org/actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0"));
    assert!(!ci.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(ci.contains("path: ~/.cargo/bin"));
    assert!(ci.contains("hashFiles('.forgejo/workflows/*.yaml')"));
    assert!(!ci.contains("hashFiles('.forgejo/workflows/ci.yaml', '.github/workflows/ci.yaml')"));
    assert!(ci.contains(
        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version 0.9.100"
    ));
    assert!(!ci.contains("&>/dev/null"));
    assert!(!ci.contains("run: apk add --no-cache git build-base"));
    assert!(ci.contains("run: rustup component add clippy rustfmt"));
    assert!(ci.contains("run: cargo nextest run --all-features"));
    assert!(!ci.contains("uses: https://github.com/cachix/install-nix-action@v31"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(publish.contains("runs-on: atlas"));
    assert!(publish.contains("container: rust:1.85-bookworm"));
    assert!(publish.contains("uses: https://code.forgejo.org/actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0"));
    assert!(!publish.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(publish.contains("path: ~/.cargo/bin"));
    assert!(publish.contains("hashFiles('.forgejo/workflows/*.yaml')"));
    assert!(
        !publish.contains("hashFiles('.forgejo/workflows/ci.yaml', '.github/workflows/ci.yaml')")
    );
    assert!(publish.contains(
        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version 0.9.100"
    ));
    assert!(
        publish.contains(r#"cargo pkgid -p demo | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#)
    );
}

#[test]
fn rendered_ci_yaml_parses_as_valid_yaml() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--with-nextest",
            "--with-audit",
            "--with-deny",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_yaml_parses(&ci);
}

#[test]
fn forgejo_runner_override_applies_to_all_jobs() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--runner",
            "codeberg-medium-lazy",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: codeberg-medium-lazy"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: codeberg-medium-lazy"));
}

#[test]
fn forgejo_runner_override_does_not_require_user_config() {
    let temp = init_package(false);

    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runner",
            "atlas",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: atlas"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: atlas"));
}

#[test]
fn forgejo_user_config_can_render_structured_runner_labels() {
    let temp = init_package(false);

    let status = simit_with_multilabel_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--publish-crates"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: [\"self-hosted\", \"atlas\"]"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: [\"self-hosted\", \"atlas\"]"));
}

#[test]
fn forgejo_nix_runtime_uses_nix_runner_for_publish_jobs() {
    let temp = init_package(true);

    let status = simit_with_split_runtime_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: atlas-nix-trusted"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: atlas-nix-trusted"));
}

#[test]
fn featureful_package_gets_no_default_feature_checks() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "demo"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"

[features]
default = ["sync"]
sync = []
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

    let status = simit_with_user_config(root)
        .current_dir(root)
        .args(["init", "ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&root.join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("run: cargo test --all-features"));
    assert!(ci.contains("run: cargo test --no-default-features"));
    assert!(ci.contains("run: cargo clippy --all-targets --all-features -- --deny warnings"));
    assert!(
        ci.contains("run: cargo clippy --all-targets --no-default-features -- --deny warnings")
    );
}

#[test]
fn self_check_preserves_runner_override() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "simit"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();

    let status = simit_with_user_config(root)
        .current_dir(root)
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runner",
            "codeberg-medium",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&root.join(".forgejo/workflows/ci.yaml"));
    assert!(
        ci.contains("cargo run -- init ci --platform forgejo --runner codeberg-medium --check")
    );
}

#[test]
fn self_check_preserves_windows_packager_flags() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "simit"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
authors = ["Example Maintainers"]
license = "MIT"
description = "demo binary"
homepage = "https://example.com/demo"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    write_windows_packager_config(root);

    let status = simit_with_user_config(root)
        .current_dir(root)
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-chocolatey",
            "--with-scoop",
            "--windows-runner",
            "windows-latest",
            "--publish-strategy",
            "members",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&root.join(".github/workflows/ci.yaml"));
    assert!(ci.contains(
        "cargo run -- init ci --platform github --runner ubuntu-latest --windows-runner windows-latest --with-artifacts --publish-crates --with-chocolatey --with-scoop --check"
    ));
}

#[test]
fn check_succeeds_when_workflows_are_current() {
    let temp = init_package(true);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--publish-crates"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let check_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn project_check_command_replaces_generic_cargo_lane() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "forgejo"
runtime = "nix"
runner = "atlas"
check_command = "cargo xtask ci"
nix_flake_check = false
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--runner",
            "atlas",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("name: Project checks"));
    assert!(ci.contains("nix develop -c sh -c 'cargo xtask ci'"));
    assert!(!ci.contains("cargo fmt --all -- --check"));
    assert!(!ci.contains("cargo clippy"));
    assert!(!ci.contains("nix flake check"));
}

#[test]
fn project_check_command_keeps_nix_flake_check_by_default() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
runtime = "nix"
runner = "ubuntu-latest"
check_command = "cargo xtask ci"
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--runner",
            "ubuntu-latest",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains("name: Check flake"));
    assert!(ci.contains("name: Project checks"));
}

#[test]
fn nix_cargo_cache_is_opt_in_for_nix_runtime() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
runtime = "nix"
runner = "ubuntu-latest"
with_nix_cargo_cache = true
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains("name: Cache Nix Cargo registry + target"));
    assert!(ci.contains("/tmp/.cargo/registry"));
    assert!(ci.contains("/tmp/.cargo/git"));
    assert!(ci.contains("target"));
    assert!(ci.contains(
        "hashFiles('Cargo.lock', 'flake.lock', 'flake.nix', 'rust-toolchain.toml', '.cargo/config', '.cargo/config.toml')"
    ));
    let disabled = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--with-nix-cargo-cache=false"])
        .output()
        .unwrap();
    assert!(
        disabled.status.success(),
        "{}",
        String::from_utf8_lossy(&disabled.stderr)
    );
    assert!(!read(&temp.path().join(".github/workflows/ci.yaml")).contains("Cache Nix Cargo"));
    let check = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--check"])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn invalid_nix_cargo_cache_requests_preserve_project_files() {
    for policy in [
        "runtime = \"cargo\"\n",
        "runtime = \"nix\"\nprovider = \"crow\"\nplatform = \"forgejo\"\n",
        "runtime = \"nix\"\n[ci.extra_env]\nCARGO_HOME = \"/custom\"\n",
        "runtime = \"nix\"\n[ci.extra_env]\nCARGO_TARGET_DIR = \"/custom\"\n",
    ] {
        let temp = init_package(true);
        let config = format!("[ci]\nwith_nix_cargo_cache = true\n{policy}");
        fs::write(temp.path().join("simit.toml"), &config).unwrap();
        let output = simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{policy}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("with_nix_cargo_cache"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(read(&temp.path().join("simit.toml")), config);
        assert!(!temp.path().join(".github/workflows/ci.yaml").exists());
        assert!(!temp.path().join(".crow/build.yaml").exists());
    }
}

#[test]
fn project_check_and_flake_check_override_round_trip() {
    let temp = init_package(true);
    fs::write(temp.path().join("simit.toml"), "[ci]\nplatform = \"github\"\nruntime = \"nix\"\ncheck_command = \"cargo xtask ci\"\nnix_flake_check = false\n").unwrap();
    for (flag, enabled) in [
        ("--nix-flake-check=true", true),
        ("--nix-flake-check=false", false),
    ] {
        let output = simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(["init", "ci", flag])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
        assert_eq!(ci.contains("run: nix flake check"), enabled);
        assert!(ci.contains("Project checks"));
        let check = simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(["init", "ci", "--check"])
            .output()
            .unwrap();
        assert!(
            check.status.success(),
            "{}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

#[test]
fn project_check_commands_render_as_yaml_blocks() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
runtime = "nix"
runner = "ubuntu-latest"
check_command = "printf 'url: # safe' && cargo xtask ci"
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());
    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains("run: |\n          nix develop -c sh -c 'printf"));
    assert!(ci.contains(" && cargo xtask ci'"));
    assert_yaml_parses(&ci);
}

#[test]
fn granular_project_check_commands_render_as_yaml_blocks() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "forgejo"
runtime = "nix"
runner = "atlas"
check_command = "printf 'url: # safe' && cargo xtask ci"
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--runner",
            "atlas",
            "--granular",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("run: |\n          nix develop -c sh -c 'printf"));
    assert!(ci.contains(" && cargo xtask ci'"));
    assert_yaml_parses(&ci);
}

#[test]
fn crow_cargo_steps_install_rust_components_before_use() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
provider = "crow"
platform = "forgejo"
runtime = "cargo"
runner = "atlas"
check_command = "cargo xtask ci"
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "cargo",
            "--runner",
            "atlas",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".crow/build.yaml"));
    assert!(ci.contains("rustup component add clippy rustfmt && cargo xtask ci"));
}

#[test]
fn crow_ci_check_ignores_release_workflow_owned_by_init_release() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
provider = "crow"
platform = "forgejo"
runtime = "nix"
runner = "atlas"
"#,
    )
    .unwrap();

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--runner",
            "atlas",
        ])
        .status()
        .unwrap();
    assert!(write_status.success());
    fs::write(
        temp.path().join(".crow/release.yaml"),
        "# Generated by simit. Manual edits will be reported as ci=drift.\n",
    )
    .unwrap();

    let check_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--runner",
            "atlas",
            "--check",
        ])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn workspace_flag_generates_per_package_workflows() {
    let temp = init_workspace_fixture();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    assert!(!temp.path().join(".forgejo/workflows/ci.yaml").exists());
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/publish-crate.yaml")
            .exists()
    );

    let alpha_ci = read(&temp.path().join(".forgejo/workflows/ci-alpha.yaml"));
    assert_yaml_parses(&alpha_ci);
    assert!(alpha_ci.contains("container: rust:1.85-bookworm"));
    assert!(alpha_ci.contains("run: cargo test -p alpha --all-features"));
    assert!(
        alpha_ci
            .contains("run: cargo clippy -p alpha --all-targets --all-features -- --deny warnings")
    );
    assert!(alpha_ci.contains("run: cargo package -p alpha --allow-dirty --list"));
    assert!(!alpha_ci.contains("--no-default-features"));

    let beta_ci = read(&temp.path().join(".forgejo/workflows/ci-beta.yaml"));
    assert_yaml_parses(&beta_ci);
    assert!(beta_ci.contains("run: cargo test -p beta --all-features"));
    assert!(beta_ci.contains("run: cargo test -p beta --no-default-features"));
    assert!(beta_ci.contains(
        "run: cargo clippy -p beta --all-targets --no-default-features -- --deny warnings"
    ));
    assert!(beta_ci.contains("run: cargo package -p beta --allow-dirty --list"));

    let alpha_publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-alpha.yaml"),
    );
    assert_yaml_parses(&alpha_publish);
    assert!(alpha_publish.contains("run: cargo publish -p alpha --dry-run"));
    assert!(alpha_publish.contains("cargo publish -p alpha"));

    let beta_publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-beta.yaml"),
    );
    assert_yaml_parses(&beta_publish);
    assert!(beta_publish.contains("run: cargo publish -p beta --dry-run"));
    assert!(beta_publish.contains("cargo publish -p beta"));
}

#[test]
fn github_workspace_workflows_have_path_scoped_concurrency() {
    let temp = init_workspace_fixture();
    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--workspace"])
        .status()
        .unwrap();
    assert!(status.success());

    for member in ["alpha", "beta"] {
        let workflow = read(
            &temp
                .path()
                .join(format!(".github/workflows/ci-{member}.yaml")),
        );
        let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
        let expected_name = format!("CI ({member})");
        assert_eq!(yaml["name"].as_str(), Some(expected_name.as_str()));
        let group = yaml["concurrency"]["group"].as_str().unwrap();
        // Workflow identity must still isolate source paths and event types.
        assert!(group.contains("${{ github.workflow_ref }}"), "{group}");
        assert!(group.contains("${{ github.event_name }}"), "{group}");
        assert!(
            group.contains("github.head_ref || github.ref_name"),
            "{group}"
        );
        assert_eq!(
            yaml["concurrency"]["cancel-in-progress"].as_bool(),
            Some(true)
        );
    }
}

#[test]
fn aggregate_workspace_strategy_generates_one_workspace_workflow() {
    let temp = init_workspace_fixture();
    fs::write(temp.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = temp.path().join(".github/workflows/ci.yaml");
    assert!(workflow.exists());
    assert!(!temp.path().join(".github/workflows/ci-alpha.yaml").exists());
    assert!(!temp.path().join(".github/workflows/ci-beta.yaml").exists());

    let ci = read(&workflow);
    assert_yaml_parses(&ci);
    assert!(ci.contains("run: nix develop -c cargo test --workspace --all-features"));
    assert!(ci.contains(
        "run: nix develop -c cargo clippy --workspace --all-targets --all-features -- --deny warnings"
    ));
    assert!(!ci.contains("cargo package"));
}

#[test]
fn aggregate_workspace_ci_can_retain_independent_original_events() {
    for separate_events in [false, true] {
        let temp = init_workspace_fixture();
        let setting = if separate_events {
            "separate_events = true\n"
        } else {
            ""
        };
        fs::write(
            temp.path().join("simit.toml"),
            format!("[ci]\nworkspace = true\nworkspace_strategy = \"aggregate\"\n{setting}"),
        )
        .unwrap();
        let generate = || {
            simit_with_user_config(temp.path())
                .current_dir(temp.path())
                .args(["init", "ci", "--platform", "github", "--runtime", "cargo"])
                .output()
                .unwrap()
        };
        let result = generate();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let workflow = read(&temp.path().join(".github/workflows/ci.yaml"));
        let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
        let group = yaml["concurrency"]["group"].as_str().unwrap();
        let resolve = |event: &str, source_repository: &str, branch: &str| {
            let event_ref = if event == "pull_request" {
                "refs/pull/9/merge".to_owned()
            } else {
                format!("refs/heads/{branch}")
            };
            let resolved = group
                .replace(
                    "${{ github.workflow_ref }}",
                    &format!("owner/project/.github/workflows/ci.yaml@{event_ref}"),
                )
                .replace("${{ github.event_name }}", event)
                .replace(
                    "${{ github.event.pull_request.head.repo.full_name || github.repository }}",
                    source_repository,
                )
                .replace(
                    "${{ github.event.pull_request.head.ref || github.ref_name }}",
                    branch,
                )
                .replace(
                    "${{ github.ref_type == 'tag' && github.ref || format('refs/heads/{0}', github.event.pull_request.head.ref || github.ref_name) }}",
                    &format!("refs/heads/{branch}"),
                );
            assert!(!resolved.contains("${{"), "unrecognized group: {resolved}");
            resolved
        };
        let push = resolve("push", "owner/project", "feature");
        let pr = resolve("pull_request", "owner/project", "feature");
        assert_eq!(push != pr, separate_events);
        assert_ne!(pr, resolve("pull_request", "alice/project", "feature"));
        assert_ne!(pr, resolve("pull_request", "bob/project", "feature"));
        assert_ne!(push, resolve("push", "owner/project", "other-feature"));
        assert_eq!(push, resolve("push", "owner/project", "feature"));
        assert_eq!(yaml["concurrency"]["cancel-in-progress"], true);
        assert!(workflow.contains("cargo test --workspace --all-features"));

        // Regeneration must preserve the explicit producer policy and remain managed.
        assert!(generate().status.success());
        let config = simit::config::ProjectConfig::load(temp.path()).unwrap();
        assert_eq!(
            read(&temp.path().join("simit.toml")).contains("separate_events = true"),
            separate_events
        );
        assert_eq!(
            config.ci.workspace_strategy,
            simit::cli::WorkspaceStrategy::Aggregate
        );
        let check = simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(["init", "ci", "--platform", "github", "--check", "--diff"])
            .output()
            .unwrap();
        assert!(
            check.status.success(),
            "{}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

#[test]
fn aggregate_separate_events_preserve_completion_custody() {
    let temp = init_workspace_fixture();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
provider = "actions"
runtime = "cargo"
workspace = true
workspace_strategy = "aggregate"
separate_events = true
[ci.custody]
repository = "owner/project"
producer_attempt = 1
status_context = "simit/producer-custody"
[ci.custody.producers.CI]
workflow = ".github/workflows/ci.yaml"
jobs = ["test"]
"#,
    )
    .unwrap();
    let generate = || {
        simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap()
    };
    let result = generate();
    assert!(result.status.success(), "{result:?}");
    let config_path = temp.path().join("simit.toml");
    let config = simit::config::ProjectConfig::load(temp.path()).unwrap();
    let observer_path = temp.path().join(".github/workflows/producer-custody.yaml");
    let observer = read(&observer_path);
    assert!(config.ci.separate_events);
    assert_eq!(
        observer,
        simit::render::ci::custody::file(config.ci.custody.as_ref().unwrap())
            .unwrap()
            .content
    );
    assert!(
        read(&temp.path().join(".github/workflows/ci.yaml")).contains("${{ github.event_name }}")
    );
    let persisted_config = read(&config_path);
    assert!(generate().status.success());
    assert_eq!(read(&config_path), persisted_config);
    assert_eq!(read(&observer_path), observer);
    for args in [
        vec!["init", "ci", "--primary-only", "--check", "--diff"],
        vec!["init", "ci", "--primary-only"],
        vec!["init", "ci", "--primary-only", "--check", "--diff"],
    ] {
        let result = simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(args)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        assert_eq!(read(&config_path), persisted_config);
        assert_eq!(read(&observer_path), observer);
        assert!(
            read(&temp.path().join(".github/workflows/ci.yaml"))
                .contains("${{ github.event_name }}")
        );
    }
    assert!(
        simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(["init", "ci", "--check", "--diff"])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
}

#[test]
fn separate_rust_ci_events_reject_unsupported_backends_without_effects() {
    for (platform, provider, workflow_path) in [
        ("forgejo", "actions", ".forgejo/workflows/ci.yaml"),
        ("forgejo", "crow", ".crow/build.yaml"),
    ] {
        let temp = init_package(false);
        fs::write(
            temp.path().join("simit.toml"),
            format!("[ci]\nplatform = '{platform}'\nprovider = '{provider}'\nruntime = 'cargo'\n"),
        )
        .unwrap();
        let generate = || {
            simit_with_user_config(temp.path())
                .current_dir(temp.path())
                .args(["init", "ci"])
                .output()
                .unwrap()
        };
        assert!(generate().status.success());
        let workflow = read(&temp.path().join(workflow_path));
        let config = read(&temp.path().join("simit.toml")).replacen(
            "[ci]\n",
            "[ci]\nseparate_events = true\n",
            1,
        );
        fs::write(temp.path().join("simit.toml"), &config).unwrap();
        let result = generate();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("separate_events"));
        assert_eq!(read(&temp.path().join("simit.toml")), config);
        assert_eq!(read(&temp.path().join(workflow_path)), workflow);
        let error = simit::registry::audit_ci(temp.path()).unwrap_err();
        assert!(format!("{error:#}").contains("separate_events"));
    }
}

#[test]
fn separate_rust_ci_events_reject_non_rust_projects_without_effects() {
    for python in [false, true] {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();
        if python {
            fs::write(
                temp.path().join("pyproject.toml"),
                "[project]\nname = 'demo'\nversion = '0.1.0'\nrequires-python = '>=3.12'\n",
            )
            .unwrap();
            fs::write(temp.path().join("uv.lock"), "version = 1\n").unwrap();
        }
        fs::write(
            temp.path().join("simit.toml"),
            "[ci]\nplatform = 'github'\nprovider = 'actions'\nruntime = 'nix'\n",
        )
        .unwrap();
        let generate = || {
            simit_with_user_config(temp.path())
                .current_dir(temp.path())
                .args(["init", "ci"])
                .output()
                .unwrap()
        };
        assert!(generate().status.success());
        let workflow = read(&temp.path().join(".github/workflows/ci.yaml"));
        let config = read(&temp.path().join("simit.toml")).replacen(
            "[ci]\n",
            "[ci]\nseparate_events = true\n",
            1,
        );
        fs::write(temp.path().join("simit.toml"), &config).unwrap();
        let result = generate();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("separate_events"));
        assert_eq!(read(&temp.path().join("simit.toml")), config);
        assert_eq!(
            read(&temp.path().join(".github/workflows/ci.yaml")),
            workflow
        );
        let error = simit::registry::audit_ci(temp.path()).unwrap_err();
        assert!(format!("{error:#}").contains("separate_events"));
    }
}

#[test]
fn aggregate_workspace_strategy_can_skip_all_feature_checks() {
    let temp = init_workspace_fixture();
    fs::write(temp.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nall_features = false\n",
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains("run: nix develop -c cargo test --workspace\n"));
    assert!(
        ci.contains(
            "run: nix develop -c cargo clippy --workspace --all-targets -- --deny warnings"
        )
    );
    assert!(!ci.contains("--all-features"));
}

#[test]
fn aggregate_workspace_strategy_can_limit_tests_to_library_targets() {
    let temp = init_workspace_fixture();
    fs::write(temp.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nunit_tests_only = true\n",
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(
        ci.contains("run: nix develop -c cargo test --workspace --lib"),
        "generated CI:\n{ci}"
    );
}

#[test]
fn workspace_publish_false_package_keeps_ci_but_skips_package_and_publish_workflows() {
    let temp = init_workspace_fixture();
    let beta_manifest = temp.path().join("crates/beta/Cargo.toml");
    let mut beta = fs::read_to_string(&beta_manifest).unwrap();
    beta = beta.replacen(
        "license = \"MIT\"\n",
        "license = \"MIT\"\npublish = false\n",
        1,
    );
    fs::write(&beta_manifest, beta).unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let beta_ci = read(&temp.path().join(".forgejo/workflows/ci-beta.yaml"));
    assert_yaml_parses(&beta_ci);
    assert!(beta_ci.contains("run: cargo test -p beta --all-features"));
    assert!(
        beta_ci
            .contains("run: cargo clippy -p beta --all-targets --all-features -- --deny warnings")
    );
    assert!(!beta_ci.contains("cargo package -p beta"));
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/publish-crate-beta.yaml")
            .exists()
    );

    let alpha_ci = read(&temp.path().join(".forgejo/workflows/ci-alpha.yaml"));
    assert!(alpha_ci.contains("run: cargo package -p alpha --allow-dirty --list"));
    assert!(
        temp.path()
            .join(".forgejo/workflows/publish-crate-alpha.yaml")
            .exists()
    );
}

#[test]
fn workspace_publish_tag_validation_is_package_scoped_for_diverging_versions() {
    let temp = init_diverging_workspace_fixture();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let member_a_publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-member-a.yaml"),
    );
    assert_yaml_parses(&member_a_publish);
    assert!(
        member_a_publish
            .contains(r#"cargo pkgid -p member-a | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#)
    );
    assert!(!member_a_publish.contains("cargo pkgid -p member-b"));
    assert!(!member_a_publish.contains("cargo metadata --no-deps --format-version 1"));

    let member_b_publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-member-b.yaml"),
    );
    assert_yaml_parses(&member_b_publish);
    assert!(
        member_b_publish
            .contains(r#"cargo pkgid -p member-b | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#)
    );
    assert!(!member_b_publish.contains("cargo pkgid -p member-a"));
    assert!(!member_b_publish.contains("cargo metadata --no-deps --format-version 1"));
}

#[test]
fn package_flag_generates_selected_package_workflows() {
    let temp = init_workspace_fixture();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--package",
            "beta",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    assert!(!temp.path().join(".forgejo/workflows/ci.yaml").exists());
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/ci-alpha.yaml")
            .exists()
    );
    assert!(temp.path().join(".forgejo/workflows/ci-beta.yaml").exists());

    let ci = read(&temp.path().join(".forgejo/workflows/ci-beta.yaml"));
    assert_yaml_parses(&ci);
    assert!(ci.contains("run: cargo test -p beta --all-features"));

    let publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-beta.yaml"),
    );
    assert_yaml_parses(&publish);
    assert!(publish.contains("run: cargo publish -p beta --dry-run"));
}

#[test]
fn workspace_check_diff_detects_stale_generated_workflow() {
    let temp = init_workspace_fixture();

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--workspace"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(
        temp.path().join(".forgejo/workflows/ci-alpha.yaml"),
        "name: stale\n",
    )
    .unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--check",
            "--diff",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(".forgejo/workflows/ci-alpha.yaml differs"));
    assert!(stderr.contains("--- .forgejo/workflows/ci-alpha.yaml"));
}

#[test]
fn workspace_check_rejects_extra_managed_workflows() {
    let temp = init_workspace_fixture();

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--workspace"])
        .status()
        .unwrap();
    assert!(write_status.success());

    for name in ["ci-old.yaml", "nix-builds.yaml", "prebuild.yaml"] {
        fs::write(
            temp.path().join(".forgejo/workflows").join(name),
            format!(
                "{}\nname: old\n",
                simit::render::ci::GENERATED_WORKFLOW_MARKER
            ),
        )
        .unwrap();
    }

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--check",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(".forgejo/workflows/nix-builds.yaml is extra"));
    assert!(stderr.contains(".forgejo/workflows/ci-old.yaml is extra"));
    assert!(stderr.contains(".forgejo/workflows/prebuild.yaml is extra"));
}

#[test]
fn workspace_check_ignores_generated_release_workflow() {
    let temp = init_workspace_fixture();

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--workspace"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(
        temp.path().join(".forgejo/workflows/release.yml"),
        format!(
            "{}\nname: release\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        ),
    )
    .unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--check",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn init_ci_blocks_without_exportable_maintainer_key() {
    let temp = init_package(false);
    let isolated_home = TempDir::new().unwrap();
    write_user_runner_config(isolated_home.path());

    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(temp.path())
        .env_remove("SIMIT_MAINTAINERS_GPG")
        .env("HOME", isolated_home.path())
        .env("XDG_CONFIG_HOME", isolated_home.path().join(".xdg"))
        .env("XDG_DATA_HOME", common::data_home_path())
        .env("GIT_CONFIG_GLOBAL", isolated_home.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "true")
        .args(["init", "ci", "--platform", "forgejo", "--publish-crates"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("release signing key not configured"));
    assert!(stderr.contains("git config user.signingkey"));
}

#[test]
fn release_trust_init_writes_maintainer_key() {
    let temp = init_package(false);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["release", "trust", "init"])
        .status()
        .unwrap();

    assert!(status.success());
    assert_maintainer_key_written(temp.path());
}

#[test]
fn release_trust_check_accepts_existing_trust_root_without_local_key() {
    let temp = init_package(false);

    let init_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["release", "trust", "init"])
        .status()
        .unwrap();
    assert!(init_status.success());

    let isolated_home = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(temp.path())
        .env_remove("SIMIT_MAINTAINERS_GPG")
        .env("HOME", isolated_home.path())
        .env("XDG_CONFIG_HOME", isolated_home.path().join("xdg"))
        .env("XDG_DATA_HOME", common::data_home_path())
        .env("GIT_CONFIG_GLOBAL", isolated_home.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "true")
        .args(["release", "trust", "check"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("keys/maintainers.gpg is present and parseable"));
}

#[test]
fn check_fails_when_workflows_differ() {
    let temp = init_package(true);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--publish-crates"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(
        temp.path().join(".forgejo/workflows/ci.yaml"),
        "name: stale\n",
    )
    .unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--publish-crates",
            "--check",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("CI workflows are not up to date"));
    assert!(
        stderr.contains("run `simit init ci --platform forgejo --ci-provider actions`"),
        "stderr:\n{stderr}"
    );
    assert!(stderr.contains(".forgejo/workflows/ci.yaml differs"));
}

#[test]
fn check_failure_hint_includes_transient_generation_flags() {
    let temp = init_workspace_fixture();
    fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--runner",
            "atlas",
            "--workspace",
            "--with-deny",
            "--with-artifacts",
        ])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(
        temp.path().join(".forgejo/workflows/ci-alpha.yaml"),
        "name: stale\n",
    )
    .unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--runner",
            "atlas-nix-trusted",
            "--workspace",
            "--with-deny",
            "--with-artifacts",
            "--check",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(
            "run `simit init ci --platform forgejo --ci-provider actions --runtime nix --runner atlas-nix-trusted"
        ),
        "stderr:\n{stderr}"
    );
    assert!(stderr.contains("--workspace --with-deny --with-artifacts"));
    assert!(stderr.contains(".forgejo/workflows/ci-alpha.yaml differs"));
    let hint = stderr.split('`').nth(1).unwrap();
    assert!(
        simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(hint.split_whitespace().skip(1))
            .status()
            .unwrap()
            .success()
    );
    assert!(
        simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(["init", "ci", "--check"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn init_ci_persists_resolved_options_in_simit_toml() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-audit",
            "--with-deny",
            "--with-docs",
            "--with-msrv",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let config = read(&temp.path().join("simit.toml"));
    assert!(config.contains("[ci]"));
    assert!(config.contains("runtime = \"nix\""));
    assert!(config.contains("runner = \"atlas\""));
    assert!(config.contains("with_audit = true"));
    assert!(config.contains("with_deny = true"));
    assert!(config.contains("with_docs = true"));
    assert!(config.contains("with_msrv = true"));
}

#[test]
fn bare_check_uses_persisted_ci_options() {
    let temp = init_package(true);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-audit",
            "--with-deny",
        ])
        .status()
        .unwrap();
    assert!(write_status.success());

    let check_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check", "--diff"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn cli_flags_override_persisted_ci_values() {
    let temp = init_package(false);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
with_audit = false
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--with-audit"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains("cargo audit --db \"$db\" --no-fetch --stale"));

    let config = read(&temp.path().join("simit.toml"));
    assert!(config.contains("with_audit = true"));
}

#[test]
fn init_ci_write_preserves_existing_distribution_sections() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"# keep this comment
[homebrew]
tap_url = "https://example.com/homebrew-demo.git"
download_repo = "foo/demo"

[chocolatey]
download_repo = "foo/demo"

[chocolatey.push]
source = "https://push.chocolatey.org/"

[scoop]
bucket_url = "https://example.com/scoop-demo.git"
download_repo = "foo/demo"
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-audit",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let config = read(&temp.path().join("simit.toml"));
    assert!(config.contains("# keep this comment"));
    assert!(config.contains("[homebrew]"));
    assert!(config.contains("tap_url = \"https://example.com/homebrew-demo.git\""));
    assert!(config.contains("[chocolatey]"));
    assert!(config.contains("[chocolatey.push]"));
    assert!(config.contains("[scoop]"));
    assert!(config.contains("with_audit = true"));
}

#[test]
fn check_fails_when_publish_workflow_is_missing() {
    let temp = init_package(true);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--publish-crates"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::remove_file(temp.path().join(".forgejo/workflows/publish-crate.yaml")).unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("CI workflows are not up to date"));
    assert!(stderr.contains(".forgejo/workflows/publish-crate.yaml is missing"));
}

#[test]
fn simit_package_gets_self_check_step() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "simit"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();

    let status = simit_with_user_config(root)
        .current_dir(root)
        .args(["init", "ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&root.join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("cargo run -- init ci --platform forgejo --runner atlas --check"));
    assert!(ci.contains("cargo run -- init flake --check"));
    assert!(!ci.contains("cargo run -- ci"));
}

#[test]
fn runtime_ci_namespace_cannot_be_used_as_the_old_generation_command() {
    let temp = init_package(false);
    for args in [vec!["ci"], vec!["ci", "--platform", "forgejo"]] {
        let output = simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("Usage: simit ci"));
        assert!(!temp.path().join(".forgejo/workflows").exists());
        assert!(!temp.path().join(".github/workflows").exists());
    }
    let output = simit_with_user_config(temp.path())
        .args(["ci", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("nix-group"));
}

#[test]
fn old_init_ci_command_is_not_available() {
    let temp = init_package(false);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unrecognized subcommand 'init-ci'"));
}

#[test]
fn optional_strict_flags_render_expected_steps() {
    let temp = init_package(false);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--with-msrv",
            "--with-audit",
            "--with-deny",
            "--with-docs",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains(
        "run: command -v cargo-audit >/dev/null 2>&1 || cargo install cargo-audit --locked"
    ));
    assert!(ci.contains("      - name: Fetch RustSec advisory database"));
    assert!(ci.contains(
        "git -c http.lowSpeedLimit=1024 -c http.lowSpeedTime=30 clone --depth 1 https://github.com/RustSec/advisory-db.git \"$db\""
    ));
    assert!(ci.contains("cargo audit --db \"$db\" --no-fetch --stale"));
    assert!(ci.contains(
        "run: command -v cargo-deny >/dev/null 2>&1 || cargo install cargo-deny --locked --version 0.18.3"
    ));
    assert!(ci.contains("run: cargo deny check bans licenses sources"));
    assert!(ci.contains("rustup toolchain install 1.85 --profile minimal"));
    assert!(ci.contains("cargo +1.85 check --all-targets --all-features"));
    assert!(ci.contains("run: cargo doc --no-deps --all-features"));
    assert!(!ci.contains("&>/dev/null"));

    let deny = read(&temp.path().join("deny.toml"));
    assert!(deny.contains("\"MIT\""));
    assert!(deny.contains("\"BSD-2-Clause\""));
    assert!(deny.contains("\"BSL-1.0\""));
    assert!(deny.contains("\"CC0-1.0\""));
    assert!(deny.contains("\"ISC\""));
    assert!(deny.contains("\"LicenseRef-UFL-1.0\""));
    assert!(deny.contains("\"Apache-2.0\""));
    assert!(deny.contains("\"OFL-1.1\""));
    assert!(deny.contains("\"Unicode-3.0\""));
    assert!(deny.contains("\"Unlicense\""));
    assert!(deny.contains("\"Zlib\""));
    assert!(deny.contains("allow-registry = [\"https://github.com/rust-lang/crates.io-index\"]"));
}

#[test]
fn forgejo_nix_homebrew_step_matches_hardened_shape() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        "[release]\ntag_prefix = 'v'\n",
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-artifacts",
            "--with-homebrew",
            "--homebrew-tap",
            "https://example.com/homebrew-demo.git",
            "--homebrew-description",
            "demo binary",
            "--homebrew-homepage",
            "https://example.com",
            "--homebrew-download-repo",
            "foo/demo",
            "--homebrew-binary",
            "demo",
            "--homebrew-binary",
            "demo-ui",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(
        &temp
            .path()
            .join(".forgejo/workflows/release-artifacts.yaml"),
    );
    assert_homebrew_run_block_indentation(&workflow);
    assert_release_integrity_steps(&workflow);
    assert!(workflow.contains("enable-openid-connect: true"));
    assert!(workflow.contains("name: Publish Homebrew tap"));
    assert!(workflow.contains("HOMEBREW_TAP_TOKEN: ${{ secrets.homebrew_tap_token }}"));
    assert!(workflow.contains("HOMEBREW_TAP_REPO: homebrew-demo"));
    assert!(workflow.contains("HOMEBREW_TAP_URL: https://example.com/homebrew-demo.git"));
    assert!(workflow.contains("set -euo pipefail"));
    assert!(
        workflow.contains(
            "HOMEBREW_TAP_TOKEN is required because Homebrew tap publishing is configured."
        )
    );
    assert!(workflow.contains("\"release/demo-${VERSION}-aarch64-darwin.tar.gz\""));
    assert!(!workflow.contains("\"release/demo-${VERSION}-x86_64-darwin.tar.gz\""));
    assert!(workflow.contains("\"release/demo-${VERSION}-aarch64-linux.tar.gz\""));
    assert!(workflow.contains("\"release/demo-${VERSION}-x86_64-linux.tar.gz\""));
    assert!(workflow.contains("credential_helper='!f() { echo username=caniko; echo \"password=$HOMEBREW_TAP_TOKEN\"; }; f'"));
    assert!(workflow.contains(
        "git -c credential.helper=\"$credential_helper\" clone \"$HOMEBREW_TAP_URL\" tap"
    ));
    assert!(!workflow.contains("https://$HOMEBREW_TAP_TOKEN"));
    assert!(workflow.contains("nix run '.#harbor-rs' -- brew bump \\"));
    assert!(workflow.contains("--name demo \\"));
    assert!(workflow.contains("--description 'demo binary' \\"));
    assert!(workflow.contains("--homepage 'https://example.com' \\"));
    assert!(workflow.contains("--license MIT \\"));
    assert!(workflow.contains("RELEASE_TAG=\"$VERSION\""));
    assert!(workflow.contains("VERSION=\"${VERSION#v}\""));
    assert!(workflow.contains("--archive \"darwin_arm=https://codeberg.org/foo/demo/releases/download/${RELEASE_TAG}/demo-${VERSION}-aarch64-darwin.tar.gz,release/demo-${VERSION}-aarch64-darwin.tar.gz\" \\"));
    assert!(!workflow.contains("--archive \"darwin_intel=https://codeberg.org/foo/demo/releases/download/${RELEASE_TAG}/demo-${VERSION}-x86_64-darwin.tar.gz,release/demo-${VERSION}-x86_64-darwin.tar.gz\" \\"));
    assert!(workflow.contains("--archive \"linux_arm=https://codeberg.org/foo/demo/releases/download/${RELEASE_TAG}/demo-${VERSION}-aarch64-linux.tar.gz,release/demo-${VERSION}-aarch64-linux.tar.gz\" \\"));
    assert!(workflow.contains("--archive \"linux_intel=https://codeberg.org/foo/demo/releases/download/${RELEASE_TAG}/demo-${VERSION}-x86_64-linux.tar.gz,release/demo-${VERSION}-x86_64-linux.tar.gz\" \\"));
    assert_eq!(workflow.matches("--binary ").count(), 2);
    assert!(workflow.contains("--binary demo \\"));
    assert!(workflow.contains("--binary demo-ui \\"));
    assert!(workflow.contains("if [ -z \"$(git status --porcelain -- Formula/demo.rb)\" ]; then"));
    assert!(workflow.contains("tap already contains demo ${VERSION}; nothing to push"));
    assert!(workflow.contains("git push origin \"HEAD:${DEFAULT_BRANCH}\""));

    let check_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-artifacts",
            "--with-homebrew",
            "--homebrew-tap",
            "https://example.com/homebrew-demo.git",
            "--homebrew-description",
            "demo binary",
            "--homebrew-homepage",
            "https://example.com",
            "--homebrew-download-repo",
            "foo/demo",
            "--homebrew-binary",
            "demo",
            "--homebrew-binary",
            "demo-ui",
            "--check",
        ])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn homebrew_rejects_cargo_runtime() {
    let temp = init_package(true);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "cargo",
            "--with-homebrew",
            "--homebrew-tap",
            "https://example.com/homebrew-demo.git",
            "--homebrew-description",
            "demo binary",
            "--homebrew-homepage",
            "https://example.com",
            "--homebrew-download-repo",
            "foo/demo",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Homebrew tap publish requires --runtime nix"));
}

#[test]
fn homebrew_supports_github_platform() {
    let temp = init_package(true);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--with-homebrew",
            "--homebrew-tap",
            "https://example.com/homebrew-demo.git",
            "--homebrew-description",
            "demo binary",
            "--homebrew-homepage",
            "https://example.com",
            "--homebrew-download-repo",
            "foo/demo",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("name: Publish Homebrew tap"));
    assert!(workflow.contains("https://github.com/foo/demo/releases/download/"));
    assert!(workflow.contains("VERSION=\"${GITHUB_REF_NAME:-"));
}

#[test]
fn github_chocolatey_flag_resolves_when_config_is_present() {
    let temp = init_package(false);
    write_chocolatey_config(temp.path());

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-chocolatey",
            "--windows-runner",
            "windows-latest",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    let workflow = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_yaml_parses(&workflow);
    assert_release_integrity_steps(&workflow);
    assert!(workflow.contains("id-token: write"));
    assert!(workflow.contains("name: Release Artifacts"));
    assert!(workflow.contains("build-linux:"));
    assert!(workflow.contains("build-windows:"));
    assert!(workflow.contains("runs-on: windows-latest"));
    assert!(workflow.contains("target: x86_64-pc-windows-msvc"));
    assert!(workflow.contains("demo-$version-x86_64-windows.zip"));
    assert!(workflow.contains("name: Install Chocolatey"));
    assert!(workflow.contains("name: Publish Chocolatey package"));
    assert!(workflow.contains("CHOCOLATEY_API_KEY: ${{ secrets.chocolatey_api_key }}"));
    assert!(workflow.contains(
        "CHOCOLATEY_API_KEY is required because Chocolatey package publishing is configured."
    ));
    assert!(workflow.contains("simit dist chocolatey bump `"));
    assert!(workflow.contains("--archive \"x64=release/demo-$version-x86_64-windows.zip\" `"));
    assert!(workflow.contains("--push-source \"$env:CHOCO_PUSH_SOURCE\" `"));
}

#[test]
fn release_artifact_workflow_runs_configured_smoke_before_publish() {
    let temp = init_package(true);
    write_release_smoke_config(temp.path());

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-artifacts",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(
        &temp
            .path()
            .join(".forgejo/workflows/release-artifacts.yaml"),
    );
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("name: Run release smoke checks"));
    assert!(!workflow.contains("FORCE_PUBLISH: ${{ inputs.force_publish }}"));
    assert!(!workflow.contains("inputs.force_publish"));
    assert!(workflow.contains("nix run .#release-smoke -- \"$VERSION\" release"));
    assert!(
        workflow
            .find("name: Generate signed checksums and provenance")
            .unwrap()
            < workflow.find("name: Run release smoke checks").unwrap()
    );
}

#[test]
fn github_chocolatey_flag_errors_when_config_is_absent() {
    let temp = init_package(false);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--with-chocolatey"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("chocolatey.download_repo not set"));
}

#[test]
fn github_scoop_flag_resolves_when_config_is_present() {
    let temp = init_package(false);
    write_scoop_config(temp.path());

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--with-scoop"])
        .status()
        .unwrap();

    assert!(status.success());
    let workflow = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("name: Release Artifacts"));
    assert!(workflow.contains("build-linux:"));
    assert!(workflow.contains("build-windows:"));
    assert!(workflow.contains("target: x86_64-pc-windows-msvc"));
    assert!(workflow.contains("target: aarch64-pc-windows-msvc"));
    assert!(workflow.contains("windows-${{ matrix.arch }}"));
    assert!(workflow.contains("name: Publish Scoop bucket"));
    assert!(workflow.contains("SCOOP_BUCKET_TOKEN: ${{ secrets.SCOOP_BUCKET_TOKEN }}"));
    assert!(
        workflow.contains(
            "SCOOP_BUCKET_TOKEN is required because Scoop bucket publishing is configured."
        )
    );
    assert!(workflow.contains(
        "git -c credential.helper=\"$credentialHelper\" clone \"$env:SCOOP_BUCKET_URL\" bucket"
    ));
    assert!(workflow.contains("simit dist scoop bump `"));
    assert!(workflow.contains("--archive \"x64=release/demo-$version-x86_64-windows.zip\" `"));
    assert!(workflow.contains("--archive \"arm64=release/demo-$version-aarch64-windows.zip\" `"));
}

#[test]
fn github_scoop_respects_no_arch_and_custom_archive_pattern() {
    let temp = init_package(false);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[scoop]
bucket_url = "https://example.com/scoop-demo.git"
description = "demo binary"
homepage = "https://example.com/demo"
license = "MIT"
download_repo = "foo/demo"
archive_pattern = "demo-windows-{arch}-{version}.zip"

[scoop.architectures]
arm64 = false
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--with-scoop"])
        .status()
        .unwrap();

    assert!(status.success());
    let workflow = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("demo-windows-x86_64-$version.zip"));
    assert!(workflow.contains("--archive \"x64=release/demo-windows-x86_64-$version.zip\" `"));
    assert!(!workflow.contains("aarch64-pc-windows-msvc"));
    assert!(!workflow.contains("--archive \"arm64="));
}

#[test]
fn github_scoop_flag_errors_when_config_is_absent() {
    let temp = init_package(false);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--with-scoop"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("scoop.bucket_url not set"));
}

#[test]
fn github_chocolatey_and_scoop_share_windows_artifacts() {
    let temp = init_package(false);
    write_windows_packager_config(temp.path());

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-chocolatey",
            "--with-scoop",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    let workflow = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_yaml_parses(&workflow);
    assert_eq!(workflow.matches("build-windows:").count(), 1);
    assert_eq!(
        workflow
            .matches("run: cargo build --release --locked\n")
            .count(),
        1
    );
    assert_eq!(
        workflow
            .matches("run: cargo build --release --locked --target ${{ matrix.target }}")
            .count(),
        1
    );
    assert!(workflow.contains("name: Publish Chocolatey package"));
    assert!(workflow.contains("name: Publish Scoop bucket"));
    assert!(workflow.contains("needs: [validate, build-windows]"));
    assert!(workflow.contains(&format!("run: {}\n", simit::render::simit_cargo_install())));
    assert!(!workflow.contains("cargo install --locked simit\n"));
    let parsed: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    for name in ["build-linux", "build-windows"] {
        assert_eq!(parsed["jobs"][name]["needs"].as_str(), Some("validate"));
    }
    for name in [
        "validate",
        "build-linux",
        "build-windows",
        "publish-windows-packages",
    ] {
        let job = &parsed["jobs"][name];
        assert!(job["if"].is_null(), "validation failure must skip {name}");
        let checkout = job["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["name"].as_str() == Some("Checkout"))
            .unwrap();
        assert_eq!(checkout["with"]["ref"].as_str(), Some("${{ github.sha }}"));
        assert_eq!(
            checkout["with"]["persist-credentials"].as_bool(),
            Some(false)
        );
    }
    assert!(
        serde_yaml::to_string(&parsed["jobs"]["validate"])
            .unwrap()
            .contains("Signed tag commit does not match the checkout")
    );
    assert!(workflow.contains("pattern: windows-*"));

    let first = workflow;
    let second_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-chocolatey",
            "--with-scoop",
        ])
        .status()
        .unwrap();
    assert!(second_status.success());
    let second = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_eq!(first, second);
}

#[test]
fn github_chocolatey_scoop_and_homebrew_keep_single_linux_job() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[homebrew]
tap_url = "https://example.com/homebrew-demo.git"
description = "demo binary"
homepage = "https://example.com/demo"
license = "MIT"
download_repo = "foo/demo"

[chocolatey]
authors = "Example Maintainers"
description = "demo binary"
project_url = "https://example.com/demo"
download_repo = "foo/demo"

[scoop]
bucket_url = "https://example.com/scoop-demo.git"
description = "demo binary"
homepage = "https://example.com/demo"
license = "MIT"
download_repo = "foo/demo"
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-homebrew",
            "--with-chocolatey",
            "--with-scoop",
            "--windows-runner",
            "windows-atlas",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    let workflow = read(
        &temp
            .path()
            .join(".forgejo/workflows/release-artifacts.yaml"),
    );
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("build-linux:"));
    assert!(workflow.contains("runs-on: atlas"));
    assert!(!workflow.contains("uses: https://github.com/cachix/install-nix-action@v31"));
    assert!(workflow.contains("name: Publish Homebrew tap"));
    assert!(workflow.contains("build-windows:"));
    assert!(workflow.contains("runs-on: windows-atlas"));
    assert_eq!(workflow.matches("name: Build package").count(), 1);
    assert_eq!(workflow.matches("name: Publish Homebrew tap").count(), 1);
    assert_eq!(
        workflow.matches("name: Publish Chocolatey package").count(),
        1
    );
    assert_eq!(workflow.matches("name: Publish Scoop bucket").count(), 1);
}

#[test]
fn forgejo_requires_user_runner_config_when_no_override_exists() {
    let temp = init_package(false);
    write_chocolatey_config(temp.path());
    let isolated_home = TempDir::new().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(temp.path())
        .env("HOME", isolated_home.path())
        .env("XDG_CONFIG_HOME", isolated_home.path().join(".xdg"))
        .env("XDG_DATA_HOME", common::data_home_path())
        .env("GIT_CONFIG_GLOBAL", isolated_home.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "true")
        .env("SIMIT_MAINTAINERS_GPG", common::maintainer_key_path())
        .args(["init", "ci", "--platform", "forgejo", "--with-chocolatey"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("simit user config"));
    assert!(stderr.contains("simit config init"));
}

#[test]
fn chocolatey_implies_artifacts_even_when_artifacts_false() {
    let temp = init_package(false);
    write_chocolatey_config(temp.path());

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-chocolatey",
            "--with-artifacts=false",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--with-chocolatey implies --with-artifacts; enabling it."));
    assert!(
        temp.path()
            .join(".github/workflows/release-artifacts.yaml")
            .exists()
    );
}

#[test]
fn check_ignores_existing_deny_policy_customization() {
    let temp = init_package(false);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--with-deny"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(temp.path().join("deny.toml"), "[licenses]\n").unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--with-deny",
            "--check",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
}

#[test]
fn forgejo_python_ci_applies_ci_cache_to_the_preinstalled_nix_environment() {
    let temp = init_python_project();
    let path = temp.path().join("simit.toml");
    let config = read(&path);
    fs::write(&path, format!("{config}\n[ci.cache]\nsubstituters = [\"https://ci-cache.example\"]\ntrusted_public_keys = [\"ci-cache.example:YWJj\"]\n\n[release.artifacts]\nsubstituters = [\"https://release-cache.example\"]\ntrusted_public_keys = [\"release-cache.example:YWJj\"]\n")).unwrap();
    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let workflow = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    let value: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let step = value["jobs"]["test"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "Configure preinstalled Nix caches")
        .expect("CI cache configuration step");
    assert_eq!(
        step["env"]["SIMIT_NIX_CACHE_CONFIG"].as_str().unwrap(),
        "substituters = https://ci-cache.example\ntrusted-public-keys = ci-cache.example:YWJj\n"
    );
    let env_file = temp.path().join("github-env");
    // The combined probe exceeds pipe capacity; an early-exit grep can make
    // printf receive SIGPIPE and hide the match under bash's pipefail.
    let existing = format!(
        "experimental-features = nix-command flakes\nSIMIT_NIX_CACHE\nsystem-features = kvm\n# \"quoted\" \\literal\n{}",
        "# x\n".repeat(24 * 1024)
    );
    let cache_config = format!(
        "{}{}",
        step["env"]["SIMIT_NIX_CACHE_CONFIG"].as_str().unwrap(),
        "# cache\n".repeat(12 * 1024)
    );
    let output = Command::new("bash")
        .args(["-e", "-o", "pipefail", "-c", step["run"].as_str().unwrap()])
        .env("GITHUB_ENV", &env_file)
        .env("NIX_CONFIG", &existing)
        .env("SIMIT_NIX_CACHE_CONFIG", &cache_config)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let exported = read(&env_file);
    assert!(exported.contains(&existing));
    assert!(exported.contains(&cache_config));
    assert!(exported.contains("substituters = https://ci-cache.example"));
    assert!(exported.contains("trusted-public-keys = ci-cache.example:YWJj"));
    assert!(!exported.contains("https://release-cache.example"));
    let lines: Vec<_> = exported.lines().collect();
    let delimiter = lines[0].strip_prefix("NIX_CONFIG<<").unwrap();
    assert_eq!(lines.last().unwrap(), &delimiter);
    assert!(!lines[1..lines.len() - 1].contains(&delimiter));
    assert!(!workflow.contains("cachix/install-nix-action"));
    assert_eq!(
        simit::config::ProjectConfig::load(temp.path())
            .unwrap()
            .release
            .artifacts
            .substituters,
        ["https://release-cache.example"]
    );
    assert!(
        simit_with_user_config(temp.path())
            .current_dir(temp.path())
            .args(["init", "ci", "--platform", "forgejo", "--check", "--diff"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn forgejo_python_uv_ci_uses_nix_checks() {
    let temp = init_python_project();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("runs-on: atlas"));
    assert!(workflow.contains(&simit::render::flake_wiring_check()));
    assert!(!workflow.contains("nix run git+"));
    assert!(!workflow.contains("caniko/simit.git -- "));
    assert!(workflow.contains("nix flake check --no-build"));
    assert!(workflow.contains("nix build .#checks.x86_64-linux.offline-tests"));
    assert!(workflow.contains("nix build .#checks.x86_64-linux.typecheck"));
    assert!(!workflow.contains("cargo test"));
    assert!(!workflow.contains("CARGO_HOME"));
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/publish-crate.yaml")
            .exists()
    );
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/release-artifacts.yaml")
            .exists()
    );

    let check_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn python_ci_component_selection_is_granular() {
    let temp = init_python_project();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[flake]
scope = "full"
mode = "custom"
backend = "py-harbor"

[flake.expected_outputs]
checks = ["offline-tests", "typecheck"]

[ci]
runtime = "nix"
components = ["checks"]
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(workflow.contains(
        "group: ${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.head_ref || github.ref_name }}"
    ));
    assert!(workflow.contains("nix build .#checks.x86_64-linux.offline-tests"));
    assert!(!workflow.contains("Check generated flake wiring"));
    assert!(!workflow.contains("Check flake evaluation"));
    assert!(!workflow.contains("CARGO_HOME"));
    let check = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--check"])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn python_publish_uses_configured_pypi_token_secret() {
    let temp = init_python_project();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[flake]
scope = "full"
mode = "custom"
backend = "py-harbor"

[flake.expected_outputs]
checks = ["offline-tests", "typecheck"]

[ci]
runtime = "nix"
with_pypi_publish = true
pypi_token_secret = "PYPI_API_TOKEN"
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/publish-pypi.yaml"));
    assert!(workflow.contains("UV_PUBLISH_TOKEN: ${{ secrets.PYPI_API_TOKEN }}"));
    assert!(!workflow.contains("secrets.PYPI_TOKEN"));
    assert!(workflow.contains("Tag must be an exact semver version"));
    assert!(workflow.contains("builtins.fromTOML"));
    assert!(!read(&temp.path().join(".github/workflows/ci.yaml")).contains("Validate release tag"));

    let config = read(&temp.path().join("simit.toml")).replace(
        "pypi_token_secret = \"PYPI_API_TOKEN\"",
        "pypi_trusted_publishing = true",
    );
    fs::write(temp.path().join("simit.toml"), config).unwrap();
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--platform", "github"])
            .status()
            .unwrap()
            .success()
    );

    let workflow = read(&temp.path().join(".github/workflows/publish-pypi.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("environment: pypi"));
    assert!(workflow.contains("contents: read"));
    assert!(workflow.contains("id-token: write"));
    assert!(workflow.contains("uv publish --trusted-publishing always"));
    assert!(!workflow.contains("UV_PUBLISH_TOKEN"));
}

#[test]
#[cfg(unix)]
fn pypi_publishers_validate_the_configured_tag_prefix_before_version_lookup() {
    use simit::cli::{Platform, Runtime};
    use simit::release_identity::TagPrefix;
    use simit::render::ci::{CiOptions, maturin_publish_file, python_publish_file};
    use simit::user_config::ResolvedRunner;
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let nix = temp.path().join("nix");
    fs::write(
        &nix,
        "#!/bin/sh\nprintf '%s' \"${FIXTURE_VERSION:-1.2.3}\"\n",
    )
    .unwrap();
    fs::set_permissions(&nix, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        temp.path().display(),
        std::env::var("PATH").unwrap()
    );
    let runner = ResolvedRunner {
        name: None,
        labels: vec!["ubuntu-24.04".into()],
    };
    for (prefix, expected) in [(TagPrefix::V, "v1.2.3"), (TagPrefix::None, "1.2.3")] {
        let options = CiOptions {
            tag_prefix: prefix,
            ..Default::default()
        };
        for platform in [Platform::Github, Platform::Forgejo] {
            for file in [
                python_publish_file(platform, Runtime::Nix, &runner, &options).unwrap(),
                maturin_publish_file(platform, Runtime::Nix, &runner, &options).unwrap(),
            ] {
                let workflow: serde_yaml::Value = serde_yaml::from_str(&file.content).unwrap();
                assert_eq!(
                    workflow["on"]["push"]["tags"][0].as_str(),
                    Some(if prefix == TagPrefix::V {
                        "v[0-9]*"
                    } else {
                        "[0-9]*"
                    })
                );
                let script = workflow["jobs"]["publish"]["steps"]
                    .as_sequence()
                    .unwrap()
                    .iter()
                    .find(|step| step["name"] == "Validate release tag")
                    .unwrap()["run"]
                    .as_str()
                    .unwrap();
                for (tag, success) in [
                    (expected, true),
                    (
                        if prefix == TagPrefix::V {
                            "1.2.3"
                        } else {
                            "v1.2.3"
                        },
                        false,
                    ),
                    ("v1.2", false),
                    ("v1.2.3;exit 0", false),
                ] {
                    let output = Command::new("bash")
                        .args(["-euo", "pipefail", "-c", script])
                        .env("PATH", &path)
                        .env("GITHUB_REF_NAME", tag)
                        .output()
                        .unwrap();
                    assert_eq!(output.status.success(), success, "{tag}: {output:?}");
                }
                let output = Command::new("bash")
                    .args(["-euo", "pipefail", "-c", script])
                    .env("PATH", &path)
                    .env("GITHUB_REF_NAME", expected)
                    .env("FIXTURE_VERSION", "1.2.4")
                    .output()
                    .unwrap();
                assert!(!output.status.success(), "{output:?}");
            }
        }
    }
}

#[test]
fn python_poetry_dynamic_version_generates_convergent_publish_workflow() {
    let temp = init_python_project();
    fs::write(
        temp.path().join("pyproject.toml"),
        "[project]\nname = 'hybrid'\ndynamic = ['version']\n[tool.poetry]\nname = 'hybrid'\nversion = '1.2.3'\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        "[flake]\nmode = 'custom'\nbackend = 'py-harbor'\n[ci]\nruntime = 'nix'\nwith_pypi_publish = true\npypi_token_secret = 'PYPI_API_TOKEN'\n",
    )
    .unwrap();
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--platform", "github"])
            .status()
            .unwrap()
            .success()
    );
    let workflow = read(&temp.path().join(".github/workflows/publish-pypi.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("metadata.tool.poetry.version"));
    assert!(workflow.contains("builtins.elem \"version\""));
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--platform", "github", "--check"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn generates_crow_yaml_and_jsonnet_without_a_crow_cli() {
    let project = init_package(false);
    let output = simit()
        .current_dir(project.path())
        .args([
            "init",
            "ci",
            "--ci-provider",
            "crow",
            "--runner",
            "crow-agent",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let yaml = read(&project.path().join(".crow/build.yaml"));
    assert!(yaml.contains("name: build"));
    assert!(yaml.contains("labels:"));
    assert!(yaml.contains("platform:"));

    let project = init_package(false);
    let output = simit()
        .current_dir(project.path())
        .args([
            "init",
            "ci",
            "--ci-provider",
            "crow",
            "--crow-format",
            "jsonnet",
            "--runner",
            "crow-agent",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        read(&project.path().join(".crow/build.jsonnet")).starts_with("// Generated by simit.")
    );
}

fn local_gate_project(local_gate: bool) -> TempDir {
    let temp = init_package(true);
    write_local_gate_config(temp.path(), local_gate);
    temp
}

fn write_local_gate_config(root: &Path, local_gate: bool) {
    fs::write(
        root.join("simit.toml"),
        format!(
            r#"[ci]
platform = "github"
provider = "actions"
runtime = "nix"
with_nextest = true
with_deny = true
local_gate = {local_gate}
nix_builds = [".#oci api"]

[ci.nix_build]
max_jobs = 1
cores = 2
"#
        ),
    )
    .unwrap();
}

fn run_init_ci(root: &Path, check: bool) -> std::process::Output {
    let mut command = simit();
    command.current_dir(root).args([
        "init",
        "ci",
        "--platform",
        "github",
        "--ci-provider",
        "actions",
    ]);
    if check {
        command.arg("--check");
    }
    command.output().unwrap()
}

fn assert_ok(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn local_gate_script_mirrors_ci_steps_without_runner_state() {
    let temp = local_gate_project(true);
    assert_ok(&run_init_ci(temp.path(), false));
    let script_path = temp.path().join("scripts/ci-local.sh");
    let script = read(&script_path);

    assert!(script.contains("# Generated by simit. Manual edits will be reported as ci=drift."));
    assert!(script.contains("cargo nextest run"));
    assert!(script.contains("cargo clippy"));
    assert!(script.contains("cargo deny check"));
    // The CI matrix step is runner-bound; the local build is explicit and quoted.
    assert!(script.contains(
        "nix build --no-link --no-update-lock-file --max-jobs 1 --cores 2 --print-build-logs '\\''.#oci api'\\''"
    ));
    for runner_only in [
        "$GITHUB_",
        "${{",
        "git config --global",
        "CARGO_HOME: \"/tmp",
    ] {
        assert!(
            !script.contains(runner_only),
            "{runner_only} leaked:\n{script}"
        );
    }
    assert_eq!(script.matches("step 'Format check'").count(), 1, "{script}");

    assert_ok(
        &Command::new("bash")
            .arg("-n")
            .arg(&script_path)
            .output()
            .unwrap(),
    );
    let listed = Command::new("bash")
        .arg(&script_path)
        .arg("--list")
        .output()
        .unwrap();
    assert_ok(&listed);
    let listed = String::from_utf8(listed.stdout).unwrap();
    assert!(listed.contains("Format check"), "{listed}");
    assert!(listed.contains("Nix build .#oci api"), "{listed}");

    assert_ok(&run_init_ci(temp.path(), true));
    fs::write(&script_path, format!("{script}echo hand edit\n")).unwrap();
    assert!(!run_init_ci(temp.path(), true).status.success());
}

#[test]
fn local_gate_is_opt_in_and_disabling_it_retires_the_script() {
    let temp = local_gate_project(false);
    assert_ok(&run_init_ci(temp.path(), false));
    let script_path = temp.path().join("scripts/ci-local.sh");
    assert!(!script_path.exists());

    write_local_gate_config(temp.path(), true);
    assert_ok(&run_init_ci(temp.path(), false));
    assert!(script_path.exists());
    write_local_gate_config(temp.path(), false);
    assert!(!run_init_ci(temp.path(), true).status.success());
    assert_ok(&run_init_ci(temp.path(), false));
    assert!(!script_path.exists());
}

#[test]
fn prebuild_only_accepts_crow_options_only_for_a_crow_prebuild() {
    let crow = r#"[prebuild]
provider = "crow"
nix_builds = [".#packages.x86_64-linux.server"]

[ci]
provider = "crow"
platform = "github"
runtime = "nix"

[ci.crow]
nix_image = "nixos/nix:2.34.8"
"#;
    let temp = init_flake_only();
    fs::write(temp.path().join("simit.toml"), crow).unwrap();
    let prebuild_only = |extra: &[&str]| {
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--prebuild-only"])
            .args(extra)
            .output()
            .unwrap()
    };
    let generated = prebuild_only(&["--ci-provider", "crow"]);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    assert!(temp.path().join(".crow/prebuild.yaml").exists());
    assert!(
        prebuild_only(&["--ci-provider", "crow", "--check"])
            .status
            .success()
    );
    assert!(
        prebuild_only(&["--crow-format", "yaml", "--check"])
            .status
            .success()
    );

    // A GitHub prebuild still rejects Crow-only options, and Actions stays accepted.
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nplatform = 'github'\nprovider = 'actions'\nruntime = 'nix'\nrunner = 'ubuntu-24.04'\nnix_builds = ['.#default']\n[prebuild]\npublish_attic = false\n[prebuild.system_runners]\nx86_64-linux = 'ubuntu-24.04'\n",
    )
    .unwrap();
    let rejected = prebuild_only(&["--ci-provider", "crow"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("require [prebuild] provider = \"crow\""),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(!prebuild_only(&["--crow-format", "yaml"]).status.success());
    let actions = prebuild_only(&["--ci-provider", "actions"]);
    assert!(
        actions.status.success(),
        "{}",
        String::from_utf8_lossy(&actions.stderr)
    );
}

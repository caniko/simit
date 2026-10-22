use std::fs;

use serde_yaml::Value;
use tempfile::TempDir;

mod common;

#[test]
fn trusted_engine_checkout_is_a_workspace_independent_of_the_candidate() {
    let candidate = publisher_project("coordinated");
    let engine = candidate.path().join(".simit-publisher-engine");
    fs::create_dir_all(engine.join("src")).unwrap();
    fs::write(engine.join("Cargo.toml"), include_str!("../Cargo.toml")).unwrap();
    fs::write(engine.join("Cargo.lock"), include_str!("../Cargo.lock")).unwrap();
    fs::write(engine.join("src/main.rs"), "fn main() {}\n").unwrap();

    let output = std::process::Command::new(env!("CARGO"))
        .current_dir(&engine)
        .args([
            "metadata",
            "--locked",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "trusted engine must not join the candidate workspace: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        std::path::Path::new(metadata["workspace_root"].as_str().unwrap()),
        engine
    );
    assert_eq!(metadata["workspace_members"].as_array().unwrap().len(), 1);
    assert_eq!(metadata["packages"][0]["name"], "simit");
}

fn publisher_project(mode: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    let workspace = mode != "single";
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            "[package]\nname='publisher-demo'\nversion='0.1.0'\nedition='2024'\nlicense='MIT'\n{}",
            if workspace {
                "[workspace]\nmembers=['dependent']\nresolver='3'\n"
            } else {
                ""
            }
        ),
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/lib.rs"), "").unwrap();
    if workspace {
        fs::create_dir_all(temp.path().join("dependent/src")).unwrap();
        fs::write(
            temp.path().join("dependent/Cargo.toml"),
            "[package]\nname='publisher-dependent'\nversion='0.1.0'\nedition='2024'\nlicense='MIT'\n[dependencies]\npublisher-demo={path='..',version='0.1.0'}\n",
        )
        .unwrap();
        fs::write(temp.path().join("dependent/src/lib.rs"), "").unwrap();
    }
    fs::write(
        temp.path().join("simit.toml"),
        format!(
            "[ci]\nplatform='github'\nprovider='actions'\nruntime='cargo'\nrunner='ubuntu-latest'\npublish_crates=true\nworkspace={workspace}\npublish_strategy='{}'\n",
            if mode == "coordinated" {
                "coordinated"
            } else {
                "members"
            }
        ),
    )
    .unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{mode}: {output:?}");
    let check = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(check.status.success(), "{mode}: {check:?}");
    temp
}

fn has_publication_credential(value: &Value) -> bool {
    match value {
        Value::String(value) => {
            value.contains("secrets.CARGO_REGISTRY_TOKEN")
                || value.contains("secrets.CRATES_IO_API_TOKEN")
        }
        Value::Sequence(values) => values.iter().any(has_publication_credential),
        Value::Mapping(values) => values.values().any(has_publication_credential),
        _ => false,
    }
}

fn assert_trusted_credential_boundary(mode: &str) {
    let project = publisher_project(mode);
    let mut credential_jobs = 0;
    let mut signed_tag_routes = 0;
    for entry in fs::read_dir(project.path().join(".github/workflows")).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        let workflow: Value = serde_yaml::from_str(&text).unwrap();
        if workflow["on"]["push"]["tags"].is_sequence() {
            signed_tag_routes += 1;
            assert!(workflow["permissions"]["id-token"].is_null());
            for job in workflow["jobs"].as_mapping().unwrap().values() {
                assert!(
                    job["permissions"]["id-token"].is_null(),
                    "tag builds must not mint OIDC credentials"
                );
            }
            // GitHub loads a tag-push workflow from that tag. A writer can
            // remove every signature check from that definition; credentials
            // therefore belong to the independently trusted default-branch
            // publisher, never this candidate-controlled execution route.
            assert!(
                !has_publication_credential(&workflow),
                "{mode}: tag-controlled {} still requests publication credentials",
                path.display()
            );
        }
        for job in workflow["jobs"].as_mapping().unwrap().values() {
            if !has_publication_credential(job) {
                continue;
            }
            credential_jobs += 1;
            assert!(workflow["on"]["push"].is_null(), "{path:?}");
            assert!(workflow["on"]["pull_request"].is_null(), "{path:?}");
            assert_eq!(
                workflow["on"]["workflow_run"]["types"],
                serde_yaml::to_value(["completed"]).unwrap(),
                "{mode}: {path:?} must execute its independently trusted default-branch definition"
            );
            let environment = job["environment"]
                .as_str()
                .or_else(|| job["environment"]["name"].as_str())
                .expect("publication credentials require a protected environment");
            assert!(!environment.is_empty(), "{path:?}");
            assert!(
                !environment.contains("${{"),
                "candidate input must not select the credential environment: {path:?}"
            );
        }
    }
    assert!(signed_tag_routes > 0, "retain signed original-run recovery");
    assert!(credential_jobs > 0, "retain a supported publishing route");
    // The actual environment/ref restrictions and credential enrollment are
    // operator-owned acceptance gates, not properties this YAML test can prove.
}

#[test]
fn single_crate_credentials_are_outside_the_tag_controlled_workflow() {
    assert_trusted_credential_boundary("single");
}

#[test]
fn member_publish_credentials_are_outside_the_tag_controlled_workflow() {
    assert_trusted_credential_boundary("members");
}

#[test]
fn coordinated_credentials_are_outside_the_tag_controlled_workflow() {
    assert_trusted_credential_boundary("coordinated");
}

#[test]
fn member_completion_triggers_only_its_paired_trusted_publisher() {
    let temp = publisher_project("members");
    let directory = temp.path().join(".github/workflows");
    let mut names = std::collections::BTreeSet::new();
    for member in ["publisher-demo", "publisher-dependent"] {
        let source: Value = serde_yaml::from_str(
            &fs::read_to_string(directory.join(format!("publish-crate-{member}.yaml"))).unwrap(),
        )
        .unwrap();
        let trusted: Value = serde_yaml::from_str(
            &fs::read_to_string(directory.join(format!("trusted-publish-crate-{member}.yaml")))
                .unwrap(),
        )
        .unwrap();
        let name = source["name"].as_str().unwrap();
        assert!(
            names.insert(name.to_owned()),
            "member build display names must be unique"
        );
        assert_eq!(
            trusted["on"]["workflow_run"]["workflows"][0].as_str(),
            Some(name)
        );
    }
}

#[test]
fn retained_archive_builds_use_an_explicit_target_directory_without_jq() {
    for mode in ["single", "members", "coordinated"] {
        let temp = publisher_project(mode);
        for entry in fs::read_dir(temp.path().join(".github/workflows")).unwrap() {
            let path = entry.unwrap().path();
            if !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("publish-")
            {
                continue;
            }
            let text = fs::read_to_string(path).unwrap();
            assert!(
                !text.contains(" | jq"),
                "source archive route cannot assume jq is installed"
            );
            assert!(text.contains("--target-dir \"$RUNNER_TEMP/simit-package-target\""));
            assert!(text.contains("$RUNNER_TEMP/simit-package-target/package/"));
        }
    }
}

#[test]
fn publisher_dry_run_is_rejected_before_provider_or_registry_access() {
    let result = common::simit()
        .args([
            "release",
            "publisher",
            "--publisher-stage",
            "publish",
            "--package",
            "publisher-demo",
            "--dry-run",
        ])
        .env_remove("GITHUB_REPOSITORY")
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("--dry-run is not supported for the trusted publisher"),
        "{stderr}"
    );
}

#[test]
fn trusted_nix_gates_provision_runtime_before_execution_without_upload_credentials() {
    for mode in ["single", "members", "coordinated"] {
        let temp = publisher_project(mode);
        let mut policy: toml_edit::DocumentMut = fs::read_to_string(temp.path().join("simit.toml"))
            .unwrap()
            .parse()
            .unwrap();
        policy["ci"]["runtime"] = toml_edit::value("nix");
        fs::write(temp.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();
        fs::write(temp.path().join("simit.toml"), format!("{policy}\n[[ci.required_gates]]\nid='nix-integration'\nrun='nix run .#test-gel'\ntimeout_minutes=30\n")).unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for entry in fs::read_dir(temp.path().join(".github/workflows")).unwrap() {
            let path = entry.unwrap().path();
            if !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("trusted-publish-")
            {
                continue;
            }
            let text = fs::read_to_string(path).unwrap();
            let workflow: Value = serde_yaml::from_str(&text).unwrap();
            for (name, job) in workflow["jobs"].as_mapping().unwrap() {
                if !name.as_str().unwrap().starts_with("verify-") {
                    continue;
                }
                let steps = job["steps"].as_sequence().unwrap();
                let install = steps
                    .iter()
                    .position(|step| {
                        step["uses"]
                            .as_str()
                            .is_some_and(|uses| uses.starts_with("cachix/install-nix-action@"))
                    })
                    .expect("credential-free Nix gates need a provisioned Nix runtime");
                let gate = steps
                    .iter()
                    .position(|step| step["name"] == "Required gate nix-integration")
                    .unwrap();
                assert!(install < gate);
                let checkout = steps
                    .iter()
                    .position(|step| step["with"]["path"] == ".simit-release-source")
                    .unwrap();
                let verification = steps
                    .iter()
                    .position(|step| {
                        step["name"]
                            .as_str()
                            .is_some_and(|name| name.starts_with("Verify packaged contents"))
                    })
                    .unwrap();
                assert!(checkout < verification);
                assert_eq!(
                    steps[verification]["working-directory"],
                    ".simit-release-source"
                );
                assert!(
                    steps[verification]["run"]
                        .as_str()
                        .unwrap()
                        .starts_with("nix develop -c cargo publish --dry-run")
                );
                assert_eq!(
                    steps[checkout]["with"]["ref"],
                    "${{ github.event.workflow_run.head_sha }}"
                );
                assert!(job["environment"].is_null());
                assert!(
                    steps
                        .iter()
                        .all(|step| step["env"]["CRATES_IO_API_TOKEN"].is_null())
                );
            }
        }
    }
}

#[test]
fn coordinated_package_names_keep_distinct_jobs_and_dependency_edges() {
    let temp = publisher_project("coordinated");
    fs::write(temp.path().join("Cargo.toml"), "[package]\nname='foo-bar'\nversion='0.1.0'\nedition='2024'\nlicense='MIT'\n[workspace]\nmembers=['dependent']\nresolver='3'\n").unwrap();
    fs::write(temp.path().join("dependent/Cargo.toml"), "[package]\nname='foo_bar'\nversion='0.1.0'\nedition='2024'\nlicense='MIT'\n[dependencies]\nfoo-bar={path='..',version='0.1.0'}\n").unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow: Value = serde_yaml::from_str(
        &fs::read_to_string(
            temp.path()
                .join(".github/workflows/trusted-publish-workspace.yaml"),
        )
        .unwrap(),
    )
    .unwrap();
    let jobs = &workflow["jobs"];
    assert_eq!(jobs.as_mapping().unwrap().len(), 5);
    for name in ["foo-bar", "foo_bar"] {
        assert!(jobs[format!("verify-{name}")].is_mapping());
        assert_eq!(
            jobs[format!("publish-{name}")]["needs"][0],
            format!("verify-{name}")
        );
    }
    assert!(
        jobs["verify-foo_bar"]["needs"]
            .as_sequence()
            .unwrap()
            .contains(&Value::String("publish-foo-bar".into()))
    );
}

#[test]
fn wide_publisher_report_keeps_formatter_stable_dependency_edges() {
    let temp = publisher_project("coordinated");
    let first = "publisher-alpha-with-a-long-package-name";
    let second = "publisher-beta-with-a-long-package-name";
    fs::write(
        temp.path().join("Cargo.toml"),
        format!("[package]\nname='{first}'\nversion='0.1.0'\nedition='2024'\nlicense='MIT'\n[workspace]\nmembers=['dependent']\nresolver='3'\n"),
    )
    .unwrap();
    fs::write(
        temp.path().join("dependent/Cargo.toml"),
        format!("[package]\nname='{second}'\nversion='0.1.0'\nedition='2024'\nlicense='MIT'\n[dependencies]\n{first}={{path='..',version='0.1.0'}}\n"),
    )
    .unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = fs::read_to_string(
        temp.path()
            .join(".github/workflows/trusted-publish-workspace.yaml"),
    )
    .unwrap();
    assert!(text.contains("  publish-report:\n    needs:\n      - publish-"));
    let workflow: Value = serde_yaml::from_str(&text).unwrap();
    let needs = &workflow["jobs"]["publish-report"]["needs"];
    assert_eq!(needs[0], format!("publish-{first}"));
    assert_eq!(needs[1], format!("publish-{second}"));
    assert_eq!(needs.as_sequence().unwrap().len(), 2);
    assert_eq!(workflow["jobs"]["publish-report"]["if"], "always()");
    let check = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(check.status.success(), "{check:?}");
}

#[test]
fn tag_archive_generation_rejects_secret_contexts_in_all_publication_modes() {
    for mode in ["single", "members", "coordinated"] {
        for policy in [
            "[ci.extra_env]\nTOKEN = '${{ secrets.TOKEN }}'\n",
            "[ci.extra_env]\nTOKEN = '${{ sEcReTs[\"TOKEN\"] }}'\n",
            "[ci.extra_env]\nTOKEN = '${{ toJSON(secrets) }}'\n",
            "extra_setup = ['echo ${{ secrets.TOKEN }}']\n",
            "[[ci.required_gates]]\nid='integration'\nrun='cargo test'\nenv={TOKEN='${{ secrets.TOKEN }}'}\n",
        ] {
            let temp = publisher_project(mode);
            let files = fs::read_dir(temp.path().join(".github/workflows"))
                .unwrap()
                .map(|entry| {
                    let path = entry.unwrap().path();
                    let contents = fs::read(&path).unwrap();
                    (path, contents)
                })
                .collect::<Vec<_>>();
            let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
            fs::write(
                temp.path().join("simit.toml"),
                format!("{config}\n{policy}"),
            )
            .unwrap();
            let output = common::simit()
                .current_dir(temp.path())
                .args(["init", "ci"])
                .output()
                .unwrap();
            assert!(!output.status.success(), "{mode}: accepted {policy}");
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(
                stderr.contains("release source workflows must not reference GitHub secrets"),
                "{stderr}"
            );
            for (path, original) in files {
                assert_eq!(fs::read(path).unwrap(), original);
            }
        }
    }
}

#[test]
fn candidate_archive_and_verification_jobs_never_use_configured_trusted_runners() {
    for mode in ["single", "members", "coordinated"] {
        let temp = publisher_project(mode);
        let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
        fs::write(
            temp.path().join("simit.toml"),
            config.replace("runner='ubuntu-latest'", "runner='atlas-nix-trusted'"),
        )
        .unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--runner", "atlas-nix-trusted"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        for entry in fs::read_dir(temp.path().join(".github/workflows")).unwrap() {
            let path = entry.unwrap().path();
            let workflow: Value =
                serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            let tag_build = workflow["on"]["push"]["tags"].is_sequence();
            for (name, job) in workflow["jobs"].as_mapping().unwrap() {
                if tag_build || name.as_str().unwrap().starts_with("verify-") {
                    assert_eq!(
                        job["runs-on"], "ubuntu-24.04",
                        "{mode}: {path:?}/{name:?} exposes candidate code to a private runner"
                    );
                }
            }
        }
        let ci = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml"))
            .or_else(|_| {
                fs::read_to_string(temp.path().join(".github/workflows/ci-publisher-demo.yaml"))
            })
            .unwrap();
        assert!(
            ci.contains("atlas-nix-trusted"),
            "ordinary CI runner selection must be preserved"
        );
    }
}

#[test]
fn fresh_verification_reapplies_nonsecret_build_setup_without_influencing_upload() {
    for mode in ["single", "members", "coordinated"] {
        let temp = publisher_project(mode);
        let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
        fs::write(temp.path().join("simit.toml"), format!("{config}\nextra_setup=['test -f Cargo.toml; printf ready > build-tool-state']\nrequired_env=['BUILD_TOOL_MODE']\n[ci.extra_env]\nBUILD_TOOL_MODE='required-native-library'\n")).unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        for entry in fs::read_dir(temp.path().join(".github/workflows")).unwrap() {
            let path = entry.unwrap().path();
            if !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("trusted-publish-")
            {
                continue;
            }
            let workflow: Value = serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            for (name, job) in workflow["jobs"].as_mapping().unwrap() {
                let steps = job["steps"].as_sequence().unwrap();
                if name.as_str().unwrap().starts_with("verify-") {
                    let admission = steps
                        .iter()
                        .position(|step| {
                            step["name"]
                                .as_str()
                                .is_some_and(|name| name.starts_with("Admit signed source"))
                        })
                        .unwrap();
                    let checkout = steps
                        .iter()
                        .position(|step| step["with"]["path"] == ".simit-release-source")
                        .expect("setup must run against the signed source");
                    let setup = steps
                        .iter()
                        .position(|step| step["name"] == "Project setup")
                        .expect("fresh verification is missing required native build setup");
                    let verify = steps
                        .iter()
                        .position(|step| {
                            step["name"]
                                .as_str()
                                .is_some_and(|name| name.starts_with("Verify packaged contents"))
                        })
                        .unwrap();
                    assert!(admission < checkout && checkout < setup && setup < verify);
                    for index in [setup, verify] {
                        assert_eq!(
                            steps[index]["env"]["BUILD_TOOL_MODE"],
                            "required-native-library"
                        );
                    }
                    assert_eq!(steps[setup]["working-directory"], ".simit-release-source");
                    assert!(
                        steps
                            .iter()
                            .any(|step| step["name"] == "Validate required environment")
                    );
                    for step in &steps[..=admission] {
                        assert!(
                            step["env"]["BUILD_TOOL_MODE"].is_null(),
                            "build environment must not influence independent admission/bootstrap"
                        );
                    }
                } else if name.as_str().unwrap().starts_with("publish-")
                    && name.as_str() != Some("publish-report")
                {
                    assert!(job["env"]["BUILD_TOOL_MODE"].is_null());
                    assert!(!steps.iter().any(|step| step["name"] == "Project setup"));
                    assert!(
                        steps
                            .iter()
                            .all(|step| step["env"]["BUILD_TOOL_MODE"].is_null())
                    );
                }
            }
        }
    }
}

#[test]
fn coordinated_audit_propagates_unsafe_publisher_policy_without_modifying_workflows() {
    let temp = publisher_project("coordinated");
    let files = fs::read_dir(temp.path().join(".github/workflows"))
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        format!("{config}\n[ci.extra_env]\nTOKEN='${{{{ secrets.TOKEN }}}}'\n"),
    )
    .unwrap();
    let result = simit::registry::audit_ci(temp.path());
    assert!(
        result.is_err(),
        "unsafe coordinated publisher policy was swallowed as generic drift: {result:?}"
    );
    assert!(
        format!("{:#}", result.unwrap_err())
            .contains("release source workflows must not reference GitHub secrets")
    );
    for (path, bytes) in files {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn credential_free_inputs_reject_github_token_and_indirect_context_access() {
    for mode in ["single", "members", "coordinated"] {
        let temp = publisher_project(mode);
        let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
        let files = fs::read_dir(temp.path().join(".github/workflows"))
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect::<Vec<_>>();
        for expression in [
            "${{ github.token }}",
            "${{ GitHub . TOKEN }}",
            "${{ github['token'] }}",
            "${{ toJSON(github) }}",
            "${{ github.* }}",
            "${{ github[format('to{0}', 'ken')] }}",
            "${{ format('}}{0}', github.token) }}",
            "${{ format('${{}}{0}', github.token) }}",
        ] {
            for policy in [
                format!(
                    "[ci.extra_env]\nTOKEN={}\n",
                    toml_edit::Value::from(expression)
                ),
                format!(
                    "extra_setup=[{}]\n",
                    toml_edit::Value::from(format!("echo {expression}"))
                ),
                format!(
                    "[[ci.required_gates]]\nid='integration'\nrun='cargo test'\nenv={{TOKEN={}}}\n",
                    toml_edit::Value::from(expression)
                ),
                format!(
                    "[[ci.required_gates]]\nid='integration'\nrun={}\n",
                    toml_edit::Value::from(format!("echo {expression}"))
                ),
                format!(
                    "[[ci.required_gates]]\nid='integration'\nrun='cargo test'\n[ci.required_gates.artifacts]\nname='evidence'\npaths=[{}]\n",
                    toml_edit::Value::from(expression)
                ),
            ] {
                fs::write(
                    temp.path().join("simit.toml"),
                    format!("{config}\n{policy}"),
                )
                .unwrap();
                let output = common::simit()
                    .current_dir(temp.path())
                    .args(["init", "ci"])
                    .output()
                    .unwrap();
                assert!(
                    !output.status.success(),
                    "{mode}: accepted credential expression {policy}"
                );
                let stderr = String::from_utf8(output.stderr).unwrap();
                assert!(
                    stderr.contains("must not reference GitHub authentication contexts"),
                    "{stderr}"
                );
                for (path, bytes) in &files {
                    assert_eq!(fs::read(path).unwrap(), *bytes);
                }
            }
        }
    }
}

#[test]
fn credential_free_artifact_names_cannot_expose_the_source_token() {
    for mode in ["single", "members", "coordinated"] {
        let temp = publisher_project(mode);
        let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
        fs::write(temp.path().join("simit.toml"), format!("{config}\n[[ci.required_gates]]\nid='integration'\nrun='cargo test'\n[ci.required_gates.artifacts]\nname='${{{{ github.token }}}}'\npaths=['target/evidence/']\n")).unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("must not reference GitHub authentication contexts"),
            "{output:?}"
        );
    }
}

#[test]
fn isolated_artifact_uploads_reject_ambiguous_leading_expression_roots() {
    for mode in ["single", "members", "coordinated"] {
        let temp = publisher_project(mode);
        let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
        let files = fs::read_dir(temp.path().join(".github/workflows"))
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect::<Vec<_>>();
        for path in [
            "${{ vars.EVIDENCE_DIR }}/report.xml",
            "!${{ vars.EVIDENCE_DIR }}/private/**",
            "${{ vars.env }}/evidence",
            "${{ format('{0}/env', runner.temp) }}/evidence",
            "${{ format('prefix/{0}', runner.temp) }}/evidence",
            "${{ true && runner.temp || 'relative' }}/evidence",
        ] {
            fs::write(temp.path().join("simit.toml"), format!("{config}\n[[ci.required_gates]]\nid='integration'\nrun='cargo test'\n[ci.required_gates.artifacts]\nname='evidence'\npaths=[{}]\n", toml_edit::Value::from(path))).unwrap();
            let output = common::simit()
                .current_dir(temp.path())
                .args(["init", "ci"])
                .output()
                .unwrap();
            assert!(!output.status.success(), "{mode}: {path}: {output:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("artifact path has ambiguous rootedness"),
                "{output:?}"
            );
            for (path, bytes) in &files {
                assert_eq!(fs::read(path).unwrap(), *bytes);
            }
        }
        // Without isolated verification, the gate and upload share the workspace.
        let output = common::simit()
            .current_dir(temp.path())
            .args([
                "init",
                "ci",
                "--publish-crates=false",
                "--coordinated-publish=false",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
    }
}

#[test]
fn artifact_gate_job_budgets_cover_all_commands_and_uploads() {
    for mode in ["single", "members", "coordinated"] {
        let temp = publisher_project(mode);
        let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
        let mut gates = String::from(
            "\n[[ci.required_gates]]\nid='setup'\nrun='cargo test'\ntimeout_minutes=40\n",
        );
        for id in ["first", "second"] {
            gates.push_str(&format!("[[ci.required_gates]]\nid='{id}'\nrun='cargo test'\ntimeout_minutes=60\n[ci.required_gates.artifacts]\nname='{id}'\npaths=['target/evidence/']\n"));
        }
        fs::write(temp.path().join("simit.toml"), format!("{config}{gates}")).unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        let mut jobs = 0;
        for entry in fs::read_dir(temp.path().join(".github/workflows")).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            if !name.starts_with("publish-") && !name.starts_with("trusted-publish-") {
                continue;
            }
            let workflow: Value =
                serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            for job in workflow["jobs"].as_mapping().unwrap().values() {
                if job["steps"]
                    .as_sequence()
                    .unwrap()
                    .iter()
                    .any(|step| step["name"] == "Required gate first")
                {
                    // Single/member source jobs retain GitHub's 360-minute default.
                    let timeout = job["timeout-minutes"].as_u64().unwrap_or(360);
                    assert!((180..=360).contains(&timeout), "{mode}: {path:?}");
                    jobs += 1;
                }
            }
        }
        assert!(jobs >= 2, "{mode}");
        fs::write(
            temp.path().join("simit.toml"),
            format!(
                "{config}{}",
                gates.replace("timeout_minutes=60", "timeout_minutes=160")
            ),
        )
        .unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{mode}: accepted an over-budget shared job"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("360"),
            "{output:?}"
        );
        // Independent CI gates do not share a job budget or require a publisher.
        let output = common::simit()
            .current_dir(temp.path())
            .args([
                "init",
                "ci",
                "--publish-crates=false",
                "--coordinated-publish=false",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
    }
}

#[test]
fn gate_job_budgets_cover_gates_without_artifacts() {
    for mode in ["single", "members", "coordinated"] {
        let temp = publisher_project(mode);
        let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
        let gates = "\n[[ci.required_gates]]\nid='first'\nrun='cargo test'\ntimeout_minutes=100\n[[ci.required_gates]]\nid='second'\nrun='cargo test'\ntimeout_minutes=100\n";
        fs::write(temp.path().join("simit.toml"), format!("{config}{gates}")).unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        let mut jobs = 0;
        for entry in fs::read_dir(temp.path().join(".github/workflows")).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            if !name.starts_with("publish-") && !name.starts_with("trusted-publish-") {
                continue;
            }
            let workflow: Value =
                serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            for job in workflow["jobs"].as_mapping().unwrap().values() {
                if job["steps"]
                    .as_sequence()
                    .unwrap()
                    .iter()
                    .any(|step| step["name"] == "Required gate first")
                {
                    // Both gates run serially in this job: 10 setup + 100 + 100.
                    let timeout = job["timeout-minutes"].as_u64().unwrap_or(360);
                    assert!((210..=360).contains(&timeout), "{mode}: {path:?}");
                    jobs += 1;
                }
            }
        }
        assert!(jobs >= 2, "{mode}");
        fs::write(
            temp.path().join("simit.toml"),
            format!(
                "{config}{}",
                gates.replace("timeout_minutes=100", "timeout_minutes=176")
            ),
        )
        .unwrap();
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{mode}: accepted an over-budget shared job"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("360"),
            "{output:?}"
        );
        // Independent CI gates do not share a job budget or require a publisher.
        let output = common::simit()
            .current_dir(temp.path())
            .args([
                "init",
                "ci",
                "--publish-crates=false",
                "--coordinated-publish=false",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
    }
}

#[test]
fn tag_archive_generation_retains_nonsecret_build_environment() {
    let temp = publisher_project("coordinated");
    let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        format!(
            "{config}\n[ci.extra_env]\nCARGO_BUILD_JOBS='2'\nLABEL='${{{{ github.ref_name }}}}'\nBRACKET_LABEL=\"${{{{ github['ref_name'] }}}}\"\nEVENT='${{{{ toJSON(github.event) }}}}'\nLITERAL_LABEL=\"${{{{ format('github', github['ref_name']) }}}}\"\n"
        ),
    )
    .unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let yaml: Value = serde_yaml::from_str(
        &fs::read_to_string(temp.path().join(".github/workflows/publish-workspace.yaml")).unwrap(),
    )
    .unwrap();
    assert_eq!(yaml["jobs"]["build"]["env"]["CARGO_BUILD_JOBS"], "2");
    assert_eq!(
        yaml["jobs"]["build"]["env"]["LABEL"],
        "${{ github.ref_name }}"
    );
    assert_eq!(
        yaml["jobs"]["build"]["env"]["BRACKET_LABEL"],
        "${{ github['ref_name'] }}"
    );
    assert_eq!(
        yaml["jobs"]["build"]["env"]["EVENT"],
        "${{ toJSON(github.event) }}"
    );
    assert_eq!(
        yaml["jobs"]["build"]["env"]["LITERAL_LABEL"],
        "${{ format('github', github['ref_name']) }}"
    );
}

#[test]
fn required_environment_names_cannot_inject_candidate_shell_or_credentials() {
    for mode in ["single", "members", "coordinated"] {
        let temp = publisher_project(mode);
        let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
        let files = fs::read_dir(temp.path().join(".github/workflows"))
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect::<Vec<_>>();
        for name in [
            "SIMIT_UNSET:-$(printf '%s' '${{ github.token }}' > candidate-token)",
            "SIMIT_UNSET:-$(touch candidate-effect)",
            "${{ github.token }}",
            "BUILD-MODE",
            " BUILD_MODE",
            "1BUILD_MODE",
        ] {
            fs::write(
                temp.path().join("simit.toml"),
                format!(
                    "{config}\nrequired_env=[{}]\n",
                    toml_edit::Value::from(name)
                ),
            )
            .unwrap();
            let output = common::simit()
                .current_dir(temp.path())
                .args(["init", "ci"])
                .output()
                .unwrap();
            assert!(
                !output.status.success(),
                "{mode}: accepted executable required_env {name}"
            );
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(
                stderr.contains("required_env")
                    && stderr.contains("environment variable identifier"),
                "{stderr}"
            );
            for (path, bytes) in &files {
                assert_eq!(fs::read(path).unwrap(), *bytes);
            }
            assert!(!temp.path().join("candidate-token").exists());
            assert!(!temp.path().join("candidate-effect").exists());
        }
    }
}

#[test]
fn direct_publisher_rendering_validates_all_candidate_environment_names() {
    use simit::cli::{Platform, Runtime};
    use simit::config::RequiredGate;
    use simit::render::ci::{CiOptions, PublishWorkspaceInputs, publish_workspace_files};
    use simit::user_config::ResolvedRunner;
    let runner = ResolvedRunner {
        name: None,
        labels: vec!["ubuntu-latest".into()],
    };
    let render = |options| {
        publish_workspace_files(
            Platform::Github,
            PublishWorkspaceInputs {
                runtime: Runtime::Cargo,
                runner: &runner,
                options,
                plan: vec![("publisher-demo".into(), vec![])],
                versions: vec![("publisher-demo".into(), "0.1.0".into())],
            },
        )
    };
    for name in [
        "VALUE:-$(touch effect)",
        "${{ github.token }}",
        "[invalid]",
        "",
        "1VALUE",
    ] {
        for location in ["required_env", "extra_env", "gate.env"] {
            let mut options = CiOptions::default();
            match location {
                "required_env" => options.required_env.push(name.into()),
                "extra_env" => options.extra_env.push((name.into(), "safe".into())),
                _ => options.required_gates.push(RequiredGate {
                    id: "build".into(),
                    run: "cargo test".into(),
                    timeout_minutes: 1,
                    env: [(name.into(), "safe".into())].into(),
                    artifacts: None,
                }),
            }
            let result = render(options);
            assert!(
                result.is_err(),
                "direct renderer accepted {location} name {name}: {result:?}"
            );
            assert!(
                format!("{:#}", result.unwrap_err()).contains("environment variable identifier")
            );
        }
    }
    let valid = CiOptions {
        required_env: vec!["BUILD_MODE".into(), "_BUILD_MODE2".into()],
        extra_env: vec![
            ("BUILD_MODE".into(), "release".into()),
            ("_BUILD_MODE2".into(), "release".into()),
        ],
        ..CiOptions::default()
    };
    let files = render(valid).unwrap();
    assert_eq!(files.len(), 2);
    assert!(
        files
            .iter()
            .all(|file| file.content.contains("${BUILD_MODE:-}"))
    );
}

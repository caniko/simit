use std::{fs, path::Path, process::Command};

mod common;

fn project(config: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/lib.rs"), "pub fn hello() {}\n").unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT\"\n",
    )
    .unwrap();
    fs::write(temp.path().join("simit.toml"), config).unwrap();
    temp
}

fn init(root: &Path, flags: &[&str]) -> std::process::Output {
    common::simit()
        .current_dir(root)
        .args(["init", "ci", "--platform", "github", "--runtime", "cargo"])
        .args(flags)
        .output()
        .unwrap()
}

#[test]
fn new_publishing_defaults_to_release_plz_and_round_trips() {
    let temp = project("");
    let output = init(temp.path(), &["--publish-crates"]);
    assert!(output.status.success(), "{output:?}");
    let workflow =
        fs::read_to_string(temp.path().join(".github/workflows/release-plz.yaml")).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    assert_eq!(yaml["permissions"]["contents"], "read");
    assert_eq!(
        yaml["concurrency"]["group"],
        "release-plz-${{ github.repository }}-${{ github.run_id }}"
    );
    assert_eq!(yaml["concurrency"]["cancel-in-progress"], false);
    assert_eq!(yaml["jobs"]["select"]["timeout-minutes"], 360);
    assert_eq!(yaml["jobs"]["select"]["permissions"]["actions"], "read");
    let select_steps = yaml["jobs"]["select"]["steps"].as_sequence().unwrap();
    let prior = select_steps
        .iter()
        .position(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.ends_with(" prior-run"))
        })
        .unwrap();
    let selection = select_steps
        .iter()
        .position(|step| step["id"] == "select")
        .unwrap();
    assert!(
        prior < selection,
        "failed predecessors must block selection and every later mutation"
    );
    let recheck = yaml["jobs"]["publish"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "Recheck immutable release selection")
        .unwrap();
    assert_eq!(
        recheck["working-directory"],
        "${{ runner.temp }}/release-plz-event-source"
    );
    let prepare_if = yaml["jobs"]["prepare-pr"]["if"].as_str().unwrap();
    assert!(
        prepare_if.contains(
            "needs.select.outputs.release_sha == '' && needs.publish.result == 'skipped'"
        )
    );
    assert!(!prepare_if.contains("|| needs.publish.result == 'skipped')"));
    assert!(workflow.contains("update --config"));
    assert!(workflow.contains("apply-pr"));
    assert!(workflow.contains("run release "));
    assert!(workflow.contains("tag.gpgSign true"));
    assert!(workflow.contains("Qualify exact release revision"));
    let reconcile = yaml["jobs"]["publish"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .position(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.contains("--release-plz-check-registry"))
        })
        .unwrap();
    let signing = yaml["jobs"]["publish"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .position(|step| step["name"] == "Configure authorized tag signing")
        .unwrap();
    assert!(
        reconcile < signing,
        "tags-only releases must reconcile every registry version before signing"
    );
    assert!(
        !temp
            .path()
            .join(".github/workflows/publish-crate.yaml")
            .exists()
    );
    let config = fs::read_to_string(temp.path().join("release-plz.toml")).unwrap();
    let config: toml_edit::DocumentMut = config.parse().unwrap();
    assert_eq!(config["workspace"]["release_always"].as_bool(), Some(false));
    assert_eq!(
        config["workspace"]["git_tag_name"].as_str(),
        Some("{{ version }}")
    );
    let persisted = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    assert!(persisted.contains("publish_strategy = \"release-plz\""));
    let check = init(temp.path(), &["--check", "--diff"]);
    assert!(check.status.success(), "{check:?}");
    let audit = simit::registry::audit_ci(temp.path()).unwrap();
    assert_eq!(
        audit.status,
        simit::registry::FeatureStatus::Managed,
        "{audit:?}"
    );
    assert_eq!(
        simit::registry::detect_feature_status(temp.path())["ci"],
        simit::registry::FeatureStatus::Managed
    );
    fs::write(
        temp.path().join("release-plz.toml"),
        "[workspace]\nrelease_always = true\n",
    )
    .unwrap();
    assert!(!init(temp.path(), &["--check"]).status.success());
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Drift
    );
}

#[test]
fn existing_implicit_and_explicit_members_are_preserved() {
    for extra in ["", "publish_strategy = \"members\"\n"] {
        let temp = project(&format!("[ci]\npublish_crates = true\n{extra}"));
        let output = init(temp.path(), &[]);
        assert!(output.status.success(), "{output:?}");
        assert!(
            temp.path()
                .join(".github/workflows/publish-crate.yaml")
                .exists()
        );
        assert!(!temp.path().join("release-plz.toml").exists());
    }
}

#[test]
fn changing_ci_preserves_custom_release_plz_credentials() {
    for scope in [
        "ci",
        "package.metadata.simit.ci",
        "workspace.metadata.simit.ci",
    ] {
        for authentication in [
            "token_secret = \"CUSTOM_TOKEN\"\n",
            "app_id_variable = \"CUSTOM_APP_ID\"\napp_private_key_secret = \"CUSTOM_APP_KEY\"\n",
        ] {
            let declaration = format!(
                "[{scope}]\npublish_crates = true\npublish_strategy = \"release-plz\"\n[{scope}.release_plz]\n{authentication}registry_token_secret = \"CUSTOM_REGISTRY\"\nsigning_key_secret = \"CUSTOM_SIGNING_KEY\"\nsigning_passphrase_secret = \"CUSTOM_SIGNING_PASSWORD\"\n"
            );
            let temp = project(if scope == "ci" { &declaration } else { "" });
            if scope != "ci" {
                let manifest = temp.path().join("Cargo.toml");
                let text = fs::read_to_string(&manifest).unwrap();
                fs::write(&manifest, format!("{text}\n{declaration}")).unwrap();
            }
            let before = simit::config::ProjectConfig::load(temp.path())
                .unwrap()
                .ci
                .release_plz;
            let output = init(temp.path(), &["--with-docs"]);
            assert!(output.status.success(), "{output:?}");
            let config = simit::config::ProjectConfig::load(temp.path()).unwrap();
            assert!(config.ci.with_docs);
            assert_eq!(config.ci.release_plz, before, "{scope}: {authentication}");
            let output = init(temp.path(), &["--check", "--diff"]);
            assert!(output.status.success(), "{output:?}");
        }
    }
}

#[test]
fn migration_retires_owned_publishers_and_preserves_metadata_source() {
    let temp = project("[ci]\npublish_crates = true\nwith_docs = true\n");
    assert!(init(temp.path(), &[]).status.success());
    let output = init(temp.path(), &["--publish-strategy", "release-plz"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        !temp
            .path()
            .join(".github/workflows/publish-crate.yaml")
            .exists()
    );
    assert!(
        !temp
            .path()
            .join(".github/workflows/trusted-publish-crate.yaml")
            .exists()
    );
    let output = init(
        temp.path(),
        &[
            "--publish-strategy",
            "members",
            "--coordinated-publish=false",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert!(!temp.path().join("release-plz.toml").exists());
    assert!(!temp.path().join(".simit/release-plz.py").exists());
    assert!(
        !temp
            .path()
            .join(".github/workflows/release-plz.yaml")
            .exists()
    );
    fs::write(
        temp.path().join("release-plz.toml"),
        format!(
            "{}\n[workspace]\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        ),
    )
    .unwrap();
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Drift
    );
    assert_eq!(
        simit::registry::detect_feature_status(temp.path())["ci"],
        simit::registry::FeatureStatus::Drift
    );
    assert!(!init(temp.path(), &["--check"]).status.success());
    assert!(init(temp.path(), &[]).status.success());

    fs::remove_file(temp.path().join("simit.toml")).unwrap();
    let manifest = temp.path().join("Cargo.toml");
    let mut text = fs::read_to_string(&manifest).unwrap();
    text.push_str("\n[package.metadata.simit.ci]\nplatform = \"github\"\npublish_crates = false\nwith_docs = false\n");
    fs::write(&manifest, text).unwrap();
    let output = init(
        temp.path(),
        &["--publish-crates", "--publish-strategy", "release-plz"],
    );
    assert!(output.status.success(), "{output:?}");
    assert!(!temp.path().join("simit.toml").exists());
    assert!(
        fs::read_to_string(&manifest)
            .unwrap()
            .contains("publish_strategy = \"release-plz\"")
    );
    assert!(init(temp.path(), &["--check"]).status.success());
    let ci = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
    assert!(!ci.contains("Build docs"));
}

#[test]
fn rejects_conflicting_flags_unsupported_backends_and_unowned_publishers() {
    let temp = project("");
    let output = init(
        temp.path(),
        &[
            "--publish-crates",
            "--publish-strategy",
            "release-plz",
            "--coordinated-publish",
        ],
    );
    assert!(!output.status.success());
    let output = common::simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--publish-crates",
            "--publish-strategy",
            "release-plz",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("GitHub Actions"));
    fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
    fs::write(temp.path().join(".github/workflows/custom.yaml"), "name: Manual publisher\non: push\njobs:\n  publish:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo publish\n").unwrap();
    let output = init(
        temp.path(),
        &["--publish-crates", "--publish-strategy", "release-plz"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("competing publisher"));
    assert!(!temp.path().join("release-plz.toml").exists());
}

#[test]
fn migration_keeps_qualification_workflows_that_only_reference_release_tools() {
    let temp = project("");
    fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
    fs::write(temp.path().join(".github/workflows/qualify.yaml"), "name: Qualify\non: push\njobs:\n  qualify:\n    runs-on: ubuntu-latest\n    steps:\n      - run: simit init ci --publish-crates\n      - run: actionlint .github/workflows/release-plz.yaml\n").unwrap();
    let output = init(temp.path(), &["--publish-crates"]);
    assert!(output.status.success(), "{output:?}");
    assert!(temp.path().join(".github/workflows/qualify.yaml").exists());
    assert!(init(temp.path(), &["--check"]).status.success());
}

#[test]
fn migration_requires_reconciling_a_competing_project_template() {
    let temp = project("");
    fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
    fs::write(temp.path().join(".github/workflows/custom.yaml"), format!("# Simit workflow template: .simit/custom.yaml.in\n{}\nname: Publish\non: push\njobs:\n  publish:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo publish\n", simit::render::ci::GENERATED_WORKFLOW_MARKER)).unwrap();
    let output = init(temp.path(), &["--publish-crates"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("competing publisher"));
}

#[test]
fn migration_inspects_marked_publishers_unless_the_plan_replaces_or_retires_them() {
    for name in ["ship.yaml", "ship.yml", "ci.yaml", "publish-crate.yaml"] {
        let temp = project("");
        fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
        let workflow = temp.path().join(".github/workflows").join(name);
        let content = format!(
            "{}\njobs:\n  publish:\n    steps:\n      - run: cargo publish\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        );
        fs::write(&workflow, &content).unwrap();
        let output = init(
            temp.path(),
            &["--publish-crates", "--publish-strategy", "release-plz"],
        );
        if name.starts_with("ship") {
            assert!(!output.status.success(), "{name}: {output:?}");
            assert_eq!(fs::read_to_string(&workflow).unwrap(), content);
            assert!(!temp.path().join("release-plz.toml").exists());
            assert_eq!(
                fs::read_to_string(temp.path().join("simit.toml")).unwrap(),
                ""
            );
        } else {
            assert!(output.status.success(), "{name}: {output:?}");
            if name == "publish-crate.yaml" {
                assert!(!workflow.exists());
            } else {
                assert!(
                    !fs::read_to_string(&workflow)
                        .unwrap()
                        .contains("cargo publish")
                );
            }
            assert!(init(temp.path(), &["--check"]).status.success());
        }
    }
}

#[test]
fn migration_rejects_a_new_competing_template_before_writing_outputs() {
    let temp = project(
        "[ci.workflow_templates]\n\".github/workflows/publish.yaml\" = \".simit/publish.yaml.in\"\n",
    );
    fs::create_dir(temp.path().join(".simit")).unwrap();
    fs::write(temp.path().join(".simit/publish.yaml.in"), "name: Publish\non: push\njobs:\n  publish:\n    runs-on: ubuntu-24.04\n    steps:\n      - run: cargo publish\n").unwrap();
    let before = fs::read(temp.path().join("simit.toml")).unwrap();
    let output = init(temp.path(), &["--publish-crates"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("competing publisher"));
    assert!(!temp.path().join(".github").exists());
    assert!(!temp.path().join("release-plz.toml").exists());
    assert_eq!(fs::read(temp.path().join("simit.toml")).unwrap(), before);
}

#[test]
fn workspace_uses_package_tags_and_credential_free_exact_source_checks() {
    let temp = project(
        "[ci]\nworkspace = true\npublish_crates = true\npublish_strategy = \"release-plz\"\n[ci.release_plz]\napp_id_variable = \"RELEASE_APP_ID\"\napp_private_key_secret = \"RELEASE_APP_KEY\"\n",
    );
    let manifest = temp.path().join("Cargo.toml");
    let mut text = fs::read_to_string(&manifest).unwrap();
    text.push_str("\n[workspace]\nmembers = [\"helper\"]\n");
    fs::write(&manifest, text).unwrap();
    fs::create_dir_all(temp.path().join("helper/src")).unwrap();
    fs::write(temp.path().join("helper/Cargo.toml"), "[package]\nname = \"helper\"\nversion = \"0.2.0\"\nedition = \"2024\"\nlicense = \"MIT\"\n").unwrap();
    fs::write(temp.path().join("helper/src/lib.rs"), "").unwrap();
    let output = init(temp.path(), &[]);
    assert!(output.status.success(), "{output:?}");
    let config = fs::read_to_string(temp.path().join("release-plz.toml")).unwrap();
    assert!(config.contains("{{ package }}/v{{ version }}"));
    assert!(config.contains("name = \"helper\""));
    let text = fs::read_to_string(temp.path().join(".github/workflows/release-plz.yaml")).unwrap();
    let workflow: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    let qualify = serde_yaml::to_string(&workflow["jobs"]["qualify"]).unwrap();
    assert!(!qualify.contains("secrets."));
    assert!(qualify.contains("cargo test -p demo"));
    assert!(qualify.contains("cargo test -p helper"));
    assert!(qualify.contains("package --locked --allow-dirty"));
    for job in ["publish", "release-pr"] {
        let job = serde_yaml::to_string(&workflow["jobs"][job]).unwrap();
        assert!(!job.contains("nix develop"));
        assert!(!job.contains("cargo publish"));
    }
    assert!(text.contains("permission-pull-requests: write"));
    assert!(init(temp.path(), &["--check"]).status.success());
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Managed
    );
    let output = init(temp.path(), &["--with-artifacts"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("repository-wide binary"));
}

#[test]
fn real_git_merge_squash_and_cargo_revision_guard() {
    let output = Command::new("python3")
        .args([
            "tests/fixtures/release-plz-admission.py",
            "src/render/ci/release_plz.py",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn workspace_archives_are_independently_reconstructed_without_project_execution() {
    let output = Command::new("python3")
        .args([
            "tests/fixtures/release-plz-archives.py",
            env!("CARGO_BIN_EXE_simit"),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn release_pr_delivery_only_applies_admitted_data_and_preserves_manual_edits() {
    let output = Command::new("python3")
        .args([
            "tests/fixtures/release-plz-pr.py",
            "src/render/ci/release_plz.py",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn git_credentials_are_scoped_to_this_https_repository() {
    use std::{io::Write, process::Stdio};

    let temp = project("");
    assert!(init(temp.path(), &["--publish-crates"]).status.success());
    let text = fs::read_to_string(temp.path().join(".github/workflows/release-plz.yaml")).unwrap();
    let workflow: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    let prepare = workflow["jobs"]["publish"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find_map(|step| {
            step["run"]
                .as_str()
                .filter(|run| run.contains("SIMIT_CREDENTIALS"))
        })
        .unwrap();
    let runtime = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(temp.path())
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new("bash")
        .args(["-c", prepare])
        .current_dir(temp.path())
        .env("RUNNER_TEMP", runtime.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    for (protocol, host, path, allowed) in [
        ("https", "github.com", "owner/repo.git", true),
        ("https", "github.com", "owner/repo", true),
        ("https", "other.example", "owner/repo.git", false),
        ("https", "github.com", "other/repo.git", false),
        ("http", "github.com", "owner/repo.git", false),
    ] {
        let mut process = Command::new(runtime.path().join("release-plz-credentials"))
            .arg("get")
            .env("GITHUB_REPOSITORY", "owner/repo")
            .env("GIT_TOKEN", "fixture-token")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        write!(
            process.stdin.take().unwrap(),
            "protocol={protocol}\nhost={host}\npath={path}\n\n"
        )
        .unwrap();
        let output = process.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(!output.stdout.is_empty(), allowed);
    }
}

#[test]
fn explicit_metadata_members_overrides_new_default_in_both_cargo_scopes() {
    for scope in ["package", "workspace"] {
        let temp = project("");
        fs::remove_file(temp.path().join("simit.toml")).unwrap();
        let manifest = temp.path().join("Cargo.toml");
        let mut text = fs::read_to_string(&manifest).unwrap();
        text.push_str(&format!("\n[{scope}.metadata.simit.ci]\n# Keep the opted-out policy.\nplatform = \"github\"\npublish_crates = false\npublish_strategy = \"members\"\n"));
        fs::write(&manifest, text).unwrap();
        let output = init(temp.path(), &["--publish-crates"]);
        assert!(output.status.success(), "{output:?}");
        assert!(
            temp.path()
                .join(".github/workflows/publish-crate.yaml")
                .exists()
        );
        assert!(!temp.path().join("simit.toml").exists());
        assert!(!temp.path().join("release-plz.toml").exists());
        assert!(
            fs::read_to_string(&manifest)
                .unwrap()
                .contains("# Keep the opted-out policy.")
        );
        assert!(init(temp.path(), &["--check"]).status.success());
    }
}

#[test]
fn migration_detects_publish_after_cargo_global_options() {
    for command in [
        "cargo --locked publish",
        "cargo +stable --config 'build.jobs = 1' --offline publish --dry-run",
        "/usr/bin/cargo --color=never --frozen publish -p demo",
    ] {
        let temp = project("");
        fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
        let workflow = format!(
            "name: Existing publisher\non: push\njobs:\n  publish:\n    runs-on: ubuntu-latest\n    steps:\n      - run: {command}\n"
        );
        fs::write(temp.path().join(".github/workflows/custom.yaml"), workflow).unwrap();
        let output = init(temp.path(), &["--publish-crates"]);
        assert!(!output.status.success(), "accepted {command}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("competing publisher"));
        assert!(!temp.path().join("release-plz.toml").exists());
        assert_eq!(
            fs::read_to_string(temp.path().join("simit.toml")).unwrap(),
            ""
        );
    }
}

#[test]
fn migration_follows_local_publisher_wrappers_and_reusable_jobs() {
    for (workflow, script) in [
        (
            "jobs:\n  ship:\n    runs-on: ubuntu-latest\n    steps:\n      - run: ./scripts/ship.sh\n",
            "cargo --locked publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: scripts/ship.sh\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: scripts/ship\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: nix develop -c scripts/ship\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: true; scripts/ship\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    runs-on: ubuntu-latest\n    steps:\n      - run: nix develop -c bash scripts/ship.sh\n",
            "bash scripts/second.sh\n",
        ),
        (
            "jobs:\n  ship:\n    uses: ./.github/workflows/reusable.yaml\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: bash -e -o pipefail scripts/ship.sh\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: source scripts/ship.sh\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: . scripts/ship.sh\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: source -- scripts/ship.sh\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: . -- scripts/ship.sh\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: |\n          cargo --locked \\\n            publish\n",
            "echo unused\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: |\n          bash \\\n            scripts/ship.sh\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - run: |\n          # A comment does not continue the next command \\\n          cargo publish\n",
            "echo unused\n",
        ),
        (
            "defaults:\n  run:\n    working-directory: scripts\njobs:\n  ship:\n    steps:\n      - run: ./ship.sh\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    defaults:\n      run:\n        working-directory: scripts\n    steps:\n      - run: bash ship.sh\n",
            "cargo publish\n",
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - working-directory: scripts\n        run: python3 ship.sh\n",
            "import subprocess\nsubprocess.run(['cargo', 'publish'])\n",
        ),
    ] {
        let temp = project("");
        fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
        fs::create_dir(temp.path().join("scripts")).unwrap();
        fs::write(temp.path().join("scripts/ship.sh"), script).unwrap();
        fs::write(temp.path().join("scripts/ship"), script).unwrap();
        fs::write(temp.path().join("scripts/second.sh"), "cargo publish\n").unwrap();
        if workflow.contains("uses:") {
            fs::write(
                temp.path().join(".github/workflows/reusable.yaml"),
                "jobs:\n  publish:\n    steps:\n      - run: bash scripts/ship.sh\n",
            )
            .unwrap();
        }
        fs::write(temp.path().join(".github/workflows/custom.yaml"), workflow).unwrap();
        let output = init(temp.path(), &["--publish-crates"]);
        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("publisher"));
        assert!(!temp.path().join("release-plz.toml").exists());
        assert_eq!(
            fs::read_to_string(temp.path().join("simit.toml")).unwrap(),
            ""
        );
    }
}

#[test]
fn migration_follows_environment_and_wrapper_prefixes() {
    for command in [
        "RELEASE=1 ./scripts/ship",
        "env RELEASE=1 ./scripts/ship",
        "env RELEASE=1 ./scripts/ship.sh",
        "env -i -u SECRET RELEASE=1 scripts/ship",
        "env --unset SECRET -- RELEASE=1 scripts/ship",
        "/usr/bin/env --unset=SECRET RELEASE=1 scripts/ship",
        "sudo -u root env RELEASE=1 scripts/ship",
        "env MESSAGE='release request' ./scripts/ship",
        "./scripts/ship=now",
        "env A/B=1 ./scripts/ship",
        "command -p env RELEASE=1 ./scripts/ship",
        "exec -a ship ./scripts/ship",
        "env --chdir scripts RELEASE=1 ./ship",
        "env -S 'RELEASE=1 ./scripts/ship'",
    ] {
        let temp = project("");
        fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
        fs::create_dir(temp.path().join("scripts")).unwrap();
        for name in ["ship", "ship.sh", "ship=now"] {
            fs::write(temp.path().join("scripts").join(name), "cargo publish\n").unwrap();
        }
        fs::write(
            temp.path().join(".github/workflows/custom.yaml"),
            format!("jobs:\n  ship:\n    steps:\n      - run: {command}\n"),
        )
        .unwrap();
        let output = init(temp.path(), &["--publish-crates"]);
        assert!(!output.status.success(), "accepted {command}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("publisher"));
        assert!(!temp.path().join("release-plz.toml").exists());
        assert_eq!(
            fs::read_to_string(temp.path().join("simit.toml")).unwrap(),
            ""
        );
    }
    for command in [
        "env MESSAGE='release request' ./scripts/check",
        "env -i -u SECRET RELEASE=1 ./scripts/check",
        "sudo -u root env RELEASE=1 ./scripts/check",
        "env RELEASE=1 ./scripts/check ./scripts/ship",
        "env ./scripts/ship=now",
        "printf -- -f",
        "env RELEASE=1 ./scripts/check -- ./scripts/ship",
        "command -v ./scripts/ship",
        "RESULT=$(command -v cargo) true",
    ] {
        let temp = project("");
        fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
        fs::create_dir(temp.path().join("scripts")).unwrap();
        fs::write(temp.path().join("scripts/check"), "cargo test\n").unwrap();
        fs::write(temp.path().join("scripts/ship"), "cargo publish\n").unwrap();
        fs::write(temp.path().join("scripts/ship=now"), "cargo publish\n").unwrap();
        fs::write(
            temp.path().join(".github/workflows/custom.yaml"),
            format!("jobs:\n  check:\n    steps:\n      - run: {command}\n"),
        )
        .unwrap();
        let output = init(temp.path(), &["--publish-crates"]);
        assert!(output.status.success(), "{command}: {output:?}");
    }
}

#[test]
fn migration_inspects_indirect_shell_commands_without_executing_them() {
    for (command, rejects) in [
        ("PUBLISHER=cargo; $PUBLISHER publish", true),
        ("PUBLISHER=cargo; ${PUBLISHER} --locked publish", true),
        ("PUBLISHER='./scripts/ship'; \"$PUBLISHER\"", true),
        ("PUBLISHER=./scripts/ship; ${PUBLISHER}", true),
        ("EVALUATOR=eval; $EVALUATOR './scripts/ship'", true),
        ("eval './scripts/ship'", true),
        ("command eval './scripts/ship'", true),
        ("builtin eval './scripts/ship'", true),
        ("env PUBLISHER=cargo $PUBLISHER publish", true),
        ("PUBLISHER=./scripts/check; \"$PUBLISHER\"", true),
        ("echo 'eval ./scripts/ship'", false),
        ("RESULT=$(./scripts/ship) true", true),
        ("RESULT=$(scripts/ship) true", true),
        ("RESULT=`./scripts/ship` true", true),
        ("RESULT=${UNSET:-$(./scripts/ship)} true", true),
        ("RESULT=<(./scripts/ship) true", true),
        ("RESULT=>(./scripts/ship) true", true),
        ("env RESULT=$(./scripts/ship) true", true),
        ("env --unset=$(./scripts/ship) true", true),
        ("sudo -u $(./scripts/ship) true", true),
        ("RESULT=$(env RELEASE=1 ./scripts/ship) true", true),
        ("RESULT=$(echo $(./scripts/ship)) true", true),
        ("RESULT=$(./scripts/ship|head) true", true),
        ("RESULT=$(true&&./scripts/ship) true", true),
        ("RESULT=$(false||scripts/ship) true", true),
        ("RESULT=$( (./scripts/ship) ) true", true),
        ("true \"$(./scripts/ship)\"", true),
        ("RESULT=$(uname -m) true", false),
        ("RESULT='$(./scripts/ship)' true", false),
        ("RESULT=$(./scripts/check) true", false),
        ("git -c \"http.extraHeader=$HEADER\" fetch origin", false),
        ("ROOT=$(dirname \"$(command -v true)\") true", false),
        ("PATH=\"$TOOLS/bin:$PATH\" cargo test", false),
        ("PATH=\"$TOOLS/bin:$PATH\" cargo publish", true),
    ] {
        let temp = project("");
        fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
        fs::create_dir(temp.path().join("scripts")).unwrap();
        fs::write(temp.path().join("scripts/ship"), "cargo publish\n").unwrap();
        fs::write(temp.path().join("scripts/check"), "cargo test\n").unwrap();
        fs::write(
            temp.path().join(".github/workflows/custom.yaml"),
            format!("jobs:\n  check:\n    steps:\n      - run: {command}\n"),
        )
        .unwrap();
        let output = init(temp.path(), &["--publish-crates"]);
        assert_eq!(!output.status.success(), rejects, "{command}: {output:?}");
        if rejects {
            assert!(String::from_utf8_lossy(&output.stderr).contains("publisher"));
            assert!(!temp.path().join("release-plz.toml").exists());
            assert_eq!(
                fs::read_to_string(temp.path().join("simit.toml")).unwrap(),
                ""
            );
        }
    }
}

#[test]
fn migration_uses_the_invoked_interpreter_instead_of_a_script_extension() {
    let temp = project("");
    fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
    fs::create_dir(temp.path().join("scripts")).unwrap();
    fs::write(
        temp.path().join("scripts/custom.py"),
        "#!/usr/bin/env python3\nPUBLISHER=cargo\n$PUBLISHER publish\n",
    )
    .unwrap();
    fs::write(temp.path().join(".github/workflows/custom.yaml"), "jobs:\n  check:\n    steps:\n      - run: |\n          python3 scripts/custom.py || true\n          bash scripts/custom.py\n").unwrap();
    let output = init(temp.path(), &["--publish-crates"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("publisher"));
    assert!(!temp.path().join("release-plz.toml").exists());
}

#[test]
fn migration_inspects_custom_shells_and_their_default_scopes() {
    for (workflow, script, rejects) in [
        (
            "jobs:\n  ship:\n    steps:\n      - shell: ./scripts/ship.sh {0}\n        run: echo benign\n",
            "cargo publish\n",
            true,
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - shell: scripts/ship.sh {0}\n        run: echo benign\n",
            "cargo publish\n",
            true,
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - shell: scripts/ship {0}\n        run: echo benign\n",
            "cargo publish\n",
            true,
        ),
        (
            "defaults:\n  run:\n    shell: bash scripts/ship.sh {0}\njobs:\n  ship:\n    steps:\n      - run: echo benign\n",
            "cargo publish\n",
            true,
        ),
        (
            "jobs:\n  ship:\n    defaults:\n      run:\n        shell: ./scripts/ship.sh {0}\n    steps:\n      - run: echo benign\n",
            "cargo publish\n",
            true,
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - working-directory: scripts\n        shell: ./ship.sh {0}\n        run: echo benign\n",
            "cargo publish\n",
            true,
        ),
        (
            "jobs:\n  ship:\n    steps:\n      - shell: ./scripts/ship.sh {0}\n        run: echo benign\n",
            "cargo test\n",
            false,
        ),
        (
            "defaults:\n  run:\n    shell: ./scripts/ship.sh {0}\njobs:\n  ship:\n    steps:\n      - shell: bash\n        run: echo benign\n",
            "cargo publish\n",
            false,
        ),
    ] {
        let temp = project("");
        fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
        fs::create_dir(temp.path().join("scripts")).unwrap();
        fs::write(temp.path().join("scripts/ship.sh"), script).unwrap();
        fs::write(temp.path().join("scripts/ship"), script).unwrap();
        fs::write(temp.path().join(".github/workflows/custom.yaml"), workflow).unwrap();
        let output = init(temp.path(), &["--publish-crates"]);
        assert_eq!(!output.status.success(), rejects, "{workflow}: {output:?}");
        if rejects {
            assert!(!temp.path().join("release-plz.toml").exists());
            assert_eq!(
                fs::read_to_string(temp.path().join("simit.toml")).unwrap(),
                ""
            );
        }
    }
}

#[test]
fn migration_does_not_treat_computed_path_fragments_as_literal_scripts() {
    for command in [
        "bash \"$(dirname \"$0\")/check-command.sh\"\n",
        "bash \"${SCRIPT_DIR}/check-command.sh\"\n",
        "bash \"${{ github.workspace }}/check-command.sh\"\n",
    ] {
        let temp = project("");
        fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
        fs::create_dir(temp.path().join("scripts")).unwrap();
        fs::write(temp.path().join("scripts/check.sh"), command).unwrap();
        fs::write(
            temp.path().join(".github/workflows/custom.yaml"),
            "jobs:\n  check:\n    steps:\n      - run: bash scripts/check.sh\n",
        )
        .unwrap();
        let output = init(temp.path(), &["--publish-crates"]);
        assert!(output.status.success(), "{command}: {output:?}");
    }
}

#[test]
fn migration_distinguishes_source_scripts_from_ignored_build_outputs() {
    let temp = project("");
    assert!(
        Command::new("git")
            .current_dir(temp.path())
            .args(["init", "-b", "trunk"])
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
    fs::create_dir(temp.path().join("build-output")).unwrap();
    fs::write(temp.path().join(".gitignore"), "build-output/\n").unwrap();
    fs::write(
        temp.path().join("build-output/qualifier"),
        b"\x7fELF\xff\xfe",
    )
    .unwrap();
    fs::write(
        temp.path().join(".github/workflows/custom.yaml"),
        "jobs:\n  check:\n    steps:\n      - run: build-output/qualifier\n",
    )
    .unwrap();
    let output = init(temp.path(), &["--publish-crates"]);
    assert!(output.status.success(), "{output:?}");
    // A tracked source is still inspected even if its path matches .gitignore.
    fs::write(
        temp.path().join("build-output/qualifier"),
        "cargo publish\n",
    )
    .unwrap();
    assert!(
        Command::new("git")
            .current_dir(temp.path())
            .args(["add", "--force", "build-output/qualifier"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let output = init(temp.path(), &[]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("competing publisher"));
}

#[test]
fn migration_inspects_benign_scripts_and_stops_at_opaque_local_actions() {
    let temp = project("");
    fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
    fs::create_dir(temp.path().join("scripts")).unwrap();
    fs::write(
        temp.path().join("scripts/check.sh"),
        "#!/bin/sh\n# bash comments are not commands\nbash scripts/cycle.sh\ncp -a ./. \"$tmp\"\ncargo test\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("scripts/cycle.sh"),
        "bash scripts/check.sh\n",
    )
    .unwrap();
    let workflow = temp.path().join(".github/workflows/custom.yaml");
    fs::write(
        &workflow,
        "jobs:\n  check:\n    steps:\n      - run: bash scripts/check.sh\n",
    )
    .unwrap();
    let output = init(temp.path(), &["--publish-crates"]);
    assert!(output.status.success(), "{output:?}");
    for step in [
        "uses: ./.github/actions/ship",
        "run: bash scripts/missing.sh",
    ] {
        fs::write(
            &workflow,
            format!("jobs:\n  ship:\n    steps:\n      - {step}\n"),
        )
        .unwrap();
        let output = init(temp.path(), &[]);
        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("publisher"));
    }
}

#[test]
fn persistence_freezes_members_even_when_the_typed_config_is_unchanged() {
    let temp = project("[ci]\npublish_crates = true\n");
    let config = simit::config::ProjectConfig::load(temp.path()).unwrap();
    assert!(simit::config::ProjectConfig::write_ci(temp.path(), &config.ci).unwrap());
    assert!(
        fs::read_to_string(temp.path().join("simit.toml"))
            .unwrap()
            .contains("publish_strategy = \"members\"")
    );
    assert!(!simit::config::ProjectConfig::write_ci(temp.path(), &config.ci).unwrap());
}

#[test]
fn default_valued_cargo_policy_keeps_its_source_and_explicit_strategy() {
    for declaration in [
        "[package.metadata.simit.ci]\npublish_crates = false\npublish_strategy = \"members\"\n",
        "[package.metadata]\nsimit = {ci = {publish_crates = false, publish_strategy = \"members\"}}\n",
    ] {
        let temp = project("");
        fs::remove_file(temp.path().join("simit.toml")).unwrap();
        let manifest = temp.path().join("Cargo.toml");
        let text = fs::read_to_string(&manifest).unwrap();
        fs::write(&manifest, format!("{text}\n{declaration}")).unwrap();
        let output = init(temp.path(), &["--publish-crates"]);
        assert!(output.status.success(), "{output:?}");
        assert!(!temp.path().join("simit.toml").exists());
        assert!(!temp.path().join("release-plz.toml").exists());
        assert!(init(temp.path(), &["--check"]).status.success());
    }
}

#[test]
fn empty_sources_do_not_shadow_or_block_explicit_cargo_policy() {
    for empty in ["# Placeholder.\n", "[ci]\n"] {
        for declaration in [
            "[package.metadata.simit.ci]\nplatform = \"github\"\npublish_crates = false\npublish_strategy = \"members\"\n",
            "[workspace.metadata.simit.ci]\npublish_crates = false\npublish_strategy = \"members\"\n",
        ] {
            let temp = project(empty);
            let manifest = temp.path().join("Cargo.toml");
            let text = fs::read_to_string(&manifest).unwrap();
            fs::write(&manifest, format!("{text}\n{declaration}")).unwrap();
            let output = init(temp.path(), &["--publish-crates"]);
            assert!(output.status.success(), "{output:?}");
            assert!(
                temp.path()
                    .join(".github/workflows/publish-crate.yaml")
                    .exists()
            );
            assert!(!temp.path().join("release-plz.toml").exists());
            assert_eq!(
                fs::read_to_string(temp.path().join("simit.toml")).unwrap(),
                empty
            );
            assert!(
                fs::read_to_string(&manifest)
                    .unwrap()
                    .contains("publish_crates = true")
            );
            assert!(init(temp.path(), &["--check"]).status.success());
        }
    }
}

#[test]
fn single_crate_artifacts_keep_hosted_release_ownership_and_tag_prefix() {
    let temp = project("[release]\ntag_prefix = \"v\"\n");
    let output = init(temp.path(), &["--with-artifacts"]);
    assert!(output.status.success(), "{output:?}");
    let config: toml_edit::DocumentMut = fs::read_to_string(temp.path().join("release-plz.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        config["workspace"]["git_release_enable"].as_bool(),
        Some(false)
    );
    assert_eq!(
        config["workspace"]["git_tag_name"].as_str(),
        Some("v{{ version }}")
    );
    assert!(
        temp.path()
            .join(".github/workflows/release-artifacts.yaml")
            .exists()
    );
    assert!(init(temp.path(), &["--check"]).status.success());
}

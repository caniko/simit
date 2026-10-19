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
    assert!(workflow.contains("update --config"));
    assert!(workflow.contains("apply-pr"));
    assert!(workflow.contains("run release "));
    assert!(workflow.contains("tag.gpgSign true"));
    assert!(workflow.contains("Qualify exact release revision"));
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

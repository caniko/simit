use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

mod common;
#[cfg(unix)]
#[path = "common/monorepo_caller.rs"]
mod monorepo_caller;

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    write(
        temp.path(),
        "simit.toml",
        r#"[ci]
platform = "github"
provider = "actions"
runtime = "nix"

[monorepo]
schema_version = 1

[[monorepo.components]]
id = "core"
paths = ["nix/core"]

[[monorepo.components]]
id = "rust"
paths = ["nix/rust"]
cargo_packages = ["engine"]
depends_on = ["core"]

[[monorepo.components]]
id = "python"
paths = ["python"]
depends_on = ["core"]

[[monorepo.components]]
id = "worker"
cargo_packages = ["worker"]
depends_on = ["python"]
"#,
    );
    write(
        temp.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/engine\", \"crates/worker\"]\nresolver = \"3\"\n",
    );
    for (name, dependency) in [
        ("engine", ""),
        (
            "worker",
            "engine = { path = \"../engine\", version = \"0.1.0\" }\n",
        ),
    ] {
        write(
            temp.path(),
            &format!("crates/{name}/Cargo.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\n{dependency}"
            ),
        );
        write(
            temp.path(),
            &format!("crates/{name}/src/lib.rs"),
            "pub fn ready() {}\n",
        );
    }
    for path in [
        "nix/core/default.nix",
        "nix/rust/default.nix",
        "python/server.py",
    ] {
        write(temp.path(), path, "\n");
    }
    temp
}

fn plan(root: &Path, changed: &[&str]) -> std::process::Output {
    let mut command = common::simit();
    command
        .current_dir(root)
        .args(["monorepo", "plan", "--json"]);
    for path in changed {
        command.args(["--changed-path", path]);
    }
    command.output().unwrap()
}

fn enroll_release_checks(root: &Path) {
    let mut config: toml_edit::DocumentMut = fs::read_to_string(root.join("simit.toml"))
        .unwrap()
        .parse()
        .unwrap();
    for component in config["monorepo"]["components"]
        .as_array_of_tables_mut()
        .unwrap()
        .iter_mut()
    {
        let mut gate = toml_edit::InlineTable::new();
        gate.insert("id", "test".into());
        gate.insert("run", "true".into());
        let mut checks = toml_edit::Array::new();
        checks.push(gate);
        component["checks"] = toml_edit::value(checks);
    }
    write(root, "simit.toml", &config.to_string());
}

#[test]
fn cargo_member_paths_normalize_to_repo_contract() {
    let temp = fixture();
    let output = plan(temp.path(), &["crates/engine/src/lib.rs"]);
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["selected"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("rust"))
    );
    assert!(
        value["reasons"]["rust"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("changed:crates/engine/src/lib.rs"))
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_git_paths_select_full_graph_without_losing_known_paths() {
    use std::os::unix::ffi::OsStringExt;
    for tracked in [false, true] {
        let temp = fixture();
        let root = temp.path();
        fixture_git(root, &["init", "-q"]);
        fixture_git(root, &["config", "user.name", "Simit fixture"]);
        fixture_git(root, &["config", "user.email", "fixture@example.invalid"]);
        fixture_git(root, &["add", "."]);
        fixture_git(
            root,
            &[
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-qm",
                "base",
            ],
        );
        let mut base = fixture_git(root, &["rev-parse", "HEAD"]);
        let filename = std::ffi::OsString::from_vec(b"python/invalid-\xff.py".to_vec());
        if tracked {
            // Git objects can contain byte paths that APFS cannot materialize.
            // Put one in an ancestor tree, then compare its deletion against
            // our representable working tree without creating an invalid file.
            let blob = fixture_git(root, &["rev-parse", "HEAD:python/server.py"]);
            let mut cacheinfo = format!("100644,{},", blob.trim()).into_bytes();
            cacheinfo.extend_from_slice(filename.as_encoded_bytes());
            let status = Command::new("git")
                .current_dir(root)
                .args(["update-index", "--add", "--cacheinfo"])
                .arg(std::ffi::OsString::from_vec(cacheinfo))
                .status()
                .unwrap();
            assert!(status.success());
            let tree = fixture_git(root, &["write-tree"]);
            base = fixture_git(
                root,
                &[
                    "-c",
                    "commit.gpgsign=false",
                    "commit-tree",
                    tree.trim(),
                    "-p",
                    "HEAD",
                    "-m",
                    "byte path ancestor",
                ],
            );
            fixture_git(root, &["read-tree", "HEAD"]);
        } else if let Err(error) = fs::write(root.join(filename), "changed\n") {
            // No untracked byte path exists if the native filesystem rejects
            // it. The decoder test still exercises that input on every OS.
            assert!(
                cfg!(target_os = "macos") && error.raw_os_error() == Some(92),
                "{error}"
            );
            continue;
        }
        write(root, "python/server.py", "known change\n");
        let output = common::simit()
            .current_dir(root)
            .args(["monorepo", "plan", "--json", "--base", base.trim()])
            .output()
            .unwrap();
        assert!(output.status.success(), "{tracked}: {output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["full"], true);
        assert_eq!(
            value["selected"],
            serde_json::json!(["core", "python", "rust", "worker"])
        );
        assert!(
            value["changedPaths"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("python/server.py"))
        );
    }
}

#[test]
fn core_changes_select_transitive_consumers_and_prerequisites() {
    let temp = fixture();
    let output = plan(temp.path(), &["nix/core/default.nix"]);
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schemaVersion"], 1);
    assert_eq!(
        value["selected"],
        serde_json::json!(["core", "python", "rust", "worker"])
    );
    assert_eq!(
        value["reasons"]["core"],
        serde_json::json!(["changed:nix/core/default.nix"])
    );
    assert!(
        value["reasons"]["worker"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().starts_with("dependent:"))
    );
}

#[test]
fn monorepo_rejects_unsupported_extension_publication_before_reconciliation() {
    for (section, path) in [
        (
            "[vscode]\ncodeberg_repo='example/extension'\n",
            ".forgejo/workflows/publish-vscode-extension.yaml",
        ),
        (
            "[jetbrains]\nplugin_xml_id='example.plugin'\n",
            ".forgejo/workflows/publish-jetbrains-plugin.yaml",
        ),
    ] {
        let temp = fixture();
        let root = temp.path();
        enroll_release_checks(root);
        let config = format!(
            "{}\n{section}",
            fs::read_to_string(root.join("simit.toml")).unwrap()
        );
        write(root, "simit.toml", &config);
        let publisher = format!(
            "{}\nname: Retained extension publisher\non: [push]\njobs: {{}}\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        );
        write(root, path, &publisher);
        let output = common::simit()
            .current_dir(root)
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("extension publication"),
            "{output:?}"
        );
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), publisher);
        assert_eq!(fs::read_to_string(root.join("simit.toml")).unwrap(), config);
        let output = common::simit()
            .current_dir(root)
            .args(["init", "ci", "--primary-only"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let audit = simit::registry::audit_ci(root).unwrap();
        assert_eq!(
            audit.status,
            simit::registry::FeatureStatus::Managed,
            "{audit:?}"
        );
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), publisher);
        let ci: serde_yaml::Value = serde_yaml::from_str(
            &fs::read_to_string(root.join(".github/workflows/ci.yaml")).unwrap(),
        )
        .unwrap();
        let step = ci["jobs"]["plan"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["name"] == "Verify generated workflows")
            .unwrap();
        let (_, command) = step["run"]
            .as_str()
            .unwrap()
            .split_once("--command simit ")
            .unwrap();
        let output = common::simit()
            .current_dir(root)
            .args(command.split_whitespace())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), publisher);
        assert_eq!(fs::read_to_string(root.join("simit.toml")).unwrap(), config);
    }
}

#[test]
fn cargo_dependencies_include_worker_and_its_other_prerequisites() {
    let temp = fixture();
    let output = plan(temp.path(), &["crates/engine/src/lib.rs"]);
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["selected"],
        serde_json::json!(["core", "python", "rust", "worker"])
    );
    assert_eq!(
        value["reasons"]["python"],
        serde_json::json!(["prerequisite:worker"])
    );
}

#[test]
fn unknown_paths_and_shared_lock_changes_select_full_qualification() {
    let temp = fixture();
    for changed in ["new-source/file.rs", "Cargo.lock", "flake.lock"] {
        let output = plan(temp.path(), &[changed]);
        assert!(output.status.success(), "{output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["selected"].as_array().unwrap().len(), 4);
        assert!(value["full"].as_bool().unwrap());
    }
}

#[test]
fn member_invocation_resolves_the_monorepo_root() {
    let temp = fixture();
    let root = plan(temp.path(), &[]);
    let member = plan(&temp.path().join("crates/worker"), &[]);
    assert!(root.status.success(), "{root:?}");
    assert!(member.status.success(), "{member:?}");
    assert_eq!(root.stdout, member.stdout);
}

#[test]
fn cycles_duplicate_ownership_and_traversal_are_rejected() {
    let temp = fixture();
    let original = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    for (config, expected) in [
        (
            original.replace(
                "paths = [\"nix/core\"]",
                "paths = [\"nix/core\"]\ndepends_on = [\"worker\"]",
            ),
            "cycle",
        ),
        (
            original.replace("paths = [\"python\"]", "paths = [\"nix\"]"),
            "ownership",
        ),
        (
            original.replace("paths = [\"python\"]", "paths = [\"../outside\"]"),
            "relative",
        ),
    ] {
        write(temp.path(), "simit.toml", &config);
        let output = plan(temp.path(), &[]);
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{output:?}"
        );
    }
}

#[test]
fn component_gate_environments_resolve_provider_expressions_only_in_ci() {
    for (value, valid) in [
        ("${{ secrets.TEST_TOKEN }}", false),
        ("prefix-${{ github.ref }}", false),
        ("${{\n github.event.pull_request.title\n}}", false),
        ("literal $TOKEN {{ braces }}\nsecond line", true),
    ] {
        let temp = fixture();
        let root = temp.path();
        enroll_release_checks(root);
        let mut config: toml_edit::DocumentMut = fs::read_to_string(root.join("simit.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let mut env = toml_edit::InlineTable::new();
        env.insert("TEST_VALUE", value.into());
        let gate = config["monorepo"]["components"]
            .as_array_of_tables_mut()
            .unwrap()
            .get_mut(0)
            .unwrap()["checks"]
            .as_array_mut()
            .unwrap()
            .get_mut(0)
            .unwrap()
            .as_inline_table_mut()
            .unwrap();
        gate.insert("env", env.into());
        let config = config.to_string();
        write(root, "simit.toml", &config);
        for args in [
            vec!["monorepo", "plan", "--json"],
            vec!["init", "ci", "--primary-only"],
        ] {
            let output = common::simit()
                .current_dir(root)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{value:?}: {output:?}");
        }
        let output = common::simit()
            .current_dir(root)
            .args([
                "monorepo",
                "check",
                "--component",
                "core",
                "--check",
                "test",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.success(), valid, "{value:?}: {output:?}");
        if !valid {
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("provider expression requires"),
                "{output:?}"
            );
            assert!(!String::from_utf8_lossy(&output.stderr).contains(value));
        }
        assert_eq!(fs::read_to_string(root.join("simit.toml")).unwrap(), config);
    }
}

#[test]
fn git_diff_includes_both_sides_of_renames_and_deleted_paths() {
    let temp = fixture();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(temp.path())
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "Simit disposable fixture"]);
    git(&["config", "user.email", "fixture@example.invalid"]);
    git(&["add", "."]);
    // Identity and hook isolation are confined to this disposable repository.
    git(&[
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-qm",
        "fixture",
    ]);
    let base = Command::new("git")
        .current_dir(temp.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    fs::rename(
        temp.path().join("nix/rust/default.nix"),
        temp.path().join("python/moved.nix"),
    )
    .unwrap();
    fs::remove_file(temp.path().join("nix/core/default.nix")).unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args([
            "monorepo",
            "plan",
            "--base",
            String::from_utf8_lossy(&base.stdout).trim(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let paths = value["changedPaths"].as_array().unwrap();
    assert!(paths.contains(&serde_json::json!("nix/core/default.nix")));
    assert!(paths.contains(&serde_json::json!("nix/rust/default.nix")));
    assert!(paths.contains(&serde_json::json!("python/moved.nix")));
}

#[test]
fn cargo_owners_require_a_root_manifest_and_unique_package_names() {
    let temp = fixture();
    let original = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    write(
        temp.path(),
        "simit.toml",
        &original.replace(
            "cargo_packages = [\"worker\"]",
            "cargo_packages = [\"engine\"]",
        ),
    );
    let output = plan(temp.path(), &[]);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Cargo ownership"),
        "{output:?}"
    );
    write(temp.path(), "simit.toml", &original);
    fs::remove_file(temp.path().join("Cargo.toml")).unwrap();
    let output = plan(temp.path(), &[]);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Cargo.toml"),
        "{output:?}"
    );
}

#[test]
fn metadata_configuration_resolves_from_members() {
    let temp = fixture();
    let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    let cargo = fs::read_to_string(temp.path().join("Cargo.toml")).unwrap();
    write(
        temp.path(),
        "Cargo.toml",
        &format!(
            "{cargo}\n{}",
            config
                .replace("[ci]", "[workspace.metadata.simit.ci]")
                .replace("[monorepo]", "[workspace.metadata.simit.monorepo]")
                .replace(
                    "[[monorepo.components]]",
                    "[[workspace.metadata.simit.monorepo.components]]"
                )
        ),
    );
    fs::remove_file(temp.path().join("simit.toml")).unwrap();
    let output = plan(&temp.path().join("crates/engine"), &[]);
    assert!(output.status.success(), "{output:?}");
}

#[cfg(unix)]
#[test]
fn flake_configuration_resolves_from_non_cargo_members_and_stops_at_git_boundaries() {
    use std::os::unix::fs::PermissionsExt;

    let temp = fixture();
    let json = serde_json::json!({
        "ci": {"platform": "github", "provider": "actions", "runtime": "nix"},
        "monorepo": {"schema_version": 1, "components": [
            {"id": "rust", "cargo_packages": ["engine", "worker"], "checks": [{"id": "test", "run": "true"}]},
            {"id": "python", "paths": ["python"], "checks": [{"id": "test", "run": "true"}]}
        ]}
    });
    fs::remove_file(temp.path().join("simit.toml")).unwrap();
    write(
        temp.path(),
        "flake.nix",
        "{ outputs = { self }: { simitConfig = {}; }; }\n",
    );
    write(
        temp.path(),
        "bin/nix",
        &format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", json),
    );
    fs::set_permissions(
        temp.path().join("bin/nix"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let mut paths = vec![temp.path().join("bin")];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(paths).unwrap();
    let output = common::simit()
        .current_dir(temp.path().join("python"))
        .env("PATH", &path)
        .args([
            "monorepo",
            "plan",
            "--changed-path",
            "python/test.py",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["selected"], serde_json::json!(["python"]));
    let output = common::simit()
        .current_dir(temp.path().join("python"))
        .env("PATH", &path)
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(temp.path().join(".github/workflows/ci.yaml").is_file());
    assert!(!temp.path().join("python/.github").exists());
    let workflow = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
    assert_eq!(
        workflow.matches("name: Verify generated workflows").count(),
        2
    );
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    assert!(
        !serde_yaml::to_string(&yaml["jobs"]["plan"])
            .unwrap()
            .contains("init ci --check")
    );
    fs::create_dir(temp.path().join("python/.git")).unwrap();
    let output = common::simit()
        .current_dir(temp.path().join("python"))
        .env("PATH", &path)
        .args(["monorepo", "plan", "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no [monorepo]"));
}

#[test]
fn qualification_generation_is_complete_and_member_stable() {
    let temp = fixture();
    let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    write(
        temp.path(),
        "simit.toml",
        &config
            .replace(
                "id = \"core\"",
                "id = \"core\"\nchecks = [{ id = \"evaluate\", run = \"true\" }]",
            )
            .replace(
                "id = \"rust\"",
                "id = \"rust\"\nchecks = [{ id = \"test\", run = \"cargo test -p engine\" }]",
            )
            .replace(
                "id = \"python\"",
                "id = \"python\"\nchecks = [{ id = \"test\", run = \"python3 -m unittest\" }]",
            )
            .replace(
                "id = \"worker\"",
                "id = \"worker\"\nchecks = [{ id = \"test\", run = \"cargo test -p worker\" }]",
            ),
    );
    write(temp.path(), "flake.nix", "{}\n");
    write(
        temp.path(),
        ".github/workflows/handwritten.yaml",
        "name: Manual\n",
    );
    for directory in [
        temp.path().to_path_buf(),
        temp.path().join("python"),
        temp.path().join("crates/worker"),
    ] {
        let output = common::simit()
            .current_dir(&directory)
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let output = common::simit()
            .current_dir(&directory)
            .args(["init", "ci", "--check"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let text = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    assert_eq!(text.matches("name: Verify generated workflows").count(), 1);
    let plan_steps = yaml["jobs"]["plan"]["steps"].as_sequence().unwrap();
    let drift = plan_steps
        .iter()
        .position(|step| step["name"] == "Verify generated workflows")
        .unwrap();
    let selection = plan_steps
        .iter()
        .position(|step| step["id"] == "plan")
        .unwrap();
    assert!(drift < selection);
    assert!(
        yaml["jobs"]["qualified"]["needs"]
            .as_sequence()
            .unwrap()
            .contains(&serde_yaml::Value::from("plan"))
    );
    assert_eq!(
        yaml["concurrency"]["group"],
        "monorepo-${{ github.event.pull_request.head.repo.full_name || github.repository }}-${{ github.event.pull_request.head.ref || github.ref_name }}"
    );
    for id in [
        "plan",
        "component-core",
        "component-rust",
        "component-python",
        "component-worker",
        "qualified",
    ] {
        assert!(yaml["jobs"][id].is_mapping(), "missing {id}");
        if id != "qualified" {
            assert_public_input_transport(&yaml["jobs"][id]);
        }
    }
    assert!(text.contains("--base \"$BASE_REVISION\""));
    assert!(text.contains("fetch-depth: 0"));
    assert!(text.contains("persist-credentials: false"));
    assert!(text.contains("nix develop --print-build-logs .#ci --command simit monorepo plan"));
    assert!(!temp.path().join("python/.github").exists());
    assert_eq!(
        fs::read_to_string(temp.path().join(".github/workflows/handwritten.yaml")).unwrap(),
        "name: Manual\n"
    );
}

#[test]
fn monorepo_branch_filters_preserve_order_prs_and_primary_scope() {
    for (literal, branches) in [
        ("[]", vec!["**"]),
        ("[\"trunk\"]", vec!["trunk"]),
        (
            "[\"trunk\", \"release/**\", \"!release/archive/**\"]",
            vec!["trunk", "release/**", "!release/archive/**"],
        ),
    ] {
        let temp = TempDir::new().unwrap();
        write(
            temp.path(),
            "simit.toml",
            &format!(
                "[ci]\nplatform = \"github\"\nruntime = \"nix\"\nrunner = \"ubuntu-24.04\"\npush_branches = {literal}\n[monorepo]\nschema_version = 1\n[[monorepo.components]]\nid = \"python\"\npaths = [\"python\"]\nchecks = [{{id = \"test\", run = \"true\"}}]\n"
            ),
        );
        write(temp.path(), "python/example.py", "pass\n");
        write(temp.path(), "flake.nix", "{}\n");
        for primary in [false, true] {
            for check in [false, true] {
                let mut command = common::simit();
                command.current_dir(temp.path()).args(["init", "ci"]);
                if primary {
                    command.arg("--primary-only");
                }
                if check {
                    command.args(["--check", "--diff"]);
                }
                let output = command.output().unwrap();
                assert!(output.status.success(), "{output:?}");
            }
            let text = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
            let yaml: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
            assert_eq!(
                yaml["on"]["push"]["branches"],
                serde_yaml::to_value(&branches).unwrap()
            );
            assert!(yaml["on"]["pull_request"].is_mapping());
            assert!(yaml["on"]["workflow_dispatch"].is_mapping());
            assert!(yaml["on"]["push"]["tags"].is_null());
        }
    }
}

#[test]
fn monorepo_branch_filters_reject_explicit_dual_event_producers() {
    let temp = fixture();
    let path = temp.path().join("simit.toml");
    let config = fs::read_to_string(&path).unwrap().replacen(
        "runtime = \"nix\"",
        "runtime = \"nix\"\npush_branches = [\"trunk\"]\nseparate_events = true",
        1,
    );
    fs::write(path, config).unwrap();
    for primary in [false, true] {
        let mut command = common::simit();
        command.current_dir(temp.path()).args(["init", "ci"]);
        if primary {
            command.arg("--primary-only");
        }
        let output = command.output().unwrap();
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
fn component_release_plan_keeps_independent_versions_and_dependency_order() {
    let temp = fixture();
    let manifest = fs::read_to_string(temp.path().join("crates/worker/Cargo.toml")).unwrap();
    write(
        temp.path(),
        "crates/worker/Cargo.toml",
        &manifest.replacen("version = \"0.1.0\"", "version = \"0.7.0\"", 1),
    );
    let output = common::simit()
        .current_dir(temp.path().join("python"))
        .args(["release", "plan", "--component", "worker", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["component"], "worker");
    assert_eq!(value["entries"][0]["name"], "engine");
    assert_eq!(value["entries"][1]["name"], "worker");
    assert_eq!(value["entries"][0]["version"], "0.1.0");
    assert_eq!(value["entries"][1]["tag"], "worker/v0.7.0");
}

#[test]
fn qualification_concurrency_coalesces_repository_events_but_isolates_forks() {
    let temp = fixture();
    enroll_release_checks(temp.path());
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let workflow: serde_yaml::Value =
        serde_yaml::from_slice(&fs::read(temp.path().join(".github/workflows/ci.yaml")).unwrap())
            .unwrap();
    let group = workflow["concurrency"]["group"].as_str().unwrap();
    // Evaluate the generated Actions interpolation against distinct event contexts.
    let evaluate = |context: Value| {
        let mut remaining = group;
        let mut result = String::new();
        while let Some((literal, expression)) = remaining.split_once("${{") {
            result.push_str(literal);
            let (expression, rest) = expression.split_once("}}").unwrap();
            let value = expression
                .split("||")
                .find_map(|selector| {
                    selector
                        .trim()
                        .split('.')
                        .try_fold(&context, |value, key| value.get(key))
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                })
                .expect("concurrency selector must resolve for each event");
            result.push_str(value);
            remaining = rest;
        }
        result.push_str(remaining);
        result
    };
    let push =
        |repo: &str| serde_json::json!({"github": {"repository": repo, "ref_name": "repair"}});
    let pr = |repo: &str| serde_json::json!({"github": {"repository": "caniko/simit", "ref_name": "64/merge", "event": {"pull_request": {"head": {"ref": "repair", "repo": {"full_name": repo}}}}}});
    assert_eq!(workflow["concurrency"]["cancel-in-progress"], true);
    assert_eq!(evaluate(push("caniko/simit")), evaluate(pr("caniko/simit")));
    assert_eq!(evaluate(push("fork-a/simit")), evaluate(pr("fork-a/simit")));
    assert_ne!(evaluate(pr("fork-a/simit")), evaluate(pr("fork-b/simit")));
    assert_ne!(evaluate(pr("fork-a/simit")), evaluate(push("caniko/simit")));
}

#[cfg(unix)]
#[test]
fn generated_component_gate_propagates_pipeline_failure() {
    use std::os::unix::fs::PermissionsExt;
    let temp = fixture();
    let root = temp.path();
    enroll_release_checks(root);
    let config = fs::read_to_string(root.join("simit.toml"))
        .unwrap()
        .replace(
            "run = \"true\"",
            "run = \"false | tee pipeline-output; touch incorrectly-continued\"",
        );
    write(root, "simit.toml", &config);
    let output = common::simit()
        .current_dir(root)
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let workflow: serde_yaml::Value =
        serde_yaml::from_slice(&fs::read(root.join(".github/workflows/ci.yaml")).unwrap()).unwrap();
    let step = workflow["jobs"]["component-core"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "test")
        .unwrap();
    write(
        root,
        "bin/nix",
        "#!/bin/sh\nwhile [ \"$1\" != --command ]; do shift; done\nshift\nexec \"$@\"\n",
    );
    fs::set_permissions(root.join("bin/nix"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_simit"), root.join("bin/simit")).unwrap();
    let script = monorepo_caller::script(
        root,
        &workflow,
        "component-core",
        step["run"].as_str().unwrap(),
    );
    let output = Command::new("bash")
        .current_dir(root)
        .args(["-euo", "pipefail", "-c", &script])
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(root.join("pipeline-output").exists());
    assert!(!root.join("incorrectly-continued").exists());
}

#[test]
fn primary_regeneration_preserves_independent_publishers_and_signing_from_members() {
    let temp = fixture();
    let root = temp.path();
    enroll_release_checks(root);
    let config = fs::read_to_string(root.join("simit.toml"))
        .unwrap()
        .replace(
            "runtime = \"nix\"",
            "runtime = \"nix\"\npublish_crates = true",
        );
    write(root, "simit.toml", &config);
    let run = |cwd: &Path, args: &[&str]| {
        common::simit()
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(root, &["init", "ci"]);
    assert!(output.status.success(), "{output:?}");
    let full_ci = fs::read(root.join(".github/workflows/ci.yaml")).unwrap();
    for args in [
        vec!["init", "ci", "--check"],
        vec!["init", "ci", "--check", "--diff"],
        vec!["init", "ci"],
    ] {
        let output = run(root, &args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert_eq!(
            fs::read(root.join(".github/workflows/ci.yaml")).unwrap(),
            full_ci
        );
    }
    // Drift in independent outputs must not widen a primary-only operation.
    let publisher = ".github/workflows/trusted-publish-crate-worker.yaml";
    let original = fs::read_to_string(root.join(publisher)).unwrap();
    write(
        root,
        publisher,
        &format!("{original}\n# Independently owned publisher change\n"),
    );
    write(
        root,
        ".github/workflows/publish-npm-independent.yaml",
        "name: Independent npm publisher\non: [workflow_dispatch]\njobs: {}\n",
    );
    let config = fs::read(root.join("simit.toml")).unwrap();
    let key = fs::read(root.join("keys/maintainers.gpg")).unwrap();
    let publishers: Vec<_> = fs::read_dir(root.join(".github/workflows"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap() != "ci.yaml")
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    let output = run(root, &["init", "ci", "--check", "--diff"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains(publisher));
    // The first primary regeneration changes CI's self-check scope. Snapshot
    // that output before checking repeatability from the root and members.
    let output = run(root, &["init", "ci", "--primary-only"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read(root.join("simit.toml")).unwrap(), config);
    assert_eq!(fs::read(root.join("keys/maintainers.gpg")).unwrap(), key);
    for (path, bytes) in &publishers {
        assert_eq!(&fs::read(path).unwrap(), bytes);
    }
    let ci = fs::read(root.join(".github/workflows/ci.yaml")).unwrap();
    for cwd in [
        root.to_path_buf(),
        root.join("crates/worker"),
        root.join("python"),
    ] {
        for args in [
            vec!["init", "ci", "--primary-only"],
            vec!["init", "ci", "--primary-only", "--check"],
            vec!["init", "ci", "--primary-only", "--check", "--diff"],
            vec!["init", "ci", "--primary-only"],
        ] {
            let output = run(&cwd, &args);
            assert!(output.status.success(), "{args:?}: {output:?}");
            assert_eq!(
                fs::read(root.join(".github/workflows/ci.yaml")).unwrap(),
                ci
            );
            assert_eq!(fs::read(root.join("simit.toml")).unwrap(), config);
            assert_eq!(fs::read(root.join("keys/maintainers.gpg")).unwrap(), key);
            for (path, bytes) in &publishers {
                assert_eq!(&fs::read(path).unwrap(), bytes);
            }
        }
    }
    write(root, ".github/workflows/ci.yaml", "primary drift\n");
    let output = run(root, &["init", "ci", "--primary-only", "--check", "--diff"]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.join(".github/workflows/ci.yaml")).unwrap(),
        "primary drift\n"
    );
    for (path, bytes) in &publishers {
        assert_eq!(&fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn flake_generation_from_python_members_composes_root_formatter_policy() {
    let temp = fixture();
    let config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    write(
        temp.path(),
        "simit.toml",
        &config.replace(
            "schema_version = 1",
            "schema_version = 1\nformatter_modules = [\"nix/formatters/python.nix\"]",
        ),
    );
    write(
        temp.path(),
        "nix/formatters/python.nix",
        "{...}: { programs.ruff-format.enable = true; }\n",
    );
    let run = |path: &Path| {
        common::simit()
            .current_dir(path)
            .args(["init", "flake", "--scope", "full", "--print"])
            .output()
            .unwrap()
    };
    let root = run(temp.path());
    let member = run(&temp.path().join("python"));
    assert!(root.status.success(), "{root:?}");
    assert!(member.status.success(), "{member:?}");
    assert_eq!(root.stdout, member.stdout);
    assert!(
        String::from_utf8_lossy(&root.stdout).contains("(../. + \"/nix/formatters/python.nix\")")
    );
}

#[test]
fn component_release_dry_run_uses_package_tag_without_changing_other_versions() {
    let temp = fixture();
    let output = common::simit()
        .current_dir(temp.path())
        .args([
            "release",
            "patch",
            "--component",
            "worker",
            "--dry-run",
            "-m",
            "release worker",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("worker/v0.1.1"), "{text}");
    assert!(!text.contains("package engine:"), "{text}");
    assert!(
        fs::read_to_string(temp.path().join("crates/worker/Cargo.toml"))
            .unwrap()
            .contains("version = \"0.1.0\"")
    );
}

#[test]
fn independent_cargo_prereleases_fail_before_release_mutation() {
    for existing in ["0.1.0", "0.1.0-rc.1"] {
        let temp = fixture();
        let root = temp.path();
        enroll_release_checks(root);
        let config = fs::read_to_string(root.join("simit.toml"))
            .unwrap()
            .replace(
                "runtime = \"nix\"",
                "runtime = \"nix\"\npublish_crates = true",
            );
        write(root, "simit.toml", &config);
        let manifest = fs::read_to_string(root.join("crates/worker/Cargo.toml"))
            .unwrap()
            .replacen(
                "version = \"0.1.0\"",
                &format!("version = \"{existing}\""),
                1,
            );
        write(root, "crates/worker/Cargo.toml", &manifest);
        write(root, ".gitignore", "target/\n");
        assert!(
            Command::new("cargo")
                .current_dir(root)
                .arg("generate-lockfile")
                .status()
                .unwrap()
                .success()
        );
        fixture_git(root, &["init"]);
        fixture_git(root, &["add", "."]);
        fixture_git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "fixture",
            ],
        );
        let before = fixture_git(root, &["rev-parse", "HEAD"]);
        let mut args = vec![
            "release",
            "patch",
            "--component",
            "worker",
            "--no-sign",
            "--no-changelog",
            "-m",
            "release worker",
        ];
        fixture_git(root, &["config", "user.name", "Test"]);
        fixture_git(root, &["config", "user.email", "test@example.com"]);
        args.extend(["--pre", "rc"]);
        let output = common::simit()
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("independent Cargo publication does not support prereleases"),
            "{output:?}"
        );
        assert_eq!(fixture_git(root, &["rev-parse", "HEAD"]), before);
        assert_eq!(
            fs::read_to_string(root.join("crates/worker/Cargo.toml")).unwrap(),
            manifest
        );
        assert!(fixture_git(root, &["status", "--porcelain"]).is_empty());
        assert!(fixture_git(root, &["tag", "--list"]).is_empty());
    }
}

#[test]
fn component_bumps_reject_incompatible_local_dependency_requirements() {
    for inherited in [false, true] {
        let temp = fixture();
        let root = temp.path();
        enroll_release_checks(root);
        write(root, ".gitignore", "target/\n");
        if inherited {
            let workspace = fs::read_to_string(root.join("Cargo.toml")).unwrap();
            write(
                root,
                "Cargo.toml",
                &format!(
                    "{workspace}\n[workspace.dependencies]\nalias = {{package='engine',path='crates/engine',version='0.1'}}\n"
                ),
            );
            write(
                root,
                "crates/worker/Cargo.toml",
                "[package]\nname='worker'\nversion='0.1.0'\nedition='2024'\n[dependencies]\nalias.workspace = true\n",
            );
        }
        assert!(
            Command::new("cargo")
                .current_dir(root)
                .arg("generate-lockfile")
                .status()
                .unwrap()
                .success()
        );
        fixture_git(root, &["init"]);
        fixture_git(root, &["config", "user.name", "Test"]);
        fixture_git(root, &["config", "user.email", "test@example.com"]);
        fixture_git(root, &["add", "."]);
        fixture_git(root, &["commit", "-m", "fixture"]);
        let before = fixture_git(root, &["rev-parse", "HEAD"]);
        let manifest = fs::read(root.join("crates/engine/Cargo.toml")).unwrap();
        let lock = fs::read(root.join("Cargo.lock")).unwrap();
        let output = common::simit()
            .current_dir(root)
            .args([
                "release",
                "minor",
                "--component",
                "rust",
                "--no-sign",
                "--no-changelog",
                "-m",
                "bump engine",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("excludes component version"),
            "{output:?}"
        );
        assert_eq!(fixture_git(root, &["rev-parse", "HEAD"]), before);
        assert_eq!(
            fs::read(root.join("crates/engine/Cargo.toml")).unwrap(),
            manifest
        );
        assert_eq!(fs::read(root.join("Cargo.lock")).unwrap(), lock);
        assert!(fixture_git(root, &["status", "--porcelain"]).is_empty());
        assert!(fixture_git(root, &["tag", "--list"]).is_empty());
        // A patch bump remains within the admitted requirement.
        let output = common::simit()
            .current_dir(root)
            .args([
                "release",
                "patch",
                "--component",
                "rust",
                "--dry-run",
                "-m",
                "bump engine",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}

#[test]
fn independent_sync_up_uses_owned_package_tag_with_divergent_versions() {
    let temp = fixture();
    let root = temp.path();
    let worker = fs::read_to_string(root.join("crates/worker/Cargo.toml")).unwrap();
    write(
        root,
        "crates/worker/Cargo.toml",
        &worker.replacen("version = \"0.1.0\"", "version = \"0.2.0\"", 1),
    );
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "Simit disposable fixture"]);
    git(&["config", "user.email", "fixture@example.invalid"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.hooksPath=/dev/null",
        "commit",
        "-qm",
        "fixture",
    ]);
    git(&["-c", "tag.gpgsign=false", "tag", "worker/v0.2.0"]);
    let output = common::simit()
        .current_dir(root)
        .args([
            "release",
            "sync-up",
            "--component",
            "worker",
            "--dry-run",
            "--no-sign",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("worker/v0.2.0 already points to HEAD"));
    assert!(!text.contains("engine/v"));
    let output = common::simit()
        .current_dir(root)
        .args([
            "release",
            "sync-up",
            "--component",
            "rust",
            "--package",
            "worker",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let output = common::simit()
        .current_dir(root)
        .args(["release", "sync-up", "--dry-run"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires --component"));
}

#[test]
fn cargo_sync_up_refuses_to_move_tags_after_prerequisite_gate_failure() {
    let temp = fixture();
    let root = temp.path();
    let config = fs::read_to_string(root.join("simit.toml"))
        .unwrap()
        .replace(
            "id = \"core\"",
            "id = \"core\"\nchecks = [{id = 'core', run = 'true'}]",
        )
        .replace(
            "id = \"rust\"",
            "id = \"rust\"\nchecks = [{id = 'rust', run = 'true'}]",
        )
        .replace(
            "id = \"python\"",
            "id = \"python\"\nchecks = [{id = 'python', run = 'false | tee /dev/null'}]",
        )
        .replace(
            "id = \"worker\"",
            "id = \"worker\"\nchecks = [{id = 'worker', run = 'true'}]",
        );
    write(root, "simit.toml", &config);
    fixture_git(root, &["init", "-q"]);
    fixture_git(root, &["config", "user.name", "Simit fixture"]);
    fixture_git(root, &["config", "user.email", "fixture@example.invalid"]);
    fixture_git(root, &["config", "commit.gpgsign", "false"]);
    fixture_git(root, &["config", "tag.gpgsign", "false"]);
    fixture_git(root, &["add", "."]);
    fixture_git(root, &["commit", "-qm", "original source"]);
    fixture_git(root, &["tag", "worker/v0.1.0"]);
    let original = fixture_git(root, &["rev-parse", "refs/tags/worker/v0.1.0"]);
    write(root, "crates/worker/src/lib.rs", "pub fn updated() {}\n");
    fixture_git(root, &["add", "."]);
    fixture_git(root, &["commit", "-qm", "updated source"]);
    let output = common::simit()
        .current_dir(root)
        .args([
            "release",
            "sync-up",
            "--component",
            "worker",
            "--package",
            "worker",
            "--no-sign",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("component python check python failed"),
        "{output:?}"
    );
    assert_eq!(
        fixture_git(root, &["rev-parse", "refs/tags/worker/v0.1.0"]),
        original
    );
    let head = fixture_git(root, &["rev-parse", "HEAD"]);
    let manifest = fs::read(root.join("crates/worker/Cargo.toml")).unwrap();
    let bump = common::simit()
        .current_dir(root)
        .args([
            "release",
            "patch",
            "--component",
            "worker",
            "--package",
            "worker",
            "--no-sign",
            "--no-changelog",
            "-m",
            "release worker",
        ])
        .output()
        .unwrap();
    assert!(!bump.status.success(), "{bump:?}");
    assert!(
        String::from_utf8_lossy(&bump.stderr).contains("component python check python failed"),
        "{bump:?}"
    );
    assert_eq!(fixture_git(root, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fs::read(root.join("crates/worker/Cargo.toml")).unwrap(),
        manifest
    );
    assert!(fixture_git(root, &["tag", "--list", "worker/v0.1.1"]).is_empty());
}

#[test]
fn root_cargo_package_uses_explicit_source_ownership() {
    let temp = fixture();
    let root = temp.path();
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    write(
        root,
        "Cargo.toml",
        &format!("{manifest}\n[package]\nname='root-package'\nversion='0.1.0'\nedition='2024'\n"),
    );
    write(root, "src/lib.rs", "pub fn root() {}\n");
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    write(
        root,
        "simit.toml",
        &format!(
            "{config}\n[[monorepo.components]]\nid='root'\ncargo_packages=['root-package']\npaths=['src']\n"
        ),
    );
    for changed in ["src/lib.rs", "Cargo.toml", "python/server.py"] {
        let output = plan(root, &[changed]);
        assert!(output.status.success(), "{output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        let selected = value["selected"].as_array().unwrap();
        assert_eq!(
            selected.contains(&Value::String("root".into())),
            changed != "python/server.py",
            "{value}"
        );
    }
}

#[test]
fn cargo_release_rejects_gate_created_changes_and_commits() {
    for commit in [false, true] {
        let temp = fixture();
        let root = temp.path();
        enroll_release_checks(root);
        write(root, ".gitignore", "target/\n");
        let gate = if commit {
            "printf '\\n' >> crates/worker/src/lib.rs && git add crates/worker/src/lib.rs && git -c core.hooksPath=/dev/null commit -qm gate-change"
        } else {
            "printf '\\n' >> crates/worker/src/lib.rs"
        };
        let config = fs::read_to_string(root.join("simit.toml")).unwrap();
        let mut config: toml_edit::DocumentMut = config.parse().unwrap();
        let components = config["monorepo"]["components"]
            .as_array_of_tables_mut()
            .unwrap();
        let worker = components
            .iter_mut()
            .find(|component| component["id"].as_str() == Some("worker"))
            .unwrap();
        worker["checks"]
            .as_array_mut()
            .unwrap()
            .get_mut(0)
            .unwrap()
            .as_inline_table_mut()
            .unwrap()
            .insert("run", gate.into());
        write(root, "simit.toml", &config.to_string());
        let lock = Command::new("cargo")
            .current_dir(root)
            .args(["generate-lockfile", "--offline"])
            .output()
            .unwrap();
        assert!(lock.status.success(), "{lock:?}");
        fixture_git(root, &["init", "-q"]);
        fixture_git(root, &["config", "user.name", "Simit fixture"]);
        fixture_git(root, &["config", "user.email", "fixture@example.invalid"]);
        fixture_git(root, &["config", "commit.gpgsign", "false"]);
        fixture_git(root, &["add", "."]);
        fixture_git(root, &["commit", "-qm", "original source"]);
        let before = fs::read(root.join("crates/worker/Cargo.toml")).unwrap();
        let output = common::simit()
            .current_dir(root)
            .args([
                "release",
                "patch",
                "--component",
                "worker",
                "--no-sign",
                "--no-changelog",
                "-m",
                "release worker",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains(if commit {
                "release HEAD changed during qualification"
            } else {
                "worktree"
            }),
            "{output:?}"
        );
        assert_eq!(
            fs::read(root.join("crates/worker/Cargo.toml")).unwrap(),
            before
        );
        assert!(fixture_git(root, &["tag", "--list"]).is_empty());
    }
}

#[test]
fn independent_verification_uses_adjacent_notes_and_owned_tag() {
    let temp = fixture();
    let root = temp.path();
    write(
        root,
        "CHANGELOG.md",
        "# Changelog\n\n## [0.9.0]\n\nWrong root notes.\n",
    );
    write(
        root,
        "crates/engine/CHANGELOG.md",
        "# Changelog\n\n## [0.1.0]\n\nPackage release.\n",
    );
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "Simit disposable fixture"]);
    git(&["config", "user.email", "fixture@example.invalid"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.hooksPath=/dev/null",
        "commit",
        "-qm",
        "fixture",
    ]);
    git(&["-c", "tag.gpgsign=false", "tag", "0.1.0"]);
    for tagged in [false, true] {
        if tagged {
            git(&["-c", "tag.gpgsign=false", "tag", "engine/v0.1.0"]);
        }
        let output = common::simit()
            .current_dir(root)
            .args(["release", "verify", "--component", "rust", "--json"])
            .output()
            .unwrap();
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        let results = report["results"].as_array().unwrap();
        let notes = results
            .iter()
            .find(|r| r["check"] == "CHANGELOG entry exists")
            .unwrap();
        assert_eq!(notes["status"], "pass");
        let tag = results
            .iter()
            .find(|r| r["check"] == "tag presence")
            .unwrap();
        assert_eq!(tag["status"], if tagged { "pass" } else { "fail" });
        assert!(tag["message"].as_str().unwrap().contains("engine/v0.1.0"));
        assert!(
            !results
                .iter()
                .any(|r| r["check"].as_str().unwrap().starts_with("crates.io"))
        );
        assert!(
            !results.iter().any(|r| r["check"] == "remote secrets"),
            "{report}"
        );
    }
}

#[test]
fn cargo_release_rechecks_target_tag_after_qualification_before_mutation() {
    let temp = fixture();
    let root = temp.path();
    enroll_release_checks(root);
    let config = fs::read_to_string(root.join("simit.toml"))
        .unwrap()
        .replacen(
            "run = \"true\"",
            "run = \"git -c tag.gpgsign=false tag engine/v0.1.1\"",
            1,
        );
    write(root, "simit.toml", &config);
    write(root, ".gitignore", "target/\n");
    assert!(
        Command::new("cargo")
            .current_dir(root)
            .arg("generate-lockfile")
            .status()
            .unwrap()
            .success()
    );
    fixture_git(root, &["init"]);
    fixture_git(root, &["config", "user.name", "Test"]);
    fixture_git(root, &["config", "user.email", "test@example.invalid"]);
    fixture_git(root, &["add", "."]);
    fixture_git(root, &["commit", "-m", "tag-producing gate"]);
    let head = fixture_git(root, &["rev-parse", "HEAD"]);
    let manifest = fs::read(root.join("crates/engine/Cargo.toml")).unwrap();
    let output = common::simit()
        .current_dir(root)
        .args([
            "release",
            "patch",
            "--component",
            "rust",
            "--no-sign",
            "--no-changelog",
            "-m",
            "release",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fixture_git(root, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fs::read(root.join("crates/engine/Cargo.toml")).unwrap(),
        manifest
    );
    assert_eq!(fixture_git(root, &["status", "--porcelain"]), "");
}

#[cfg(unix)]
#[test]
fn cargo_component_releases_reject_symlinked_changelogs() {
    let temp = fixture();
    let external = TempDir::new().unwrap();
    let root = temp.path();
    enroll_release_checks(root);
    write(root, ".gitignore", "target/\n");
    let original = "# Changelog\n\n## [Unreleased]\n\n### Fixed\n\n- Original note.\n\n## [0.1.0] - 2026-10-01\n\nPrevious release.\n";
    let target = external.path().join("CHANGELOG.md");
    fs::write(&target, original).unwrap();
    std::os::unix::fs::symlink(&target, root.join("crates/engine/CHANGELOG.md")).unwrap();
    assert!(
        Command::new("cargo")
            .current_dir(root)
            .arg("generate-lockfile")
            .status()
            .unwrap()
            .success()
    );
    fixture_git(root, &["init"]);
    fixture_git(root, &["config", "user.name", "Test"]);
    fixture_git(root, &["config", "user.email", "test@example.invalid"]);
    fixture_git(root, &["add", "."]);
    fixture_git(root, &["commit", "-m", "symlink source"]);
    let head = fixture_git(root, &["rev-parse", "HEAD"]);
    let output = common::simit()
        .current_dir(root)
        .args([
            "release",
            "patch",
            "--component",
            "rust",
            "--no-sign",
            "-m",
            "release",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("release changelog must not be a symlink"),
        "{output:?}"
    );
    assert_eq!(fs::read_to_string(target).unwrap(), original);
    assert_eq!(fixture_git(root, &["rev-parse", "HEAD"]), head);
    assert!(fixture_git(root, &["tag", "--list"]).is_empty());
}

#[test]
fn historical_cargo_verification_locates_moved_manifest_paths() {
    let temp = fixture();
    let root = temp.path();
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    write(
        root,
        "simit.toml",
        &format!("{config}\n[release.notes]\nsource='none'\n"),
    );
    fixture_git(root, &["init"]);
    fixture_git(root, &["config", "user.name", "Test"]);
    fixture_git(root, &["config", "user.email", "test@example.invalid"]);
    fixture_git(root, &["add", "."]);
    fixture_git(root, &["commit", "-m", "original layout"]);
    fixture_git(root, &["tag", "engine/v0.1.0"]);
    fs::rename(root.join("crates/engine"), root.join("crates/moved-engine")).unwrap();
    for path in ["Cargo.toml", "crates/worker/Cargo.toml"] {
        let content = fs::read_to_string(root.join(path))
            .unwrap()
            .replace("crates/engine", "crates/moved-engine")
            .replace("../engine", "../moved-engine");
        write(root, path, &content);
    }
    let output = common::simit()
        .current_dir(root)
        .args([
            "release",
            "verify",
            "--component",
            "rust",
            "--version",
            "0.1.0",
            "--json",
        ])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["check"] == "tag manifest engine")
            .unwrap()["status"],
        "pass",
        "{report}"
    );
}

#[test]
fn cargo_component_verification_checks_the_tagged_manifest() {
    for notes in ["none", "changelog"] {
        for inherited in [false, true] {
            let temp = fixture();
            let root = temp.path();
            let config = fs::read_to_string(root.join("simit.toml")).unwrap();
            write(
                root,
                "simit.toml",
                &format!("{config}\n[release.notes]\nsource='{notes}'\n"),
            );
            let manifest = fs::read_to_string(root.join("crates/engine/Cargo.toml")).unwrap();
            write(
                root,
                "crates/engine/Cargo.toml",
                &manifest.replace("edition = \"2024\"", "edition = \"2024\"\npublish=false"),
            );
            if inherited {
                let manifest = fs::read_to_string(root.join("crates/engine/Cargo.toml"))
                    .unwrap()
                    .replace("version = \"0.1.0\"", "version.workspace = true");
                write(root, "crates/engine/Cargo.toml", &manifest);
                let workspace = fs::read_to_string(root.join("Cargo.toml")).unwrap();
                write(
                    root,
                    "Cargo.toml",
                    &format!("{workspace}\n[workspace.package]\nversion='0.1.0'\n"),
                );
            }
            fixture_git(root, &["init"]);
            fixture_git(root, &["config", "user.name", "Test"]);
            fixture_git(root, &["config", "user.email", "test@example.com"]);
            fixture_git(root, &["add", "."]);
            fixture_git(root, &["commit", "-m", "old source"]);
            fixture_git(root, &["tag", "engine/v0.2.0"]);
            let path = if inherited {
                "Cargo.toml"
            } else {
                "crates/engine/Cargo.toml"
            };
            let manifest = fs::read_to_string(root.join(path))
                .unwrap()
                .replace("0.1.0", "0.2.0");
            write(root, path, &manifest);
            if inherited {
                let manifest = fs::read_to_string(root.join("crates/engine/Cargo.toml"))
                    .unwrap()
                    .replace("version.workspace = true", "version = \"0.2.0\"");
                write(root, "crates/engine/Cargo.toml", &manifest);
            }
            fixture_git(root, &["add", "."]);
            fixture_git(root, &["commit", "-m", "current source"]);
            let verify = || {
                let output = common::simit()
                    .current_dir(root)
                    .args(["release", "verify", "--component", "rust", "--json"])
                    .output()
                    .unwrap();
                let report: Value = serde_json::from_slice(&output.stdout)
                    .unwrap_or_else(|error| panic!("{error}: {output:?}"));
                report["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|result| result["check"] == "tag manifest engine")
                    .expect("verify tagged Cargo source")
                    .clone()
            };
            assert_eq!(verify()["status"], "fail");
            fixture_git(root, &["tag", "-f", "engine/v0.2.0"]);
            assert_eq!(verify()["status"], "pass");
        }
    }
}

#[cfg(unix)]
#[test]
fn independent_verification_requires_crates_io_eligibility() {
    use std::os::unix::fs::PermissionsExt;

    for publish_crates in [false, true] {
        for (policy, eligible) in [
            ("publish = false", false),
            ("publish = [\"internal\"]", false),
            ("publish = [\"crates-io\"]", true),
            ("publish = [\"internal\", \"crates-io\"]", true),
            ("", true),
        ] {
            let temp = fixture();
            let root = temp.path();
            let manifest = fs::read_to_string(root.join("crates/engine/Cargo.toml")).unwrap();
            write(
                root,
                "crates/engine/Cargo.toml",
                &manifest.replace(
                    "edition = \"2024\"",
                    &format!("edition = \"2024\"\n{policy}"),
                ),
            );
            let config = fs::read_to_string(root.join("simit.toml")).unwrap();
            write(
                root,
                "simit.toml",
                &config.replace(
                    "runtime = \"nix\"",
                    &format!("runtime = \"nix\"\npublish_crates = {publish_crates}"),
                ),
            );
            write(
                root,
                "bin/curl",
                "#!/bin/sh\nprintf queried > \"$FIXTURE_CURL_MARKER\"\nprintf '%s\\n200' '{\"versions\":[{\"num\":\"0.1.0\"}]}'\n",
            );
            fs::set_permissions(root.join("bin/curl"), fs::Permissions::from_mode(0o755)).unwrap();
            let path = std::env::join_paths(
                std::iter::once(root.join("bin"))
                    .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
            )
            .unwrap();
            let marker = root.join("queried");
            let output = common::simit()
                .current_dir(root)
                .env("PATH", path)
                .env("FIXTURE_CURL_MARKER", &marker)
                .args(["release", "verify", "--component", "rust", "--json"])
                .output()
                .unwrap();
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            let results = report["results"].as_array().unwrap();
            assert_eq!(
                marker.exists(),
                eligible && publish_crates,
                "{policy}: {output:?}"
            );
            assert_eq!(
                results
                    .iter()
                    .any(|r| r["check"] == "crates.io engine" && r["status"] == "pass"),
                eligible && publish_crates,
                "{policy}: {report}"
            );
            assert_eq!(
                results.iter().any(|r| r["check"] == "remote secrets"
                    && r["message"]
                        .as_str()
                        .unwrap()
                        .contains("CRATES_IO_API_TOKEN")),
                eligible && publish_crates,
                "{policy}: {report}"
            );
        }
    }
}

#[test]
fn component_release_mutates_only_its_package_and_adjacent_changelog() {
    let temp = fixture();
    enroll_release_checks(temp.path());
    write(temp.path(), ".gitignore", "target/\n");
    write(temp.path(), "CHANGELOG.md", "# Unrelated root changelog\n");
    write(
        temp.path(),
        "crates/worker/CHANGELOG.md",
        "# Changelog\n\n## [Unreleased]\n\n### Fixed\n\n- Worker correction.\n",
    );
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(temp.path())
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "Simit disposable fixture"]);
    git(&["config", "user.email", "fixture@example.invalid"]);
    let lock = Command::new("cargo")
        .current_dir(temp.path())
        .args(["generate-lockfile", "--offline"])
        .output()
        .unwrap();
    assert!(lock.status.success(), "{lock:?}");
    git(&["add", "."]);
    git(&[
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-qm",
        "fixture",
    ]);
    let engine = fs::read_to_string(temp.path().join("crates/engine/Cargo.toml")).unwrap();
    let root = fs::read_to_string(temp.path().join("Cargo.toml")).unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args([
            "release",
            "patch",
            "--component",
            "worker",
            "--no-sign",
            "-m",
            "release worker",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(git(&["tag", "--list"]).trim(), "worker/v0.1.1");
    assert!(git(&["status", "--porcelain"]).is_empty());
    assert_eq!(
        fs::read_to_string(temp.path().join("Cargo.toml")).unwrap(),
        root
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("crates/engine/Cargo.toml")).unwrap(),
        engine
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("CHANGELOG.md")).unwrap(),
        "# Unrelated root changelog\n"
    );
    assert!(
        fs::read_to_string(temp.path().join("crates/worker/CHANGELOG.md"))
            .unwrap()
            .contains("## [0.1.1]")
    );
}

#[test]
fn unsupported_ci_options_fail_before_writing_and_shared_bumps_are_rejected() {
    let temp = fixture();
    for flag in ["--with-audit", "--with-nextest", "--with-pages", "--diff"] {
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci", flag])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{flag}: {output:?}");
        assert!(!temp.path().join(".github").exists());
    }
    let output = common::simit()
        .current_dir(temp.path())
        .args([
            "release",
            "patch",
            "--package",
            "worker",
            "--dry-run",
            "-m",
            "release",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("require --component"));
}

#[test]
#[cfg(unix)]
fn component_native_system_guard_rejects_wrong_architecture_and_eval_failure() {
    use std::os::unix::fs::PermissionsExt;
    let temp = fixture();
    let root = temp.path();
    enroll_release_checks(root);
    let config = fs::read_to_string(root.join("simit.toml"))
        .unwrap()
        .replace(
            "runtime = \"nix\"",
            "runtime = \"nix\"\nnix_system_runners = { aarch64-linux = 'ubuntu-24.04' }",
        )
        .replace(
            "id = \"python\"",
            "id = \"python\"\nsystems = ['aarch64-linux']",
        );
    write(root, "simit.toml", &config);
    let output = common::simit()
        .current_dir(root)
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&fs::read_to_string(root.join(".github/workflows/ci.yaml")).unwrap())
            .unwrap();
    let steps = workflow["jobs"]["component-python"]["steps"]
        .as_sequence()
        .unwrap();
    let index = steps
        .iter()
        .position(|step| step["name"] == "Verify runner system")
        .unwrap();
    assert!(
        index
            < steps
                .iter()
                .position(|step| step["name"] == "test")
                .unwrap()
    );
    let script = steps[index]["run"].as_str().unwrap();
    write(
        root,
        "bin/nix",
        "#!/bin/sh\nprintf '%s' \"$FIXTURE_NIX_SYSTEM\"\nexit \"$FIXTURE_NIX_STATUS\"\n",
    );
    fs::set_permissions(root.join("bin/nix"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        root.join("bin").display(),
        std::env::var("PATH").unwrap()
    );
    for (system, status, success) in [
        ("aarch64-linux", "0", true),
        ("x86_64-linux", "0", false),
        ("aarch64-linux", "1", false),
    ] {
        let output = Command::new("bash")
            .args(["-euo", "pipefail", "-c", script])
            .env("PATH", &path)
            .env("EXPECTED_NIX_SYSTEM", "aarch64-linux")
            .env("FIXTURE_NIX_SYSTEM", system)
            .env("FIXTURE_NIX_STATUS", status)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), success, "{output:?}");
    }
}

#[test]
fn independent_publishers_require_crates_io_allowlist_and_preserve_qualification() {
    for (policy, publish) in [
        ("publish = [\"internal\"]", false),
        ("publish = false", false),
        ("publish = [\"internal\", \"crates-io\"]", true),
        ("publish = [\"crates-io\"]", true),
    ] {
        let temp = fixture();
        let root = temp.path();
        enroll_release_checks(root);
        let config = fs::read_to_string(root.join("simit.toml"))
            .unwrap()
            .replace(
                "runtime = \"nix\"",
                "runtime = \"nix\"\npublish_crates = true",
            );
        write(root, "simit.toml", &config);
        let generate = || {
            common::simit()
                .current_dir(root)
                .args(["init", "ci"])
                .output()
                .unwrap()
        };
        let output = generate();
        assert!(output.status.success(), "{output:?}");
        let manifest = fs::read_to_string(root.join("crates/worker/Cargo.toml"))
            .unwrap()
            .replace(
                "edition = \"2024\"",
                &format!("edition = \"2024\"\n{policy}"),
            );
        write(root, "crates/worker/Cargo.toml", &manifest);
        let output = generate();
        assert!(output.status.success(), "{policy}: {output:?}");
        for prefix in ["publish-crate-", "trusted-publish-crate-"] {
            assert_eq!(
                root.join(format!(".github/workflows/{prefix}worker.yaml"))
                    .exists(),
                publish,
                "{policy}: {prefix}"
            );
            assert!(
                root.join(format!(".github/workflows/{prefix}engine.yaml"))
                    .exists()
            );
        }
        let ci = fs::read_to_string(root.join(".github/workflows/ci.yaml")).unwrap();
        assert!(ci.contains("component-worker:"));
    }
}

#[test]
fn independent_publishers_require_crates_io_dependency_closure() {
    for transitive in [false, true] {
        for eligible in [false, true] {
            let temp = fixture();
            let root = temp.path();
            enroll_release_checks(root);
            let mut config = fs::read_to_string(root.join("simit.toml"))
                .unwrap()
                .replace(
                    "runtime = \"nix\"",
                    "runtime = \"nix\"\npublish_crates = true",
                );
            let dependency = if transitive {
                let workspace = fs::read_to_string(root.join("Cargo.toml"))
                    .unwrap()
                    .replace(
                        "\"crates/engine\", \"crates/worker\"",
                        "\"crates/engine\", \"crates/worker\", \"crates/leaf\"",
                    );
                write(root, "Cargo.toml", &workspace);
                write(
                    root,
                    "crates/leaf/Cargo.toml",
                    "[package]\nname='leaf'\nversion='0.1.0'\nedition='2024'\n",
                );
                write(root, "crates/leaf/src/lib.rs", "pub fn leaf() {}\n");
                config = config.replace(
                    "id = \"core\"",
                    "id = \"core\"\ncargo_packages = [\"leaf\"]",
                );
                let manifest = fs::read_to_string(root.join("crates/engine/Cargo.toml")).unwrap();
                write(
                    root,
                    "crates/engine/Cargo.toml",
                    &format!("{manifest}leaf = {{path='../leaf',version='0.1.0'}}\n"),
                );
                "leaf"
            } else {
                "engine"
            };
            write(root, "simit.toml", &config);
            let path = format!("crates/{dependency}/Cargo.toml");
            let manifest = fs::read_to_string(root.join(&path)).unwrap();
            let policy = if eligible {
                "publish = ['internal', 'crates-io']"
            } else {
                "publish = ['internal']"
            };
            write(
                root,
                &path,
                &manifest.replacen("[package]", &format!("[package]\n{policy}"), 1),
            );
            let output = common::simit()
                .current_dir(root)
                .args(["init", "ci"])
                .output()
                .unwrap();
            assert_eq!(
                output.status.success(),
                eligible,
                "{transitive}/{eligible}: {output:?}"
            );
            if !eligible {
                let error = String::from_utf8_lossy(&output.stderr);
                assert!(
                    error.contains("not authorized for crates.io") && error.contains(dependency),
                    "{output:?}"
                );
                assert!(!root.join(".github/workflows/ci.yaml").exists());
                let output = common::simit()
                    .current_dir(root)
                    .args(["init", "ci", "--primary-only"])
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{output:?}");
            }
        }
    }
}

#[test]
fn independent_publication_qualifies_native_components_and_keeps_version_drift_stable() {
    let temp = fixture();
    let mut config = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    config = config.replace("runtime = \"nix\"", "runtime = \"nix\"\npublish_crates = true\n[ci.nix_system_runners]\naarch64-darwin = \"macos-14\"");
    for id in ["core", "rust", "python", "worker"] {
        config = config.replace(
            &format!("id = \"{id}\""),
            &format!("id = \"{id}\"\nchecks = [{{ id = \"qualify\", run = \"true\" }}]"),
        );
    }
    config = config.replace(
        "id = \"python\"",
        "id = \"python\"\nsystems = [\"aarch64-darwin\"]",
    );
    write(temp.path(), "simit.toml", &config);
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let path = temp
        .path()
        .join(".github/workflows/publish-crate-worker.yaml");
    let text = fs::read_to_string(&path).unwrap();
    let workflow: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    assert_eq!(workflow["on"]["push"]["tags"][0], "worker/v[0-9]*");
    assert_eq!(workflow["permissions"]["contents"], "read");
    assert_eq!(workflow["jobs"]["build"]["needs"][0], "qualified");
    assert_eq!(text.matches("name: Verify generated workflows").count(), 1);
    assert!(
        workflow["jobs"]["plan"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|step| step["name"] == "Verify generated workflows")
    );
    assert!(workflow["on"]["workflow_dispatch"].is_null());
    assert!(!text.contains("secrets."));
    assert_eq!(
        workflow["jobs"]["plan"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["id"] == "plan")
            .unwrap()["env"]["BASE_REVISION"],
        ""
    );
    for id in [
        "plan",
        "component-core",
        "component-rust",
        "component-python",
        "component-worker",
    ] {
        assert_public_input_transport(&workflow["jobs"][id]);
    }
    assert_eq!(
        workflow["jobs"]["component-python"]["strategy"]["matrix"]["include"][0]["runner"],
        "macos-14"
    );
    let steps = workflow["jobs"]["component-python"]["steps"]
        .as_sequence()
        .unwrap();
    let system_guard = steps
        .iter()
        .position(|step| step["name"] == "Verify runner system")
        .unwrap();
    assert!(
        system_guard
            < steps
                .iter()
                .position(|step| step["name"] == "qualify")
                .unwrap()
    );
    assert_eq!(
        steps[system_guard]["env"]["EXPECTED_NIX_SYSTEM"],
        "${{ matrix.system }}"
    );
    assert!(
        steps[system_guard]["run"]
            .as_str()
            .unwrap()
            .contains("builtins.currentSystem")
    );
    let publishes: Vec<_> = workflow["jobs"]
        .as_mapping()
        .unwrap()
        .keys()
        .filter_map(|k| k.as_str())
        .filter(|k| k.starts_with("publish-") && *k != "publish-report")
        .collect();
    assert!(publishes.is_empty());
    assert!(text.contains("git verify-tag"));
    assert!(text.contains("git rev-parse --verify \"refs/tags/${tag}^{commit}\""));
    assert!(text.contains("checkout_sha=\"$(git rev-parse --verify HEAD)\""));
    assert!(text.contains("ref: ${{ github.sha }}"));
    assert!(text.contains("version=\"${GITHUB_REF_NAME#worker/v}\""));
    assert!(text.contains("cargo package --locked --allow-dirty --no-verify -p worker"));
    let publisher_path = temp
        .path()
        .join(".github/workflows/trusted-publish-crate-worker.yaml");
    let publisher_text = fs::read_to_string(&publisher_path).unwrap();
    let publisher: serde_yaml::Value = serde_yaml::from_str(&publisher_text).unwrap();
    assert_eq!(
        publisher["on"]["workflow_run"]["workflows"][0],
        "Build Crate worker"
    );
    assert!(publisher["on"]["push"].is_null());
    assert_eq!(
        publisher["jobs"]["publish-worker"]["environment"]["name"],
        "simit-crates-io"
    );
    assert_eq!(
        publisher["jobs"]["verify-worker"]["runs-on"],
        "ubuntu-24.04"
    );
    assert!(publisher_text.contains("--tag-namespace worker"));
    assert!(publisher_text.contains("vars.SIMIT_PUBLISHER_REVISION"));
    let manifest = fs::read_to_string(temp.path().join("crates/worker/Cargo.toml")).unwrap();
    write(
        temp.path(),
        "crates/worker/Cargo.toml",
        &manifest.replacen("version = \"0.1.0\"", "version = \"0.2.0\"", 1),
    );
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--check"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(path).unwrap(), text);
    assert_eq!(fs::read_to_string(publisher_path).unwrap(), publisher_text);
    fs::remove_file(
        temp.path()
            .join(".github/workflows/publish-crate-engine.yaml"),
    )
    .unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--check"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
}

fn assert_public_input_transport(job: &serde_yaml::Value) {
    let steps = job["steps"].as_sequence().unwrap();
    let first_eval = steps
        .iter()
        .position(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.contains("nix develop"))
        })
        .unwrap();
    let transport = steps
        .iter()
        .position(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.contains("insteadOf"))
        })
        .unwrap();
    assert!(transport < first_eval);
    let run = steps[transport]["run"].as_str().unwrap();
    for forge in ["github.com", "codeberg.org"] {
        assert!(run.contains(&format!(
            "url.https://{forge}/.insteadOf ssh://git@{forge}/"
        )));
        assert!(run.contains(&format!("url.https://{forge}/.insteadOf git@{forge}:")));
    }
    assert!(!run.contains("secrets."));
}

#[test]
fn independent_cargo_sources_refuse_private_workers_and_candidate_authentication() {
    let publication_config = |temp: &TempDir| {
        let mut config = fs::read_to_string(temp.path().join("simit.toml"))
            .unwrap()
            .replace(
                "runtime = \"nix\"",
                "runtime = \"nix\"\npublish_crates = true",
            );
        for id in ["core", "rust", "python", "worker"] {
            config = config.replace(
                &format!("id = \"{id}\""),
                &format!("id = \"{id}\"\nchecks = [{{ id = \"qualify\", run = \"true\" }}]"),
            );
        }
        config
    };
    for mutation in [
        "runner = \"atlas-nix-trusted\"",
        "[ci.nix_system_runners]\naarch64-darwin = \"private-macos\"",
    ] {
        let temp = fixture();
        let config = publication_config(&temp).replace(
            "publish_crates = true",
            &format!("publish_crates = true\n{mutation}"),
        );
        let config = if mutation.contains("nix_system_runners") {
            config.replace(
                "id = \"python\"",
                "id = \"python\"\nsystems = [\"aarch64-darwin\"]",
            )
        } else {
            config
        };
        write(temp.path(), "simit.toml", &config);
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted {mutation}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("recognized GitHub-hosted runners"),
            "{output:?}"
        );
    }
    for value in [
        "${{ github.token }}",
        "${{ toJSON(github) }}",
        "${{ secrets.CRATES_IO_API_TOKEN }}",
    ] {
        let temp = fixture();
        let config = publication_config(&temp).replace("id = \"core\"\nchecks = [{ id = \"qualify\", run = \"true\" }]", &format!("id = \"core\"\nchecks = [{{ id = \"qualify\", run = \"true\", env = {{ TOKEN = '{value}' }} }}]"));
        write(temp.path(), "simit.toml", &config);
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted {value}: {output:?}");
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains("cannot contain workflow expressions"),
            "{output:?}"
        );
        assert!(!temp.path().join(".github/workflows/ci.yaml").exists());
    }
}

#[test]
fn qualification_actions_follow_hosted_and_mixed_private_capacity() {
    for (runner, matrix_runner, hosted_component) in [
        (Some("ubuntu-24.04"), None, true),
        (None, Some("ubuntu-24.04-arm"), true),
        (None, Some("atlas"), false),
        (Some("atlas"), None, false),
    ] {
        let temp = fixture();
        let runner_config = if let Some(runner) = runner {
            format!("runner = '{runner}'")
        } else {
            format!(
                "nix_system_runners = {{ x86_64-linux = 'ubuntu-24.04', aarch64-linux = '{}' }}",
                matrix_runner.unwrap()
            )
        };
        let mut config = fs::read_to_string(temp.path().join("simit.toml"))
            .unwrap()
            .replace(
                "runtime = \"nix\"",
                &format!("runtime = \"nix\"\n{runner_config}"),
            );
        for id in ["core", "rust", "python", "worker"] {
            config = config.replace(
                &format!("id = \"{id}\""),
                &format!("id = \"{id}\"\nchecks = [{{ id = \"qualify\", run = \"true\" }}]"),
            );
        }
        if matrix_runner.is_some() {
            config = config.replace(
                "id = \"core\"",
                "id = \"core\"\nsystems = ['x86_64-linux', 'aarch64-linux']",
            );
        }
        write(temp.path(), "simit.toml", &config);
        let output = common::simit()
            .current_dir(temp.path())
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let text = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
        let workflow: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        for (job, hosted) in [
            ("plan", runner != Some("atlas")),
            ("component-core", hosted_component),
        ] {
            let actions = workflow["jobs"][job]["steps"]
                .as_sequence()
                .unwrap()
                .iter()
                .filter_map(|step| step["uses"].as_str())
                .collect::<Vec<_>>();
            let expected = simit::render::ci::github_action_ref(
                "actions/checkout",
                if hosted { "v7.0.1" } else { "v4.3.1" },
            );
            assert_eq!(actions[0], expected.split(" #").next().unwrap());
            let expected = simit::render::ci::github_action_ref(
                "cachix/install-nix-action",
                if hosted { "v31.11.1" } else { "v31" },
            );
            assert_eq!(actions[1], expected.split(" #").next().unwrap());
        }
    }
}

fn fixture_git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn history_fixture() -> TempDir {
    let temp = fixture();
    let root = temp.path();
    write(root, ".gitignore", "target/\n");
    fixture_git(root, &["init", "-q"]);
    fixture_git(root, &["config", "user.name", "Simit disposable fixture"]);
    fixture_git(root, &["config", "user.email", "fixture@example.invalid"]);
    fixture_git(root, &["config", "commit.gpgsign", "false"]);
    fixture_git(root, &["config", "tag.gpgsign", "false"]);
    fixture_git(root, &["config", "core.hooksPath", "/dev/null"]);
    fixture_git(root, &["add", "."]);
    fixture_git(root, &["commit", "-qm", "Initial components"]);
    fixture_git(root, &["tag", "engine/v0.1.0"]);
    write(
        root,
        "crates/engine/src/lib.rs",
        "pub fn ready() {}\npub fn improved() {}\n",
    );
    fixture_git(root, &["add", "."]);
    fixture_git(root, &["commit", "-qm", "Improve engine behavior"]);
    write(root, "python/server.py", "# Unrelated Python behavior\n");
    fixture_git(root, &["add", "."]);
    fixture_git(root, &["commit", "-qm", "Unrelated Python change"]);
    fixture_git(root, &["tag", "worker/v9.0.0"]);
    temp
}

#[test]
fn component_git_notes_verify_the_owned_tag_from_non_cargo_members() {
    let temp = history_fixture();
    let root = temp.path();
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    write(
        root,
        "simit.toml",
        &format!("{config}\n[release.notes]\nsource = \"git\"\n"),
    );
    fixture_git(root, &["add", "."]);
    fixture_git(root, &["commit", "-qm", "Select Git release notes"]);
    fixture_git(root, &["tag", "engine/v0.1.1"]);
    let output = common::simit()
        .current_dir(root.join("python"))
        .args([
            "release",
            "verify",
            "--component",
            "rust",
            "--version",
            "0.1.1",
            "--json",
        ])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let notes = report["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["check"] == "release notes")
        .unwrap();
    assert_eq!(notes["status"], "pass", "{report}");
    assert!(notes["message"].as_str().unwrap().contains("engine/v0.1.1"));
    write(root, "crates/engine/src/lib.rs", "pub fn unreleased() {}\n");
    fixture_git(root, &["add", "."]);
    fixture_git(root, &["commit", "-qm", "Unreleased engine change"]);
    let script =
        simit::release_notes::component_git_notes_script("engine", &["crates/engine".to_owned()])
            .unwrap();
    let notes = Command::new("bash")
        .current_dir(root.join("python"))
        .env("TAG", "engine/v0.1.1")
        .args(["-c", &script])
        .output()
        .unwrap();
    assert!(notes.status.success(), "{notes:?}");
    let notes = String::from_utf8(notes.stdout).unwrap();
    assert!(notes.contains("Improve engine behavior"), "{notes}");
    assert!(!notes.contains("Initial components"), "{notes}");
    assert!(!notes.contains("Unrelated Python change"), "{notes}");
    assert!(!notes.contains("Unreleased engine change"), "{notes}");
}

#[cfg(unix)]
#[test]
fn component_automatic_changelog_drafting_uses_only_owned_history() {
    use std::os::unix::fs::PermissionsExt as _;
    let temp = history_fixture();
    let root = temp.path();
    enroll_release_checks(root);
    let lock = Command::new("cargo")
        .current_dir(root)
        .args(["generate-lockfile", "--offline"])
        .output()
        .unwrap();
    assert!(lock.status.success(), "{lock:?}");
    let support = TempDir::new().unwrap();
    let prompt = support.path().join("prompt");
    let script = support.path().join("codex");
    fs::write(&script, format!("#!/bin/sh\nwhile [ \"$1\" != '-o' ]; do shift; done\nshift; out=$1\ncat > '{}'\nprintf '%s\\n' '### Fixed' '' '- Improve engine behavior.' > \"$out\"\n", prompt.display())).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    write(
        root,
        "crates/engine/CHANGELOG.md",
        &format!(
            "{}\n## [Unreleased]\n\n## [0.1.0] - 2026-01-01\n\n### Added\n\n- Initial engine.\n",
            simit::changelog::HEADER
        ),
    );
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    write(
        root,
        "simit.toml",
        &format!("{config}\n[release.changelog]\nauto_draft = true\n"),
    );
    fixture_git(
        root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/monorepo.git",
        ],
    );
    fixture_git(root, &["add", "."]);
    fixture_git(root, &["commit", "-qm", "Configure engine release notes"]);
    let output = common::simit()
        .current_dir(root)
        .env("SIMIT_CODEX", &script)
        .args([
            "release",
            "patch",
            "--component",
            "rust",
            "--no-sign",
            "-m",
            "release engine",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let prompt = fs::read_to_string(prompt).unwrap();
    assert!(prompt.contains("Base: engine/v0.1.0"), "{prompt}");
    assert!(prompt.contains("Improve engine behavior"), "{prompt}");
    assert!(!prompt.contains("Unrelated Python change"), "{prompt}");
    assert!(!prompt.contains("python/server.py"), "{prompt}");
    let notes = fs::read_to_string(root.join("crates/engine/CHANGELOG.md")).unwrap();
    assert!(notes.contains("## [0.1.1]"), "{notes}");
    assert!(
        notes.contains("/compare/engine/v0.1.0...engine/v0.1.1"),
        "{notes}"
    );
    assert_eq!(fixture_git(root, &["status", "--porcelain"]), "");
}

#[cfg(unix)]
#[test]
fn submodule_components_fetch_recursively_in_every_generated_checkout() {
    let temp = fixture();
    let root = temp.path();
    enroll_release_checks(root);
    let config = fs::read_to_string(root.join("simit.toml"))
        .unwrap()
        .replace(
            "runtime = \"nix\"",
            "runtime = \"nix\"\npublish_crates = true",
        )
        .replace(
            "schema_version = 1",
            "schema_version = 1\nsubmodules = true",
        );
    write(root, "simit.toml", &config);
    let output = common::simit()
        .current_dir(root)
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    for workflow in ["ci.yaml", "publish-crate-engine.yaml"] {
        let workflow: serde_yaml::Value = serde_yaml::from_slice(
            &fs::read(root.join(".github/workflows").join(workflow)).unwrap(),
        )
        .unwrap();
        let checkouts: Vec<_> = workflow["jobs"]
            .as_mapping()
            .unwrap()
            .values()
            .flat_map(|job| job["steps"].as_sequence().unwrap())
            .filter(|step| {
                step["uses"]
                    .as_str()
                    .is_some_and(|uses| uses.starts_with("actions/checkout@"))
            })
            .collect();
        assert!(checkouts.len() > 1);
        for step in checkouts {
            assert_eq!(step["with"]["submodules"], "recursive");
        }
    }
    // Both publisher stages admit the pinned submodules; only the signed
    // release-source checkout fetches them (not the publisher root or engine).
    let publisher: serde_yaml::Value = serde_yaml::from_slice(
        &fs::read(root.join(".github/workflows/trusted-publish-crate-engine.yaml")).unwrap(),
    )
    .unwrap();
    let steps: Vec<_> = publisher["jobs"]
        .as_mapping()
        .unwrap()
        .values()
        .flat_map(|job| job["steps"].as_sequence().unwrap())
        .collect();
    let stages = steps
        .iter()
        .filter_map(|step| step["run"].as_str())
        .filter(|run| run.contains("release publisher --publisher-stage"));
    assert_eq!(
        stages
            .map(|run| run.contains(" --source-submodules"))
            .collect::<Vec<_>>(),
        [true, true]
    );
    let fetched: Vec<_> = steps
        .iter()
        .filter(|step| step["with"]["submodules"] == "recursive")
        .map(|step| step["with"]["path"].as_str())
        .collect();
    assert_eq!(fetched, [Some(".simit-release-source")]);
}

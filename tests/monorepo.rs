use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

mod common;

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
fn independent_verification_uses_adjacent_notes_and_owned_tag() {
    let temp = fixture();
    let root = temp.path();
    let engine = fs::read_to_string(root.join("crates/engine/Cargo.toml")).unwrap();
    write(
        root,
        "crates/engine/Cargo.toml",
        &engine.replace("edition = \"2024\"", "edition = \"2024\"\npublish = false"),
    );
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
    }
}

#[test]
fn component_release_mutates_only_its_package_and_adjacent_changelog() {
    let temp = fixture();
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
    assert_eq!(workflow["jobs"]["validate"]["needs"][0], "qualified");
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
    let publishes: Vec<_> = workflow["jobs"]
        .as_mapping()
        .unwrap()
        .keys()
        .filter_map(|k| k.as_str())
        .filter(|k| k.starts_with("publish-") && *k != "publish-report")
        .collect();
    assert_eq!(publishes, ["publish-worker"]);
    assert!(text.contains("git verify-tag"));
    assert!(text.contains("git rev-list -n 1 \"$tag\""));
    assert!(text.contains("${GITHUB_SHA:?missing workflow event SHA}"));
    assert!(text.contains("version=\"${version#*/v}\""));
    assert!(text.contains("cargo package -p worker"));
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

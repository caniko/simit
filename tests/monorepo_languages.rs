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

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write(
        root,
        "simit.toml",
        r#"[ci]
platform = "github"
provider = "actions"
runtime = "nix"

[monorepo]
schema_version = 1

[[monorepo.components]]
id = "python"
paths = ["python"]
releases = [{ manifest = "python/pyproject.toml", namespace = "py-engine", publish = false }]
checks = [{ id = "test", run = "true" }]

[[monorepo.components]]
id = "node"
paths = ["node"]
releases = [{ manifest = "node/package.json", namespace = "node-engine", publish = false }]
checks = [{ id = "test", run = "true" }]
"#,
    );
    write(
        root,
        "python/pyproject.toml",
        "# Keep this comment\n[project]\nname = 'py-engine'\nversion = '0.2.0'\nlicense = 'MIT'\ndependencies = ['dep==1.4.0']\n",
    );
    write(
        root,
        "python/uv.lock",
        "version = 1\n[[package]]\nname = 'py-engine'\nversion = '0.2.0'\nsource = { editable = '.' }\n[[package]]\nname = 'dep'\nversion = '1.4.0'\nsource = { registry = 'https://pypi.org/simple' }\n",
    );
    write(
        root,
        "node/package.json",
        "{\"name\":\"@example/node-engine\",\"version\":\"1.3.0\",\"private\":true,\"license\":\"Apache-2.0\",\"dependencies\":{\"dep\":\"~2.0.0\"}}\n",
    );
    write(
        root,
        "node/package-lock.json",
        "{\"name\":\"@example/node-engine\",\"version\":\"1.3.0\",\"lockfileVersion\":3,\"packages\":{\"\":{\"name\":\"@example/node-engine\",\"version\":\"1.3.0\"},\"node_modules/dep\":{\"version\":\"2.0.3\",\"integrity\":\"retained\"}}}\n",
    );
    for directory in ["python", "node"] {
        write(
            root,
            &format!("{directory}/CHANGELOG.md"),
            &format!(
                "{}\n## [Unreleased]\n\n### Fixed\n\n- Correct component behavior.\n",
                simit::changelog::HEADER
            ),
        );
    }
    git(root, &["init", "-q"]);
    git(root, &["config", "user.name", "Simit disposable fixture"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgsign", "false"]);
    git(root, &["config", "core.hooksPath", "/dev/null"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "Initial components"]);
    temp
}

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    common::simit()
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn native_private_build_capacity_never_receives_publication_credentials() {
    let temp = fixture();
    let root = temp.path();
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    write(
        root,
        "simit.toml",
        &config
            .replace("[ci]", "[ci]\nrunner = 'private-builder'")
            .replace("publish = false", "publish = true"),
    );
    let generated = run(root, &["init", "ci"]);
    assert!(generated.status.success(), "{generated:?}");
    let source =
        fs::read_to_string(root.join(".github/workflows/publish-python-py-engine.yaml")).unwrap();
    assert!(!source.contains("secrets."), "{source}");
    let source: serde_yaml::Value = serde_yaml::from_str(&source).unwrap();
    assert_eq!(source["jobs"]["validate"]["runs-on"], "ubuntu-24.04");
    assert_eq!(
        source["jobs"]["component-python"]["runs-on"],
        "private-builder"
    );
    let publisher =
        fs::read_to_string(root.join(".github/workflows/trusted-publish-python-py-engine.yaml"))
            .unwrap();
    let publisher: serde_yaml::Value = serde_yaml::from_str(&publisher).unwrap();
    assert!(publisher["on"].get("push").is_none());
    assert_eq!(publisher["jobs"]["publish"]["runs-on"], "ubuntu-24.04");
    assert_eq!(
        publisher["jobs"]["publish"]["environment"]["name"],
        "simit-pypi"
    );
}

#[test]
fn generated_component_gates_match_local_bash_and_pipeline_failure_semantics() {
    let temp = fixture();
    let root = temp.path();
    for (command, success) in [
        ("[[ $CHECK_VALUE == \"it's bash\" ]]", true),
        ("false | tee output", false),
    ] {
        let mut config: toml_edit::DocumentMut = fs::read_to_string(root.join("simit.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let checks = config["monorepo"]["components"]
            .as_array_of_tables_mut()
            .unwrap()
            .get_mut(0)
            .unwrap()["checks"]
            .as_array_mut()
            .unwrap();
        let gate = checks.get_mut(0).unwrap().as_inline_table_mut().unwrap();
        gate.insert("run", command.into());
        gate.insert("timeout_minutes", 7.into());
        let mut env = toml_edit::InlineTable::new();
        env.insert("CHECK_VALUE", "it's bash".into());
        gate.insert("env", env.into());
        write(root, "simit.toml", &config.to_string());
        let generated = run(root, &["init", "ci"]);
        assert!(generated.status.success(), "{generated:?}");
        let workflow: serde_yaml::Value = serde_yaml::from_str(
            &fs::read_to_string(root.join(".github/workflows/ci.yaml")).unwrap(),
        )
        .unwrap();
        let steps = workflow["jobs"]["component-python"]["steps"]
            .as_sequence()
            .unwrap();
        let gate = steps.iter().find(|step| step["name"] == "test").unwrap();
        assert_eq!(gate["timeout-minutes"], 7);
        let script = gate["run"]
            .as_str()
            .unwrap()
            .split_once("--command ")
            .unwrap()
            .1;
        let output = Command::new("bash")
            .args(["-euo", "pipefail", "-c", script])
            .current_dir(root)
            .env("CHECK_VALUE", "it's bash")
            .output()
            .unwrap();
        assert_eq!(output.status.success(), success, "{output:?}");
        assert!(
            workflow["jobs"]["qualified"]["steps"][0]["run"]
                .as_str()
                .unwrap()
                .contains("result == \"success\"")
        );
    }
}

#[test]
fn non_cargo_plan_and_python_bump_preserve_independent_metadata() {
    let temp = fixture();
    let root = temp.path();
    let node = fs::read(root.join("node/package.json")).unwrap();
    let plan = run(
        &root.join("node"),
        &["release", "plan", "--component", "python", "--json"],
    );
    assert!(plan.status.success(), "{plan:?}");
    let plan: Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(plan["entries"][0]["name"], "py-engine");
    assert_eq!(plan["entries"][0]["version"], "0.2.0");
    assert_eq!(plan["entries"][0]["tag"], "py-engine/v0.2.0");
    assert_eq!(plan["entries"][0]["publish"], false);
    let output = run(
        root,
        &[
            "release",
            "patch",
            "--component",
            "python",
            "--no-sign",
            "-m",
            "release Python engine",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let manifest = fs::read_to_string(root.join("python/pyproject.toml")).unwrap();
    assert!(manifest.contains("# Keep this comment"));
    let manifest: toml_edit::DocumentMut = manifest.parse().unwrap();
    assert_eq!(manifest["project"]["version"].as_str(), Some("0.2.1"));
    assert_eq!(manifest["project"]["license"].as_str(), Some("MIT"));
    assert_eq!(fs::read(root.join("node/package.json")).unwrap(), node);
    let lock: toml_edit::DocumentMut = fs::read_to_string(root.join("python/uv.lock"))
        .unwrap()
        .parse()
        .unwrap();
    let packages = lock["package"].as_array_of_tables().unwrap();
    assert_eq!(packages.get(0).unwrap()["version"].as_str(), Some("0.2.1"));
    assert_eq!(packages.get(1).unwrap()["version"].as_str(), Some("1.4.0"));
    assert_eq!(git(root, &["tag", "--list"]), "py-engine/v0.2.1\n");
    assert_eq!(git(root, &["status", "--porcelain"]), "");
    let verify = run(
        &root.join("node"),
        &["release", "verify", "--component", "python", "--json"],
    );
    let report: Value = serde_json::from_slice(&verify.stdout).unwrap();
    assert!(
        report["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["check"] == "tagged package identity" && check["status"] == "pass"),
        "{report}"
    );
    write(root, "python/feature.py", "# Next qualified source\n");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "Improve Python engine"]);
    let sync = run(
        &root.join("node"),
        &["release", "sync-up", "--component", "python", "--no-sign"],
    );
    assert!(sync.status.success(), "{sync:?}");
    assert_eq!(
        git(root, &["rev-parse", "py-engine/v0.2.1^{}"]),
        git(root, &["rev-parse", "HEAD"])
    );
}

#[test]
fn scoped_private_npm_bump_updates_only_root_lock_records() {
    let temp = fixture();
    let root = temp.path();
    let python = fs::read(root.join("python/pyproject.toml")).unwrap();
    let output = run(
        &root.join("python"),
        &[
            "release",
            "minor",
            "--component",
            "node",
            "--package",
            "@example/node-engine",
            "--no-sign",
            "-m",
            "release Node engine",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("node/package.json")).unwrap()).unwrap();
    assert_eq!(manifest["version"], "1.4.0");
    assert_eq!(manifest["private"], true);
    assert_eq!(manifest["dependencies"]["dep"], "~2.0.0");
    let lock: Value =
        serde_json::from_slice(&fs::read(root.join("node/package-lock.json")).unwrap()).unwrap();
    assert_eq!(lock["version"], "1.4.0");
    assert_eq!(lock["packages"][""]["version"], "1.4.0");
    assert_eq!(lock["packages"]["node_modules/dep"]["version"], "2.0.3");
    assert_eq!(
        lock["packages"]["node_modules/dep"]["integrity"],
        "retained"
    );
    assert_eq!(
        fs::read(root.join("python/pyproject.toml")).unwrap(),
        python
    );
    assert_eq!(git(root, &["tag", "--list"]), "node-engine/v1.4.0\n");
    assert_eq!(git(root, &["status", "--porcelain"]), "");
}

#[test]
fn nested_npm_workspace_bump_updates_both_lock_formats_without_touching_other_members() {
    let temp = fixture();
    let root = temp.path();
    let lock = r#"{"name":"workspace","version":"9.0.0","lockfileVersion":3,"packages":{"":{"name":"workspace","version":"9.0.0"},"node":{"name":"@example/node-engine","version":"1.3.0"},"other":{"version":"3.0.0"},"node_modules/@example/node-engine":{"resolved":"node","link":true}}}"#;
    for name in ["package-lock.json", "npm-shrinkwrap.json"] {
        write(root, name, lock);
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "workspace locks"]);
    let output = run(
        root,
        &[
            "release",
            "minor",
            "--component",
            "node",
            "--no-sign",
            "-m",
            "release node",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    for name in ["package-lock.json", "npm-shrinkwrap.json"] {
        let updated: Value = serde_json::from_slice(&fs::read(root.join(name)).unwrap()).unwrap();
        assert_eq!(updated["version"], "9.0.0");
        assert_eq!(updated["packages"][""]["version"], "9.0.0");
        assert_eq!(updated["packages"]["node"]["version"], "1.4.0");
        assert_eq!(updated["packages"]["other"]["version"], "3.0.0");
        assert_eq!(
            updated["packages"]["node_modules/@example/node-engine"]["link"],
            true
        );
    }
    assert_eq!(git(root, &["status", "--porcelain"]), "");
}

#[test]
fn native_notes_and_repository_trust_resolve_without_cargo_metadata() {
    let temp = fixture();
    let root = temp.path();
    let config = fs::read_to_string(root.join("simit.toml"))
        .unwrap()
        .replace("platform = \"github\"\n", "");
    write(
        root,
        "simit.toml",
        &format!("{config}\n[release.github]\nrepo='example/native'\nbody_from_changelog=false\n"),
    );
    write(
        root,
        "python/CHANGELOG.md",
        "# Changelog\n\n## [Unreleased]\n\n",
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "disable GitHub notes"]);
    let notes = fs::read(root.join("python/CHANGELOG.md")).unwrap();
    let output = run(
        root,
        &[
            "release",
            "patch",
            "--component",
            "python",
            "--no-sign",
            "-m",
            "release python",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read(root.join("python/CHANGELOG.md")).unwrap(), notes);
    let verify = run(
        root,
        &["release", "verify", "--component", "python", "--json"],
    );
    let report: Value = serde_json::from_slice(&verify.stdout).unwrap();
    assert!(
        report
            .to_string()
            .contains("release notes explicitly disabled"),
        "{report}"
    );
    write(
        root,
        "simit.toml",
        &format!(
            "{}\n[release.codeberg]\nrepo='example/native'\nbody_from_changelog=true\n",
            fs::read_to_string(root.join("simit.toml"))
                .unwrap()
                .replace("[ci]", "[ci]\nplatform='forgejo'")
        ),
    );
    let explicit = run(
        root,
        &[
            "release",
            "verify",
            "--component",
            "python",
            "--platform",
            "github",
            "--json",
        ],
    );
    let report: Value = serde_json::from_slice(&explicit.stdout).unwrap();
    assert!(
        report
            .to_string()
            .contains("release notes explicitly disabled"),
        "{report}"
    );
    let status = run(&root.join("node"), &["release", "trust", "status"]);
    assert!(status.status.success(), "{status:?}");
    let check = run(&root.join("python"), &["release", "trust", "check"]);
    assert!(!check.status.success());
    assert!(
        String::from_utf8_lossy(&check.stderr).contains("maintainers.gpg"),
        "{check:?}"
    );
}

#[test]
fn ancestor_locks_are_skipped_unless_they_own_or_declare_the_workspace_package() {
    for declared in [false, true] {
        let temp = fixture();
        let root = temp.path();
        let unrelated = r#"{"name":"independent-root","version":"9.0.0","lockfileVersion":3,"packages":{"":{"name":"independent-root","version":"9.0.0"}}}"#;
        write(root, "package-lock.json", unrelated);
        let workspaces = if declared {
            ",\"workspaces\":[\"n*\"]"
        } else {
            ""
        };
        write(
            root,
            "package.json",
            &format!("{{\"name\":\"independent-root\",\"version\":\"9.0.0\"{workspaces}}}"),
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "ancestor npm project"]);
        let original = fs::read(root.join("node/package.json")).unwrap();
        let output = run(
            root,
            &[
                "release",
                "patch",
                "--component",
                "node",
                "--no-sign",
                "-m",
                "release node",
            ],
        );
        assert_eq!(output.status.success(), !declared, "{output:?}");
        assert_eq!(
            fs::read_to_string(root.join("package-lock.json")).unwrap(),
            unrelated
        );
        if declared {
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("npm workspace lock has no package record"),
                "{output:?}"
            );
            assert_eq!(fs::read(root.join("node/package.json")).unwrap(), original);
            assert!(git(root, &["tag", "--list"]).is_empty());
        }
    }
}

#[test]
fn python_ancestor_locks_require_declared_workspace_ownership() {
    for (declared, record, excluded) in [
        (false, false, false),
        (false, true, false),
        (true, false, false),
        (true, true, false),
        (true, true, true),
    ] {
        let temp = fixture();
        let root = temp.path();
        fs::remove_file(root.join("python/uv.lock")).unwrap();
        let workspace = if declared {
            format!(
                "\n[tool.uv.workspace]\nmembers=['py*']\nexclude={}\n",
                if excluded { "['python']" } else { "[]" }
            )
        } else {
            String::new()
        };
        write(
            root,
            "pyproject.toml",
            &format!("[project]\nname='independent-root'\nversion='9.0.0'\n{workspace}"),
        );
        let lock = format!(
            "version=1\n[[package]]\nname='independent-root'\nversion='9.0.0'\nsource={{editable='.'}}\n{}",
            if record {
                "[[package]]\nname='py-engine'\nversion='0.2.0'\nsource={editable='python'}\n"
            } else {
                ""
            }
        );
        write(root, "uv.lock", &lock);
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "ancestor Python project"]);
        let output = run(
            root,
            &[
                "release",
                "patch",
                "--component",
                "python",
                "--no-sign",
                "--no-changelog",
                "-m",
                "release python",
            ],
        );
        assert_eq!(
            output.status.success(),
            !declared || excluded || record,
            "{output:?}"
        );
        let after = fs::read_to_string(root.join("uv.lock")).unwrap();
        if declared && !excluded && record {
            let document: toml_edit::DocumentMut = after.parse().unwrap();
            let packages = document["package"].as_array_of_tables().unwrap();
            assert_eq!(
                packages
                    .iter()
                    .find(|package| package["name"].as_str() == Some("py-engine"))
                    .unwrap()["version"]
                    .as_str(),
                Some("0.2.1")
            );
            assert_eq!(
                packages
                    .iter()
                    .find(|package| package["name"].as_str() == Some("independent-root"))
                    .unwrap()["version"]
                    .as_str(),
                Some("9.0.0")
            );
        } else {
            assert_eq!(after, lock);
        }
    }
}

#[test]
fn native_namespace_length_matches_protected_publisher_admission() {
    for length in [64, 65] {
        let temp = fixture();
        let root = temp.path();
        let namespace = "a".repeat(length);
        let config = fs::read_to_string(root.join("simit.toml"))
            .unwrap()
            .replace("py-engine\"", &format!("{namespace}\""));
        write(root, "simit.toml", &config);
        let output = run(root, &["init", "ci"]);
        assert_eq!(output.status.success(), length == 64, "{output:?}");
        if length == 65 {
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("64 bytes"),
                "{output:?}"
            );
        }
    }
}

#[test]
fn invalid_public_npm_dist_tags_reject_during_generation() {
    for version in ["1.3.0", "1.3.0-rc.1"] {
        for tag in [
            serde_json::json!("release candidate"),
            serde_json::json!(""),
            serde_json::json!("1latest"),
            serde_json::json!(null),
            serde_json::json!(7),
        ] {
            let temp = fixture();
            let root = temp.path();
            let config = fs::read_to_string(root.join("simit.toml"))
                .unwrap()
                .replace("publish = false", "publish = true");
            write(root, "simit.toml", &config);
            write(root, "node/package.json", &serde_json::json!({"name":"@example/node-engine","version":version,"publishConfig":{"tag":tag}}).to_string());
            let output = run(root, &["init", "ci"]);
            assert!(!output.status.success(), "{output:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("npm"),
                "{output:?}"
            );
            assert!(
                !root
                    .join(".github/workflows/trusted-publish-npm-node-engine.yaml")
                    .exists()
            );
        }
    }
}

#[test]
fn registry_names_can_overlap_but_release_namespaces_select_one_owner() {
    let temp = fixture();
    let root = temp.path();
    write(
        root,
        "simit.toml",
        r#"[monorepo]
schema_version = 1
[[monorepo.components]]
id = "shared"
paths = ["python", "node"]
releases = [
  { manifest = "python/pyproject.toml", namespace = "shared-python", publish = false },
  { manifest = "node/package.json", namespace = "shared-node", publish = false },
]
checks = [{ id = "test", run = "true" }]
"#,
    );
    write(
        root,
        "python/pyproject.toml",
        "[project]\nname = 'shared'\nversion = '0.2.0'\n",
    );
    write(
        root,
        "python/uv.lock",
        "version = 1\n[[package]]\nname = 'shared'\nversion = '0.2.0'\nsource = { editable = '.' }\n",
    );
    write(
        root,
        "node/package.json",
        "{\"name\":\"shared\",\"version\":\"1.3.0\",\"private\":true}\n",
    );
    write(
        root,
        "node/package-lock.json",
        "{\"name\":\"shared\",\"version\":\"1.3.0\",\"lockfileVersion\":3,\"packages\":{\"\":{\"name\":\"shared\",\"version\":\"1.3.0\"}}}\n",
    );
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-qm", "Retain separate registry identities"],
    );
    let before = git(root, &["status", "--porcelain"]);
    let ambiguous = run(
        root,
        &[
            "release",
            "plan",
            "--component",
            "shared",
            "--package",
            "shared",
            "--json",
        ],
    );
    assert!(!ambiguous.status.success(), "{ambiguous:?}");
    assert!(
        String::from_utf8_lossy(&ambiguous.stderr).contains("release namespace"),
        "{ambiguous:?}"
    );
    assert_eq!(git(root, &["status", "--porcelain"]), before);
    for (selector, version) in [("shared-python", "0.2.0"), ("shared-node", "1.3.0")] {
        let plan = run(
            root,
            &[
                "release",
                "plan",
                "--component",
                "shared",
                "--package",
                selector,
                "--json",
            ],
        );
        assert!(plan.status.success(), "{plan:?}");
        let plan: Value = serde_json::from_slice(&plan.stdout).unwrap();
        assert_eq!(plan["entries"][0]["name"], "shared");
        assert_eq!(plan["entries"][0]["version"], version);
        assert_eq!(plan["entries"][0]["tag"], format!("{selector}/v{version}"));
    }
    let npm = fs::read(root.join("node/package.json")).unwrap();
    let bumped = run(
        root,
        &[
            "release",
            "patch",
            "--component",
            "shared",
            "--package",
            "shared-python",
            "--no-sign",
            "-m",
            "Release Python identity",
        ],
    );
    assert!(bumped.status.success(), "{bumped:?}");
    assert!(
        fs::read_to_string(root.join("python/pyproject.toml"))
            .unwrap()
            .contains("0.2.1")
    );
    assert_eq!(fs::read(root.join("node/package.json")).unwrap(), npm);
    assert_eq!(git(root, &["tag", "--list"]), "shared-python/v0.2.1\n");
    let verify = run(
        root,
        &[
            "release",
            "verify",
            "--component",
            "shared",
            "--package",
            "shared-python",
            "--json",
        ],
    );
    let report: Value = serde_json::from_slice(&verify.stdout).unwrap();
    for name in [
        "tag presence",
        "tagged package identity",
        "CHANGELOG entry exists",
    ] {
        assert!(
            report["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["check"] == name && check["status"] == "pass"),
            "{report}"
        );
    }
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    write(
        root,
        "simit.toml",
        &config.replace("namespace = \"shared-node\"", "namespace = \"shared\""),
    );
    let namespaced = run(
        root,
        &[
            "release",
            "plan",
            "--component",
            "shared",
            "--package",
            "shared",
            "--json",
        ],
    );
    assert!(namespaced.status.success(), "{namespaced:?}");
    let plan: Value = serde_json::from_slice(&namespaced.stdout).unwrap();
    assert_eq!(plan["entries"][0]["tag"], "shared/v1.3.0");
    write(
        root,
        "node/pyproject.toml",
        "[project]\nname = 'shared'\nversion = '1.3.0'\n",
    );
    write(
        root,
        "simit.toml",
        &config.replace("node/package.json", "node/pyproject.toml"),
    );
    let duplicate = run(root, &["monorepo", "plan", "--json"]);
    assert!(!duplicate.status.success(), "{duplicate:?}");
    assert!(
        String::from_utf8_lossy(&duplicate.stderr)
            .contains("duplicate python release package name"),
        "{duplicate:?}"
    );
}

#[test]
fn non_cargo_failures_leave_versions_tags_and_index_untouched() {
    for (replacement, diagnostic) in [("run = \"exit 9\"", "test"), ("run = \"true\"", "worktree")]
    {
        let temp = fixture();
        let root = temp.path();
        let config = fs::read_to_string(root.join("simit.toml")).unwrap();
        write(
            root,
            "simit.toml",
            &config.replace("run = \"true\"", replacement),
        );
        if diagnostic == "test" {
            git(root, &["add", "."]);
            git(root, &["commit", "-qm", "Fail test gate"]);
        } else {
            write(root, "pending.txt", "Operator work\n");
        }
        let before = git(root, &["status", "--porcelain"]);
        let manifest = fs::read(root.join("python/pyproject.toml")).unwrap();
        let head = git(root, &["rev-parse", "HEAD"]);
        let output = run(
            root,
            &[
                "release",
                "patch",
                "--component",
                "python",
                "--no-sign",
                "-m",
                "release",
            ],
        );
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "{output:?}"
        );
        assert_eq!(
            fs::read(root.join("python/pyproject.toml")).unwrap(),
            manifest
        );
        assert_eq!(git(root, &["status", "--porcelain"]), before);
        assert_eq!(git(root, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(root, &["tag", "--list"]), "");
    }
}

#[test]
fn prerequisite_gate_blocks_non_cargo_mutation() {
    let temp = fixture();
    let root = temp.path();
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    write(
        root,
        "simit.toml",
        &config
            .replace("id = \"node\"", "id = \"node\"\ndepends_on = [\"python\"]")
            .replacen("run = \"true\"", "run = \"exit 7\"", 1),
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "Require Python prerequisite"]);
    let before = fs::read(root.join("node/package.json")).unwrap();
    let output = run(
        root,
        &[
            "release",
            "patch",
            "--component",
            "node",
            "--no-sign",
            "-m",
            "release",
        ],
    );
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("component python check test failed"),
        "{output:?}"
    );
    assert_eq!(fs::read(root.join("node/package.json")).unwrap(), before);
    assert_eq!(git(root, &["status", "--porcelain"]), "");
    assert_eq!(git(root, &["tag", "--list"]), "");
}

#[test]
fn invalid_native_release_metadata_and_stale_locks_fail_before_mutation() {
    for (path, text, diagnostic) in [
        (
            "python/pyproject.toml",
            "[project]\nname = 'py-engine'\ndynamic = ['version']\n",
            "dynamic Python versions",
        ),
        (
            "python/uv.lock",
            "[[package]]\nname = 'py-engine'\nversion = '0.0.1'\nsource = {editable = '.'}\n",
            "uv.lock package version disagrees",
        ),
        (
            "python/uv.lock",
            "[[package]]\nversion = '0.2.0'\nsource = {editable = '.'}\n",
            "uv.lock has no local version record",
        ),
    ] {
        let temp = fixture();
        let root = temp.path();
        write(root, path, text);
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "Broken release prerequisite"]);
        let before = fs::read(root.join("python/pyproject.toml")).unwrap();
        let output = run(
            root,
            &[
                "release",
                "patch",
                "--component",
                "python",
                "--no-sign",
                "-m",
                "release",
            ],
        );
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "{output:?}"
        );
        assert_eq!(
            fs::read(root.join("python/pyproject.toml")).unwrap(),
            before
        );
        assert_eq!(git(root, &["status", "--porcelain"]), "");
        assert_eq!(git(root, &["tag", "--list"]), "");
    }
}

#[cfg(unix)]
#[test]
fn native_registry_verification_checks_exact_identity_and_available_files() {
    use std::os::unix::fs::PermissionsExt as _;

    for (component, body, status, expected) in [
        (
            "python",
            r#"{"info":{"name":"Py_Engine","version":"0.2.0"},"urls":[{"yanked":false,"digests":{"sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}]}"#,
            "200",
            "pass",
        ),
        (
            "python",
            r#"{"info":{"name":"another-project","version":"0.2.0"},"urls":[{"yanked":false}]}"#,
            "200",
            "fail",
        ),
        (
            "python",
            r#"{"info":{"name":"py-engine","version":"0.2.0"},"urls":[{"yanked":true}]}"#,
            "200",
            "fail",
        ),
        ("python", "{}", "404", "fail"),
        ("python", "unavailable", "503", "blocked"),
        ("node", "not JSON", "200", "blocked"),
        (
            "node",
            r#"{"name":"@example/node-engine","versions":{"1.3.0":{"name":"@example/node-engine","version":"1.3.0","dist":{"integrity":"sha512-retained"}}}}"#,
            "200",
            "pass",
        ),
        (
            "node",
            r#"{"name":"@example/node-engine","versions":{"1.3.0":{"name":"@example/node-engine","version":"1.4.0","dist":{"integrity":"sha512-retained"}}}}"#,
            "200",
            "fail",
        ),
        (
            "node",
            r#"{"name":"@example/node-engine","versions":{}}"#,
            "200",
            "fail",
        ),
    ] {
        let temp = fixture();
        let root = temp.path();
        let config = fs::read_to_string(root.join("simit.toml")).unwrap();
        write(
            root,
            "simit.toml",
            &config.replace("publish = false", "publish = true"),
        );
        let npm: Value =
            serde_json::from_slice(&fs::read(root.join("node/package.json")).unwrap()).unwrap();
        let mut npm = npm;
        npm["private"] = false.into();
        write(root, "node/package.json", &npm.to_string());
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "Declare registry publication"]);
        let tag = if component == "python" {
            "py-engine/v0.2.0"
        } else {
            "node-engine/v1.3.0"
        };
        git(root, &["tag", tag]);
        let tools = TempDir::new().unwrap();
        let curl = tools.path().join("curl");
        let request = tools.path().join("request");
        fs::write(
            &curl,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n%s' '{}' {}\n",
                request.display(),
                body,
                status
            ),
        )
        .unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(tools.path().to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let output = common::simit()
            .current_dir(root)
            .env("PATH", path)
            .args(["release", "verify", "--component", component, "--json"])
            .output()
            .unwrap();
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        let registry = report["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["check"] == "registry publication")
            .unwrap();
        assert_eq!(registry["status"], expected, "{report}");
        let request = fs::read_to_string(request).unwrap();
        assert!(request.starts_with("--disable\n"));
        assert!(
            request.contains(if component == "python" {
                "https://pypi.org/pypi/py-engine/0.2.0/json"
            } else {
                "https://registry.npmjs.org/@example%2Fnode-engine"
            }),
            "{request}"
        );
        assert_eq!(git(root, &["status", "--porcelain"]), "");
    }
}

#[test]
fn python_registry_normalization_rejects_duplicate_version_owners() {
    let temp = fixture();
    let root = temp.path();
    write(
        root,
        "node/pyproject.toml",
        "[project]\nname = 'Py_Engine'\nversion = '1.3.0'\n",
    );
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    write(
        root,
        "simit.toml",
        &config.replace("node/package.json", "node/pyproject.toml"),
    );
    let output = run(root, &["monorepo", "plan", "--json"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("duplicate python release package name"),
        "{output:?}"
    );
}

#[test]
fn unsupported_public_native_versions_fail_before_mutation_and_uv_tracks_normalized_prereleases() {
    let temp = fixture();
    let root = temp.path();
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    write(
        root,
        "simit.toml",
        &config.replace("publish = false", "publish = true"),
    );
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-qm", "Enable Python registry publication"],
    );
    let head = git(root, &["rev-parse", "HEAD"]);
    let result = run(
        root,
        &[
            "release",
            "minor",
            "--component",
            "python",
            "--pre",
            "preview.1",
            "--no-sign",
            "--no-changelog",
            "-m",
            "unsupported version",
        ],
    );
    assert!(!result.status.success(), "{result:?}");
    assert_eq!(git(root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(root, &["status", "--porcelain"]), "");
    assert_eq!(git(root, &["tag", "--list"]), "");
    let result = run(
        root,
        &[
            "release",
            "minor",
            "--component",
            "python",
            "--pre",
            "rc.4",
            "--no-sign",
            "--no-changelog",
            "-m",
            "Release Python prerelease",
        ],
    );
    assert!(result.status.success(), "{result:?}");
    assert!(
        fs::read_to_string(root.join("python/pyproject.toml"))
            .unwrap()
            .contains("0.3.0-rc.4")
    );
    assert!(
        fs::read_to_string(root.join("python/uv.lock"))
            .unwrap()
            .contains("0.3.0rc4")
    );
    assert_eq!(
        git(root, &["tag", "--list"]).trim(),
        "py-engine/v0.3.0-rc.4"
    );
}

#[test]
fn eligible_native_publication_qualifies_the_full_graph_and_preserves_private_owners() {
    let temp = fixture();
    let root = temp.path();
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    write(
        root,
        "simit.toml",
        &config.replace("publish = false", "publish = true"),
    );
    let generated = run(root, &["init", "ci"]);
    assert!(generated.status.success(), "{generated:?}");
    let path = root.join(".github/workflows/publish-python-py-engine.yaml");
    let source = fs::read_to_string(&path).unwrap();
    let workflow: serde_yaml::Value = serde_yaml::from_str(&source).unwrap();
    assert_eq!(workflow["on"]["push"]["tags"][0], "py-engine/v[0-9]*");
    assert_eq!(workflow["jobs"]["validate"]["needs"][0], "qualified");
    assert!(workflow["jobs"].get("publish").is_none());
    assert_eq!(workflow["permissions"]["contents"], "read");
    for job in workflow["jobs"].as_mapping().unwrap().values() {
        for step in job["steps"].as_sequence().unwrap() {
            assert!(
                step.get("env")
                    .is_none_or(|env| env.as_mapping().is_none_or(|env| !env.is_empty()))
            );
        }
    }
    let plan_steps = workflow["jobs"]["plan"]["steps"].as_sequence().unwrap();
    let plan = plan_steps.iter().find(|step| step["id"] == "plan").unwrap();
    assert_eq!(plan["env"]["BASE_REVISION"], "");
    assert!(source.contains("git verify-tag"));
    assert!(!source.contains("PYPI_API_TOKEN"));
    let publisher =
        fs::read_to_string(root.join(".github/workflows/trusted-publish-python-py-engine.yaml"))
            .unwrap();
    assert!(publisher.contains("PYPI_API_TOKEN"));
    assert!(publisher.contains("github.workflow_sha"));
    for (id, job) in workflow["jobs"].as_mapping().unwrap() {
        if id.as_str().unwrap() != "publish" {
            assert!(!serde_yaml::to_string(job).unwrap().contains("secrets."));
        }
    }
    assert!(
        !root
            .join(".github/workflows/publish-npm-node-engine.yaml")
            .exists()
    );
    let npm: Value =
        serde_json::from_slice(&fs::read(root.join("node/package.json")).unwrap()).unwrap();
    let mut npm = npm;
    npm["private"] = false.into();
    npm["publishConfig"] = serde_json::json!({"access": "public", "provenance": true});
    write(root, "node/package.json", &npm.to_string());
    let generated = run(root, &["init", "ci"]);
    assert!(generated.status.success(), "{generated:?}");
    let npm =
        fs::read_to_string(root.join(".github/workflows/publish-npm-node-engine.yaml")).unwrap();
    assert!(!npm.contains("NPM_TOKEN"));
    let publisher =
        fs::read_to_string(root.join(".github/workflows/trusted-publish-npm-node-engine.yaml"))
            .unwrap();
    assert!(publisher.contains("NPM_TOKEN"));
    let trusted: serde_yaml::Value = serde_yaml::from_str(&publisher).unwrap();
    assert_eq!(
        trusted["jobs"]["publish"]["permissions"]["id-token"],
        "write"
    );
    assert!(publisher.contains("--native-provenance"));
    let npm_workflow: serde_yaml::Value = serde_yaml::from_str(&npm).unwrap();
    assert!(npm_workflow["jobs"].get("publish").is_none());
    assert!(
        npm_workflow["jobs"]["validate"]
            .get("permissions")
            .is_none()
    );
    for entry in fs::read_dir(root.join(".github/workflows")).unwrap() {
        let output = Command::new("actionlint")
            .arg(entry.unwrap().path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    assert!(
        root.join(".github/scripts/simit-native-release.py")
            .exists()
    );
    let original = fs::read_to_string(root.join("python/pyproject.toml")).unwrap();
    write(
        root,
        "python/pyproject.toml",
        &original.replace("0.2.0", "0.2.1"),
    );
    let check = run(root, &["init", "ci", "--check", "--diff"]);
    assert!(check.status.success(), "{check:?}");
    assert_eq!(fs::read_to_string(path).unwrap(), source);
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    write(
        root,
        "simit.toml",
        &config.replace("publish = true", "publish = false"),
    );
    let check = run(root, &["init", "ci", "--check"]);
    assert!(!check.status.success(), "{check:?}");
    let generated = run(root, &["init", "ci"]);
    assert!(generated.status.success(), "{generated:?}");
    assert!(
        !root
            .join(".github/workflows/publish-python-py-engine.yaml")
            .exists()
    );
    assert!(
        !root
            .join(".github/workflows/publish-npm-node-engine.yaml")
            .exists()
    );
}

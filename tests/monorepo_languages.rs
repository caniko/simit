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

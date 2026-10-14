//! CLI enrollment binds the exact loaded bytes to the observed source commit.
use serde_json::json;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn git(root: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", root.join("empty-global"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("empty-global"), b"").unwrap();
    git(root.path(), &["init"]);
    // Disposable fixture metadata only; operator identity and hooks are untouched.
    git(root.path(), &["config", "user.name", "Fixture"]);
    git(
        root.path(),
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(root.path(), &["config", "commit.gpgSign", "false"]);
    fs::create_dir(root.path().join("empty-hooks")).unwrap();
    git(root.path(), &["config", "core.hooksPath", "empty-hooks"]);
    fs::create_dir(root.path().join(".github")).unwrap();
    let spec = json!({"schema":"simit.original-observer-spec.v1", "repository":"owner/repo",
        "workflows":{".github/workflows/ci.yaml":["test"]},"timeout_seconds":120,"poll_seconds":10});
    fs::write(
        root.path().join(".github/spec.json"),
        serde_json::to_vec_pretty(&spec).unwrap(),
    )
    .unwrap();
    git(root.path(), &["add", ".github/spec.json"]);
    git(root.path(), &["commit", "-m", "fixture"]);
    root
}

fn invoke(root: &Path, spec: &Path, retention: bool) -> Output {
    let head = String::from_utf8(git(root, &["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let event = root.join("event.json");
    fs::write(
        &event,
        json!({"after":head,"ref":"refs/heads/feature","deleted":false,
        "repository":{"id":101,"full_name":"owner/repo"}})
        .to_string(),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_simit"));
    command
        .current_dir(root)
        .args([
            "ci",
            if retention {
                "verify-original-retention"
            } else {
                "observe-original"
            },
        ])
        .arg("--spec")
        .arg(spec)
        .arg("--results")
        .arg(root.join("packet"))
        .env("RUNNER_ENVIRONMENT", "github-hosted")
        .env("GITHUB_REPOSITORY", "owner/repo")
        .env("GITHUB_EVENT_PATH", event)
        .env("GITHUB_EVENT_NAME", "push")
        .env("GITHUB_SHA", &head)
        .env("GITHUB_RUN_ID", "99")
        .env("GITHUB_RUN_ATTEMPT", "1")
        .env(
            "GITHUB_WORKFLOW_REF",
            "owner/repo/.github/workflows/custody.yaml@refs/heads/feature",
        )
        .env("GITHUB_WORKFLOW_SHA", &head)
        .env("GIT_CONFIG_GLOBAL", root.join("empty-global"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN");
    if retention {
        command.args(["--artifact-id", "42", "--artifact-digest", &"a".repeat(64)]);
    }
    command.output().unwrap()
}

#[test]
fn both_cli_routes_reject_substituted_source_owned_spec_bytes() {
    for retention in [false, true] {
        for kind in ["modified", "staged", "untracked", "outside", "replacement"] {
            let root = fixture();
            let spec = root.path().join(".github/spec.json");
            let mut path = spec.clone();
            let outside = tempfile::tempdir().unwrap();
            if matches!(kind, "modified" | "staged" | "replacement") {
                // A valid policy substitution must fail even though HEAD is unchanged.
                let mut value: serde_json::Value =
                    serde_json::from_slice(&fs::read(&spec).unwrap()).unwrap();
                value["workflows"][".github/workflows/ci.yaml"] = json!(["weaker-job"]);
                fs::write(&spec, value.to_string()).unwrap();
                if kind == "staged" {
                    git(root.path(), &["add", ".github/spec.json"]);
                }
                if kind == "replacement" {
                    let original =
                        String::from_utf8(git(root.path(), &["rev-parse", "HEAD"]).stdout).unwrap();
                    git(root.path(), &["add", ".github/spec.json"]);
                    git(root.path(), &["commit", "-m", "substituted fixture"]);
                    let substituted =
                        String::from_utf8(git(root.path(), &["rev-parse", "HEAD"]).stdout).unwrap();
                    git(root.path(), &["update-ref", "HEAD", original.trim()]);
                    git(
                        root.path(),
                        &["replace", original.trim(), substituted.trim()],
                    );
                }
            } else if kind == "untracked" {
                path = root.path().join("copy.json");
                fs::copy(&spec, &path).unwrap();
            } else {
                path = outside.path().join("spec.json");
                fs::copy(&spec, &path).unwrap();
            }
            let output = invoke(root.path(), &path, retention);
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success());
            assert!(
                error.contains("source-owned observer spec"),
                "{kind}, retention={retention}: {error}"
            );
            assert!(!root.path().join("packet").exists());
        }
    }
}

#[test]
fn committed_spec_is_admitted_even_when_unrelated_source_is_dirty() {
    let root = fixture();
    fs::write(root.path().join("unrelated"), b"local work").unwrap();
    for retention in [false, true] {
        let output = invoke(
            root.path(),
            &root.path().join(".github/spec.json"),
            retention,
        );
        let error = String::from_utf8_lossy(&output.stderr);
        // Exact enrollment reaches credential admission without any provider access.
        assert!(!output.status.success());
        assert!(error.contains("environment variable"), "{error}");
        assert!(!error.contains("source-owned observer spec"), "{error}");
        assert!(!root.path().join("packet").exists());
    }
}

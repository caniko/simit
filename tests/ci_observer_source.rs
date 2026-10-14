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
fn clean_crlf_checkout_is_bound_to_committed_policy_on_both_routes() {
    let root = fixture();
    fs::write(
        root.path().join(".gitattributes"),
        b"*.json text eol=crlf\n",
    )
    .unwrap();
    git(root.path(), &["add", ".gitattributes"]);
    git(root.path(), &["commit", "-m", "CRLF checkout attribute"]);
    let path = root.path().join(".github/spec.json");
    let raw = fs::read_to_string(&path).unwrap();
    fs::write(&path, raw.replace('\n', "\r\n")).unwrap();
    for retention in [false, true] {
        let output = invoke(root.path(), &path, retention);
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("environment variable"), "{error}");
        assert!(!error.contains("source-owned observer spec"), "{error}");
    }
    // A real policy edit must still fail even under the admitted conversion.
    fs::write(
        &path,
        raw.replace("test", "weaker-job").replace('\n', "\r\n"),
    )
    .unwrap();
    for retention in [false, true] {
        let output = invoke(root.path(), &path, retention);
        assert!(String::from_utf8_lossy(&output.stderr).contains("source-owned observer spec"));
    }
}

#[test]
fn configured_clean_filter_cannot_replace_the_committed_policy_that_is_parsed() {
    let root = fixture();
    let path = root.path().join(".github/spec.json");
    let valid_checkout = fs::read(&path).unwrap();
    let mut committed: serde_json::Value = serde_json::from_slice(&valid_checkout).unwrap();
    committed["schema"] = json!("unadmitted-policy");
    fs::write(&path, committed.to_string()).unwrap();
    fs::write(
        root.path().join(".gitattributes"),
        b"*.json filter=policy\n",
    )
    .unwrap();
    git(root.path(), &["add", ".gitattributes", ".github/spec.json"]);
    git(
        root.path(),
        &["commit", "-m", "unadmitted canonical policy"],
    );
    git(
        root.path(),
        &[
            "config",
            "filter.policy.clean",
            "git --no-replace-objects cat-file blob HEAD:.github/spec.json",
        ],
    );
    fs::write(&path, valid_checkout).unwrap();
    for retention in [false, true] {
        let output = invoke(root.path(), &path, retention);
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("unknown observer schema"), "{error}");
        assert!(!root.path().join("packet").exists());
    }
}

#[test]
fn committed_blob_probe_admits_the_full_cli_policy_envelope() {
    let root = fixture();
    let spec = json!({"schema":"simit.original-observer-spec.v1","repository":"owner/repo",
        "timeout_seconds":20700,"poll_seconds":60,
        "workflows":(0..10).map(|i| (format!(".github/workflows/workflow-{i}.yaml"),
            json!((0..256).map(|job| format!("{job:03}{}","x".repeat(509))).collect::<Vec<_>>()))).collect::<serde_json::Map<_,_>>()});
    let raw = serde_json::to_vec_pretty(&spec).unwrap();
    assert!(raw.len() > 1024 * 1024);
    let path = root.path().join(".github/spec.json");
    fs::write(&path, raw).unwrap();
    git(root.path(), &["add", ".github/spec.json"]);
    git(root.path(), &["commit", "-m", "maximum admitted policy"]);
    for retention in [false, true] {
        let output = invoke(root.path(), &path, retention);
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("environment variable"), "{error}");
        assert!(!root.path().join("packet").exists());
    }
}

#[test]
fn event_snapshot_is_not_reloaded_after_source_blob_authorization() {
    let root = fixture();
    let path = root.path().join(".github/spec.json");
    fs::write(
        root.path().join(".gitattributes"),
        b"*.json filter=event-race\n",
    )
    .unwrap();
    fs::write(
        root.path().join(".github/clean.sh"),
        b"printf '{}' >event.json\ncat\n",
    )
    .unwrap();
    git(root.path(), &["add", ".gitattributes", ".github/clean.sh"]);
    git(
        root.path(),
        &["commit", "-m", "fixture source probe event drift"],
    );
    git(
        root.path(),
        &["config", "filter.event-race.clean", "sh .github/clean.sh"],
    );
    for retention in [false, true] {
        let output = invoke(root.path(), &path, retention);
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(fs::read(root.path().join("event.json")).unwrap(), b"{}");
        assert!(
            error.contains("environment variable"),
            "event reloaded after authorization: {error}"
        );
        assert!(!root.path().join("packet").exists());
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

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

fn observer_command(root: &Path, spec: &Path, retention: bool) -> Command {
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
        .env("GITHUB_REF", "refs/heads/feature")
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
    command
}

fn invoke(root: &Path, spec: &Path, retention: bool) -> Output {
    observer_command(root, spec, retention).output().unwrap()
}

#[cfg(unix)]
#[test]
fn blocking_event_paths_fail_promptly_with_incomplete_preflight_evidence() {
    use std::time::{Duration, Instant};
    for retention in [false, true] {
        for kind in ["fifo", "symlink", "device"] {
            let root = fixture();
            let mut command = observer_command(
                root.path(),
                &root.path().join(".github/spec.json"),
                retention,
            );
            let event = root.path().join("event.json");
            let original = fs::read(&event).unwrap();
            fs::remove_file(&event).unwrap();
            match kind {
                "fifo" => {
                    assert!(
                        Command::new("mkfifo")
                            .args(["-m", "600"])
                            .arg(&event)
                            .status()
                            .unwrap()
                            .success()
                    );
                }
                "symlink" => {
                    fs::write(root.path().join("real-event.json"), &original).unwrap();
                    std::os::unix::fs::symlink("real-event.json", &event).unwrap();
                }
                _ => std::os::unix::fs::symlink("/dev/zero", &event).unwrap(),
            }
            command
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let mut child = command.spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("{kind} event blocked preflight, retention={retention}");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(!status.success());
            let prefix = if retention {
                "packet.retention.preflight-"
            } else {
                "packet.preflight-"
            };
            let diagnostic = fs::read_dir(root.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(prefix)
                })
                .unwrap();
            let receipt: serde_json::Value =
                serde_json::from_slice(&fs::read(diagnostic.join("receipt.json")).unwrap())
                    .unwrap();
            assert_eq!(receipt["qualified"], false);
            assert_eq!(receipt["capture_complete"], false);
            assert_eq!(receipt["preflight_complete"], false);
            assert!(receipt["error"].as_str().unwrap().contains("regular file"));
            assert!(!root.path().join("packet").exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn cli_signal_cancels_a_blocked_real_git_probe_and_retains_preflight_failure() {
    use std::{
        os::unix::fs::OpenOptionsExt,
        time::{Duration, Instant},
    };
    for retention in [false, true] {
        let root = fixture();
        let mut command = observer_command(
            root.path(),
            &root.path().join(".github/spec.json"),
            retention,
        );
        let head = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"]).stdout).unwrap();
        let head = head.trim();
        let object = root
            .path()
            .join(".git/objects")
            .join(&head[..2])
            .join(&head[2..]);
        fs::remove_file(&object).unwrap();
        assert!(
            Command::new("mkfifo")
                .args(["-m", "600"])
                .arg(&object)
                .status()
                .unwrap()
                .success()
        );
        let mut child = command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        // Connecting a nonblocking writer proves Git opened this exact object;
        // keeping it empty and open holds the real probe inside a blocking read.
        let writer = loop {
            if let Ok(file) = fs::OpenOptions::new()
                .write(true)
                .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
                .open(&object)
            {
                break file;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("Git never entered the blocking source probe, retention={retention}");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let start = Instant::now();
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(child.id() as i32).unwrap(),
            rustix::process::Signal::TERM,
        )
        .unwrap();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if start.elapsed() >= Duration::from_secs(3) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("CLI cancellation ignored the running Git probe, retention={retention}");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        drop(writer);
        assert!(!status.success());
        let prefix = if retention {
            "packet.retention.preflight-"
        } else {
            "packet.preflight-"
        };
        let diagnostic = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(prefix)
            })
            .unwrap();
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(diagnostic.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(receipt["preflight_complete"], false);
        assert_eq!(receipt["capture_complete"], false);
        assert!(receipt["error"].as_str().unwrap().contains("cancelled"));
        assert!(!root.path().join("packet").exists());
    }
}

fn replace_fixture_object(path: &Path, bytes: &[u8]) {
    // Git creates read-only loose objects. Windows forbids deleting them until
    // that attribute is cleared; these paths belong only to disposable fixtures.
    #[cfg(windows)]
    {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
    }
    fs::remove_file(path).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn forged_loose_blob_tree_and_commit_objects_cannot_authorize_weaker_policy() {
    use std::io::Write;
    for retention in [false, true] {
        for kind in ["blob", "tree", "commit"] {
            let root = fixture();
            let spec = root.path().join(".github/spec.json");
            let mut command = observer_command(root.path(), &spec, retention);
            command.env_remove("GH_TOKEN");
            let oid = |reference: &str| {
                String::from_utf8(git(root.path(), &["rev-parse", reference]).stdout)
                    .unwrap()
                    .trim()
                    .to_owned()
            };
            let old_blob = oid("HEAD:.github/spec.json");
            let old_tree = oid("HEAD:.github");
            let head = oid("HEAD");
            let old_commit = git(root.path(), &["cat-file", "commit", &head]).stdout;
            let tree = git(root.path(), &["cat-file", "tree", &old_tree]).stdout;
            let mut policy: serde_json::Value =
                serde_json::from_slice(&fs::read(&spec).unwrap()).unwrap();
            policy["workflows"][".github/workflows/ci.yaml"] = json!(["weaker-job"]);
            let weak = serde_json::to_vec(&policy).unwrap();
            fs::write(&spec, &weak).unwrap();
            git(root.path(), &["add", ".github/spec.json"]);
            let new_tree = String::from_utf8(git(root.path(), &["write-tree"]).stdout)
                .unwrap()
                .trim()
                .to_owned();
            let new_blob = oid(":.github/spec.json");
            let (target, substituted) = match kind {
                "blob" => (old_blob.clone(), weak),
                "tree" => {
                    let original = hex::decode(&old_blob).unwrap();
                    let replacement = hex::decode(&new_blob).unwrap();
                    let mut forged = tree;
                    let index = forged
                        .windows(20)
                        .position(|bytes| bytes == original)
                        .unwrap();
                    forged[index..index + 20].copy_from_slice(&replacement);
                    (old_tree, forged)
                }
                _ => {
                    let old = String::from_utf8(old_commit).unwrap();
                    let first = old.lines().next().unwrap();
                    (
                        head,
                        old.replacen(first, &format!("tree {new_tree}"), 1)
                            .into_bytes(),
                    )
                }
            };
            let object = root
                .path()
                .join(".git/objects")
                .join(&target[..2])
                .join(&target[2..]);
            let mut compressed =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            compressed
                .write_all(format!("{kind} {}\0", substituted.len()).as_bytes())
                .unwrap();
            compressed.write_all(&substituted).unwrap();
            replace_fixture_object(&object, &compressed.finish().unwrap());
            let output = command.output().unwrap();
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains("source object integrity mismatch"),
                "{kind}/{retention}: {error}"
            );
            assert!(!root.path().join("packet").exists());
        }
    }
}

#[test]
fn forged_protected_merge_object_cannot_substitute_its_parents() {
    use std::io::Write;
    for retention in [false, true] {
        let root = fixture();
        let old = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
        fs::write(root.path().join("new-head"), "current PR head").unwrap();
        git(root.path(), &["add", "new-head"]);
        git(root.path(), &["commit", "-m", "current head"]);
        let current = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
        let merge = String::from_utf8(
            git(
                root.path(),
                &[
                    "commit-tree",
                    "HEAD^{tree}",
                    "-p",
                    &old,
                    "-p",
                    &current,
                    "-m",
                    "protected merge",
                ],
            )
            .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        let raw =
            String::from_utf8(git(root.path(), &["cat-file", "commit", &merge]).stdout).unwrap();
        let forged = raw.replace(&format!("parent {current}\n"), &format!("parent {old}\n"));
        let mut compressed =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        compressed
            .write_all(format!("commit {}\0", forged.len()).as_bytes())
            .unwrap();
        compressed.write_all(forged.as_bytes()).unwrap();
        let object = root
            .path()
            .join(".git/objects")
            .join(&merge[..2])
            .join(&merge[2..]);
        replace_fixture_object(&object, &compressed.finish().unwrap());
        git(root.path(), &["checkout", "--detach", &old]);
        let mut command = observer_command(
            root.path(),
            &root.path().join(".github/spec.json"),
            retention,
        );
        let mut event: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("event.json")).unwrap()).unwrap();
        event["pull_request"] = json!({"number":71,"head":{"sha":old},"base":{"sha":old}});
        fs::write(root.path().join("event.json"), event.to_string()).unwrap();
        command
            .env("GITHUB_EVENT_NAME", "pull_request")
            .env("GITHUB_REF", "refs/pull/71/merge")
            .env("GITHUB_SHA", &merge);
        let output = command.output().unwrap();
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("source object integrity mismatch"),
            "{error}"
        );
        assert!(!root.path().join("packet").exists());
    }
}

#[test]
fn protected_pr_merge_revision_rejects_a_replayed_head_and_admits_its_actual_parent() {
    let root = fixture();
    let old = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    fs::write(root.path().join("current-head"), "current").unwrap();
    git(root.path(), &["add", "current-head"]);
    git(root.path(), &["commit", "-m", "current PR head"]);
    let current = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let merge = String::from_utf8(
        git(
            root.path(),
            &[
                "commit-tree",
                "HEAD^{tree}",
                "-p",
                &old,
                "-p",
                &current,
                "-m",
                "protected runner merge",
            ],
        )
        .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    for retention in [false, true] {
        for (head, base, admitted) in [
            (&old, &old, false),
            (&current, &old, true),
            (&current, &current, false),
        ] {
            git(root.path(), &["checkout", "--detach", head]);
            let mut command = observer_command(
                root.path(),
                &root.path().join(".github/spec.json"),
                retention,
            );
            let mut event: serde_json::Value =
                serde_json::from_slice(&fs::read(root.path().join("event.json")).unwrap()).unwrap();
            event["pull_request"] = json!({"number":71,"head":{"sha":head},"base":{"sha":base}});
            fs::write(root.path().join("event.json"), event.to_string()).unwrap();
            command
                .env("GITHUB_EVENT_NAME", "pull_request")
                .env("GITHUB_REF", "refs/pull/71/merge")
                .env("GITHUB_SHA", &merge);
            let output = command.output().unwrap();
            let error = String::from_utf8_lossy(&output.stderr);
            if admitted {
                assert!(error.contains("token environment variable"), "{error}");
            } else {
                assert!(error.contains("protected merge revision"), "{error}");
            }
            assert!(!root.path().join("packet").exists());
        }
    }
}

#[test]
fn substituted_event_branch_and_pr_number_cannot_change_the_native_runner_target() {
    for retention in [false, true] {
        for pull_request in [false, true] {
            let root = fixture();
            let spec = root.path().join(".github/spec.json");
            let mut command = observer_command(root.path(), &spec, retention);
            let mut event: serde_json::Value =
                serde_json::from_slice(&fs::read(root.path().join("event.json")).unwrap()).unwrap();
            if pull_request {
                event["pull_request"] = json!({"number":72,"head":{"sha":event["after"]}});
                command
                    .env("GITHUB_EVENT_NAME", "pull_request")
                    .env("GITHUB_REF", "refs/pull/71/merge");
            } else {
                event["ref"] = json!("refs/heads/other-feature");
            }
            fs::write(root.path().join("event.json"), event.to_string()).unwrap();
            let output = command.output().unwrap();
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("runner target"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!root.path().join("packet").exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn workspace_git_prepended_to_path_cannot_forge_the_committed_policy() {
    use std::os::unix::fs::PermissionsExt;
    for retention in [false, true] {
        let root = fixture();
        let spec = root.path().join(".github/spec.json");
        let mut command = observer_command(root.path(), &spec, retention);
        let mut policy: serde_json::Value =
            serde_json::from_slice(&fs::read(&spec).unwrap()).unwrap();
        policy["workflows"][".github/workflows/ci.yaml"] = json!(["weaker-job"]);
        fs::write(&spec, policy.to_string()).unwrap();
        let head = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"]).stdout).unwrap();
        let bin = root.path().join("fake-bin");
        fs::create_dir(&bin).unwrap();
        let fake = bin.join("git");
        fs::write(&fake, format!("#!/bin/sh\nprintf 'executed' >fake-git-executed\ncase \"$*\" in\n*'rev-parse HEAD'*) printf '%s\\n' '{}' ;;\n*'--show-toplevel'*) printf '%s\\n' '{}' ;;\n*'cat-file blob'*) cat .github/spec.json ;;\n*) printf '%s\\n' '{}' ;;\nesac\n", head.trim(), root.path().display(), "a".repeat(40))).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let mut entries = vec![bin];
        entries.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        command.env("PATH", std::env::join_paths(entries).unwrap());
        let output = command.output().unwrap();
        assert!(
            !root.path().join("fake-git-executed").exists(),
            "source admission executed mutable PATH Git"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("source-owned observer spec"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!root.path().join("packet").exists());
    }
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
fn configured_clean_filter_is_never_executed_for_source_policy_admission() {
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
            "printf 'filter executed' >filter-executed; git --no-replace-objects cat-file blob HEAD:.github/spec.json",
        ],
    );
    fs::write(&path, valid_checkout).unwrap();
    for retention in [false, true] {
        let output = invoke(root.path(), &path, retention);
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            !root.path().join("filter-executed").exists(),
            "source admission executed an external clean filter"
        );
        assert!(error.contains("source-owned observer spec"), "{error}");
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
fn external_filter_cannot_mutate_the_authorizing_event_snapshot() {
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
        let event: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("event.json")).unwrap()).unwrap();
        assert_eq!(
            event["repository"]["full_name"], "owner/repo",
            "source admission ran the event-mutating clean filter"
        );
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

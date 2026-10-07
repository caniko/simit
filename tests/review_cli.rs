use std::process::Command;

#[test]
fn review_example_and_validation_work_without_project_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let example = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["review", "example"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(
        example.status.success(),
        "{}",
        String::from_utf8_lossy(&example.stderr)
    );
    let request = temp.path().join("request.json");
    std::fs::write(&request, &example.stdout).unwrap();
    let validation = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["review", "validate"])
        .arg(&request)
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(validation.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&example.stdout).unwrap(),
        serde_json::from_slice::<serde_json::Value>(&validation.stdout).unwrap()
    );
}

#[test]
fn dispatch_requires_a_separate_branch_or_tag_selector() {
    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args([
            "review",
            "dispatch",
            "--controller",
            "caniko/controller",
            "--revision",
            &"a".repeat(40),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--dispatch-ref"));
}

#[cfg(unix)]
#[test]
fn dispatch_checks_the_named_ref_and_passes_an_exact_runtime_precondition() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let root = tempfile::tempdir().unwrap();
    let request = root.path().join("request.json");
    fs::write(
        &request,
        serde_json::to_vec(&simit::review::contract::example()).unwrap(),
    )
    .unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1" = api ]; then
  case "$4" in
    repos/caniko/controller) printf '{"id":1,"full_name":"caniko/controller"}';;
    repos/caniko/controller/commits/reviewed-release) printf '{"sha":"%s","commit":{"tree":{"sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}}' "$TEST_RESOLVED_SHA";;
    *) exit 90;;
  esac
elif [ "$1" = workflow ] && [ "$2" = run ]; then
  printf '%s\n' "$@" > "$TEST_DISPATCH_ARGV"
else
  exit 91
fi
"#,
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let expected = "a".repeat(40);
    let argv = root.path().join("dispatch.argv");
    for (selector, resolved, accepted) in [
        ("reviewed-release", "c".repeat(40), false),
        (expected.as_str(), expected.clone(), false),
        ("--unsafe", expected.clone(), false),
        ("release?ref=main", expected.clone(), false),
        ("reviewed-release", expected.clone(), true),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_simit"))
            .args(["review", "dispatch"])
            .arg(&request)
            .args(["--controller", "caniko/controller", "--revision", &expected])
            .arg(format!("--dispatch-ref={selector}"))
            .env("PATH", &path)
            .env("TEST_RESOLVED_SHA", resolved)
            .env("TEST_DISPATCH_ARGV", &argv)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), accepted, "{output:?}");
        assert_eq!(
            argv.exists(),
            accepted,
            "dispatch must fail before submission"
        );
        if accepted {
            let lines = fs::read_to_string(&argv).unwrap();
            let args: Vec<_> = lines.lines().collect();
            assert!(args.windows(2).any(|a| a == ["--ref", "reviewed-release"]));
            assert!(
                args.windows(2)
                    .any(|a| { a == ["-f", &format!("controller_revision={expected}")] })
            );
            let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(receipt["revision"], expected);
            assert_eq!(receipt["dispatch_ref"], selector);
        }
    }
}

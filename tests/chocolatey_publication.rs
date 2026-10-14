#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use tempfile::TempDir;

mod common;

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn invoke(root: &Path, mode: &str) -> std::process::Output {
    common::simit()
        .current_dir(root)
        .args([
            "dist",
            "chocolatey",
            "bump",
            "--version",
            "0.1.0",
            "--package-dir",
            "pkg",
            "--archive",
            "x64=x64.zip",
            "--push",
            "--api-key-env",
            "PUBLICATION_FIXTURE_KEY",
        ])
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("test-bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("TEST_ROOT", root)
        .env("TEST_MODE", mode)
        .env("PUBLICATION_FIXTURE_KEY", "fixture-secret-value")
        .env(
            "TEST_LOOKUP_URL",
            if root.join("official-source").exists() {
                "https://community.chocolatey.org/api/v2/package/demo/0.1.0"
            } else {
                "https://packages.example.com/api/v2/package/demo/0.1.0"
            },
        )
        .output()
        .unwrap()
}

fn exercise(mode: &str, success: bool, pushes: usize) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\nauthors = [\"Example\"]\nlicense = \"MIT\"\ndescription = \"Demo\"\nhomepage = \"https://example.com/\"\n").unwrap();
    fs::write(root.join("simit.toml"), "[chocolatey]\nid = \"demo\"\ntitle = \"Demo\"\ndownload_repo = \"example/demo\"\n[chocolatey.push]\nsource = \"https://packages.example.com/api/v2/\"\n").unwrap();
    fs::write(root.join("x64.zip"), "release payload\n").unwrap();
    let bin = root.join("test-bin");
    fs::create_dir(&bin).unwrap();
    executable(
        &bin.join("choco"),
        r#"#!/bin/sh
set -eu
case "$1" in
 pack)
   test -z "${PUBLICATION_FIXTURE_KEY:-}"
   printf 'pack\n' >> "$TEST_ROOT/packs"
   test "$TEST_MODE" != pack-failure || exit 7
   while [ "$1" != --output-directory ]; do shift; done
   printf 'candidate package\n' > "$2/demo.0.1.0.nupkg" ;;
 push)
   printf 'push\n' >> "$TEST_ROOT/pushes"
   printf '%s\n' "$PUBLICATION_FIXTURE_KEY" >&2
   case "$TEST_MODE" in race|push-failure) exit 9 ;; esac ;;
 *) exit 99 ;;
esac
"#,
    );
    executable(
        &bin.join("curl"),
        r#"#!/bin/sh
set -eu
test -z "${PUBLICATION_FIXTURE_KEY:-}"
while [ "$1" != --output ]; do shift; done
file=$2
while [ "$1" != --url ]; do shift; done
test "$2" = "$TEST_LOOKUP_URL"
case "$TEST_MODE" in
 identical) printf 'candidate package\n' > "$file"; printf 200 ;;
 conflict) printf 'different package\n' > "$file"; printf 200 ;;
 lookup-failure) exit 6 ;;
 http-failure) printf 503 ;;
 race)
   if [ -f "$TEST_ROOT/pushes" ]; then printf 'candidate package\n' > "$file"; printf 200
   else printf 404; fi ;;
 *) printf 404 ;;
esac
"#,
    );
    let output = invoke(root, mode);
    assert_eq!(
        output.status.success(),
        success,
        "{mode}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-secret-value"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture-secret-value"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let expected_error = match mode {
        "conflict" => Some("conflicting package bytes"),
        "lookup-failure" => Some("registry lookup failed"),
        "http-failure" => Some("unexpected HTTP status"),
        "push-failure" => Some("choco push failed"),
        "pack-failure" => Some("choco pack failed"),
        _ => None,
    };
    if let Some(expected) = expected_error {
        assert!(stderr.contains(expected), "{mode}: {stderr}");
    }
    let actual_pushes = fs::read_to_string(root.join("pushes"))
        .unwrap_or_default()
        .lines()
        .count();
    assert_eq!(actual_pushes, pushes, "{mode}");
    temp
}

#[test]
fn absent_package_publishes_and_identical_package_resumes_without_push() {
    exercise("absent", true, 1);
    exercise("identical", true, 0);
}

#[test]
fn conflicting_or_unavailable_registry_evidence_refuses_publication() {
    exercise("conflict", false, 0);
    exercise("lookup-failure", false, 0);
    exercise("http-failure", false, 0);
}

#[test]
fn failed_push_only_resumes_after_exact_published_bytes_are_verified() {
    exercise("race", true, 1);
    exercise("push-failure", false, 1);
    exercise("pack-failure", false, 0);
}

fn retained_package(root: &Path) -> std::path::PathBuf {
    let candidate = fs::read_dir(root.join("pkg/.simit-candidates"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    candidate.join("demo.0.1.0.nupkg")
}

#[test]
fn retry_reuses_packed_bytes_and_rejects_a_tampered_candidate() {
    let temp = exercise("push-failure", false, 1);
    let root = temp.path();
    let output = invoke(root, "identical");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("already-current"));
    assert_eq!(fs::read_to_string(root.join("packs")).unwrap(), "pack\n");
    assert_eq!(fs::read_to_string(root.join("pushes")).unwrap(), "push\n");
    fs::write(retained_package(root), "changed bytes\n").unwrap();
    let output = invoke(root, "identical");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("candidate identity changed"));
    assert_eq!(fs::read_to_string(root.join("pushes")).unwrap(), "push\n");
}

#[test]
fn retry_refuses_missing_malformed_or_changed_candidate_receipts() {
    for alteration in ["missing", "malformed", "source", "package", "unknown"] {
        let temp = exercise("push-failure", false, 1);
        let root = temp.path();
        let receipt_path = retained_package(root).with_file_name("receipt.json");
        match alteration {
            "missing" => fs::remove_file(&receipt_path).unwrap(),
            "malformed" => fs::write(&receipt_path, "not JSON\n").unwrap(),
            _ => {
                let mut receipt: serde_json::Value =
                    serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
                let field = match alteration {
                    "source" => "source_sha256",
                    "package" => "package_sha256",
                    _ => "unknown_field",
                };
                receipt[field] = "changed".into();
                fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
            }
        }
        let output = invoke(root, "identical");
        assert!(!output.status.success(), "{alteration}");
        assert_eq!(fs::read_to_string(root.join("packs")).unwrap(), "pack\n");
        assert_eq!(fs::read_to_string(root.join("pushes")).unwrap(), "push\n");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture-secret-value"));
    }
}

#[test]
fn changed_release_payload_is_repacked_and_conflicting_publication_is_refused() {
    let temp = exercise("absent", true, 1);
    let root = temp.path();
    fs::write(root.join("x64.zip"), "new release payload\n").unwrap();
    let output = invoke(root, "conflict");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("conflicting package bytes"));
    assert_eq!(
        fs::read_to_string(root.join("packs")).unwrap(),
        "pack\npack\n"
    );
    assert_eq!(fs::read_to_string(root.join("pushes")).unwrap(), "push\n");
    assert_eq!(
        fs::read_dir(root.join("pkg/.simit-candidates"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn official_push_service_is_verified_through_its_public_package_feed() {
    let temp = exercise("absent", true, 1);
    let root = temp.path();
    fs::write(root.join("official-source"), "").unwrap();
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    fs::write(
        root.join("simit.toml"),
        config.replace(
            "https://packages.example.com/api/v2/",
            "https://push.chocolatey.org/",
        ),
    )
    .unwrap();
    let output = invoke(root, "identical");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("already-current"));
    assert_eq!(fs::read_to_string(root.join("pushes")).unwrap(), "push\n");
}

#[test]
fn generated_publisher_enters_the_project_environment_before_resolving_tools() {
    let temp = exercise("absent", true, 1);
    let root = temp.path();
    let config = fs::read_to_string(root.join("simit.toml"))
        .unwrap()
        .replace(
            "title = \"Demo\"",
            "title = \"Demo\"\nnix_tool = \"fixture#choco\"",
        );
    fs::write(root.join("simit.toml"), format!("{config}\n[ci]\nplatform = \"github\"\n[release.github]\nrepo = \"example/demo\"\n[release.artifacts]\nrunner = \"ubuntu-24.04\"\nsign = false\nbuild_commands = [\"true\"]\n")).unwrap();
    let output = common::simit()
        .current_dir(root)
        .args(["init", "release", "--print"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let yaml: serde_yaml::Value = serde_yaml::from_slice(&output.stdout).unwrap();
    let body = yaml["jobs"]
        .as_mapping()
        .unwrap()
        .values()
        .filter_map(|job| job["steps"].as_sequence())
        .flatten()
        .find(|step| step["name"].as_str() == Some("Publish Chocolatey package"))
        .unwrap()["run"]
        .as_str()
        .unwrap();
    executable(
        &root.join("test-bin/nix"),
        r#"#!/bin/sh
set -eu
case "$1" in
 develop)
   test "$2" = -c
   export TEST_PROJECT_ENV=present
   shift 2; exec "$@" ;;
 shell)
   test "$TEST_PROJECT_ENV" = present
   test "$2" = fixture#choco
   test "$3" = -c
   shift 3; exec "$@" ;;
 *) exit 99 ;;
esac
"#,
    );
    executable(
        &root.join("test-bin/simit"),
        "#!/bin/sh\nset -eu\ntest \"$TEST_PROJECT_ENV\" = present\ntest \"$CHOCOLATEY_API_KEY\" = fixture-key\nprintf '%s\\n' \"$@\" > publisher-argv\n",
    );
    fs::write(
        root.join("release-env"),
        "VERSION=0.1.0\nIS_PRERELEASE=false\n",
    )
    .unwrap();
    fs::create_dir(root.join("release")).unwrap();
    fs::write(
        root.join("release/demo-0.1.0-x86_64-windows.zip"),
        "payload\n",
    )
    .unwrap();
    let output = std::process::Command::new("bash")
        .current_dir(root)
        .args(["-c", body])
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("test-bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("CHOCOLATEY_API_KEY", "fixture-key")
        .env("CHOCO_PUSH_SOURCE", "https://packages.example.com/api/v2/")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let argv = fs::read_to_string(root.join("publisher-argv")).unwrap();
    assert!(argv.starts_with("dist\nchocolatey\nbump\n--version\n0.1.0\n"));
    assert!(argv.contains("--archive\nx64=release/demo-0.1.0-x86_64-windows.zip\n"));
    assert!(argv.ends_with("--api-key-env\nCHOCOLATEY_API_KEY\n"));
    assert!(!argv.contains("fixture-key"));
}

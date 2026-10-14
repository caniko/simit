#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use tempfile::TempDir;

mod common;

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn invoke(root: &Path, mode: &str) -> std::process::Output {
    invoke_with(root, mode, "0.1.0", false)
}

fn invoke_with(root: &Path, mode: &str, version: &str, pack_only: bool) -> std::process::Output {
    let mut command = common::simit();
    command.current_dir(root).args([
        "dist",
        "chocolatey",
        "bump",
        "--version",
        version,
        "--package-dir",
        "pkg",
        "--archive",
        "x64=x64.zip",
        "--api-key-env",
        "PUBLICATION_FIXTURE_KEY",
    ]);
    command.arg(if pack_only { "--pack" } else { "--push" });
    command
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
            format!(
                "{}/package/demo/{}",
                if root.join("official-source").exists() {
                    "https://community.chocolatey.org/api/v2"
                } else {
                    "https://packages.example.com/api/v2"
                },
                if version == "1.001.2" {
                    "1.1.2"
                } else {
                    version.split('+').next().unwrap()
                }
            ),
        );
    if root.join("expected-lookup").exists() {
        command.env(
            "TEST_LOOKUP_URL",
            fs::read_to_string(root.join("expected-lookup")).unwrap(),
        );
    }
    if pack_only {
        command.env_remove("PUBLICATION_FIXTURE_KEY");
    }
    command.output().unwrap()
}

#[test]
fn normalized_packed_filename_is_retained_and_reused() {
    let temp = exercise("absent", true, 1);
    let root = temp.path();
    executable(
        &root.join("test-bin/choco"),
        r#"#!/bin/sh
set -eu
case "$1" in
 pack)
   test -z "${PUBLICATION_FIXTURE_KEY:-}"
   printf 'pack\n' >> "$TEST_ROOT/packs"
   while [ "$1" != --output-directory ]; do shift; done
   printf 'candidate package\n' > "$2/demo.1.1.2.nupkg" ;;
 push) test "$(basename "$2")" = demo.1.1.2.nupkg; printf 'push\n' >> "$TEST_ROOT/pushes" ;;
 *) exit 99 ;;
esac
"#,
    );
    for mode in ["absent", "identical"] {
        let output = invoke_with(root, mode, "1.001.2", false);
        assert!(output.status.success(), "{output:?}");
    }
    assert_eq!(
        fs::read_to_string(root.join("packs")).unwrap(),
        "pack\npack\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("pushes")).unwrap(),
        "push\npush\n"
    );
}

#[test]
fn packed_filename_and_registry_lookup_omit_build_metadata() {
    for (version, filename) in [
        ("1.0.0+githash", "demo.1.0.0.nupkg"),
        ("1.0.0-alpha+githash", "demo.1.0.0-alpha.nupkg"),
    ] {
        let temp = exercise("absent", true, 1);
        let root = temp.path();
        executable(
            &root.join("test-bin/choco"),
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
 pack)
   test -z "${{PUBLICATION_FIXTURE_KEY:-}}"
   printf 'pack\n' >> "$TEST_ROOT/packs"
   while [ "$1" != --output-directory ]; do shift; done
   printf 'candidate package\n' > "$2/{filename}" ;;
 push) test "$(basename "$2")" = {filename}; printf 'push\n' >> "$TEST_ROOT/pushes" ;;
 *) exit 99 ;;
esac
"#
            ),
        );
        for mode in ["absent", "identical"] {
            let output = invoke_with(root, mode, version, false);
            assert!(output.status.success(), "{version}: {output:?}");
        }
        assert_eq!(
            fs::read_to_string(root.join("packs")).unwrap(),
            "pack\npack\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("pushes")).unwrap(),
            "push\npush\n"
        );
    }
}

fn v3_fixture() -> TempDir {
    let temp = exercise("absent", true, 1);
    let root = temp.path();
    let config = fs::read_to_string(root.join("simit.toml"))
        .unwrap()
        .replace(
            "https://packages.example.com/api/v2/",
            "https://packages.example.com/custom-feed",
        );
    fs::write(root.join("simit.toml"), config).unwrap();
    fs::write(root.join("service-index.json"), r#"{"version":"3.0.0","resources":[{"@type":"PackageBaseAddress/3.0.0","@id":"https://content.example.com/flat/"}]}"#).unwrap();
    fs::write(
        root.join("expected-lookup"),
        "https://content.example.com/flat/demo/0.1.0/demo.0.1.0.nupkg",
    )
    .unwrap();
    temp
}

#[test]
fn v3_service_discovery_verifies_exact_bytes_before_push_and_resume() {
    for mode in [
        "absent",
        "identical",
        "race",
        "conflict",
        "http-failure",
        "index-unauthorized",
        "index-unavailable",
    ] {
        let temp = v3_fixture();
        let root = temp.path();
        let output = invoke(root, mode);
        assert_eq!(
            output.status.success(),
            matches!(mode, "absent" | "identical" | "race"),
            "{mode}: {output:?}"
        );
        assert_eq!(fs::read_to_string(root.join("packs")).unwrap(), "pack\n");
        assert_eq!(
            fs::read_to_string(root.join("pushes")).unwrap(),
            if mode == "absent" {
                "push\npush\n"
            } else {
                "push\n"
            }
        );
    }
}

#[test]
fn v2_json_service_documents_keep_the_v2_package_route() {
    let temp = exercise("absent", true, 1);
    let root = temp.path();
    fs::write(root.join("v2-json"), r#"{"d":{"EntitySets":["Packages"]}}"#).unwrap();
    let output = invoke(root, "identical");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(root.join("pushes")).unwrap(), "push\n");
}

#[test]
fn v3_content_urls_lowercase_ids_and_normalized_prereleases() {
    let temp = v3_fixture();
    let root = temp.path();
    fs::remove_file(root.join("pkg/demo.nuspec")).unwrap();
    let config = fs::read_to_string(root.join("simit.toml"))
        .unwrap()
        .replace("id = \"demo\"", "id = \"Demo.Tools\"");
    fs::write(root.join("simit.toml"), config).unwrap();
    fs::write(
        root.join("expected-lookup"),
        "https://content.example.com/flat/demo.tools/1.0.0-alpha/demo.tools.1.0.0-alpha.nupkg",
    )
    .unwrap();
    executable(
        &root.join("test-bin/choco"),
        r#"#!/bin/sh
set -eu
test "$1" = pack
test -z "${PUBLICATION_FIXTURE_KEY:-}"
while [ "$1" != --output-directory ]; do shift; done
printf 'candidate package\n' > "$2/Demo.Tools.1.0.0-AlPhA.nupkg"
"#,
    );
    let output = invoke_with(root, "identical", "01.0.0.0-AlPhA+build", false);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(root.join("pushes")).unwrap(), "push\n");
}

#[test]
fn unusable_service_indexes_do_not_admit_a_missing_package_or_upload() {
    for index in [
        r#"{"version":"3.0.0","resources":[]}"#,
        r#"{"version":"2.0.0","resources":[]}"#,
        r#"{"version":"3.0.0","resources":[{"@type":"PackageBaseAddress/3.0.0","@id":"file:///private"}]}"#,
        r#"{"version":"3.0.0","resources":[{"@type":"PackageBaseAddress/3.0.0","@id":"https://user:secret@content.example.com/flat/"}]}"#,
        "{malformed",
    ] {
        let temp = v3_fixture();
        let root = temp.path();
        fs::write(root.join("service-index.json"), index).unwrap();
        let output = invoke(root, "absent");
        assert!(!output.status.success(), "{index}: {output:?}");
        assert_eq!(fs::read_to_string(root.join("pushes")).unwrap(), "push\n");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture-secret-value"));
    }
}

#[test]
fn packing_rejects_missing_ambiguous_or_wrong_package_identity() {
    for body in [
        "true",
        "printf payload > \"$2/wrong.2.0.0.nupkg\"",
        "touch \"$2/demo.2.0.0.nupkg\"",
        "printf payload > \"$2/demo.2.0.0.nupkg\"; printf payload > \"$2/extra.2.0.0.nupkg\"",
    ] {
        let temp = exercise("absent", true, 1);
        let root = temp.path();
        executable(
            &root.join("test-bin/choco"),
            &format!(
                "#!/bin/sh\nset -eu\ntest \"$1\" = pack\nwhile [ \"$1\" != --output-directory ]; do shift; done\n{body}\n"
            ),
        );
        let output = invoke_with(root, "absent", "2.0.0", true);
        assert!(!output.status.success(), "{body}: {output:?}");
        assert_eq!(fs::read_to_string(root.join("pushes")).unwrap(), "push\n");
        assert_eq!(
            fs::read_dir(root.join("pkg/.simit-candidates"))
                .unwrap()
                .count(),
            1
        );
    }
}

#[test]
fn restored_candidate_survives_fresh_workspace_without_credentials_or_repacking() {
    let temp = exercise("absent", true, 1);
    let root = temp.path();
    fs::rename(
        root.join("pkg/.simit-candidates"),
        root.join("retained-artifact"),
    )
    .unwrap();
    fs::remove_dir_all(root.join("pkg")).unwrap();
    fs::create_dir(root.join("pkg")).unwrap();
    fs::rename(
        root.join("retained-artifact"),
        root.join("pkg/.simit-candidates"),
    )
    .unwrap();
    let output = invoke_with(root, "pack-failure", "0.1.0", true);
    assert!(output.status.success(), "{output:?}");
    let output = invoke(root, "identical");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(root.join("packs")).unwrap(), "pack\n");
    assert_eq!(fs::read_to_string(root.join("pushes")).unwrap(), "push\n");
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
case "$2" in
 https://packages.example.com/api/v2|https://community.chocolatey.org/api/v2)
   if [ -f "$TEST_ROOT/v2-json" ]; then cat "$TEST_ROOT/v2-json" > "$file"
   else printf '<service />' > "$file"; fi
   printf 200; exit ;;
 https://packages.example.com/custom-feed)
   case "$TEST_MODE" in
     index-unauthorized) printf 401; exit ;;
     index-unavailable) printf 503; exit ;;
   esac
   cat "$TEST_ROOT/service-index.json" > "$file"; printf 200; exit ;;
esac
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
    let steps = yaml["jobs"]
        .as_mapping()
        .unwrap()
        .values()
        .filter_map(|job| job["steps"].as_sequence())
        .flatten()
        .collect::<Vec<_>>();
    let index = |name| steps.iter().position(|step| step["name"] == name).unwrap();
    let prepare = index("Prepare Chocolatey package");
    let restore = index("Restore exact Chocolatey candidate");
    let pack = index("Pack Chocolatey candidate without publication credentials");
    let save = index("Persist Chocolatey candidate before publication");
    let verify = index("Require persisted Chocolatey candidate");
    let publish = index("Publish Chocolatey package");
    assert!(
        prepare < restore && restore < pack && pack < save && save < verify && verify < publish
    );
    assert_eq!(steps[verify]["with"]["fail-on-cache-miss"], true);
    assert_eq!(steps[restore]["with"]["key"], steps[save]["with"]["key"]);
    assert_eq!(steps[restore]["with"]["key"], steps[verify]["with"]["key"]);
    assert!(
        steps[restore]["with"]["key"]
            .as_str()
            .unwrap()
            .contains("hashFiles(")
    );
    assert!(steps[pack]["env"].is_null());
    assert!(
        steps[pack]["run"]
            .as_str()
            .unwrap()
            .contains("--pack --api-key-env")
    );
    assert!(!steps[pack]["run"].as_str().unwrap().contains("--push"));
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
    let mut publisher = std::process::Command::new("bash");
    publisher
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
        .env_remove("CHOCO_CANDIDATE_CACHE_HIT");
    let missing_cache = publisher.output().unwrap();
    assert!(!missing_cache.status.success());
    assert!(
        String::from_utf8_lossy(&missing_cache.stderr).contains("persistence was not verified")
    );
    assert!(!root.join("publisher-argv").exists());
    let output = publisher
        .env("CHOCO_CANDIDATE_CACHE_HIT", "true")
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

#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

use tempfile::TempDir;

mod common;

fn helper(root: &Path) -> String {
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT\"\ndescription = \"Demo\"\nhomepage = \"https://example.com/\"\n").unwrap();
    fs::write(root.join("simit.toml"), "[ci]\nplatform = \"github\"\n[release.github]\nrepo = \"example/demo\"\n[release.artifacts]\nrunner = \"ubuntu-24.04\"\nsign = false\nbuild_commands = [\"true\"]\n[aur]\ndownload_repo = \"example/demo\"\n").unwrap();
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
        .find(|step| step["name"].as_str() == Some("Publish AUR packages"))
        .unwrap()["run"]
        .as_str()
        .unwrap();
    assert!(body.contains("nixpkgs#util-linux"));
    assert!(body.contains("print_srcinfo \"aur-${pkg}\""));
    body.split_once("print_srcinfo() (\n")
        .unwrap()
        .1
        .split_once("\n)\n")
        .map(|(body, _)| format!("print_srcinfo() (\n{body}\n)\n"))
        .unwrap()
}

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn exercise(root_runner: bool, fail: bool) {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let rendered = helper(root);
    let bin = root.join("test-bin");
    fs::create_dir(&bin).unwrap();
    executable(&bin.join("id"), "#!/bin/sh\nprintf '%s\\n' \"$TEST_UID\"\n");
    executable(&bin.join("stat"), "#!/bin/sh\nprintf '1000:1000\\n'\n");
    executable(
        &bin.join("chown"),
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$TEST_LOG\"\n",
    );
    executable(
        &bin.join("setpriv"),
        "#!/bin/sh\nset -eu\ntest -f PKGBUILD\ntest \"$1\" = --reuid=65534\ntest \"$2\" = --regid=65534\ntest \"$3\" = --clear-groups\nshift 3\nexport TEST_UID=65534\nexec \"$@\"\n",
    );
    executable(
        &bin.join("makepkg"),
        "#!/bin/sh\nset -eu\ntest \"$TEST_UID\" != 0\ntest \"$1\" = --config\ntest \"$2\" = \"$MAKEPKG_CONF\"\ntest \"$3\" = --printsrcinfo\nprintf 'generation\\n' >> \"$TEST_LOG\"\ncat PKGBUILD\nexit \"$TEST_FAIL\"\n",
    );
    fs::create_dir(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/PKGBUILD"), "pkgname=demo\n").unwrap();
    fs::write(root.join("pkg/.SRCINFO"), "previous\n").unwrap();
    let output = Command::new("bash")
        .current_dir(root)
        .args([
            "-c",
            &format!("set -euo pipefail\n{rendered}\nprint_srcinfo pkg"),
        ])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("MAKEPKG_CONF", root.join("makepkg.conf"))
        .env("TEST_LOG", root.join("calls"))
        .env("TEST_UID", if root_runner { "0" } else { "1000" })
        .env("TEST_FAIL", if fail { "23" } else { "0" })
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(if fail { 23 } else { 0 }),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("pkg/.SRCINFO")).unwrap(),
        if fail { "previous\n" } else { "pkgname=demo\n" }
    );
    assert!(!root.join("pkg/.SRCINFO.tmp").exists());
    let calls = fs::read_to_string(root.join("calls")).unwrap();
    assert_eq!(
        calls.lines().filter(|line| *line == "generation").count(),
        1
    );
    if root_runner {
        assert!(calls.starts_with("-R 65534:65534 pkg\n"));
        assert!(calls.ends_with("-R 1000:1000 pkg\n"));
    } else {
        assert_eq!(calls, "generation\n");
    }
}

#[test]
fn srcinfo_succeeds_for_root_and_nonroot_runners() {
    exercise(false, false);
    exercise(true, false);
}

#[test]
fn srcinfo_failure_restores_ownership_without_replacing_previous_metadata() {
    exercise(false, true);
    exercise(true, true);
}

#[cfg(target_os = "linux")]
#[test]
fn real_srcinfo_execution_preserves_owner_on_success_and_failure() {
    use std::os::unix::fs::MetadataExt;

    for (fail, mode) in [(false, 0o755), (true, 0o755), (false, 0o700), (true, 0o700)] {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::set_permissions(root, fs::Permissions::from_mode(mode)).unwrap();
        let rendered = helper(root);
        // The tool is public, like makepkg in the Nix store; the workspace
        // deliberately need not be traversable by the dropped UID.
        let tools = TempDir::new().unwrap();
        let bin = tools.path();
        fs::set_permissions(bin, fs::Permissions::from_mode(0o755)).unwrap();
        executable(
            &bin.join("makepkg"),
            "#!/bin/sh\nset -eu\ntest \"$(id -u)\" != 0\nprintf 'pkgname=demo\\n'\nexit \"$TEST_FAIL\"\n",
        );
        fs::create_dir(root.join("pkg")).unwrap();
        fs::write(root.join("pkg/PKGBUILD"), "pkgname=demo\n").unwrap();
        fs::write(root.join("pkg/.SRCINFO"), "previous\n").unwrap();
        let original = fs::metadata(root.join("pkg")).unwrap();
        let output = Command::new("bash")
            .current_dir(root)
            .args([
                "-c",
                &format!("set -euo pipefail\n{rendered}\nprint_srcinfo pkg"),
            ])
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("MAKEPKG_CONF", root.join("makepkg.conf"))
            .env("TEST_FAIL", if fail { "23" } else { "0" })
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(if fail { 23 } else { 0 }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for path in ["pkg", "pkg/PKGBUILD", "pkg/.SRCINFO"] {
            let metadata = fs::metadata(root.join(path)).unwrap();
            assert_eq!(
                (metadata.uid(), metadata.gid()),
                (original.uid(), original.gid())
            );
        }
        assert_eq!(
            fs::read_to_string(root.join("pkg/.SRCINFO")).unwrap(),
            if fail { "previous\n" } else { "pkgname=demo\n" }
        );
        assert!(!root.join("pkg/.SRCINFO.tmp").exists());
    }
}

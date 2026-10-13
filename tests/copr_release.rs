use std::{fs, path::Path, process::Command};

use tempfile::TempDir;

mod common;

fn checked(command: &mut Command) -> std::process::Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn project(srpm_dir: Option<&str>) -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT\"\ndescription = \"Demo\"\n").unwrap();
    let staging = srpm_dir
        .map(|path| format!("srpm_dir = {path:?}\n"))
        .unwrap_or_default();
    fs::write(root.join("simit.toml"), format!("[ci]\nplatform = \"github\"\n[release.github]\nrepo = \"example/demo\"\n[release.artifacts]\nrunner = \"ubuntu-24.04\"\nsign = false\nbuild_commands = [\"true\"]\n[copr]\ndownload_repo = \"example/demo\"\nproject = \"example/demo\"\nspec_path = \"dist/rpm/demo.spec\"\n{staging}")).unwrap();
    checked(common::simit().current_dir(root).args(["init", "copr"]));
    checked(Command::new("git").current_dir(root).args(["init"]));
    checked(Command::new("git").current_dir(root).args(["add", "."]));
    checked(
        Command::new("git")
            .current_dir(root)
            .args(["commit", "-m", "fixture"]),
    );
    dir
}

fn release_step(root: &Path, name: &str) -> String {
    let output = checked(
        common::simit()
            .current_dir(root)
            .args(["init", "release", "--print"]),
    );
    let yaml: serde_yaml::Value = serde_yaml::from_slice(&output.stdout).unwrap();
    yaml["jobs"]
        .as_mapping()
        .unwrap()
        .values()
        .filter_map(|job| job["steps"].as_sequence())
        .flatten()
        .find(|step| step["name"].as_str() == Some(name))
        .unwrap()["run"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[cfg(unix)]
fn executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
fn exercise(srpm_dir: Option<&str>) {
    let dir = project(srpm_dir);
    let root = dir.path();
    let bin = root.join("test-bin");
    fs::create_dir(&bin).unwrap();
    executable(
        &bin.join("nix"),
        "#!/bin/sh\nset -eu\ncase \"$1\" in\n shell) exec bash ;;\n run) printf '%s\\n' \"$@\" > published-argv ;;\n *) exit 99 ;;\nesac\n",
    );
    executable(
        &bin.join("cargo"),
        "#!/bin/sh\nset -eu\ntest \"$1\" = vendor\nmkdir -p \"$2\"\n",
    );
    executable(
        &bin.join("rpmbuild"),
        "#!/bin/sh\nset -eu\ntest -s demo-v0.1.0.tar.gz\nfor arg in \"$@\"; do\n case \"$arg\" in '_srcrpmdir '*) dir=${arg#_srcrpmdir } ;; esac\ndone\ntest -d \"$dir\"\nprintf 'srpm fixture\\n' > \"$dir/demo.src.rpm\"\n",
    );
    fs::write(
        root.join("release-env"),
        "VERSION=0.1.0\nIS_PRERELEASE=false\n",
    )
    .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let run = |name: &str| {
        Command::new("bash")
            .current_dir(root)
            .args(["-c", &release_step(root, name)])
            .env("PATH", &path)
            .env("HOME", root)
            .env("COPR_LOGIN", "fixture")
            .env("COPR_USERNAME", "fixture")
            .env("COPR_TOKEN", "fixture")
            .output()
            .unwrap()
    };
    let build = run("Build SRPM for COPR");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let expected = format!("{}/demo.src.rpm", srpm_dir.unwrap_or("srpms"));
    assert_eq!(
        fs::read_to_string(root.join(&expected)).unwrap(),
        "srpm fixture\n"
    );
    let publish = run("Push SRPM to COPR");
    assert!(
        publish.status.success(),
        "{}",
        String::from_utf8_lossy(&publish.stderr)
    );
    let published = fs::read_to_string(root.join("published-argv")).unwrap();
    assert_eq!(published.lines().last(), Some(expected.as_str()));
    assert!(
        fs::read_to_string(root.join("dist/rpm/demo.spec"))
            .unwrap()
            .contains("#/demo-v%{version}.tar.gz")
    );
    fs::remove_file(root.join("published-argv")).unwrap();
    let extra = root.join(srpm_dir.unwrap_or("srpms")).join("extra.src.rpm");
    fs::write(&extra, "unexpected srpm").unwrap();
    assert!(!run("Push SRPM to COPR").status.success());
    fs::remove_file(extra).unwrap();
    fs::write(root.join(&expected), "").unwrap();
    assert!(!run("Push SRPM to COPR").status.success());
    fs::remove_file(root.join(&expected)).unwrap();
    assert!(!run("Push SRPM to COPR").status.success());
    assert!(!root.join("published-argv").exists());
}

#[cfg(unix)]
#[test]
fn default_copr_archive_matches_spec_and_published_srpm() {
    exercise(None);
}

#[cfg(unix)]
#[test]
fn custom_copr_staging_is_shared_by_build_and_publication() {
    exercise(Some("target/release-artifacts/srpms"));
}

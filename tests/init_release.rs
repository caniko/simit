use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn init_package(name: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
license = "MIT"
description = "Demo command line"
homepage = "https://example.com/{name}"
"#
        ),
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        root.join("simit.toml"),
        format!(
            r#"[release.codeberg]
repo = "example/{name}"

[release.artifacts]
runner = "atlas"
build_commands = ["mkdir -p release", "printf artifact > release/{name}.txt"]
sign = false

[aur]
download_repo = "example/{name}"

[copr]
download_repo = "example/{name}"
project = "example/{name}"

[apt]
repo_url = "ssh://git@codeberg.org/example/{name}-apt.git"
"#
        ),
    )
    .unwrap();

    temp
}

fn init_package_without_release_runner(name: &str) -> TempDir {
    let temp = init_package(name);
    let simit_toml = read(&temp.path().join("simit.toml"));
    fs::write(
        temp.path().join("simit.toml"),
        simit_toml.replace("runner = \"atlas\"\n", ""),
    )
    .unwrap();
    temp
}

fn init_package_with_release_runner(name: &str, runner: &str) -> TempDir {
    let temp = init_package(name);
    let simit_toml = read(&temp.path().join("simit.toml"));
    fs::write(
        temp.path().join("simit.toml"),
        simit_toml.replace("runner = \"atlas\"\n", &format!("runner = \"{runner}\"\n")),
    )
    .unwrap();
    temp
}

fn write_split_runner_config(root: &Path) {
    let config_dir = root.join(".xdg/simit");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"[ci.runners.forgejo_linux]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo"]
trusted = false

[ci.runners.forgejo_nix_trusted]
platform = "forgejo"
labels = ["atlas-nix-trusted"]
os = "linux"
arch = "x86_64"
runtimes = ["nix"]
trusted = true

[ci.defaults.forgejo]
cargo = "forgejo_linux"
nix = "forgejo_nix_trusted"
release = "forgejo_linux"
"#,
    )
    .unwrap();
}

fn simit_with_split_runner_config(root: &Path) -> Command {
    write_split_runner_config(root);
    let mut command = simit();
    command.env("XDG_CONFIG_HOME", root.join(".xdg"));
    command
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

const BUNDLE_RELEASE: &str = r#"
[release.github]
repo = "example/demo"
[release.artifacts]
runner = "ubuntu-24.04"
prebuild_binaries = true
[release.artifacts.system_runners]
x86_64-linux = "ubuntu-24.04"
"#;

const CARGO_RELEASE: &str = r#"
[release]
tag_prefix = "v"
[release.github]
repo = "example/demo"
[release.artifacts]
runner = "ubuntu-24.04"
checksum_globs = ["*.deb"]
build_commands = ["rm -rf release", "mkdir release"]

[[release.artifacts.cargo]]
id = "linux-amd64"
runner = "ubuntu-24.04"
target = "x86_64-unknown-linux-gnu"
toolchain = "1.95.0"
packages = ["demo", "demo-ui"]
binaries = ["demo", "demo-ui"]
archive = "demo-{version}-x86_64-linux.tar.gz"
setup = "sudo apt-get update && sudo apt-get install -y libwayland-dev"

[[release.artifacts.cargo]]
id = "linux-arm64"
runner = "ubuntu-24.04-arm"
target = "aarch64-unknown-linux-gnu"
toolchain = "nightly-2026-08-01"
binaries = ["demo", "demo-ui"]
archive = "demo-{version}-aarch64-linux.tar.gz"

[[release.artifacts.cargo]]
id = "macos-arm64"
runner = "macos-15"
target = "aarch64-apple-darwin"
toolchain = "1.95.0"
binaries = ["demo"]
archive = "demo-{version}-macos.zip"
prefix = "demo-{version}"

[[release.artifacts.cargo]]
id = "windows-amd64"
runner = "windows-2022"
target = "x86_64-pc-windows-msvc"
toolchain = "1.95.0"
binaries = ["demo"]
archive = "demo-{version}-windows.zip"
"#;

fn render_release(config: &str) -> String {
    let project = init_package("demo");
    fs::write(project.path().join("simit.toml"), config).unwrap();
    let output = simit()
        .current_dir(project.path())
        .args(["init", "release", "--print"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn actionlint(workflow: &str) {
    let project = TempDir::new().unwrap();
    let path = project.path().join("release.yml");
    fs::write(&path, workflow).unwrap();
    let output = Command::new("actionlint").arg(path).output().unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn nix_bundle_release_workflow_is_byte_stable() {
    use sha2::{Digest, Sha256};

    let workflow = render_release(BUNDLE_RELEASE);
    assert_eq!(
        format!("{:x}", Sha256::digest(&workflow)),
        "713bb719c6facfb1838748a9ccacaf373324d127815703d59d050c563e935677"
    );
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let validate = serde_yaml::to_string(&yaml["jobs"]["validate"]).unwrap();
    let bundles = serde_yaml::to_string(&yaml["jobs"]["bundles"]).unwrap();
    assert!(!validate.contains("install-nix-action") && !validate.contains("nix "));
    assert!(bundles.contains("install-nix-action"));
    actionlint(&workflow);
}

#[test]
fn native_cargo_release_jobs_join_existing_publication() {
    let workflow = render_release(CARGO_RELEASE);
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let jobs = &yaml["jobs"];
    for (id, runner) in [
        ("linux-amd64", "ubuntu-24.04"),
        ("linux-arm64", "ubuntu-24.04-arm"),
        ("macos-arm64", "macos-15"),
        ("windows-amd64", "windows-2022"),
    ] {
        let job = &jobs[format!("cargo-{id}")];
        assert_eq!(job["runs-on"], runner);
        assert_eq!(job["needs"], "validate");
        let rendered = serde_yaml::to_string(job).unwrap();
        assert!(rendered.contains("cargo +"));
        assert!(rendered.contains("build --release --locked"));
        assert!(!rendered.contains("install-nix-action") && !rendered.contains("nix "));
        assert_eq!(rendered.contains("readelf"), id.starts_with("linux"));
        assert_eq!(rendered.contains("/nix/store"), id.starts_with("linux"));
    }
    assert!(jobs.get("bundles").is_none());
    let mac = serde_yaml::to_string(&jobs["cargo-macos-arm64"]).unwrap();
    assert!(mac.contains("zip -q") && mac.contains(r#"prefix="demo-${VERSION}/""#));
    let windows = serde_yaml::to_string(&jobs["cargo-windows-amd64"]).unwrap();
    assert!(windows.contains("tar.exe") && windows.contains("'demo.exe'"));
    let validate = serde_yaml::to_string(&jobs["validate"]).unwrap();
    for check in [
        "keys/maintainers.gpg",
        "gpg --batch --import",
        "git verify-tag",
        "git cat-file tag",
        "immutable event checkout",
        "grep -Eq",
        "CHANGELOG.md missing section",
    ] {
        assert!(validate.contains(check), "{validate}");
    }
    for nix in ["install-nix-action", "nix-setup", "nix ", "NIX_CONFIG"] {
        assert!(!validate.contains(nix), "{validate}");
    }
    assert_eq!(
        jobs["validate"]["steps"][0]["with"]["ref"],
        "${{ github.sha }}"
    );
    let release = serde_yaml::to_string(&jobs["release"]).unwrap();
    assert!(release.contains("cargo-linux-amd64") && release.contains("cargo-linux-arm64"));
    let local_build = release.find("rm -rf release").unwrap();
    let cargo_collect = release.find("Collect Cargo release archives").unwrap();
    let checksums = release.find("Generate release checksums").unwrap();
    assert!(local_build < cargo_collect && cargo_collect < checksums);
    assert!(release.contains(r#"files+=("demo-${VERSION}-x86_64-linux.tar.gz")"#));
    assert!(release.contains(r#"files+=("release/demo-${VERSION}-aarch64-linux.tar.gz")"#));
    assert!(release.contains("minisign -S") && release.contains("Publish GitHub release"));
    actionlint(&workflow);
}

#[test]
fn cargo_archive_collection_follows_repack_but_precedes_signpath() {
    let osslsigncode = render_release(&format!(
        "{CARGO_RELEASE}\n[release.windows_signing]\n\
         binaries = ['demo']\nsign_name = 'demo'\nsign_url = 'https://example.com'\n\
         tar_archive = 'demo-{{version}}-windows.tar.gz'\nzip_archive = 'demo-{{version}}-windows.zip'\n"
    ));
    let repack = osslsigncode
        .find("Sign Windows Authenticode artifacts")
        .unwrap();
    let collect = osslsigncode.find("Collect Cargo release archives").unwrap();
    assert!(repack < collect, "collect must reject the existing ZIP");

    let signpath = render_release(&format!(
        "{CARGO_RELEASE}\n[release.windows_signing]\nprovider = 'signpath'\n\
         organization_id = '00000000-0000-0000-0000-000000000000'\nproject_slug = 'demo'\n\
         signing_policy_slug = 'release'\narchives = ['demo-{{version}}-windows.zip']\npe_files = ['demo.exe']\n"
    ));
    let collect = signpath.find("Collect Cargo release archives").unwrap();
    let sign = signpath
        .find("Check SignPath signing availability")
        .unwrap();
    assert!(collect < sign, "SignPath requires the collected archive");
}

#[test]
fn cargo_release_coexists_with_nix_bundles_and_version_check() {
    let config = CARGO_RELEASE.replace(
        "[release.artifacts]\n",
        "[release.artifacts]\nprebuild_binaries = true\nversion_attr = 'demo'\nvalidation_nix = false\nsystem_runners = { x86_64-linux = 'ubuntu-24.04' }\n",
    );
    let workflow = render_release(&config);
    assert!(workflow.contains("needs: [validate, bundles, cargo-linux-amd64, cargo-linux-arm64,"));
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let validate = serde_yaml::to_string(&yaml["jobs"]["validate"]).unwrap();
    assert!(validate.contains("install-nix-action"));
    assert!(validate.contains("nix eval --raw \"$tag_worktree#demo.version\""));
    assert!(workflow.contains("pattern: release-*"));
    assert!(workflow.contains("name: simit-cargo-linux-amd64"));
    actionlint(&workflow);
}

#[test]
fn release_validation_only_provisions_nix_for_gates_that_need_it() {
    for (run, override_config, needs_nix) in [
        ("echo 'not a nix command'", "", false),
        ("nix run .#check", "", true),
        ("./check-with-nix", "validation_nix = true\n", true),
    ] {
        let release = CARGO_RELEASE.replace(
            "[release.artifacts]\n",
            &format!("[release.artifacts]\n{override_config}"),
        );
        let workflow = render_release(&format!(
            "[ci]\nrequired_gates = [{{id='integration',run=\"{run}\"}}]\n{release}"
        ));
        let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
        let validate = serde_yaml::to_string(&yaml["jobs"]["validate"]).unwrap();
        assert!(validate.contains("Required gate integration"));
        assert_eq!(validate.contains("install-nix-action"), needs_nix);
        assert!(!validate.contains("nix eval"));
        actionlint(&workflow);
    }
}

#[test]
fn forgejo_cargo_release_uses_forgejo_artifact_transport() {
    let workflow = render_release(
        &CARGO_RELEASE
            .replace("[release.github]", "[release.codeberg]")
            .replace(
                "build_commands = [\"rm -rf release\", \"mkdir release\"]\n",
                "",
            ),
    );
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let build = serde_yaml::to_string(&yaml["jobs"]["cargo-linux-amd64"]).unwrap();
    let publish = serde_yaml::to_string(&yaml["jobs"]["release"]).unwrap();
    assert!(build.contains("https://code.forgejo.org/actions/upload-artifact@"));
    assert!(publish.contains("https://code.forgejo.org/actions/download-artifact@"));
    assert!(publish.contains("Publish Forgejo release"));
    assert!(!publish.contains("has no Nix bundles or build_commands"));
    assert!(!build.contains("install-nix-action"));
}

#[cfg(target_os = "linux")]
#[test]
fn cargo_release_packages_elf_and_rejects_nix_linkage() {
    use std::io::Read;

    fn script<'a>(job: &'a serde_yaml::Value, name: &str) -> &'a str {
        job["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["name"] == name)
            .unwrap()["run"]
            .as_str()
            .unwrap()
    }

    let workflow = render_release(CARGO_RELEASE);
    let yaml: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    let jobs = &yaml["jobs"];
    let build = script(
        &jobs["cargo-linux-amd64"],
        "Build and archive Cargo binaries",
    );
    let collect = script(&jobs["release"], "Collect Cargo release archives");
    let checksums = script(&jobs["release"], "Generate release checksums");
    let project = TempDir::new().unwrap();
    let root = project.path();
    fs::write(root.join("elf.c"), "int main(void) { return 0; }\n").unwrap();
    let output = Command::new("cc")
        .current_dir(root)
        .args(["-fno-stack-protector", "-c", "elf.c", "-o", "elf.o"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let bash = |script: &str| {
        Command::new("bash")
            .current_dir(root)
            .env("RUNNER_TEMP", root)
            .env("GITHUB_OUTPUT", root.join("outputs"))
            .env("GITHUB_REF_NAME", "v1.2.3-rc.4")
            .args(["-c", script])
            .output()
            .unwrap()
    };
    let stubs = r#"
rustup() { :; }
rustc() { printf 'host: x86_64-unknown-linux-gnu\n'; }
cargo() {
  while [ "$1" != --target-dir ]; do shift; done
  mkdir -p "$2/x86_64-unknown-linux-gnu/release"
  for name in demo demo-ui; do cp -p elf "$2/x86_64-unknown-linux-gnu/release/$name"; done
}
"#;
    let run_build = || bash(&format!("{stubs}\n{build}"));
    // Under Nix (e.g. sandboxed Crane builds), `ld` on PATH is bintools-wrapper,
    // which rejects synthetic `/lib/ld-linux.so.2` paths and injects `/nix/store`
    // RUNPATH entries. Use the unwrapped linker when `NIX_BINTOOLS` is set.
    let linker = std::env::var_os("NIX_BINTOOLS")
        .and_then(|dir| fs::read_to_string(Path::new(&dir).join("nix-support/orig-bintools")).ok())
        .map(|orig| std::path::PathBuf::from(orig.trim()).join("bin/ld"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| std::path::PathBuf::from("ld"));
    let output = Command::new(&linker)
        .current_dir(root)
        .args([
            "-shared",
            "-soname",
            "/nix/store/fake/lib/libfixture.so",
            "-o",
            "libfixture.so",
            "elf.o",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    for (interp, linker_args, acceptable) in [
        ("/lib/ld-linux.so.2", vec![], true),
        ("/nix/store/fake/ld.so", vec![], false),
        (
            "/lib/ld-linux.so.2",
            vec!["-rpath", "/nix/store/fake/lib"],
            false,
        ),
        (
            "/lib/ld-linux.so.2",
            vec!["--disable-new-dtags", "-rpath", "/nix/store/fake/lib"],
            false,
        ),
        (
            "/lib/ld-linux.so.2",
            vec!["--no-as-needed", "./libfixture.so"],
            false,
        ),
    ] {
        let output = Command::new(&linker)
            .current_dir(root)
            .args([
                "-pie",
                "--entry",
                "main",
                "--dynamic-linker",
                interp,
                "elf.o",
                "-o",
                "elf",
            ])
            .args(linker_args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let output = run_build();
        assert_eq!(output.status.success(), acceptable, "{output:?}");
        if !acceptable {
            assert!(String::from_utf8_lossy(&output.stderr).contains("::error::demo needs"));
        }
    }
    let outputs = read(&root.join("outputs"));
    let archive = root.join(outputs.trim().strip_prefix("path=").unwrap());
    assert_eq!(
        archive.file_name().unwrap(),
        "demo-1.2.3-rc.4-x86_64-linux.tar.gz"
    );
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(
        fs::File::open(&archive).unwrap(),
    ));
    let entries: Vec<_> = tar
        .entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            assert_ne!(entry.header().mode().unwrap() & 0o111, 0);
            let mut magic = [0; 4];
            entry.read_exact(&mut magic).unwrap();
            assert_eq!(&magic, b"\x7fELF");
            path
        })
        .collect();
    assert_eq!(entries, [Path::new("demo"), Path::new("demo-ui")]);

    // Simulate the action service's handoff, then execute the actual release scripts.
    fs::write(root.join("release-env"), "VERSION=1.2.3-rc.4\n").unwrap();
    for (id, ext) in [
        ("linux-amd64", "x86_64-linux.tar.gz"),
        ("linux-arm64", "aarch64-linux.tar.gz"),
        ("macos-arm64", "macos.zip"),
        ("windows-amd64", "windows.zip"),
    ] {
        let dir = root.join("simit-cargo-release").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::copy(&archive, dir.join(format!("demo-1.2.3-rc.4-{ext}"))).unwrap();
    }
    let output = bash(collect);
    assert!(output.status.success(), "{output:?}");
    let output = bash(checksums);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        read(&root.join("release/SHA256SUMS.txt")).lines().count(),
        4
    );
    let output = bash(collect);
    assert!(
        !output.status.success() && String::from_utf8_lossy(&output.stderr).contains("collision")
    );
}

#[test]
fn scoop_secret_follows_the_bucket_not_the_release_host() {
    for (host, configured, expected) in [
        ("github.com", "CODEBERG_TOKEN", "SCOOP_BUCKET_TOKEN"),
        ("codeberg.org", "CODEBERG_TOKEN", "CODEBERG_TOKEN"),
        ("github.com", "CUSTOM_BUCKET_PAT", "CUSTOM_BUCKET_PAT"),
    ] {
        let workflow = render_release(&format!(
            "{BUNDLE_RELEASE}\n[release.publish.channels]\nscoop = \"required\"\n[scoop]\nbucket_url = \"https://{host}/example/scoop-demo.git\"\nbucket_token_secret = \"{configured}\"\ndownload_repo = \"example/demo\"\n"
        ));
        assert!(workflow.contains(&format!(
            "SCOOP_BUCKET_TOKEN: ${{{{ secrets.{expected} }}}}"
        )));
        assert!(workflow.contains(&format!(
            "require_credential 'global/user secret' '{expected}'"
        )));
        assert_eq!(
            workflow.contains("secrets.CODEBERG_TOKEN"),
            expected == "CODEBERG_TOKEN"
        );
        actionlint(&workflow);
    }
}

#[test]
fn bootstraps_release_workflow_for_enabled_channels() {
    let project = init_package("demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "release"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = read(&project.path().join(".forgejo/workflows/release.yml"));
    assert!(workflow.contains("Publish Forgejo release"));
    assert!(workflow.contains("FORGEJO_TOKEN: ${{ secrets.codeberg_token }}"));
    assert!(workflow.contains("add_matches 'release/SHA256SUMS.txt'"));
    assert!(workflow.contains("add_matches 'release/*.tar.gz'"));
    assert!(!workflow.contains("files=(release/*)"));
    assert!(workflow.contains("done < <(printf '%s\\n' \"${files[@]}\" | LC_ALL=C sort -u)"));
    assert!(workflow.contains("Publish AUR packages"));
    assert!(workflow.contains("Push SRPM to COPR"));
    assert!(workflow.contains("Publish APT repository"));
    assert!(workflow.contains("mkdir -p release"));
    assert!(workflow.contains("printf artifact > release/demo.txt"));
    assert!(workflow.contains("  workflow_dispatch:\n\n"));
    assert!(!workflow.contains("      version:\n"));
    assert!(!workflow.contains("inputs.version"));
    assert!(workflow.contains("git worktree add --detach \"$tag_worktree\" \"$validated_sha\""));
    assert!(!workflow.contains("git worktree add --detach \"$tag_worktree\" \"$VERSION\""));
    let verify = workflow.find("git verify-tag \"$TAG\"").unwrap();
    let checkout = workflow
        .find("git worktree add --detach \"$tag_worktree\" \"$validated_sha\"")
        .unwrap();
    assert!(
        verify < checkout,
        "tag signature must be verified before evaluating its checkout"
    );
    assert!(workflow.contains("git checkout --detach \"$validated_sha\""));
    assert!(workflow.contains(
        "uses: https://github.com/cachix/install-nix-action@13d8dd58da0234aa297dedd986986ccb8e7f3e24 # v31.11.1"
    ));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Generated release workflow"));
    assert!(stdout.contains("git add .forgejo/workflows/release.yml"));
    assert!(stdout.contains("configure the secrets listed at the top of the workflow"));
}

#[test]
fn bootstraps_github_release_workflow_with_native_permissions_and_uploads() {
    let project = init_package("github-demo");
    let config_path = project.path().join("simit.toml");
    let config = read(&config_path)
        .replace(
            "[release.codeberg]\nrepo = \"example/github-demo\"",
            "[release.github]\nrepo = \"example/github-demo\"",
        )
        .replace(
            "[release.artifacts]",
            "[ci]\nplatform = \"github\"\n\n[release.artifacts]",
        )
        .replace(
            "[aur]",
            "[homebrew]\ntap_url = \"https://example.com/homebrew-github-demo.git\"\ndescription = \"Demo\"\nhomepage = \"https://example.com\"\nlicense = \"MIT\"\ndownload_repo = \"example/github-demo\"\n\n[aur]",
        );
    fs::write(&config_path, config).unwrap();

    let output = simit()
        .current_dir(project.path())
        .args(["init", "release", "--platform", "github"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = read(&project.path().join(".github/workflows/release.yml"));
    assert!(workflow.contains("Publish GitHub release"));
    assert!(workflow.contains("\"$GITHUB_API/repos/$GITHUB_REPO/releases/assets/$asset_id\""));
    assert!(
        !workflow
            .contains("\"$GITHUB_API/repos/$GITHUB_REPO/releases/$release_id/assets/$asset_id\"")
    );
    assert!(workflow.contains("GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}"));
    assert!(workflow.contains("contents: write"));
    assert!(workflow.contains("id-token: write"));
    assert!(workflow.contains("upload_url=$(jq -r '.upload_url' release.json"));
    assert!(workflow.contains("Authorization: Bearer $GITHUB_TOKEN"));
    assert!(!workflow.contains("enable-openid-connect: true"));
    assert!(!workflow.contains("Publish Forgejo release"));
    assert!(workflow.contains("https://github.com/example/github-demo"));
    assert!(!workflow.contains("https://codeberg.org/example/github-demo"));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("git add .github/workflows/release.yml")
    );

    let sha = "0123456789abcdef0123456789abcdef01234567";
    let config = read(&config_path);
    fs::write(
        config_path,
        config.replacen(
            "[ci]",
            &format!("[ci]\nnix_setup_action_ref = \"{sha}\""),
            1,
        ),
    )
    .unwrap();
    let output = simit()
        .current_dir(project.path())
        .args(["init", "release", "--platform", "github"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let direct = "cachix/install-nix-action@13d8dd58da0234aa297dedd986986ccb8e7f3e24 # v31.11.1";
    assert!(workflow.contains(direct));
    assert_eq!(
        read(&project.path().join(".github/workflows/release.yml")),
        workflow.replace(
            direct,
            &format!("caniko/simit/.github/actions/nix-setup@{sha}")
        ),
    );
}

#[test]
fn github_release_depends_on_prebuild_and_forwards_attic_secret() {
    let project = init_package("github-native-demo");
    fs::write(project.path().join("flake.nix"), "{}\n").unwrap();
    let config_path = project.path().join("simit.toml");
    let config = read(&config_path)
        .replace(
            "[release.codeberg]\nrepo = \"example/github-native-demo\"",
            "[release.github]\nrepo = \"example/github-native-demo\"",
        )
        .replace(
            "[release.artifacts]",
            "[prebuild]\nrelease_archives = true\npublish_attic = true\nattic_app = \".#push-flake-inputs\"\n\n[ci]\nprovider = \"actions\"\nplatform = \"github\"\nruntime = \"nix\"\nnix_builds = [\".#default\"]\n\n[ci.nix_system_runners]\n\"x86_64-linux\" = \"ubuntu-24.04\"\n\n[release.artifacts]",
        )
        .replace(
            "runner = \"atlas\"\n",
            "runner = \"ubuntu-24.04\"\nprebuild_binaries = true\nversion_attr = \"demo\"\n",
        )
        .replace(
            "[aur]",
            "[release.attic]\ncache = \"demo\"\nurl = \"https://attic.example\"\ntoken_name = \"demo\"\ntoken_secret = \"ATTIC_TOKEN\"\nresult_links = [\"result\"]\n\n[aur]",
        );
    fs::write(
        config_path,
        format!("[release]\ntag_prefix = 'v'\n{config}"),
    )
    .unwrap();

    let output = simit()
        .current_dir(project.path())
        .args(["init", "release", "--platform", "github"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let workflow = read(&project.path().join(".github/workflows/release.yml"));
    assert!(
        workflow
            .contains("# - ATTIC_TOKEN: repository secret for native prebuild Attic publication.")
    );
    assert!(!workflow.contains("# Runner credential: $ATTIC_TOKENS_DIR/demo"));
    assert!(
        workflow.contains(
            "prebuild:\n    needs: validate\n    uses: ./.github/workflows/prebuild.yaml"
        )
    );
    assert!(workflow.contains("with:\n      release: true"));
    assert!(workflow.contains("attic_token: ${{ secrets.ATTIC_TOKEN }}"));
    assert!(workflow.contains("release:\n    needs: [validate, prebuild]"));
    let parsed: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    assert_eq!(
        parsed["jobs"]["validate"]["permissions"]["contents"].as_str(),
        Some("read")
    );
    for job in ["validate", "release"] {
        let steps = parsed["jobs"][job]["steps"].as_sequence().unwrap();
        assert_eq!(steps[0]["with"]["ref"].as_str(), Some("${{ github.sha }}"));
        assert_eq!(
            steps[0]["with"]["persist-credentials"].as_bool(),
            Some(false)
        );
        let guard = steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Validate tag"))
            .unwrap()["run"]
            .as_str()
            .unwrap();
        assert!(guard.contains("Signed tag commit does not match the immutable event checkout"));
        assert!(guard.find("git verify-tag").unwrap() < guard.find("nix eval").unwrap());
        assert!(guard.contains("VERSION=\"${TAG#v}\""));
    }
    let output = simit()
        .current_dir(project.path())
        .args(["init", "ci", "--prebuild-only"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let prebuild = read(&project.path().join(".github/workflows/prebuild.yaml"));
    let parsed: serde_yaml::Value = serde_yaml::from_str(&prebuild).unwrap();
    let steps = parsed["jobs"]["build"]["steps"]
        .as_sequence()
        .expect("reusable prebuild must expose the build matrix job");
    let validation = steps
        .iter()
        .position(|step| step["name"].as_str() == Some("Validate tag"))
        .unwrap();
    assert_eq!(
        steps[validation]["if"].as_str(),
        Some("${{ inputs.release }}")
    );
    assert!(
        steps[validation]["run"]
            .as_str()
            .unwrap()
            .contains("VERSION=\"${TAG#v}\"")
    );
    assert!(steps[..=validation].iter().all(|step| {
        !serde_yaml::to_string(&step["env"])
            .unwrap()
            .contains("secrets.")
    }));
    assert!(workflow.contains("uses: actions/download-artifact@"));
    assert!(workflow.contains("pattern: release-*"));
    assert!(!workflow.contains("nix build '.#release-bundle'"));
    assert!(!workflow.contains("Push Nix closures to Attic"));
}

#[test]
fn release_attic_config_quotes_default_server() {
    let project = init_package("attic-demo");
    let config_path = project.path().join("simit.toml");
    let config = read(&config_path).replace(
        "[aur]",
        "[release.attic]\ncache = \"demo\"\nurl = \"https://attic.example\"\ntoken_name = \"demo\"\nresult_links = [\"result\"]\n\n[aur]",
    );
    fs::write(config_path, config).unwrap();

    let status = simit()
        .current_dir(project.path())
        .args(["init", "release"])
        .status()
        .unwrap();
    assert!(status.success());
    let workflow = read(&project.path().join(".forgejo/workflows/release.yml"));
    assert!(workflow.contains("default-server = \"demo\""));
    assert!(workflow.contains("test \"$(stat -c '%a' \"$attic_config\")\" = 600"));
    assert!(workflow.contains("push --stdin --no-closure --ignore-upstream-cache-filter demo"));
    assert!(!workflow.contains("attic login"));
}

#[test]
fn release_attic_push_retries_only_the_push_command() {
    use std::os::unix::fs::PermissionsExt;

    let project = init_package("attic-retry-demo");
    let config_path = project.path().join("simit.toml");
    let config = read(&config_path).replace(
        "[aur]",
        "[release.attic]\ncache = \"demo\"\nurl = \"https://attic.example\"\ntoken_name = \"attic-retry-demo\"\nresult_links = [\"result\"]\n\n[aur]",
    );
    fs::write(config_path, config).unwrap();
    let status = simit()
        .current_dir(project.path())
        .args(["init", "release"])
        .status()
        .unwrap();
    assert!(status.success());
    let workflow: serde_yaml::Value = serde_yaml::from_str(&read(
        &project.path().join(".forgejo/workflows/release.yml"),
    ))
    .unwrap();
    let script = workflow["jobs"]
        .as_mapping()
        .unwrap()
        .values()
        .find_map(|job| {
            job["steps"].as_sequence()?.iter().find_map(|step| {
                (step["name"].as_str() == Some("Push Nix closures to Attic"))
                    .then(|| step["run"].as_str().unwrap().to_owned())
            })
        })
        .expect("release renders the Attic push step");

    // Stubs: `nix` logs every call, and the Attic client fails until the
    // attempt count reaches `SUCCEED_ON`. `sleep` is stubbed out.
    let root = project.path();
    let bin = root.join("stub-bin");
    let attic = root.join("stub-attic");
    let tokens = root.join("tokens");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(attic.join("bin")).unwrap();
    fs::create_dir_all(&tokens).unwrap();
    fs::write(tokens.join("attic-retry-demo"), "token\n").unwrap();
    for (name, body) in [
        (
            "nix",
            "#!/bin/sh\necho \"$@\" >> \"$NIX_CALLS\"\ncase \"$1\" in\n  build) echo \"$STUB_ATTIC\" ;;\n  path-info) : ;;\n  *) exit 2 ;;\nesac\n",
        ),
        ("sleep", "#!/bin/sh\n"),
    ] {
        let path = bin.join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let attic_bin = attic.join("bin/attic");
    fs::write(
        &attic_bin,
        "#!/bin/sh\necho \"$@\" >> \"$ATTIC_CALLS\"\n[ \"$(wc -l < \"$ATTIC_CALLS\")\" -ge \"$SUCCEED_ON\" ]\n",
    )
    .unwrap();
    fs::set_permissions(&attic_bin, fs::Permissions::from_mode(0o755)).unwrap();

    let push = "push --stdin --no-closure --ignore-upstream-cache-filter demo\n";
    for (succeed_on, success, attempts) in [(2, true, 2), (9, false, 3)] {
        let attic_calls = root.join(format!("attic-calls-{succeed_on}"));
        let nix_calls = root.join(format!("nix-calls-{succeed_on}"));
        let status = Command::new("bash")
            .args(["-e", "-c", &script])
            .current_dir(root)
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("ATTIC_TOKENS_DIR", &tokens)
            .env("STUB_ATTIC", &attic)
            .env("ATTIC_CALLS", &attic_calls)
            .env("NIX_CALLS", &nix_calls)
            .env("SUCCEED_ON", succeed_on.to_string())
            .status()
            .unwrap();
        assert_eq!(status.success(), success, "{script}");
        assert_eq!(read(&attic_calls), push.repeat(attempts));
        // Only `attic push` is retried: the token, path listing and client
        // build above the loop run once even when every attempt fails.
        assert_eq!(
            read(&nix_calls),
            "path-info -r ./result\nbuild --inputs-from . --no-link --print-out-paths nixpkgs#attic-client\n"
        );
    }
}

#[test]
fn sole_github_release_target_uses_actions_without_changing_crow_ci() {
    let project = init_package("github-crow-demo");
    let config_path = project.path().join("simit.toml");
    let config = read(&config_path)
        .replace(
            "[release.codeberg]\nrepo = \"example/github-crow-demo\"",
            "[release.github]\nrepo = \"example/github-crow-demo\"",
        )
        .replace(
            "[release.artifacts]",
            "[ci]\nplatform = \"forgejo\"\nprovider = \"crow\"\n\n[release.artifacts]",
        );
    fs::write(config_path, config).unwrap();

    let output = simit()
        .current_dir(project.path())
        .args(["init", "release"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        project
            .path()
            .join(".github/workflows/release.yml")
            .is_file()
    );
    assert!(!project.path().join(".crow/release.yaml").exists());
}

#[test]
fn switching_release_platform_removes_only_obsolete_generated_workflows() {
    let project = init_package("release-migration-demo");
    let config_path = project.path().join("simit.toml");
    let config = read(&config_path).replace(
        "[release.codeberg]\nrepo = \"example/release-migration-demo\"",
        "[release.github]\nrepo = \"example/release-migration-demo\"",
    );
    fs::write(config_path, config).unwrap();
    fs::create_dir_all(project.path().join(".forgejo/workflows")).unwrap();
    fs::create_dir_all(project.path().join(".crow")).unwrap();
    fs::write(
        project.path().join(".forgejo/workflows/release.yml"),
        format!(
            "{}\nname: obsolete\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        ),
    )
    .unwrap();
    fs::write(
        project.path().join(".crow/release.yaml"),
        "name: hand-written\n",
    )
    .unwrap();

    let status = simit()
        .current_dir(project.path())
        .args(["init", "release"])
        .status()
        .unwrap();

    assert!(status.success());
    assert!(
        project
            .path()
            .join(".github/workflows/release.yml")
            .is_file()
    );
    assert!(
        !project
            .path()
            .join(".forgejo/workflows/release.yml")
            .exists()
    );
    assert!(project.path().join(".crow/release.yaml").is_file());
}

#[test]
fn release_plan_only_maintains_an_existing_generated_workflow() {
    let project = init_package("release-plan-demo");
    let root = project.path();
    let config_path = root.join("simit.toml");
    let config = read(&config_path).replace("[release.codeberg]", "[release.github]");
    fs::write(config_path, config).unwrap();
    let plan = || {
        let output = simit()
            .current_dir(root)
            .args(["init", "release", "--plan-json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let empty = serde_json::json!({"schema":"simit.ci-update-plan.v1","files":[],"obsolete":[]});
    assert_eq!(plan(), empty);
    fs::create_dir_all(root.join(".github/workflows")).unwrap();
    fs::write(
        root.join(".github/workflows/release.yml"),
        "name: hand-written\n",
    )
    .unwrap();
    assert_eq!(plan(), empty);

    fs::remove_file(root.join(".github/workflows/release.yml")).unwrap();
    fs::write(
        root.join(".github/workflows/release.yaml"),
        format!(
            "{}\nname: previous\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        ),
    )
    .unwrap();
    let migrated = plan();
    assert_eq!(
        migrated["files"][0]["relative_path"],
        ".github/workflows/release.yml"
    );
    assert_eq!(
        migrated["obsolete"],
        serde_json::json!([".github/workflows/release.yaml"])
    );
    assert!(!root.join(".github/workflows/release.yml").exists());
    // A hand-written destination is preserved even though the marked source
    // workflow is obsolete.
    fs::write(
        root.join(".github/workflows/release.yml"),
        "name: hand-written\n",
    )
    .unwrap();
    assert_eq!(plan(), empty);
    fs::remove_file(root.join(".github/workflows/release.yml")).unwrap();

    // With two release targets and no [ci].platform, the default backend must
    // not silently move the existing GitHub release workflow to Forgejo.
    let mut config = read(&root.join("simit.toml"));
    config.push_str("\n[release.codeberg]\nrepo = \"example/release-plan-demo\"\n");
    fs::write(root.join("simit.toml"), config).unwrap();
    let moved = simit()
        .current_dir(root)
        .args(["init", "release", "--plan-json"])
        .output()
        .unwrap();
    assert!(!moved.status.success());
    assert!(String::from_utf8_lossy(&moved.stderr).contains("--platform"));
    let explicit = simit()
        .current_dir(root)
        .args(["init", "release", "--plan-json", "--platform", "github"])
        .output()
        .unwrap();
    assert!(
        explicit.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
}

#[test]
fn crow_release_pushes_attic_only_for_tags() {
    let project = init_package("crow-attic-demo");
    let config_path = project.path().join("simit.toml");
    let config = read(&config_path)
        .replace(
            "[release.artifacts]",
            "[ci]\nprovider = \"crow\"\nplatform = \"forgejo\"\nruntime = \"nix\"\nrunner = \"crow-default\"\n\n[ci.crow]\nlabels = { group = \"codefloe-infra\", platform = \"linux/amd64\" }\n\n[release.artifacts]",
        )
        .replace(
            "build_commands = [\"mkdir -p release\", \"printf artifact > release/crow-attic-demo.txt\"]",
            "build_commands = [\"nix build .#packages.x86_64-linux.default --out-link result-attic\"]",
        )
        + "\n[release.attic]\ncache = \"canix\"\nurl = \"https://attic.example\"\ntoken_name = \"crow-attic-demo\"\ntoken_secret = \"ATTIC_TOKEN\"\nresult_links = [\"result-attic\"]\n";
    fs::write(config_path, config).unwrap();

    let output = simit()
        .current_dir(project.path())
        .args(["init", "release"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let workflow = read(&project.path().join(".crow/release.yaml"));
    assert!(workflow.contains("group: codefloe-infra"));
    assert!(workflow.contains("platform: linux/amd64"));
    assert!(workflow.contains("- event: tag"));
    assert!(!workflow.contains("- event: manual"));
    assert!(workflow.contains("from_secret: ATTIC_TOKEN"));
    assert!(workflow.contains("nix build .#packages.x86_64-linux.default --out-link result-attic"));
    assert!(workflow.contains("nix path-info -r './result-attic' > attic-paths.txt"));
    assert!(workflow.contains("push --stdin --no-closure --ignore-upstream-cache-filter canix"));
}

#[test]
fn bootstraps_explicit_nix_release_bundle() {
    let project = init_package("bundle-demo");
    let config_path = project.path().join("simit.toml");
    let config = read(&config_path).replace(
        "runner = \"atlas\"\n",
        "runner = \"atlas\"\nnix_bundle_attrs = [\"release-bundle\"]\n",
    );
    fs::write(config_path, config).unwrap();

    let output = simit()
        .current_dir(project.path())
        .args(["init", "release"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = read(&project.path().join(".forgejo/workflows/release.yml"));
    assert!(workflow.contains("nix build '.#release-bundle'"));
    assert!(workflow.contains("expected exactly one release manifest from Nix bundles"));
    assert!(workflow.contains(".schemaVersion == 2"));
}

#[test]
fn prebuild_binaries_enables_conventional_release_bundle() {
    let project = init_package("prebuilt-demo");
    let config_path = project.path().join("simit.toml");
    let config = read(&config_path).replace(
        "runner = \"atlas\"\n",
        "runner = \"atlas\"\nprebuild_binaries = true\n",
    );
    fs::write(config_path, config).unwrap();

    let output = simit()
        .current_dir(project.path())
        .args(["init", "release"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = read(&project.path().join(".forgejo/workflows/release.yml"));
    assert!(workflow.contains("nix build '.#release-bundle'"));
    assert!(workflow.contains("expected exactly one release manifest from Nix bundles"));
}

#[test]
fn github_prebuild_release_limits_nix_and_checks_disk_capacity() {
    let project = init_package("github-prebuilt-demo");
    let config_path = project.path().join("simit.toml");
    let config = read(&config_path)
        .replace(
            "[release.codeberg]\nrepo = \"example/github-prebuilt-demo\"",
            "[release.github]\nrepo = \"example/github-prebuilt-demo\"",
        )
        .replace(
            "[release.artifacts]",
            "[ci]\nplatform = \"github\"\n\n[release.artifacts]",
        )
        .replace(
            "runner = \"atlas\"\n",
            "runner = \"ubuntu-24.04\"\nprebuild_binaries = true\n",
        );
    fs::write(config_path, config).unwrap();

    let output = simit()
        .current_dir(project.path())
        .args(["init", "release", "--platform", "github"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = read(&project.path().join(".github/workflows/release.yml"));
    assert!(workflow.contains("runs-on: ubuntu-24.04"));
    assert!(workflow.contains("max-jobs = 1"));
    assert!(workflow.contains("cores = 2"));
    assert!(workflow.contains("less than 8 GiB free before Nix realization"));
    assert!(workflow.contains("disk before bundle release-bundle"));
    assert!(workflow.contains("disk after bundle release-bundle"));
    assert!(workflow.contains("nix store gc"));
    assert!(workflow.contains("less than 2 GiB free before release publication"));
    assert!(workflow.contains("add_matches 'release/*.intoto.bundle'"));
}

#[test]
fn check_succeeds_after_bootstrap_and_rerender_is_idempotent() {
    let project = init_package("demo");

    for _ in 0..2 {
        let status = simit()
            .current_dir(project.path())
            .args(["init", "release"])
            .status()
            .unwrap();
        assert!(status.success());
    }

    let check = simit()
        .current_dir(project.path())
        .args(["init", "release", "--check"])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn release_without_project_runner_uses_trusted_forgejo_nix_runner() {
    let project = init_package_without_release_runner("demo");

    let output = simit_with_split_runner_config(project.path())
        .current_dir(project.path())
        .args(["init", "release"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = read(&project.path().join(".forgejo/workflows/release.yml"));
    assert!(workflow.contains("runs-on: atlas-nix-trusted\n"));
    assert!(workflow.contains("    env:\n"));
    assert!(workflow.contains("      NIX_CONFIG: |\n"));
    assert!(workflow.contains("        experimental-features = nix-command flakes\n"));
    assert!(workflow.contains("        accept-flake-config = true\n"));
    assert!(workflow.contains("      XDG_CACHE_HOME: \"/tmp/.cache\"\n"));
    assert!(!workflow.contains("uses: https://github.com/cachix/install-nix-action"));
}

#[test]
fn explicit_trusted_forgejo_nix_runner_uses_preinstalled_nix() {
    let project = init_package_with_release_runner("demo", "atlas-nix-trusted");

    let output = simit_with_split_runner_config(project.path())
        .current_dir(project.path())
        .args(["init", "release"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = read(&project.path().join(".forgejo/workflows/release.yml"));
    assert!(workflow.contains("runs-on: atlas-nix-trusted\n"));
    assert!(workflow.contains("      NIX_CONFIG: |\n"));
    assert!(workflow.contains("        accept-flake-config = true\n"));
    assert!(!workflow.contains("uses: https://github.com/cachix/install-nix-action"));
}

#[test]
fn release_workflow_checks_credentials_before_building() {
    let project = init_package_with_release_runner("demo", "atlas-nix-trusted");
    let simit_toml = read(&project.path().join("simit.toml"));
    fs::write(
        project.path().join("simit.toml"),
        simit_toml.replace(
            "sign = false\n",
            r#"sign = true
checksum_globs = ["demo.txt"]

[release.attic]
cache = "canix"
url = "https://attic.example"
token_name = "rs-modde"
result_links = ["result"]
"#,
        ),
    )
    .unwrap();

    let output = simit_with_split_runner_config(project.path())
        .current_dir(project.path())
        .args(["init", "release"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = read(&project.path().join(".forgejo/workflows/release.yml"));
    let credential_check = workflow
        .find("      - name: Check release credentials")
        .expect("credential preflight step");
    let build = workflow
        .find("      - name: Build release artifacts")
        .expect("build step");
    assert!(credential_check < build);
    assert!(workflow.contains("FORGEJO_TOKEN: ${{ secrets.codeberg_token }}"));
    assert!(workflow.contains("MINISIGN_SECRET_KEY: ${{ secrets.MINISIGN_SECRET_KEY }}"));
    assert!(workflow.contains("MINISIGN_PASSWORD: ${{ secrets.MINISIGN_PASSWORD }}"));
    assert!(workflow.contains("require_credential 'global/user secret' 'codeberg_token'"));
    assert!(workflow.contains("require_credential 'global/user secret' 'AUR_SSH_KEY'"));
    assert!(workflow.contains("require_credential 'global/user secret' 'copr_login'"));
    assert!(workflow.contains("require_credential 'global/user variable' 'copr_username'"));
    assert!(workflow.contains("require_credential 'global/user secret' 'copr_token'"));
    assert!(workflow.contains("require_credential 'repo secret' 'apt_repo_gpg_key'"));
    assert!(workflow.contains("require_credential 'repo variable' 'apt_repo_gpg_key_id'"));
    assert!(workflow.contains("require_credential 'repo variable' 'apt_repo_gpg_fingerprint'"));
    assert!(workflow.contains("require_credential 'repo variable' 'apt_repo_gpg_public_key'"));
    assert!(workflow.contains("require_credential 'repo secret' 'apt_repo_ssh_key'"));
    assert!(workflow.contains("minisign -V -m \"$minisign_probe\""));
    assert!(!workflow.contains("test -r \"${ATTIC_TOKENS_DIR:?}/rs-modde\""));
    assert!(workflow.contains("attic_token_dir=\"${ATTIC_TOKENS_DIR:-}\""));
    assert!(workflow.contains("is required because Nix closure cache publishing is configured."));
    assert!(workflow.contains("default-server = \"canix\""));
    assert!(!workflow.contains("attic login"));
    assert!(workflow.contains("Attic push failed for configured Nix closure cache publishing."));
    assert!(workflow.contains("          nix path-info -r \\\n            ./result \\"));
    assert!(!workflow.contains("\n            result \\\n"));
    assert!(workflow.contains("awk -v version=\"$VERSION\""));
    assert!(
        workflow
            .contains("^## \\\\[\" version \"\\\\] - [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]$")
    );
    assert!(workflow.contains("found && /^## \\[/ { exit }"));
    assert!(workflow.contains("--rawfile body release-notes.md"));
    assert!(!workflow.contains("--rawfile body CHANGELOG.md"));
    assert!(
        workflow
            .find("      - name: Publish Forgejo release")
            .expect("Forgejo release step")
            < workflow
                .find("      - name: Push Nix closures to Attic")
                .expect("Attic push step")
    );
}

#[test]
fn print_matches_checked_workflow_without_writing_file() {
    let project = init_package("demo");

    let output = simit()
        .current_dir(project.path())
        .args(["init", "release", "--print"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("name: release"));
    assert!(stdout.contains("Publish Forgejo release"));
    assert!(stdout.contains("Publish AUR packages"));
    assert!(stdout.contains("Push SRPM to COPR"));
    assert!(stdout.contains("Publish APT repository"));
    assert!(
        !project
            .path()
            .join(".forgejo/workflows/release.yml")
            .exists()
    );
}

#[test]
fn runner_override_replaces_the_ci_runner_fallback() {
    let project = init_package_without_release_runner("release-runner-demo");
    let config_path = project.path().join("simit.toml");
    let config = format!(
        "[ci]\nrunner = \"codeberg-small\"\n\n{}",
        read(&config_path)
    );
    fs::write(config_path, config).unwrap();
    let print = |extra: &[&str]| {
        let output = simit()
            .current_dir(project.path())
            .args(["init", "release", "--print"])
            .args(extra)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    assert!(print(&[]).contains("codeberg-small"));
    let migrated = print(&["--runner", "ubuntu-24.04"]);
    assert!(migrated.contains("ubuntu-24.04") && !migrated.contains("codeberg-small"));
}

#[test]
fn forgejo_release_plan_needs_a_project_runner() {
    let project = init_package_without_release_runner("plan-runner-demo");
    let output = simit()
        .current_dir(project.path())
        .args(["init", "release", "--plan-json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("project policy"));
}

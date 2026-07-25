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
    assert!(workflow.contains("git worktree add --detach \"$tag_worktree\" \"$VERSION\""));
    assert!(workflow.contains("git checkout --detach \"$validated_sha\""));
    assert!(workflow.contains(
        "uses: https://github.com/cachix/install-nix-action@ba0dd844c9180cbf77aa72a116d6fbc515d0e87b # v27"
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
    assert!(workflow.contains("Publish GitHub release"));
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
    assert!(workflow.contains("Attic login failed for configured Nix closure cache publishing."));
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

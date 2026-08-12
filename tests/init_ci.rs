use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn init_package(with_flake: bool) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "demo"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "MIT"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

    if with_flake {
        fs::write(root.join("flake.nix"), "{}\n").unwrap();
    }

    temp
}

fn init_flake_only() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("flake.nix"),
        "{ outputs = { self }: {}; }\n",
    )
    .unwrap();
    temp
}

fn init_python_project() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::write(
        root.join("pyproject.toml"),
        r#"[project]
name = "py-demo"
version = "0.1.0"

[project.scripts]
py-demo = "py_demo:main"

[project.optional-dependencies]
cpu = ["pytest"]

[dependency-groups]
dev = ["mypy", "pytest", "ruff"]
"#,
    )
    .unwrap();
    fs::write(root.join("uv.lock"), "").unwrap();
    fs::write(
        root.join("flake.nix"),
        r#"{
  inputs.py-harbor.url = "git+https://codeberg.org/caniko/py-harbor.git";
  outputs = { self, py-harbor, ... }: {
    checks.x86_64-linux.offline-tests = {};
    checks.x86_64-linux.typecheck = {};
  };
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("simit.toml"),
        r#"[flake]
scope = "full"
mode = "custom"
backend = "py-harbor"

[flake.expected_outputs]
checks = ["offline-tests", "typecheck"]

[ci]
runtime = "nix"
"#,
    )
    .unwrap();
    temp
}

fn init_workspace_fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    copy_dir(Path::new("tests/fixtures/workspace-ci"), temp.path());
    temp
}

fn init_diverging_workspace_fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    copy_dir(
        Path::new("tests/fixtures/workspace-ci-diverging"),
        temp.path(),
    );
    temp
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).unwrap();
        }
    }
}

fn write_chocolatey_config(root: &Path) {
    fs::write(
        root.join("simit.toml"),
        r#"[chocolatey]
authors = "Example Maintainers"
description = "demo binary"
project_url = "https://example.com/demo"
download_repo = "foo/demo"
"#,
    )
    .unwrap();
}

fn write_scoop_config(root: &Path) {
    fs::write(
        root.join("simit.toml"),
        r#"[scoop]
bucket_url = "https://example.com/scoop-demo.git"
description = "demo binary"
homepage = "https://example.com/demo"
license = "MIT"
download_repo = "foo/demo"
"#,
    )
    .unwrap();
}

fn write_windows_packager_config(root: &Path) {
    fs::write(
        root.join("simit.toml"),
        r#"[chocolatey]
authors = "Example Maintainers"
description = "demo binary"
project_url = "https://example.com/demo"
download_repo = "foo/demo"

[scoop]
bucket_url = "https://example.com/scoop-demo.git"
description = "demo binary"
homepage = "https://example.com/demo"
license = "MIT"
download_repo = "foo/demo"
"#,
    )
    .unwrap();
}

fn write_release_smoke_config(root: &Path) {
    fs::write(
        root.join("simit.toml"),
        r#"[release.smoke]
command = "nix run .#release-smoke --"
"#,
    )
    .unwrap();
}

fn write_ci_customization_config(root: &Path) {
    fs::write(
        root.join("simit.toml"),
        r#"[ci]
extra_setup = [
  "apt-get update && apt-get install -y --no-install-recommends postgresql-client"
]
extra_env = { SKILLNET_TEST_PG_URL = "${{ secrets.SKILLNET_TEST_PG_URL }}" }
required_secrets = ["SKILLNET_TEST_PG_URL"]
required_env = ["VSCE_PAT_FILE", "OVSX_PAT_FILE"]
"#,
    )
    .unwrap();
}

fn write_user_runner_config(root: &Path) {
    let config_dir = root.join(".xdg/simit");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true

[ci.runners.windows_atlas]
platform = "forgejo"
labels = ["windows-atlas"]
os = "windows"
arch = "x86_64"
runtimes = ["cargo"]

[ci.defaults.forgejo]
cargo = "atlas"
nix = "atlas"
release = "atlas"
windows = "windows_atlas"
"#,
    )
    .unwrap();
}

fn write_user_runner_config_with_omnix_ref(root: &Path) {
    let config_dir = root.join(".xdg/simit");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true

[ci.runners.windows_atlas]
platform = "forgejo"
labels = ["windows-atlas"]
os = "windows"
arch = "x86_64"
runtimes = ["cargo"]

[ci.defaults.forgejo]
cargo = "atlas"
nix = "atlas"
release = "atlas"
windows = "windows_atlas"

[ci.tools.omnix]
ref = "github:user/pin/v3"
"#,
    )
    .unwrap();
}

fn simit_with_user_config(root: &Path) -> Command {
    write_user_runner_config(root);
    let mut command = simit();
    command.env("XDG_CONFIG_HOME", root.join(".xdg"));
    command
}

fn simit_with_multilabel_user_config(root: &Path) -> Command {
    let config_dir = root.join(".xdg/simit");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["self-hosted", "atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true

[ci.defaults.forgejo]
cargo = "atlas"
nix = "atlas"
release = "atlas"
"#,
    )
    .unwrap();
    let mut command = simit();
    command.env("XDG_CONFIG_HOME", root.join(".xdg"));
    command
}

fn simit_with_split_runtime_user_config(root: &Path) -> Command {
    let config_dir = root.join(".xdg/simit");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo"]
trusted = true

[ci.runners.atlas_nix]
platform = "forgejo"
labels = ["atlas-nix-trusted"]
os = "linux"
arch = "x86_64"
runtimes = ["nix"]
trusted = true

[ci.defaults.forgejo]
cargo = "atlas"
nix = "atlas_nix"
release = "atlas"
"#,
    )
    .unwrap();
    let mut command = simit();
    command.env("XDG_CONFIG_HOME", root.join(".xdg"));
    command
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

fn assert_yaml_parses(text: &str) {
    serde_yaml::from_str::<serde_yaml::Value>(text).unwrap();
}

fn assert_all_branch_push_trigger(workflow: &str) {
    assert!(workflow.contains("branches: [\"**\"]"));
    assert!(workflow.contains("tags-ignore: [\"**\"]"));
    assert!(!workflow.contains("branches: [trunk]"));
}

fn assert_homebrew_run_block_indentation(workflow: &str) {
    let mut in_homebrew_step = false;
    let mut in_run_block = false;
    let mut saw_script_line = false;

    for line in workflow.lines() {
        if line == "      - name: Publish Homebrew tap" {
            in_homebrew_step = true;
            continue;
        }
        if in_homebrew_step && line == "        run: |" {
            in_run_block = true;
            continue;
        }
        if in_run_block && line.starts_with("      - name: ") {
            break;
        }
        if in_run_block && !line.is_empty() {
            assert!(
                line.starts_with("          "),
                "Homebrew run block line is under-indented: {line:?}"
            );
            saw_script_line = true;
        }
    }

    assert!(saw_script_line, "Homebrew run block was not found");
}

fn assert_release_integrity_steps(workflow: &str) {
    assert!(workflow.contains("keys/maintainers.gpg"));
    assert!(workflow.contains("git verify-tag \"$tag\""));
    assert!(workflow.contains("keys/minisign.pub"));
    assert!(workflow.contains("MINISIGN_SECRET_KEY: ${{ secrets.MINISIGN_SECRET_KEY }}"));
    assert!(workflow.contains("MINISIGN_PASSWORD: ${{ secrets.MINISIGN_PASSWORD }}"));
    assert!(workflow.contains("COSIGN_PRIVATE_KEY: ${{ secrets.COSIGN_PRIVATE_KEY }}"));
    assert!(workflow.contains("release/SHA256SUMS.txt.minisig"));
    assert!(workflow.contains("jq -er '.value' > \"$oidc_token\""));
    assert!(
        workflow.contains("cosign sign-blob --yes --identity-token \"$(cat \"$oidc_token\")\"")
    );
    assert!(
        workflow.contains("cosign attest-blob --yes --identity-token \"$(cat \"$oidc_token\")\"")
    );
    assert!(workflow.contains("--type slsaprovenance1"));
    assert!(!workflow.contains("--output-attestation"));
    assert!(workflow.contains("--bundle \"${file}.intoto.bundle\""));
    assert!(
        workflow
            .contains("::error::keyless Sigstore failed and COSIGN_PRIVATE_KEY is unset for $file")
    );
    assert!(workflow.contains("COSIGN_PRIVATE_KEY fallback"));
}

fn assert_maintainer_key_written(root: &Path) {
    let key = read(&root.join("keys/maintainers.gpg"));
    assert!(key.contains("PGP PUBLIC KEY BLOCK") || !key.is_empty());
}

#[test]
fn generates_forgejo_nix_workflows() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: atlas"));
    assert!(ci.contains("NIX_CONFIG: \"experimental-features = nix-command flakes\""));
    assert!(ci.contains("XDG_CACHE_HOME: \"/tmp/.cache\""));
    assert!(ci.contains("CARGO_HOME: \"/tmp/.cargo\""));
    assert!(ci.contains("run: echo \"$CARGO_HOME/bin\" >> \"$GITHUB_PATH\""));
    assert!(ci.contains("group: ${{ github.workflow_ref }}-${{ github.ref }}"));
    assert!(ci.contains("uses: https://code.forgejo.org/actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1"));
    assert!(!ci.contains("pull_request:"));
    assert!(!ci.contains("uses: https://github.com/cachix/install-nix-action@v31"));
    assert!(!ci.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(!ci.contains("path: ~/.cargo/bin"));
    assert!(!ci.contains("command -v cargo-nextest"));
    assert!(ci.contains("run: nix flake check"));
    assert!(ci.contains("run: nix develop -c cargo clippy --all-targets -- --deny warnings"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: atlas"));
    assert!(publish.contains("NIX_CONFIG: \"experimental-features = nix-command flakes\""));
    assert!(publish.contains("XDG_CACHE_HOME: \"/tmp/.cache\""));
    assert!(publish.contains("CARGO_HOME: \"/tmp/.cargo\""));
    assert!(publish.contains("run: echo \"$CARGO_HOME/bin\" >> \"$GITHUB_PATH\""));
    assert!(!publish.contains("uses: https://github.com/cachix/install-nix-action@v31"));
    assert!(!publish.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(!publish.contains("path: ~/.cargo/bin"));
    assert!(!publish.contains("command -v cargo-nextest"));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(!publish.contains("  push:\n    tags:\n"));
    assert!(publish.contains("  workflow_dispatch:\n"));
    assert!(publish.contains("grep -Eq '^[0-9]+\\.[0-9]+\\.[0-9]+$'"));
    assert!(publish.contains("keys/maintainers.gpg"));
    assert!(publish.contains("git verify-tag \"$tag\""));
    assert!(publish.contains(
        r#"nix develop -c cargo pkgid -p demo | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#
    ));
    assert!(!publish.contains("inputs:\n"));
    assert!(publish.contains("CRATES_IO_API_TOKEN: ${{ secrets.CRATES_IO_API_TOKEN }}"));
    assert!(publish.contains("CRATES_IO_API_TOKEN is required"));
    assert!(publish.contains("export CARGO_REGISTRY_TOKEN="));
    assert!(publish.contains("https://crates.io/api/v1/crates/${crate_name}/${version}"));
    assert!(publish.contains("already published on crates.io; skipping publish"));
    assert!(!publish.contains("cargo login"));
    assert_maintainer_key_written(temp.path());
}

#[test]
fn github_ci_generates_declared_nix_installable_matrix() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
provider = "actions"
runtime = "nix"
nix_builds = [".#oci-api", ".#oci-etl"]
extra_setup = ["echo prepare-runner"]
"#,
    )
    .unwrap();
    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--ci-provider",
            "actions",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/nix-builds.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("permissions:\n  contents: read"));
    assert!(
        workflow
            .contains("group: ${{ github.workflow }}-${{ github.head_ref || github.ref_name }}")
    );
    assert!(workflow.contains("runs-on: ubuntu-latest"));
    assert!(workflow.contains("fail-fast: false"));
    assert!(workflow.contains("max-parallel: 2"));
    assert!(workflow.contains("- \".#oci-api\""));
    assert!(workflow.contains("- \".#oci-etl\""));
    assert!(workflow.contains("run: nix build --no-link \"$INSTALLABLE\""));
    assert!(workflow.contains("run: echo prepare-runner"));
    assert!(!workflow.contains("secrets."));
}

#[test]
fn github_nix_only_uses_native_system_runner_matrix() {
    let temp = init_flake_only();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
provider = "actions"
platform = "github"
runtime = "nix"

[ci.nix_system_runners]
"aarch64-darwin" = "macos-15"
"aarch64-linux" = "ubuntu-24.04-arm"
"x86_64-linux" = "ubuntu-24.04"
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("strategy:\n      fail-fast: false\n      matrix:\n"));
    assert!(workflow.contains("- system: aarch64-darwin\n            runner: macos-15"));
    assert!(workflow.contains("- system: aarch64-linux\n            runner: ubuntu-24.04-arm"));
    assert!(workflow.contains("- system: x86_64-linux\n            runner: ubuntu-24.04"));
    assert!(workflow.contains("runs-on: ${{ matrix.runner }}"));
    assert!(workflow.contains(
        "nix eval --impure --raw --expr builtins.currentSystem)\" = \"${{ matrix.system }}\""
    ));
    assert!(
        workflow
            .contains("nix flake check --no-update-lock-file --system \"${{ matrix.system }}\"")
    );
    assert!(!workflow.contains("--all-systems"));

    let check = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--check",
            "--diff",
        ])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn github_prebuild_is_reusable_publishes_attic_and_tracks_drift() {
    let temp = init_flake_only();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[prebuild]
release_archives = true
publish_attic = true
attic_app = ".#push-flake-inputs"

[prebuild.system_runners]
"aarch64-linux" = "ubuntu-24.04-arm"
"x86_64-linux" = "ubuntu-24.04"

[ci]
provider = "actions"
platform = "github"
runtime = "nix"
nix_builds = [".#server", ".#worker"]

[release.artifacts]
prebuild_binaries = true
substituters = ["https://cache.example"]
trusted_public_keys = ["cache.example:abc"]

[release.attic]
cache = "demo"
url = "https://attic.example"
token_name = "demo"
token_secret = "ATTIC_TOKEN"
"#,
    )
    .unwrap();
    fs::create_dir_all(temp.path().join(".github/workflows")).unwrap();
    fs::write(
        temp.path().join(".github/workflows/nix-builds.yaml"),
        format!(
            "{}\nname: obsolete\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        ),
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        !temp
            .path()
            .join(".github/workflows/nix-builds.yaml")
            .exists()
    );

    let workflow = read(&temp.path().join(".github/workflows/prebuild.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("workflow_call:\n    inputs:\n      release:"));
    assert!(workflow.contains("secrets:\n      attic_token:\n        required: true"));
    assert!(workflow.contains("runs-on: ${{ matrix.runner }}"));
    assert!(workflow.contains("nix build '.#server' --out-link '.simit-prebuild/ci-0'"));
    assert!(
        workflow.contains("nix build '.#release-bundle' --out-link '.simit-prebuild/release-0'")
    );
    assert!(workflow.contains("nix run '.#push-flake-inputs'"));
    assert!(workflow.contains("HARBOR_ATTIC_MANIFEST: attic-paths.txt"));
    assert!(workflow.contains("github.event.repository.default_branch"));
    assert!(workflow.contains("name: flake-inputs-${{ matrix.system }}"));
    assert!(!workflow.contains("attic push"));
    assert!(!workflow.contains("default-server"));
    assert!(!workflow.contains("attic login"));

    let audit = simit::registry::audit_ci(temp.path()).unwrap();
    assert_eq!(audit.status, simit::registry::FeatureStatus::Managed);
    fs::write(
        temp.path().join(".github/workflows/prebuild.yaml"),
        format!("{workflow}\n# drift\n"),
    )
    .unwrap();
    assert_eq!(
        simit::registry::audit_ci(temp.path()).unwrap().status,
        simit::registry::FeatureStatus::Drift
    );
}

#[test]
fn github_prebuild_is_additive_to_forgejo_crow_ci() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[prebuild]
publish_attic = true
attic_app = ".#push-flake-inputs"

[prebuild.system_runners]
"x86_64-linux" = "ubuntu-24.04"

[ci]
provider = "crow"
platform = "forgejo"
runtime = "nix"
runner = "atlas-nix-trusted"

[release.attic]
cache = "demo"
url = "https://attic.example"
token_name = "demo"
token_secret = "ATTIC_TOKEN"
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(temp.path().join(".crow/build.yaml").is_file());

    let prebuild = read(&temp.path().join(".github/workflows/prebuild.yaml"));
    assert_yaml_parses(&prebuild);
    assert!(prebuild.contains("runs-on: ${{ matrix.runner }}"));
    assert!(prebuild.contains("nix run '.#push-flake-inputs'"));
}

#[test]
fn github_nix_only_keeps_single_runner_when_no_system_map_is_configured() {
    let temp = init_flake_only();

    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--runner",
            "ubuntu-24.04",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("runs-on: ubuntu-24.04"));
    assert!(workflow.contains("run: nix flake check\n"));
    assert!(!workflow.contains("matrix:"));
}

#[test]
fn github_nix_only_can_select_flake_evaluation_without_building_checks() {
    let temp = init_flake_only();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
runtime = "nix"
runner = "ubuntu-24.04"
components = ["flake-evaluation"]
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("run: nix flake check --no-build\n"));
    assert!(!workflow.contains("run: nix flake check\n"));
    assert!(!workflow.contains("Build flake checks"));
}

#[test]
fn github_nix_only_rejects_invalid_native_runner_maps() {
    let temp = init_flake_only();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
platform = "github"
runtime = "nix"
runner = "ubuntu-24.04"

[ci.nix_system_runners]
"aarch64-linux" = ""
"#,
    )
    .unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "nix"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[ci].runner cannot be combined with [ci].nix_system_runners"));
}

#[test]
fn generic_rust_ci_does_not_generate_publish_workflow_by_default() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_yaml_parses(&ci);
    assert!(temp.path().join(".forgejo/workflows/ci.yaml").exists());
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/publish-crate.yaml")
            .exists()
    );
    assert!(!temp.path().join("keys/maintainers.gpg").exists());
}

#[test]
fn package_metadata_nix_builds_validate_and_render() {
    let temp = init_package(true);
    let manifest = temp.path().join("Cargo.toml");
    let base = read(&manifest);
    fs::write(
        &manifest,
        format!(
            "{base}\n[package.metadata.simit.ci]\nplatform = \"github\"\nprovider = \"actions\"\nruntime = \"nix\"\nnix_builds = [\"\"]\n"
        ),
    )
    .unwrap();
    let invalid = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("[ci].nix_builds"));

    fs::write(
        &manifest,
        format!(
            "{base}\n[package.metadata.simit.ci]\nplatform = \"github\"\nprovider = \"actions\"\nruntime = \"nix\"\nnix_builds = [\".#oci-api\", \".#oci-etl\"]\nextra_setup = [\"echo prepare-runner\"]\n"
        ),
    )
    .unwrap();
    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/nix-builds.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("permissions:\n  contents: read"));
    assert!(
        workflow
            .contains("group: ${{ github.workflow }}-${{ github.head_ref || github.ref_name }}")
    );
    assert!(workflow.contains("- \".#oci-api\""));
    assert!(workflow.contains("- \".#oci-etl\""));
    assert!(workflow.contains("run: echo prepare-runner"));
    assert!(workflow.contains("runs-on: ubuntu-latest"));
    assert!(workflow.contains("fail-fast: false"));
    assert!(workflow.contains("max-parallel: 2"));
    assert!(workflow.contains("run: nix build --no-link \"$INSTALLABLE\""));
    assert!(!workflow.contains("secrets."));
}

#[test]
fn generic_flake_integrated_rust_ci_auto_selects_nix_without_publish() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("flake.nix"),
        r#"{
  outputs = { self }: {
    packages.x86_64-linux.default = {};
    apps.x86_64-linux.default = { type = "app"; program = "/bin/demo"; };
    checks.x86_64-linux.demo = {};
    devShells.x86_64-linux.default = {};
    nixosModules.default = {};
    homeModules.default = {};
    lib = {};
  };
}
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--with-audit",
            "--with-deny",
            "--with-docs",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_yaml_parses(&ci);
    assert!(ci.contains("NIX_CONFIG: \"experimental-features = nix-command flakes\""));
    assert!(ci.contains("run: nix flake check"));
    assert!(ci.contains("nix develop -c cargo test"));
    assert!(ci.contains("nix develop"));
    assert!(ci.contains("cargo doc"));
    assert!(ci.contains("--no-deps --all-features"));
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/publish-crate.yaml")
            .exists()
    );
    assert!(!temp.path().join("keys/maintainers.gpg").exists());

    let simit_toml = read(&temp.path().join("simit.toml"));
    assert!(simit_toml.contains("runtime = \"nix\""));
    assert!(simit_toml.contains("with_audit = true"));
    assert!(simit_toml.contains("with_deny = true"));
    assert!(simit_toml.contains("with_docs = true"));
    assert!(!simit_toml.contains("publish_crates = true"));
}

#[test]
fn forgejo_nix_can_generate_codeberg_pages_workflow() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-codeberg-pages",
            "--pages-repo",
            "caniko/plinth",
            "--pages-canonical-domain",
            "plinth.tartanoglu.com",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let pages = read(&temp.path().join(".forgejo/workflows/pages.yaml"));
    assert_yaml_parses(&pages);
    assert!(pages.contains("# Generated by simit."));
    assert!(pages.contains("name: pages"));
    assert!(pages.contains("      - trunk"));
    assert!(pages.contains("group: ${{ codeberg.workflow }}-${{ codeberg.ref }}"));
    assert!(pages.contains("runs-on: atlas"));
    assert!(pages.contains("      - name: Validate Pages domain"));
    assert!(pages.contains("nix build .#site --no-link --out-link result-pages-site"));
    assert!(pages.contains("grep -qx plinth.tartanoglu.com result-pages-site/.domains"));
    assert!(pages.contains("CODEBERG_TOKEN: ${{ secrets.codeberg_token }}"));
    assert!(pages.contains("test -n \"$CODEBERG_TOKEN\""));
    assert!(pages.contains("git config user.name \"forgejo-actions\""));
    assert!(pages.contains(
        "git remote add pages-origin \"https://caniko:${CODEBERG_TOKEN}@codeberg.org/caniko/plinth.git\""
    ));
    assert!(pages.contains("DEPLOY_REMOTE=pages-origin nix run .#deploy-pages"));

    let simit_toml = read(&temp.path().join("simit.toml"));
    assert!(simit_toml.contains("[ci.pages]"));
    assert!(simit_toml.contains("repo = \"caniko/plinth\""));
    assert!(simit_toml.contains("canonical_domain = \"plinth.tartanoglu.com\""));

    let check = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn github_nix_can_generate_github_pages_workflow() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--runner",
            "ubuntu-latest",
            "--with-codeberg-pages",
            "--pages-repo",
            "caniko/plinth",
            "--pages-canonical-domain",
            "plinth.tartanoglu.com",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let pages = read(&temp.path().join(".github/workflows/pages.yaml"));
    assert_yaml_parses(&pages);
    assert!(pages.contains("permissions:\n  contents: read\n  pages: write\n  id-token: write"));
    assert!(pages.contains("uses: actions/checkout@"));
    assert!(pages.contains("uses: actions/upload-pages-artifact@"));
    assert!(pages.contains("uses: actions/deploy-pages@"));
    assert!(pages.contains("nix build .#site --no-link --out-link result-pages-site"));
    assert!(pages.contains("grep -qx plinth.tartanoglu.com result-pages-site/.domains"));
    assert!(!pages.contains("CODEBERG_TOKEN"));
    assert!(!pages.contains("codeberg.workflow"));
}

#[test]
fn persisted_codeberg_pages_overrides_survive_regeneration() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-codeberg-pages",
            "--pages-repo",
            "caniko/rs-harbor",
            "--pages-canonical-domain",
            "rs-harbor.tartanoglu.com",
            "--pages-site-output",
            "./site#site",
            "--pages-deploy-app",
            "./site#deploy-pages",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let simit_toml = read(&temp.path().join("simit.toml"));
    assert!(simit_toml.contains("site_output = \"./site#site\""));
    assert!(simit_toml.contains("deploy_app = \"./site#deploy-pages\""));

    let check = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check.success());

    let pages = read(&temp.path().join(".forgejo/workflows/pages.yaml"));
    assert!(pages.contains("nix build ./site#site --no-link --out-link result-pages-site"));
    assert!(pages.contains("DEPLOY_REMOTE=pages-origin nix run ./site#deploy-pages"));
}

#[test]
fn forgejo_nix_can_generate_vscode_publish_workflow_with_file_env_pats() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
runtime = "nix"
runner = "atlas-nix-trusted"

[vscode]
extension_dir = "pkl-lsp-vscode"
runner = "atlas-nix-trusted"
codeberg_repo = "caniko/pkl-lsp"
codeberg_token_secret = "codeberg_token"
pat_source = "file-env"
vsce_pat_file_env = "VSCE_PAT_FILE"
ovsx_pat_file_env = "OVSX_PAT_FILE"
cargo_package = "pkl-lsp-server"
prepublish_commands = ["nix flake check --no-build"]
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-vscode",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-vscode-extension.yaml"),
    );
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("# Generated by simit."));
    assert!(workflow.contains("name: Publish VS Code Extension"));
    assert!(workflow.contains("tags: [\"[0-9]*.[0-9]*.[0-9]*\"]"));
    assert!(workflow.contains("runs-on: atlas-nix-trusted"));
    assert!(workflow.contains("CODEBERG_TOKEN: ${{ secrets.codeberg_token }}"));
    assert!(workflow.contains("file_env=VSCE_PAT_FILE"));
    assert!(workflow.contains("file_env=OVSX_PAT_FILE"));
    assert!(workflow.contains(
        "VSCE_PUBLISHER=$(nix shell nixpkgs#jq -c jq -r '.publisher' pkl-lsp-vscode/package.json)"
    ));
    assert!(workflow.contains(
        "nix develop -c npx --yes @vscode/vsce verify-pat \"$VSCE_PUBLISHER\" --pat \"$VSCE_PUBLISH_PAT\""
    ));
    assert!(workflow.contains("https://aka.ms/vsm-create-publisher"));
    assert!(!workflow.contains("secrets.VSCE_PAT"));
    assert!(!workflow.contains("secrets.OVSX_PAT"));
    assert!(!workflow.contains("skipping VS Code"));
    assert!(!workflow.contains("skipping Open VSX"));
    assert!(workflow.contains("git verify-tag \"$GITHUB_REF_NAME\""));
    assert!(workflow.contains(
        "cargo_metadata=$(nix shell nixpkgs#cargo -c cargo metadata --no-deps --format-version 1)"
    ));
    assert!(workflow.contains("nix shell nixpkgs#jq -c jq -r --arg name pkl-lsp-server"));
    assert!(workflow.contains(
        "extension_version=$(nix shell nixpkgs#jq -c jq -r '.version' pkl-lsp-vscode/package.json)"
    ));
    assert!(workflow.contains("nix shell nixpkgs#jq -c jq -r '.id // empty'"));
    assert!(workflow.contains(
        "existing_ids=$(printf '%s' \"$release_json\" | nix shell nixpkgs#jq -c jq -r --arg name \"$name\" '.assets[]? | select(.name == $name) | .id')"
    ));
    assert!(workflow.contains(
        "curl --fail --silent --show-error --request DELETE --header \"$auth_header\" \"$api/repos/$repo/releases/$release_id/assets/$asset_id\" >/dev/null"
    ));
    assert!(workflow.contains("nix flake check --no-build"));
    assert!(workflow.contains(
        "nix develop -c npx --yes @vscode/vsce publish --packagePath release/*.vsix --pat \"$PUBLISH_PAT\" --skip-duplicate"
    ));
    assert!(workflow.contains("Verify VS Code Marketplace visibility"));
    assert!(workflow.contains(
        "nix develop -c npm --prefix \"$diag_dir\" install --silent azure-devops-node-api@15.1.2 >/dev/null"
    ));
    assert!(workflow.contains(
        "Marketplace extension is missing Public flag; setting it for ${publisher}.${extension}"
    ));
    assert!(
        workflow
            .contains("https://marketplace.visualstudio.com/_apis/public/gallery/extensionquery")
    );
    assert!(
        workflow.contains("$EXTENSION_ID is visible in the public VS Code Marketplace Gallery API")
    );
    assert!(workflow.contains("Verify VS Code Marketplace signatures"));
    assert!(workflow.contains("Microsoft.VisualStudio.Services.VSIXPackage"));
    assert!(workflow.contains("Microsoft.VisualStudio.Services.VsixSignature"));
    assert!(workflow.contains(".signature.manifest"));
    assert!(workflow.contains(".signature.p7s"));
    assert!(workflow.contains("nix shell nixpkgs#unzip -c unzip -q \"$signature_zip\""));
    assert!(workflow.contains(
        "nix develop -c npx --yes @vscode/vsce@latest verify-signature --packagePath \"$package_path\" --manifestPath \"$manifest_path\" --signaturePath \"$signature_path\""
    ));
    assert!(workflow.contains(
        "OVSX_NAMESPACE=$(nix shell nixpkgs#jq -c jq -r '.publisher' pkl-lsp-vscode/package.json)"
    ));
    assert!(workflow.contains(
        "nix develop -c npx --yes ovsx create-namespace \"$OVSX_NAMESPACE\" --pat \"$PUBLISH_PAT\" || true"
    ));
    assert!(workflow.contains(
        "nix develop -c npx --yes ovsx publish release/*.vsix --pat \"$PUBLISH_PAT\" --skip-duplicate"
    ));

    let check = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check.success());
}

#[test]
fn forgejo_nix_can_generate_jetbrains_publish_workflow() {
    let temp = init_package(true);
    let plugin_xml = temp
        .path()
        .join("example-jetbrains/src/main/resources/META-INF");
    fs::create_dir_all(&plugin_xml).unwrap();
    fs::write(
        plugin_xml.join("plugin.xml"),
        "<idea-plugin><id>com.example.demo</id></idea-plugin>\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
runtime = "nix"
runner = "atlas-nix-trusted"

[jetbrains]
        plugin_dir = "example-jetbrains"
plugin_xml_id = "com.example.demo"
package_installable = ".#jetbrains-plugin"
runner = "atlas-nix-trusted"
credential_source = "file-env"
prepublish_commands = ["nix flake check --no-build"]
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-jetbrains",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-jetbrains-plugin.yaml"),
    );
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("name: Publish JetBrains Plugin"));
    assert!(workflow.contains("runs-on: atlas-nix-trusted"));
    assert!(workflow.contains("INSTALLABLE: .#jetbrains-plugin"));
    assert!(workflow.contains("tags: [\"[0-9]*.[0-9]*.[0-9]*\", \"v[0-9]*.[0-9]*.[0-9]*\"]"));
    assert!(workflow.contains("  workflow_dispatch:\n    inputs:\n      version:"));
    assert!(workflow.contains("VERSION=\"${{ github.event.inputs.version || github.ref_name }}\""));
    assert!(workflow.contains("VERSION=\"${VERSION#v}\""));
    assert!(workflow.contains("<id>$PLUGIN_XML_ID</id>"));
    assert!(workflow.contains("CERTIFICATE_CHAIN_FILE=\"$RUNNER_TEMP/certificate-chain.pem\""));
    assert!(workflow.contains(
        "nix shell nixpkgs#gradle_9 nixpkgs#jdk21 -c gradle --no-daemon -x buildPlugin signPlugin verifyPluginSignature"
    ));
    assert!(workflow.contains("-F \"xmlId=$PLUGIN_XML_ID\""));
    assert!(
        workflow
            .contains("-F \"file=@$RUNNER_TEMP/jetbrains-plugin-signed.zip;type=application/zip\"")
    );
    assert!(workflow.contains("name: example-jetbrains-${{ github.ref_name }}"));
    assert!(workflow.contains("jetbrains-plugin-archive-name"));
    assert!(!workflow.contains("pluginId="));
    assert!(!workflow.contains("pkl-lsp-jetbrains"));
    assert!(workflow.contains("upload-artifact"));
    assert!(!workflow.contains("secrets.JETBRAINS_MARKETPLACE_TOKEN"));
}

#[test]
fn forgejo_nix_runtime_uses_devshell_quality_tools_without_cargo_install() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-audit",
            "--with-deny",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("      - name: Check cargo-audit tool\n"));
    assert!(ci.contains("run: nix develop -c cargo-audit --version"));
    assert!(ci.contains("      - name: Check cargo-deny tool\n"));
    assert!(ci.contains("run: nix develop -c cargo-deny --version"));
    assert!(!ci.contains("nix develop -c cargo install cargo-audit"));
    assert!(!ci.contains("nix develop -c cargo install cargo-deny"));
}

#[test]
fn forgejo_nix_with_om_ci_replace_emits_om_ci_step() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-om-ci",
            "--with-docs",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("OMNIX_REF:"));
    assert!(ci.contains("nix run \"$OMNIX_REF\" -- ci run"));
    assert!(ci.contains("run: nix develop .#docs -c cargo doc --no-deps --all-features"));
    assert!(!ci.contains("run: nix develop -c cargo doc --no-deps --all-features"));
    assert!(!ci.ends_with("\n\n"));
    assert!(!ci.contains("run: nix flake check"));
    assert!(!ci.contains("nix develop -c cargo test"));
    assert!(!ci.contains("nix develop -c cargo clippy"));
    assert!(!ci.contains("nix develop -c cargo package"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("OMNIX_REF:"));
    assert!(publish.contains("nix run \"$OMNIX_REF\" -- ci run"));
    assert!(publish.contains(
        r#"nix develop -c cargo pkgid -p demo | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#
    ));
    assert!(publish.contains("nix develop -c cargo publish --dry-run"));
    assert!(!publish.contains("run: nix flake check"));
    assert!(!publish.contains("nix develop -c cargo test"));
    assert!(!publish.contains("nix develop -c cargo clippy"));
}

#[test]
fn forgejo_nix_with_om_ci_augment_keeps_legacy_steps() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--om-ci-augment",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("OMNIX_REF:"));
    assert!(ci.contains("nix run \"$OMNIX_REF\" -- ci run"));
    assert!(ci.contains("run: nix flake check"));
    assert!(ci.contains("nix develop -c cargo test"));
    assert!(ci.contains("nix develop -c cargo clippy --all-targets -- --deny warnings"));
}

#[test]
fn forgejo_nix_msrv_uses_devshell_toolchain() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-msrv",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("run: nix develop -c cargo check --all-targets"));
    assert!(!ci.contains("cargo +1.85 check"));
}

#[test]
fn github_nix_with_om_ci_replace_keeps_install_nix_action() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--with-om-ci",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains(
        "uses: cachix/install-nix-action@630ae543ea3a38a9a4166f03376c02c50f408342 # v31"
    ));
    assert!(ci.contains("OMNIX_REF:"));
    assert!(ci.contains("nix run \"$OMNIX_REF\" -- ci run"));
    assert!(!ci.contains("run: nix flake check"));
}

#[test]
fn with_om_ci_without_runtime_nix_fails() {
    let temp = init_package(false);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--with-om-ci", "--platform", "forgejo"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--runtime nix"));
}

#[test]
fn omnix_ref_without_om_ci_warns_and_does_not_change_output() {
    let with_ref = init_package(true);
    let without_ref = init_package(true);

    let output = simit_with_user_config(with_ref.path())
        .current_dir(with_ref.path())
        .args([
            "init",
            "ci",
            "--omnix-ref",
            "github:x/y/z",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("ignored"));

    let status = simit_with_user_config(without_ref.path())
        .current_dir(without_ref.path())
        .args(["init", "ci", "--platform", "forgejo", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());

    assert_eq!(
        read(&with_ref.path().join(".forgejo/workflows/ci.yaml")),
        read(&without_ref.path().join(".forgejo/workflows/ci.yaml"))
    );
}

#[test]
fn project_config_om_ci_matches_cli_flag() {
    let from_config = init_package(true);
    fs::write(
        from_config.path().join("simit.toml"),
        "[ci]\nom_ci = true\n",
    )
    .unwrap();

    let config_status = simit_with_user_config(from_config.path())
        .current_dir(from_config.path())
        .args(["init", "ci", "--platform", "forgejo", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(config_status.success());

    let from_cli = init_package(true);
    let cli_status = simit_with_user_config(from_cli.path())
        .current_dir(from_cli.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-om-ci",
        ])
        .status()
        .unwrap();
    assert!(cli_status.success());

    assert_eq!(
        read(&from_config.path().join(".forgejo/workflows/ci.yaml")),
        read(&from_cli.path().join(".forgejo/workflows/ci.yaml"))
    );
}

#[test]
fn cli_omnix_ref_beats_project_config() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
om_ci = true
omnix_ref = "github:project/pin/v1"
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--omnix-ref",
            "github:cli/pin/v2",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("github:cli/pin/v2"));
    assert!(!ci.contains("github:project/pin/v1"));
}

#[test]
fn user_config_omnix_ref_used_when_no_override() {
    let temp = init_package(true);
    write_user_runner_config_with_omnix_ref(temp.path());

    let mut command = simit();
    let status = command
        .env("XDG_CONFIG_HOME", temp.path().join(".xdg"))
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-om-ci",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("github:user/pin/v3"));
}

#[test]
fn generates_github_plain_cargo_workflows() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-nextest",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: ubuntu-latest"));
    assert!(ci.contains(
        "uses: dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30 # stable"
    ));
    assert!(ci.contains("toolchain: stable"));
    assert!(ci.contains("uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0"));
    assert!(!ci.contains("https://code.forgejo.org/actions/cache@v4"));
    assert!(ci.contains("uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2"));
    assert!(ci.contains("path: ~/.cargo/bin"));
    assert!(ci.contains("hashFiles('.github/workflows/*.yaml')"));
    assert!(!ci.contains("hashFiles('.forgejo/workflows/ci.yaml', '.github/workflows/ci.yaml')"));
    assert!(ci.contains(
        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version 0.9.100"
    ));
    assert!(!ci.contains("&>/dev/null"));
    assert!(ci.contains("run: cargo nextest run --all-features"));
    assert!(ci.contains("run: cargo package --allow-dirty --list"));

    let publish = read(&temp.path().join(".github/workflows/publish-crate.yaml"));
    assert!(
        publish.contains("on:\n  push:\n    tags:\n      - \"[0-9]*\"\n  workflow_dispatch:\n")
    );
    assert!(
        publish.contains("uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0")
    );
    assert!(!publish.contains("https://code.forgejo.org/actions/cache@v4"));
    assert!(
        publish.contains("uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2")
    );
    assert!(publish.contains("path: ~/.cargo/bin"));
    assert!(publish.contains("hashFiles('.github/workflows/*.yaml')"));
    assert!(
        !publish.contains("hashFiles('.forgejo/workflows/ci.yaml', '.github/workflows/ci.yaml')")
    );
    assert!(publish.contains(
        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version 0.9.100"
    ));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(publish.contains("run: cargo publish --dry-run"));
    assert!(
        publish.contains(r#"cargo pkgid -p demo | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#)
    );
    assert!(publish.contains("export CARGO_REGISTRY_TOKEN="));
    assert!(publish.contains("already published on crates.io; skipping publish"));
    assert!(!publish.contains("cargo login"));
    // Regression: the publish workflow must not infer the release version from
    // the first package in workspace-wide cargo metadata.
    assert!(
        !publish.contains("cargo metadata --no-deps --format-version 1"),
        "publish workflow must use a package-scoped version extractor"
    );
}

#[test]
fn github_ignores_forgejo_step_runner_labels() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
provider = "actions"
platform = "github"
runtime = "nix"
runner = "ubuntu-24.04"
step_runners = { nix-check = "atlas-nix-trusted", cargo-test = "codeberg-medium" }
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--ci-provider",
            "actions",
            "--platform",
            "github",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: ubuntu-24.04"));
    assert!(!ci.contains("atlas-nix-trusted"));
    assert!(!ci.contains("codeberg-medium"));
}

#[test]
fn generated_workflows_include_project_ci_setup_and_env() {
    let temp = init_package(true);
    write_ci_customization_config(temp.path());

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-artifacts",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    for path in [
        ".forgejo/workflows/ci.yaml",
        ".forgejo/workflows/publish-crate.yaml",
        ".forgejo/workflows/release-artifacts.yaml",
    ] {
        let workflow = read(&temp.path().join(path));
        assert_yaml_parses(&workflow);
        assert!(workflow.contains("# Project-required secrets:\n# - SKILLNET_TEST_PG_URL"));
        assert!(workflow.contains(
            "    env:\n      NIX_CONFIG: \"experimental-features = nix-command flakes\"\n      XDG_CACHE_HOME: \"/tmp/.cache\"\n      CARGO_HOME: \"/tmp/.cargo\"\n      SKILLNET_TEST_PG_URL: \"${{ secrets.SKILLNET_TEST_PG_URL }}\""
        ));
        assert!(workflow.contains("      - name: Validate required environment"));
        assert!(workflow.contains(
            "if [ -z \"${VSCE_PAT_FILE:-}\" ]; then echo 'VSCE_PAT_FILE is required by simit project configuration.' >&2; exit 1; fi"
        ));
        assert!(workflow.contains(
            "if [ ! -r \"${VSCE_PAT_FILE}\" ] || [ ! -s \"${VSCE_PAT_FILE}\" ]; then echo 'VSCE_PAT_FILE must point to a readable, non-empty file.' >&2; exit 1; fi"
        ));
        assert!(workflow.contains(
            "if [ -z \"${OVSX_PAT_FILE:-}\" ]; then echo 'OVSX_PAT_FILE is required by simit project configuration.' >&2; exit 1; fi"
        ));
        assert!(workflow.contains(
            "if [ ! -r \"${OVSX_PAT_FILE}\" ] || [ ! -s \"${OVSX_PAT_FILE}\" ]; then echo 'OVSX_PAT_FILE must point to a readable, non-empty file.' >&2; exit 1; fi"
        ));
        assert!(workflow.contains("      - name: Project setup\n        run: apt-get update && apt-get install -y --no-install-recommends postgresql-client"));
    }
}

#[test]
fn forgejo_auto_runtime_uses_rust_container_even_when_flake_exists() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--with-nextest",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: atlas"));
    assert!(ci.contains("cancel-in-progress: true"));
    assert!(ci.contains("container: rust:1.85-bookworm"));
    assert!(!ci.contains("SCCACHE_REDIS_ENDPOINT:"));
    assert!(!ci.contains("SCCACHE_REDIS_KEY_PREFIX:"));
    assert!(ci.contains("name: Verify compiler cache"));
    assert!(ci.contains("/usr/local/bin/sccache --zero-stats"));
    assert!(ci.contains("name: Record compiler cache stats"));
    assert!(ci.contains("uses: https://code.forgejo.org/actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1"));
    assert!(ci.contains("uses: https://code.forgejo.org/actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0"));
    assert!(!ci.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(ci.contains("path: ~/.cargo/bin"));
    assert!(ci.contains("hashFiles('.forgejo/workflows/*.yaml')"));
    assert!(!ci.contains("hashFiles('.forgejo/workflows/ci.yaml', '.github/workflows/ci.yaml')"));
    assert!(ci.contains(
        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version 0.9.100"
    ));
    assert!(!ci.contains("&>/dev/null"));
    assert!(!ci.contains("run: apk add --no-cache git build-base"));
    assert!(ci.contains("run: rustup component add clippy rustfmt"));
    assert!(ci.contains("run: cargo nextest run --all-features"));
    assert!(!ci.contains("uses: https://github.com/cachix/install-nix-action@v31"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(publish.contains("runs-on: atlas"));
    assert!(publish.contains("container: rust:1.85-bookworm"));
    assert!(publish.contains("uses: https://code.forgejo.org/actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0"));
    assert!(!publish.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(publish.contains("path: ~/.cargo/bin"));
    assert!(publish.contains("hashFiles('.forgejo/workflows/*.yaml')"));
    assert!(
        !publish.contains("hashFiles('.forgejo/workflows/ci.yaml', '.github/workflows/ci.yaml')")
    );
    assert!(publish.contains(
        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version 0.9.100"
    ));
    assert!(
        publish.contains(r#"cargo pkgid -p demo | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#)
    );
}

#[test]
fn rendered_ci_yaml_parses_as_valid_yaml() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--with-nextest",
            "--with-audit",
            "--with-deny",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_yaml_parses(&ci);
}

#[test]
fn forgejo_runner_override_applies_to_all_jobs() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--runner",
            "codeberg-medium-lazy",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: codeberg-medium-lazy"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: codeberg-medium-lazy"));
}

#[test]
fn forgejo_runner_override_does_not_require_user_config() {
    let temp = init_package(false);

    let status = simit()
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runner",
            "atlas",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: atlas"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: atlas"));
}

#[test]
fn forgejo_user_config_can_render_structured_runner_labels() {
    let temp = init_package(false);

    let status = simit_with_multilabel_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--publish-crates"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: [\"self-hosted\", \"atlas\"]"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: [\"self-hosted\", \"atlas\"]"));
}

#[test]
fn forgejo_nix_runtime_uses_nix_runner_for_publish_jobs() {
    let temp = init_package(true);

    let status = simit_with_split_runtime_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: atlas-nix-trusted"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: atlas-nix-trusted"));
}

#[test]
fn featureful_package_gets_no_default_feature_checks() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "demo"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"

[features]
default = ["sync"]
sync = []
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

    let status = simit_with_user_config(root)
        .current_dir(root)
        .args(["init", "ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&root.join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("run: cargo test --all-features"));
    assert!(ci.contains("run: cargo test --no-default-features"));
    assert!(ci.contains("run: cargo clippy --all-targets --all-features -- --deny warnings"));
    assert!(
        ci.contains("run: cargo clippy --all-targets --no-default-features -- --deny warnings")
    );
}

#[test]
fn self_check_preserves_runner_override() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "simit"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();

    let status = simit_with_user_config(root)
        .current_dir(root)
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runner",
            "codeberg-medium",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&root.join(".forgejo/workflows/ci.yaml"));
    assert!(
        ci.contains("cargo run -- init ci --platform forgejo --runner codeberg-medium --check")
    );
}

#[test]
fn self_check_preserves_windows_packager_flags() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "simit"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
authors = ["Example Maintainers"]
license = "MIT"
description = "demo binary"
homepage = "https://example.com/demo"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    write_windows_packager_config(root);

    let status = simit_with_user_config(root)
        .current_dir(root)
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-chocolatey",
            "--with-scoop",
            "--windows-runner",
            "windows-latest",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&root.join(".github/workflows/ci.yaml"));
    assert!(ci.contains(
        "cargo run -- init ci --platform github --runner ubuntu-latest --windows-runner windows-latest --with-artifacts --publish-crates --with-chocolatey --with-scoop --check"
    ));
}

#[test]
fn check_succeeds_when_workflows_are_current() {
    let temp = init_package(true);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--publish-crates"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let check_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn workspace_flag_generates_per_package_workflows() {
    let temp = init_workspace_fixture();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    assert!(!temp.path().join(".forgejo/workflows/ci.yaml").exists());
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/publish-crate.yaml")
            .exists()
    );

    let alpha_ci = read(&temp.path().join(".forgejo/workflows/ci-alpha.yaml"));
    assert_yaml_parses(&alpha_ci);
    assert!(alpha_ci.contains("container: rust:1.85-bookworm"));
    assert!(alpha_ci.contains("run: cargo test -p alpha --all-features"));
    assert!(
        alpha_ci
            .contains("run: cargo clippy -p alpha --all-targets --all-features -- --deny warnings")
    );
    assert!(alpha_ci.contains("run: cargo package -p alpha --allow-dirty --list"));
    assert!(!alpha_ci.contains("--no-default-features"));

    let beta_ci = read(&temp.path().join(".forgejo/workflows/ci-beta.yaml"));
    assert_yaml_parses(&beta_ci);
    assert!(beta_ci.contains("run: cargo test -p beta --all-features"));
    assert!(beta_ci.contains("run: cargo test -p beta --no-default-features"));
    assert!(beta_ci.contains(
        "run: cargo clippy -p beta --all-targets --no-default-features -- --deny warnings"
    ));
    assert!(beta_ci.contains("run: cargo package -p beta --allow-dirty --list"));

    let alpha_publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-alpha.yaml"),
    );
    assert_yaml_parses(&alpha_publish);
    assert!(alpha_publish.contains("run: cargo publish -p alpha --dry-run"));
    assert!(alpha_publish.contains("cargo publish -p alpha"));

    let beta_publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-beta.yaml"),
    );
    assert_yaml_parses(&beta_publish);
    assert!(beta_publish.contains("run: cargo publish -p beta --dry-run"));
    assert!(beta_publish.contains("cargo publish -p beta"));
}

#[test]
fn aggregate_workspace_strategy_generates_one_workspace_workflow() {
    let temp = init_workspace_fixture();
    fs::write(temp.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = temp.path().join(".github/workflows/ci.yaml");
    assert!(workflow.exists());
    assert!(!temp.path().join(".github/workflows/ci-alpha.yaml").exists());
    assert!(!temp.path().join(".github/workflows/ci-beta.yaml").exists());

    let ci = read(&workflow);
    assert_yaml_parses(&ci);
    assert!(ci.contains("run: nix develop -c cargo test --workspace --all-features"));
    assert!(ci.contains(
        "run: nix develop -c cargo clippy --workspace --all-targets --all-features -- --deny warnings"
    ));
    assert!(!ci.contains("cargo package"));
}

#[test]
fn aggregate_workspace_strategy_can_skip_all_feature_checks() {
    let temp = init_workspace_fixture();
    fs::write(temp.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nall_features = false\n",
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains("run: nix develop -c cargo test --workspace\n"));
    assert!(
        ci.contains(
            "run: nix develop -c cargo clippy --workspace --all-targets -- --deny warnings"
        )
    );
    assert!(!ci.contains("--all-features"));
}

#[test]
fn aggregate_workspace_strategy_can_limit_tests_to_library_targets() {
    let temp = init_workspace_fixture();
    fs::write(temp.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        "[ci]\nunit_tests_only = true\n",
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(
        ci.contains("run: nix develop -c cargo test --workspace --lib"),
        "generated CI:\n{ci}"
    );
}

#[test]
fn workspace_publish_false_package_keeps_ci_but_skips_package_and_publish_workflows() {
    let temp = init_workspace_fixture();
    let beta_manifest = temp.path().join("crates/beta/Cargo.toml");
    let mut beta = fs::read_to_string(&beta_manifest).unwrap();
    beta = beta.replacen(
        "license = \"MIT\"\n",
        "license = \"MIT\"\npublish = false\n",
        1,
    );
    fs::write(&beta_manifest, beta).unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let beta_ci = read(&temp.path().join(".forgejo/workflows/ci-beta.yaml"));
    assert_yaml_parses(&beta_ci);
    assert!(beta_ci.contains("run: cargo test -p beta --all-features"));
    assert!(
        beta_ci
            .contains("run: cargo clippy -p beta --all-targets --all-features -- --deny warnings")
    );
    assert!(!beta_ci.contains("cargo package -p beta"));
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/publish-crate-beta.yaml")
            .exists()
    );

    let alpha_ci = read(&temp.path().join(".forgejo/workflows/ci-alpha.yaml"));
    assert!(alpha_ci.contains("run: cargo package -p alpha --allow-dirty --list"));
    assert!(
        temp.path()
            .join(".forgejo/workflows/publish-crate-alpha.yaml")
            .exists()
    );
}

#[test]
fn workspace_publish_tag_validation_is_package_scoped_for_diverging_versions() {
    let temp = init_diverging_workspace_fixture();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let member_a_publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-member-a.yaml"),
    );
    assert_yaml_parses(&member_a_publish);
    assert!(
        member_a_publish
            .contains(r#"cargo pkgid -p member-a | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#)
    );
    assert!(!member_a_publish.contains("cargo pkgid -p member-b"));
    assert!(!member_a_publish.contains("cargo metadata --no-deps --format-version 1"));

    let member_b_publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-member-b.yaml"),
    );
    assert_yaml_parses(&member_b_publish);
    assert!(
        member_b_publish
            .contains(r#"cargo pkgid -p member-b | awk -F'[#@]' 'NF > 1 {print $NF}' | tail -n 1"#)
    );
    assert!(!member_b_publish.contains("cargo pkgid -p member-a"));
    assert!(!member_b_publish.contains("cargo metadata --no-deps --format-version 1"));
}

#[test]
fn package_flag_generates_selected_package_workflows() {
    let temp = init_workspace_fixture();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--package",
            "beta",
            "--publish-crates",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    assert!(!temp.path().join(".forgejo/workflows/ci.yaml").exists());
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/ci-alpha.yaml")
            .exists()
    );
    assert!(temp.path().join(".forgejo/workflows/ci-beta.yaml").exists());

    let ci = read(&temp.path().join(".forgejo/workflows/ci-beta.yaml"));
    assert_yaml_parses(&ci);
    assert!(ci.contains("run: cargo test -p beta --all-features"));

    let publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-beta.yaml"),
    );
    assert_yaml_parses(&publish);
    assert!(publish.contains("run: cargo publish -p beta --dry-run"));
}

#[test]
fn workspace_check_diff_detects_stale_generated_workflow() {
    let temp = init_workspace_fixture();

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--workspace"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(
        temp.path().join(".forgejo/workflows/ci-alpha.yaml"),
        "name: stale\n",
    )
    .unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--check",
            "--diff",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(".forgejo/workflows/ci-alpha.yaml differs"));
    assert!(stderr.contains("--- .forgejo/workflows/ci-alpha.yaml"));
}

#[test]
fn workspace_check_rejects_extra_generated_workflow() {
    let temp = init_workspace_fixture();

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--workspace"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(
        temp.path().join(".forgejo/workflows/ci-old.yaml"),
        format!(
            "{}\nname: old\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        ),
    )
    .unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--check",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(".forgejo/workflows/ci-old.yaml is extra"));
}

#[test]
fn workspace_check_ignores_generated_release_workflow() {
    let temp = init_workspace_fixture();

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--workspace"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(
        temp.path().join(".forgejo/workflows/release.yml"),
        format!(
            "{}\nname: release\n",
            simit::render::ci::GENERATED_WORKFLOW_MARKER
        ),
    )
    .unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--workspace",
            "--check",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn init_ci_blocks_without_exportable_maintainer_key() {
    let temp = init_package(false);
    let isolated_home = TempDir::new().unwrap();
    write_user_runner_config(isolated_home.path());

    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(temp.path())
        .env_remove("SIMIT_MAINTAINERS_GPG")
        .env("HOME", isolated_home.path())
        .env("XDG_CONFIG_HOME", isolated_home.path().join(".xdg"))
        .env("XDG_DATA_HOME", common::data_home_path())
        .env("GIT_CONFIG_GLOBAL", isolated_home.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "true")
        .args(["init", "ci", "--platform", "forgejo", "--publish-crates"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("release signing key not configured"));
    assert!(stderr.contains("git config user.signingkey"));
}

#[test]
fn release_trust_init_writes_maintainer_key() {
    let temp = init_package(false);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["release", "trust", "init"])
        .status()
        .unwrap();

    assert!(status.success());
    assert_maintainer_key_written(temp.path());
}

#[test]
fn release_trust_check_accepts_existing_trust_root_without_local_key() {
    let temp = init_package(false);

    let init_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["release", "trust", "init"])
        .status()
        .unwrap();
    assert!(init_status.success());

    let isolated_home = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(temp.path())
        .env_remove("SIMIT_MAINTAINERS_GPG")
        .env("HOME", isolated_home.path())
        .env("XDG_CONFIG_HOME", isolated_home.path().join("xdg"))
        .env("XDG_DATA_HOME", common::data_home_path())
        .env("GIT_CONFIG_GLOBAL", isolated_home.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "true")
        .args(["release", "trust", "check"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("keys/maintainers.gpg is present and parseable"));
}

#[test]
fn check_fails_when_workflows_differ() {
    let temp = init_package(true);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--publish-crates"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(
        temp.path().join(".forgejo/workflows/ci.yaml"),
        "name: stale\n",
    )
    .unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--publish-crates",
            "--check",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("CI workflows are not up to date"));
    assert!(
        stderr.contains("run `simit init ci --platform forgejo`"),
        "stderr:\n{stderr}"
    );
    assert!(stderr.contains(".forgejo/workflows/ci.yaml differs"));
}

#[test]
fn check_failure_hint_includes_effective_generation_flags() {
    let temp = init_workspace_fixture();
    fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--runner",
            "atlas",
            "--workspace",
            "--with-deny",
            "--with-artifacts",
        ])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(
        temp.path().join(".forgejo/workflows/ci-alpha.yaml"),
        "name: stale\n",
    )
    .unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--runner",
            "atlas",
            "--workspace",
            "--with-deny",
            "--with-artifacts",
            "--check",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("run `simit init ci --platform forgejo --runtime nix --runner atlas"),
        "stderr:\n{stderr}"
    );
    assert!(stderr.contains(".forgejo/workflows/ci-alpha.yaml differs"));
}

#[test]
fn init_ci_persists_resolved_options_in_simit_toml() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-audit",
            "--with-deny",
            "--with-docs",
            "--with-msrv",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let config = read(&temp.path().join("simit.toml"));
    assert!(config.contains("[ci]"));
    assert!(config.contains("runtime = \"nix\""));
    assert!(config.contains("runner = \"atlas\""));
    assert!(config.contains("with_audit = true"));
    assert!(config.contains("with_deny = true"));
    assert!(config.contains("with_docs = true"));
    assert!(config.contains("with_msrv = true"));
}

#[test]
fn bare_check_uses_persisted_ci_options() {
    let temp = init_package(true);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-audit",
            "--with-deny",
        ])
        .status()
        .unwrap();
    assert!(write_status.success());

    let check_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check", "--diff"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn cli_flags_override_persisted_ci_values() {
    let temp = init_package(false);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[ci]
with_audit = false
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--with-audit"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains("cargo audit --db \"$db\" --no-fetch --stale"));

    let config = read(&temp.path().join("simit.toml"));
    assert!(config.contains("with_audit = true"));
}

#[test]
fn init_ci_write_preserves_existing_distribution_sections() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"# keep this comment
[homebrew]
tap_url = "https://example.com/homebrew-demo.git"
download_repo = "foo/demo"

[chocolatey]
download_repo = "foo/demo"

[chocolatey.push]
source = "https://push.chocolatey.org/"

[scoop]
bucket_url = "https://example.com/scoop-demo.git"
download_repo = "foo/demo"
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-audit",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let config = read(&temp.path().join("simit.toml"));
    assert!(config.contains("# keep this comment"));
    assert!(config.contains("[homebrew]"));
    assert!(config.contains("tap_url = \"https://example.com/homebrew-demo.git\""));
    assert!(config.contains("[chocolatey]"));
    assert!(config.contains("[chocolatey.push]"));
    assert!(config.contains("[scoop]"));
    assert!(config.contains("with_audit = true"));
}

#[test]
fn check_fails_when_publish_workflow_is_missing() {
    let temp = init_package(true);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--publish-crates"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::remove_file(temp.path().join(".forgejo/workflows/publish-crate.yaml")).unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("CI workflows are not up to date"));
    assert!(stderr.contains(".forgejo/workflows/publish-crate.yaml is missing"));
}

#[test]
fn simit_package_gets_self_check_step() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "simit"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
"#,
    )
    .unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();

    let status = simit_with_user_config(root)
        .current_dir(root)
        .args(["init", "ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&root.join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("cargo run -- init ci --platform forgejo --runner atlas --check"));
    assert!(ci.contains("cargo run -- init flake --check"));
    assert!(!ci.contains("cargo run -- ci"));
}

#[test]
fn ci_command_is_not_available() {
    let temp = init_package(false);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["ci"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unrecognized subcommand 'ci'"));
}

#[test]
fn old_init_ci_command_is_not_available() {
    let temp = init_package(false);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unrecognized subcommand 'init-ci'"));
}

#[test]
fn optional_strict_flags_render_expected_steps() {
    let temp = init_package(false);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--with-msrv",
            "--with-audit",
            "--with-deny",
            "--with-docs",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains(
        "run: command -v cargo-audit >/dev/null 2>&1 || cargo install cargo-audit --locked"
    ));
    assert!(ci.contains("      - name: Fetch RustSec advisory database"));
    assert!(ci.contains(
        "git -c http.lowSpeedLimit=1024 -c http.lowSpeedTime=30 clone --depth 1 https://github.com/RustSec/advisory-db.git \"$db\""
    ));
    assert!(ci.contains("cargo audit --db \"$db\" --no-fetch --stale"));
    assert!(ci.contains(
        "run: command -v cargo-deny >/dev/null 2>&1 || cargo install cargo-deny --locked --version 0.18.3"
    ));
    assert!(ci.contains("run: cargo deny check bans licenses sources"));
    assert!(ci.contains("run: cargo +1.85 check --all-targets"));
    assert!(ci.contains("run: cargo doc --no-deps --all-features"));
    assert!(!ci.contains("&>/dev/null"));

    let deny = read(&temp.path().join("deny.toml"));
    assert!(deny.contains("\"MIT\""));
    assert!(deny.contains("\"BSD-2-Clause\""));
    assert!(deny.contains("\"BSL-1.0\""));
    assert!(deny.contains("\"CC0-1.0\""));
    assert!(deny.contains("\"ISC\""));
    assert!(deny.contains("\"LicenseRef-UFL-1.0\""));
    assert!(deny.contains("\"Apache-2.0\""));
    assert!(deny.contains("\"OFL-1.1\""));
    assert!(deny.contains("\"Unicode-3.0\""));
    assert!(deny.contains("\"Unlicense\""));
    assert!(deny.contains("\"Zlib\""));
    assert!(deny.contains("allow-registry = [\"https://github.com/rust-lang/crates.io-index\"]"));
}

#[test]
fn forgejo_nix_homebrew_step_matches_hardened_shape() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-artifacts",
            "--with-homebrew",
            "--homebrew-tap",
            "https://example.com/homebrew-demo.git",
            "--homebrew-description",
            "demo binary",
            "--homebrew-homepage",
            "https://example.com",
            "--homebrew-download-repo",
            "foo/demo",
            "--homebrew-binary",
            "demo",
            "--homebrew-binary",
            "demo-ui",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(
        &temp
            .path()
            .join(".forgejo/workflows/release-artifacts.yaml"),
    );
    assert_homebrew_run_block_indentation(&workflow);
    assert_release_integrity_steps(&workflow);
    assert!(workflow.contains("enable-openid-connect: true"));
    assert!(workflow.contains("name: Publish Homebrew tap"));
    assert!(workflow.contains("HOMEBREW_TAP_TOKEN: ${{ secrets.homebrew_tap_token }}"));
    assert!(workflow.contains("HOMEBREW_TAP_REPO: homebrew-demo"));
    assert!(workflow.contains("HOMEBREW_TAP_URL: https://example.com/homebrew-demo.git"));
    assert!(workflow.contains("set -euo pipefail"));
    assert!(
        workflow.contains(
            "HOMEBREW_TAP_TOKEN is required because Homebrew tap publishing is configured."
        )
    );
    assert!(workflow.contains("\"release/demo-${VERSION}-aarch64-darwin.tar.gz\""));
    assert!(!workflow.contains("\"release/demo-${VERSION}-x86_64-darwin.tar.gz\""));
    assert!(workflow.contains("\"release/demo-${VERSION}-aarch64-linux.tar.gz\""));
    assert!(workflow.contains("\"release/demo-${VERSION}-x86_64-linux.tar.gz\""));
    assert!(workflow.contains("credential_helper='!f() { echo username=caniko; echo \"password=$HOMEBREW_TAP_TOKEN\"; }; f'"));
    assert!(workflow.contains(
        "git -c credential.helper=\"$credential_helper\" clone \"$HOMEBREW_TAP_URL\" tap"
    ));
    assert!(!workflow.contains("https://$HOMEBREW_TAP_TOKEN"));
    assert!(workflow.contains("nix run '.#rs-harbor' -- brew bump \\"));
    assert!(workflow.contains("--name demo \\"));
    assert!(workflow.contains("--description 'demo binary' \\"));
    assert!(workflow.contains("--homepage 'https://example.com' \\"));
    assert!(workflow.contains("--license MIT \\"));
    assert!(workflow.contains("--archive \"darwin_arm=https://codeberg.org/foo/demo/releases/download/${VERSION}/demo-${VERSION}-aarch64-darwin.tar.gz,release/demo-${VERSION}-aarch64-darwin.tar.gz\" \\"));
    assert!(!workflow.contains("--archive \"darwin_intel=https://codeberg.org/foo/demo/releases/download/${VERSION}/demo-${VERSION}-x86_64-darwin.tar.gz,release/demo-${VERSION}-x86_64-darwin.tar.gz\" \\"));
    assert!(workflow.contains("--archive \"linux_arm=https://codeberg.org/foo/demo/releases/download/${VERSION}/demo-${VERSION}-aarch64-linux.tar.gz,release/demo-${VERSION}-aarch64-linux.tar.gz\" \\"));
    assert!(workflow.contains("--archive \"linux_intel=https://codeberg.org/foo/demo/releases/download/${VERSION}/demo-${VERSION}-x86_64-linux.tar.gz,release/demo-${VERSION}-x86_64-linux.tar.gz\" \\"));
    assert_eq!(workflow.matches("--binary ").count(), 2);
    assert!(workflow.contains("--binary demo \\"));
    assert!(workflow.contains("--binary demo-ui \\"));
    assert!(workflow.contains("if [ -z \"$(git status --porcelain -- Formula/demo.rb)\" ]; then"));
    assert!(workflow.contains("tap already contains demo ${VERSION}; nothing to push"));
    assert!(workflow.contains("git push origin \"HEAD:${DEFAULT_BRANCH}\""));

    let check_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-artifacts",
            "--with-homebrew",
            "--homebrew-tap",
            "https://example.com/homebrew-demo.git",
            "--homebrew-description",
            "demo binary",
            "--homebrew-homepage",
            "https://example.com",
            "--homebrew-download-repo",
            "foo/demo",
            "--homebrew-binary",
            "demo",
            "--homebrew-binary",
            "demo-ui",
            "--check",
        ])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn homebrew_rejects_cargo_runtime() {
    let temp = init_package(true);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "cargo",
            "--with-homebrew",
            "--homebrew-tap",
            "https://example.com/homebrew-demo.git",
            "--homebrew-description",
            "demo binary",
            "--homebrew-homepage",
            "https://example.com",
            "--homebrew-download-repo",
            "foo/demo",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Homebrew tap publish requires --runtime nix"));
}

#[test]
fn homebrew_supports_github_platform() {
    let temp = init_package(true);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "nix",
            "--with-homebrew",
            "--homebrew-tap",
            "https://example.com/homebrew-demo.git",
            "--homebrew-description",
            "demo binary",
            "--homebrew-homepage",
            "https://example.com",
            "--homebrew-download-repo",
            "foo/demo",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let workflow = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("name: Publish Homebrew tap"));
    assert!(workflow.contains("https://github.com/foo/demo/releases/download/"));
    assert!(workflow.contains("VERSION=\"${GITHUB_REF_NAME:-"));
}

#[test]
fn github_chocolatey_flag_resolves_when_config_is_present() {
    let temp = init_package(false);
    write_chocolatey_config(temp.path());

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-chocolatey",
            "--windows-runner",
            "windows-latest",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    let workflow = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_yaml_parses(&workflow);
    assert_release_integrity_steps(&workflow);
    assert!(workflow.contains("id-token: write"));
    assert!(workflow.contains("name: Release Artifacts"));
    assert!(workflow.contains("build-linux:"));
    assert!(workflow.contains("build-windows:"));
    assert!(workflow.contains("runs-on: windows-latest"));
    assert!(workflow.contains("target: x86_64-pc-windows-msvc"));
    assert!(workflow.contains("demo-$version-x86_64-windows.zip"));
    assert!(workflow.contains("name: Install Chocolatey"));
    assert!(workflow.contains("name: Publish Chocolatey package"));
    assert!(workflow.contains("CHOCOLATEY_API_KEY: ${{ secrets.chocolatey_api_key }}"));
    assert!(workflow.contains(
        "CHOCOLATEY_API_KEY is required because Chocolatey package publishing is configured."
    ));
    assert!(workflow.contains("simit dist chocolatey bump `"));
    assert!(workflow.contains("--archive \"x64=release/demo-$version-x86_64-windows.zip\" `"));
    assert!(workflow.contains("--push-source \"$env:CHOCO_PUSH_SOURCE\" `"));
}

#[test]
fn release_artifact_workflow_runs_configured_smoke_before_publish() {
    let temp = init_package(true);
    write_release_smoke_config(temp.path());

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-artifacts",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(
        &temp
            .path()
            .join(".forgejo/workflows/release-artifacts.yaml"),
    );
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("name: Run release smoke checks"));
    assert!(!workflow.contains("FORCE_PUBLISH: ${{ inputs.force_publish }}"));
    assert!(!workflow.contains("inputs.force_publish"));
    assert!(workflow.contains("nix run .#release-smoke -- \"$VERSION\" release"));
    assert!(
        workflow
            .find("name: Generate signed checksums and provenance")
            .unwrap()
            < workflow.find("name: Run release smoke checks").unwrap()
    );
}

#[test]
fn github_chocolatey_flag_errors_when_config_is_absent() {
    let temp = init_package(false);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--with-chocolatey"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("chocolatey.download_repo not set"));
}

#[test]
fn github_scoop_flag_resolves_when_config_is_present() {
    let temp = init_package(false);
    write_scoop_config(temp.path());

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--with-scoop"])
        .status()
        .unwrap();

    assert!(status.success());
    let workflow = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("name: Release Artifacts"));
    assert!(workflow.contains("build-linux:"));
    assert!(workflow.contains("build-windows:"));
    assert!(workflow.contains("target: x86_64-pc-windows-msvc"));
    assert!(workflow.contains("target: aarch64-pc-windows-msvc"));
    assert!(workflow.contains("windows-${{ matrix.arch }}"));
    assert!(workflow.contains("name: Publish Scoop bucket"));
    assert!(workflow.contains("SCOOP_BUCKET_TOKEN: ${{ secrets.SCOOP_BUCKET_TOKEN }}"));
    assert!(
        workflow.contains(
            "SCOOP_BUCKET_TOKEN is required because Scoop bucket publishing is configured."
        )
    );
    assert!(workflow.contains(
        "git -c credential.helper=\"$credentialHelper\" clone \"$env:SCOOP_BUCKET_URL\" bucket"
    ));
    assert!(workflow.contains("simit dist scoop bump `"));
    assert!(workflow.contains("--archive \"x64=release/demo-$version-x86_64-windows.zip\" `"));
    assert!(workflow.contains("--archive \"arm64=release/demo-$version-aarch64-windows.zip\" `"));
}

#[test]
fn github_scoop_respects_no_arch_and_custom_archive_pattern() {
    let temp = init_package(false);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[scoop]
bucket_url = "https://example.com/scoop-demo.git"
description = "demo binary"
homepage = "https://example.com/demo"
license = "MIT"
download_repo = "foo/demo"
archive_pattern = "demo-windows-{arch}-{version}.zip"

[scoop.architectures]
arm64 = false
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--with-scoop"])
        .status()
        .unwrap();

    assert!(status.success());
    let workflow = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("demo-windows-x86_64-$version.zip"));
    assert!(workflow.contains("--archive \"x64=release/demo-windows-x86_64-$version.zip\" `"));
    assert!(!workflow.contains("aarch64-pc-windows-msvc"));
    assert!(!workflow.contains("--archive \"arm64="));
}

#[test]
fn github_scoop_flag_errors_when_config_is_absent() {
    let temp = init_package(false);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github", "--with-scoop"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("scoop.bucket_url not set"));
}

#[test]
fn github_chocolatey_and_scoop_share_windows_artifacts() {
    let temp = init_package(false);
    write_windows_packager_config(temp.path());

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-chocolatey",
            "--with-scoop",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    let workflow = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_yaml_parses(&workflow);
    assert_eq!(workflow.matches("build-windows:").count(), 1);
    assert_eq!(
        workflow
            .matches("run: cargo build --release --locked\n")
            .count(),
        1
    );
    assert_eq!(
        workflow
            .matches("run: cargo build --release --locked --target ${{ matrix.target }}")
            .count(),
        1
    );
    assert!(workflow.contains("name: Publish Chocolatey package"));
    assert!(workflow.contains("name: Publish Scoop bucket"));
    assert!(workflow.contains("needs: build-windows"));
    assert!(workflow.contains("pattern: windows-*"));

    let first = workflow;
    let second_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-chocolatey",
            "--with-scoop",
        ])
        .status()
        .unwrap();
    assert!(second_status.success());
    let second = read(&temp.path().join(".github/workflows/release-artifacts.yaml"));
    assert_eq!(first, second);
}

#[test]
fn github_chocolatey_scoop_and_homebrew_keep_single_linux_job() {
    let temp = init_package(true);
    fs::write(
        temp.path().join("simit.toml"),
        r#"[homebrew]
tap_url = "https://example.com/homebrew-demo.git"
description = "demo binary"
homepage = "https://example.com/demo"
license = "MIT"
download_repo = "foo/demo"

[chocolatey]
authors = "Example Maintainers"
description = "demo binary"
project_url = "https://example.com/demo"
download_repo = "foo/demo"

[scoop]
bucket_url = "https://example.com/scoop-demo.git"
description = "demo binary"
homepage = "https://example.com/demo"
license = "MIT"
download_repo = "foo/demo"
"#,
    )
    .unwrap();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--runtime",
            "nix",
            "--with-homebrew",
            "--with-chocolatey",
            "--with-scoop",
            "--windows-runner",
            "windows-atlas",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    let workflow = read(
        &temp
            .path()
            .join(".forgejo/workflows/release-artifacts.yaml"),
    );
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("build-linux:"));
    assert!(workflow.contains("runs-on: atlas"));
    assert!(!workflow.contains("uses: https://github.com/cachix/install-nix-action@v31"));
    assert!(workflow.contains("name: Publish Homebrew tap"));
    assert!(workflow.contains("build-windows:"));
    assert!(workflow.contains("runs-on: windows-atlas"));
    assert_eq!(workflow.matches("name: Build package").count(), 1);
    assert_eq!(workflow.matches("name: Publish Homebrew tap").count(), 1);
    assert_eq!(
        workflow.matches("name: Publish Chocolatey package").count(),
        1
    );
    assert_eq!(workflow.matches("name: Publish Scoop bucket").count(), 1);
}

#[test]
fn forgejo_requires_user_runner_config_when_no_override_exists() {
    let temp = init_package(false);
    write_chocolatey_config(temp.path());
    let isolated_home = TempDir::new().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_simit"))
        .current_dir(temp.path())
        .env("HOME", isolated_home.path())
        .env("XDG_CONFIG_HOME", isolated_home.path().join(".xdg"))
        .env("XDG_DATA_HOME", common::data_home_path())
        .env("GIT_CONFIG_GLOBAL", isolated_home.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "true")
        .env("SIMIT_MAINTAINERS_GPG", common::maintainer_key_path())
        .args(["init", "ci", "--platform", "forgejo", "--with-chocolatey"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("simit user config"));
    assert!(stderr.contains("simit config init"));
}

#[test]
fn chocolatey_implies_artifacts_even_when_artifacts_false() {
    let temp = init_package(false);
    write_chocolatey_config(temp.path());

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--with-chocolatey",
            "--with-artifacts=false",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--with-chocolatey implies --with-artifacts; enabling it."));
    assert!(
        temp.path()
            .join(".github/workflows/release-artifacts.yaml")
            .exists()
    );
}

#[test]
fn check_ignores_existing_deny_policy_customization() {
    let temp = init_package(false);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--with-deny"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(temp.path().join("deny.toml"), "[licenses]\n").unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "forgejo",
            "--with-deny",
            "--check",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
}

#[test]
fn forgejo_python_uv_ci_uses_nix_checks() {
    let temp = init_python_project();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("runs-on: atlas"));
    assert!(workflow.contains(
        "nix run --no-write-lock-file git+https://github.com/caniko/simit.git -- init flake --check --diff"
    ));
    assert!(
        !workflow.contains(
            "nix run git+https://github.com/caniko/simit.git -- init flake --check --diff"
        )
    );
    assert!(workflow.contains("nix flake check --no-build"));
    assert!(workflow.contains("nix build .#checks.x86_64-linux.offline-tests"));
    assert!(workflow.contains("nix build .#checks.x86_64-linux.typecheck"));
    assert!(!workflow.contains("cargo test"));
    assert!(!workflow.contains("CARGO_HOME"));
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/publish-crate.yaml")
            .exists()
    );
    assert!(
        !temp
            .path()
            .join(".forgejo/workflows/release-artifacts.yaml")
            .exists()
    );

    let check_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn python_ci_component_selection_is_granular() {
    let temp = init_python_project();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[flake]
scope = "full"
mode = "custom"
backend = "py-harbor"

[flake.expected_outputs]
checks = ["offline-tests", "typecheck"]

[ci]
runtime = "nix"
components = ["checks"]
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(workflow.contains("nix build .#checks.x86_64-linux.offline-tests"));
    assert!(!workflow.contains("Check generated flake wiring"));
    assert!(!workflow.contains("Check flake evaluation"));
    assert!(!workflow.contains("CARGO_HOME"));
}

#[test]
fn python_publish_uses_configured_pypi_token_secret() {
    let temp = init_python_project();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[flake]
scope = "full"
mode = "custom"
backend = "py-harbor"

[flake.expected_outputs]
checks = ["offline-tests", "typecheck"]

[ci]
runtime = "nix"
with_pypi_publish = true
pypi_token_secret = "PYPI_API_TOKEN"
"#,
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "github"])
        .status()
        .unwrap();
    assert!(status.success());

    let workflow = read(&temp.path().join(".github/workflows/publish-pypi.yaml"));
    assert!(workflow.contains("UV_PUBLISH_TOKEN: ${{ secrets.PYPI_API_TOKEN }}"));
    assert!(!workflow.contains("secrets.PYPI_TOKEN"));
    assert!(workflow.contains("Tag must be an exact semver version"));
    assert!(workflow.contains("builtins.fromTOML"));
    assert!(!read(&temp.path().join(".github/workflows/ci.yaml")).contains("Validate release tag"));

    let config = read(&temp.path().join("simit.toml")).replace(
        "pypi_token_secret = \"PYPI_API_TOKEN\"",
        "pypi_trusted_publishing = true",
    );
    fs::write(temp.path().join("simit.toml"), config).unwrap();
    assert!(
        simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--platform", "github"])
            .status()
            .unwrap()
            .success()
    );

    let workflow = read(&temp.path().join(".github/workflows/publish-pypi.yaml"));
    assert_yaml_parses(&workflow);
    assert!(workflow.contains("environment: pypi"));
    assert!(workflow.contains("contents: read"));
    assert!(workflow.contains("id-token: write"));
    assert!(workflow.contains("uv publish --trusted-publishing always"));
    assert!(!workflow.contains("UV_PUBLISH_TOKEN"));
}

#[test]
fn generates_crow_yaml_and_jsonnet_without_a_crow_cli() {
    let project = init_package(false);
    let output = simit()
        .current_dir(project.path())
        .args([
            "init",
            "ci",
            "--ci-provider",
            "crow",
            "--runner",
            "crow-agent",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let yaml = read(&project.path().join(".crow/build.yaml"));
    assert!(yaml.contains("name: build"));
    assert!(yaml.contains("labels:"));
    assert!(yaml.contains("platform:"));

    let project = init_package(false);
    let output = simit()
        .current_dir(project.path())
        .args([
            "init",
            "ci",
            "--ci-provider",
            "crow",
            "--crow-format",
            "jsonnet",
            "--runner",
            "crow-agent",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        read(&project.path().join(".crow/build.jsonnet")).starts_with("// Generated by simit.")
    );
}

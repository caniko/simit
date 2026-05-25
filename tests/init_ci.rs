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
    assert!(workflow.contains("cosign sign-blob --yes --identity-token \"$oidc_token\""));
    assert!(workflow.contains("cosign attest-blob --yes --identity-token \"$oidc_token\""));
    assert!(workflow.contains("--type slsaprovenance1"));
    assert!(workflow.contains("--output-attestation \"${file}.intoto.jsonl\""));
    assert!(workflow.contains("--bundle \"${file}.intoto.bundle\""));
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
        .args(["init", "ci", "--platform", "forgejo", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: atlas"));
    assert!(ci.contains("NIX_CONFIG: \"experimental-features = nix-command flakes\""));
    assert!(ci.contains("XDG_CACHE_HOME: \"/tmp/.cache\""));
    assert!(ci.contains("group: ${{ github.workflow }}-${{ github.ref }}"));
    assert!(ci.contains("uses: https://code.forgejo.org/actions/checkout@v4"));
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
    assert!(!publish.contains("uses: https://github.com/cachix/install-nix-action@v31"));
    assert!(!publish.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(!publish.contains("path: ~/.cargo/bin"));
    assert!(!publish.contains("command -v cargo-nextest"));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(publish.contains("tags:"));
    assert!(publish.contains("grep -Eq '^[0-9]+\\.[0-9]+\\.[0-9]+$'"));
    assert!(publish.contains("keys/maintainers.gpg"));
    assert!(publish.contains("git verify-tag \"$tag\""));
    assert!(publish.contains(r#"nix develop -c cargo pkgid -p demo | awk -F'[#@]' '{print $NF}'"#));
    assert!(publish.contains("CRATES_IO_API_TOKEN: ${{ secrets.CRATES_IO_API_TOKEN }}"));
    assert!(publish.contains("CRATES_IO_API_TOKEN is required"));
    assert!(publish.contains("export CARGO_REGISTRY_TOKEN="));
    assert!(publish.contains("https://crates.io/api/v1/crates/${crate_name}/${version}"));
    assert!(publish.contains("already published on crates.io; skipping publish"));
    assert!(!publish.contains("cargo login"));
    assert_maintainer_key_written(temp.path());
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
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("OMNIX_REF:"));
    assert!(ci.contains("nix run \"$OMNIX_REF\" -- ci run"));
    assert!(ci.contains("run: nix develop -c cargo doc --no-deps --all-features"));
    assert!(!ci.ends_with("\n\n"));
    assert!(!ci.contains("run: nix flake check"));
    assert!(!ci.contains("nix develop -c cargo test"));
    assert!(!ci.contains("nix develop -c cargo clippy"));
    assert!(!ci.contains("nix develop -c cargo package"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("OMNIX_REF:"));
    assert!(publish.contains("nix run \"$OMNIX_REF\" -- ci run"));
    assert!(publish.contains(r#"nix develop -c cargo pkgid -p demo | awk -F'[#@]' '{print $NF}'"#));
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
    assert!(ci.contains("uses: https://github.com/cachix/install-nix-action@v31"));
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
        .args(["init", "ci", "--platform", "github", "--with-nextest"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: ubuntu-latest"));
    assert!(ci.contains("uses: dtolnay/rust-toolchain@stable"));
    assert!(ci.contains("toolchain: stable"));
    assert!(ci.contains("uses: actions/cache@v4"));
    assert!(!ci.contains("https://code.forgejo.org/actions/cache@v4"));
    assert!(ci.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(ci.contains("path: ~/.cargo/bin"));
    assert!(ci.contains(
        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version 0.9.100"
    ));
    assert!(!ci.contains("&>/dev/null"));
    assert!(ci.contains("run: cargo nextest run --all-features"));
    assert!(ci.contains("run: cargo package --allow-dirty"));

    let publish = read(&temp.path().join(".github/workflows/publish-crate.yaml"));
    assert!(publish.contains("uses: actions/cache@v4"));
    assert!(!publish.contains("https://code.forgejo.org/actions/cache@v4"));
    assert!(publish.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(publish.contains("path: ~/.cargo/bin"));
    assert!(publish.contains(
        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version 0.9.100"
    ));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(publish.contains("run: cargo publish --dry-run"));
    assert!(publish.contains(r#"cargo pkgid -p demo | awk -F'[#@]' '{print $NF}'"#));
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
            "    env:\n      NIX_CONFIG: \"experimental-features = nix-command flakes\"\n      XDG_CACHE_HOME: \"/tmp/.cache\"\n      SKILLNET_TEST_PG_URL: \"${{ secrets.SKILLNET_TEST_PG_URL }}\""
        ));
        assert!(workflow.contains("      - name: Project setup\n        run: apt-get update && apt-get install -y --no-install-recommends postgresql-client"));
    }
}

#[test]
fn forgejo_auto_runtime_uses_rust_container_even_when_flake_exists() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--with-nextest"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: atlas"));
    assert!(ci.contains("cancel-in-progress: true"));
    assert!(ci.contains("container: rust:1.85-bookworm"));
    assert!(ci.contains("uses: https://code.forgejo.org/actions/checkout@v4"));
    assert!(ci.contains("uses: https://code.forgejo.org/actions/cache@v4"));
    assert!(!ci.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(ci.contains("path: ~/.cargo/bin"));
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
    assert!(publish.contains("uses: https://code.forgejo.org/actions/cache@v4"));
    assert!(!publish.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(publish.contains("path: ~/.cargo/bin"));
    assert!(publish.contains(
        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version 0.9.100"
    ));
    assert!(publish.contains(r#"cargo pkgid -p demo | awk -F'[#@]' '{print $NF}'"#));
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
        .args(["init", "ci", "--platform", "forgejo", "--runner", "atlas"])
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
        .args(["init", "ci", "--platform", "forgejo"])
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
        .args(["init", "ci", "--platform", "forgejo", "--runtime", "nix"])
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
        "cargo run -- init ci --platform github --windows-runner windows-latest --with-artifacts --with-chocolatey --with-scoop --check"
    ));
}

#[test]
fn check_succeeds_when_workflows_are_current() {
    let temp = init_package(true);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo"])
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
        .args(["init", "ci", "--platform", "forgejo", "--workspace"])
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
    assert!(alpha_ci.contains("run: cargo package -p alpha --allow-dirty"));
    assert!(!alpha_ci.contains("run: cargo package -p alpha --allow-dirty --no-verify"));
    assert!(!alpha_ci.contains("--no-default-features"));

    let beta_ci = read(&temp.path().join(".forgejo/workflows/ci-beta.yaml"));
    assert_yaml_parses(&beta_ci);
    assert!(beta_ci.contains("run: cargo test -p beta --all-features"));
    assert!(beta_ci.contains("run: cargo test -p beta --no-default-features"));
    assert!(beta_ci.contains(
        "run: cargo clippy -p beta --all-targets --no-default-features -- --deny warnings"
    ));
    assert!(beta_ci.contains("run: cargo package -p beta --allow-dirty --no-verify"));

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
fn workspace_publish_tag_validation_is_package_scoped_for_diverging_versions() {
    let temp = init_diverging_workspace_fixture();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--workspace"])
        .status()
        .unwrap();
    assert!(status.success());

    let member_a_publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-member-a.yaml"),
    );
    assert_yaml_parses(&member_a_publish);
    assert!(member_a_publish.contains(r#"cargo pkgid -p member-a | awk -F'[#@]' '{print $NF}'"#));
    assert!(!member_a_publish.contains("cargo pkgid -p member-b"));
    assert!(!member_a_publish.contains("cargo metadata --no-deps --format-version 1"));

    let member_b_publish = read(
        &temp
            .path()
            .join(".forgejo/workflows/publish-crate-member-b.yaml"),
    );
    assert_yaml_parses(&member_b_publish);
    assert!(member_b_publish.contains(r#"cargo pkgid -p member-b | awk -F'[#@]' '{print $NF}'"#));
    assert!(!member_b_publish.contains("cargo pkgid -p member-a"));
    assert!(!member_b_publish.contains("cargo metadata --no-deps --format-version 1"));
}

#[test]
fn package_flag_generates_selected_package_workflows() {
    let temp = init_workspace_fixture();

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo", "--package", "beta"])
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
        .args(["init", "ci", "--platform", "forgejo"])
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
        .args(["init", "ci", "--platform", "forgejo"])
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
        .args(["init", "ci", "--platform", "forgejo", "--check"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("CI workflows are not up to date"));
    assert!(stderr.contains(".forgejo/workflows/ci.yaml differs"));
}

#[test]
fn check_fails_when_publish_workflow_is_missing() {
    let temp = init_package(true);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init", "ci", "--platform", "forgejo"])
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
    assert!(ci.contains("run: cargo audit"));
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
    assert!(workflow.contains("HOMEBREW_TAP_TOKEN not configured; skipping Homebrew tap update."));
    assert!(workflow.contains("\"release/demo-${VERSION}-aarch64-darwin.tar.gz\""));
    assert!(workflow.contains("\"release/demo-${VERSION}-x86_64-darwin.tar.gz\""));
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
    assert!(workflow.contains("--archive \"darwin_intel=https://codeberg.org/foo/demo/releases/download/${VERSION}/demo-${VERSION}-x86_64-darwin.tar.gz,release/demo-${VERSION}-x86_64-darwin.tar.gz\" \\"));
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
fn homebrew_rejects_github_platform() {
    let temp = init_package(true);

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init",
            "ci",
            "--platform",
            "github",
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
    assert!(stderr.contains("Homebrew tap publish is forgejo-only for now"));
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
    assert!(workflow.contains("FORCE_PUBLISH: ${{ inputs.force_publish }}"));
    assert!(workflow.contains("release/smoke-report.txt"));
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
    assert!(workflow.contains("SCOOP_BUCKET_TOKEN: ${{ secrets.scoop_bucket_token }}"));
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
fn check_fails_when_deny_policy_differs() {
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

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("CI workflows are not up to date"));
    assert!(stderr.contains("deny.toml differs"));
}

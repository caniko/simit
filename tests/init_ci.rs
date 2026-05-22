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
        .args(["init-ci", "--platform", "forgejo", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: atlas"));
    assert!(ci.contains("group: ${{ github.workflow }}-${{ github.ref }}"));
    assert!(ci.contains("uses: https://code.forgejo.org/actions/checkout@v4"));
    assert!(!ci.contains("pull_request:"));
    assert!(!ci.contains("uses: https://github.com/cachix/install-nix-action@v31"));
    assert!(ci.contains("run: nix flake check"));
    assert!(ci.contains("run: nix develop -c cargo clippy --all-targets -- --deny warnings"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: atlas"));
    assert!(!publish.contains("uses: https://github.com/cachix/install-nix-action@v31"));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(publish.contains("tags:"));
    assert!(publish.contains("grep -Eq '^[0-9]+\\.[0-9]+\\.[0-9]+$'"));
    assert!(publish.contains("keys/maintainers.gpg"));
    assert!(publish.contains("git verify-tag \"$tag\""));
    assert!(publish.contains("nix develop -c cargo metadata --no-deps --format-version 1"));
    assert!(publish.contains("CRATES_IO_API_TOKEN: ${{ secrets.CRATES_IO_API_TOKEN }}"));
    assert!(publish.contains("CRATES_IO_API_TOKEN is required"));
    assert!(publish.contains("export CARGO_REGISTRY_TOKEN="));
    assert_maintainer_key_written(temp.path());
}

#[test]
fn generates_github_plain_cargo_workflows() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "github"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: ubuntu-latest"));
    assert!(ci.contains("uses: dtolnay/rust-toolchain@stable"));
    assert!(ci.contains("toolchain: stable"));
    assert!(ci.contains("run: cargo test --all-features"));
    assert!(ci.contains("run: cargo package --allow-dirty"));

    let publish = read(&temp.path().join(".github/workflows/publish-crate.yaml"));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(publish.contains("run: cargo publish --dry-run"));
    assert!(publish.contains("cargo metadata --no-deps --format-version 1"));
}

#[test]
fn forgejo_auto_runtime_uses_rust_container_even_when_flake_exists() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert_all_branch_push_trigger(&ci);
    assert!(ci.contains("runs-on: atlas"));
    assert!(ci.contains("cancel-in-progress: true"));
    assert!(ci.contains("container: rust:1.85-bookworm"));
    assert!(ci.contains("uses: https://code.forgejo.org/actions/checkout@v4"));
    assert!(!ci.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(!ci.contains("run: apk add --no-cache git build-base"));
    assert!(ci.contains("run: rustup component add clippy rustfmt"));
    assert!(ci.contains("run: cargo test --all-features"));
    assert!(!ci.contains("uses: https://github.com/cachix/install-nix-action@v31"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(publish.contains("runs-on: atlas"));
    assert!(publish.contains("container: rust:1.85-bookworm"));
    assert!(!publish.contains("uses: https://github.com/Swatinem/rust-cache@v2"));
    assert!(publish.contains("cargo metadata --no-deps --format-version 1"));
}

#[test]
fn forgejo_runner_override_applies_to_all_jobs() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init-ci",
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
fn forgejo_user_config_can_render_structured_runner_labels() {
    let temp = init_package(false);

    let status = simit_with_multilabel_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: [\"self-hosted\", \"atlas\"]"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: [\"self-hosted\", \"atlas\"]"));
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
        .args(["init-ci", "--platform", "forgejo"])
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
            "init-ci",
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
        ci.contains("cargo run -- init-ci --platform forgejo --runner codeberg-medium --check")
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
            "init-ci",
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
        "cargo run -- init-ci --platform github --windows-runner windows-latest --with-artifacts --with-chocolatey --with-scoop --check"
    ));
}

#[test]
fn check_succeeds_when_workflows_are_current() {
    let temp = init_package(true);

    let write_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let check_status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
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
        .env("GIT_CONFIG_GLOBAL", isolated_home.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "true")
        .args(["init-ci", "--platform", "forgejo"])
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
        .args(["init-ci", "--platform", "forgejo"])
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
        .args(["init-ci", "--platform", "forgejo", "--check"])
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
        .args(["init-ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::remove_file(temp.path().join(".forgejo/workflows/publish-crate.yaml")).unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo", "--check"])
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
        .args(["init-ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&root.join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("cargo run -- init-ci --platform forgejo --check"));
    assert!(ci.contains("cargo run -- init-flake --check"));
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
fn optional_strict_flags_render_expected_steps() {
    let temp = init_package(false);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init-ci",
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
    assert!(ci.contains("run: cargo install cargo-audit --locked"));
    assert!(ci.contains("run: cargo audit"));
    assert!(ci.contains("run: cargo install cargo-deny --locked"));
    assert!(ci.contains("run: cargo deny check"));
    assert!(ci.contains("run: cargo +1.85 check --all-targets"));
    assert!(ci.contains("run: cargo doc --no-deps --all-features"));

    let deny = read(&temp.path().join("deny.toml"));
    assert!(deny.contains("\"MIT\""));
    assert!(deny.contains("\"Apache-2.0\""));
    assert!(deny.contains("\"Unicode-3.0\""));
    assert!(deny.contains("\"Unlicense\""));
    assert!(deny.contains("allow-registry = [\"https://github.com/rust-lang/crates.io-index\"]"));
}

#[test]
fn forgejo_nix_homebrew_step_matches_hardened_shape() {
    let temp = init_package(true);

    let status = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args([
            "init-ci",
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
            "init-ci",
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
            "init-ci",
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
            "init-ci",
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
            "init-ci",
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
    assert!(workflow.contains("simit chocolatey bump `"));
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
            "init-ci",
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
        .args(["init-ci", "--platform", "github", "--with-chocolatey"])
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
        .args(["init-ci", "--platform", "github", "--with-scoop"])
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
    assert!(workflow.contains("simit scoop bump `"));
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
        .args(["init-ci", "--platform", "github", "--with-scoop"])
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
        .args(["init-ci", "--platform", "github", "--with-scoop"])
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
            "init-ci",
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
            "init-ci",
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
            "init-ci",
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
        .env("GIT_CONFIG_GLOBAL", isolated_home.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "true")
        .env("SIMIT_MAINTAINERS_GPG", common::maintainer_key_path())
        .args(["init-ci", "--platform", "forgejo", "--with-chocolatey"])
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
            "init-ci",
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
        .args(["init-ci", "--platform", "forgejo", "--with-deny"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(temp.path().join("deny.toml"), "[licenses]\n").unwrap();

    let output = simit_with_user_config(temp.path())
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo", "--with-deny", "--check"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("CI workflows are not up to date"));
    assert!(stderr.contains("deny.toml differs"));
}

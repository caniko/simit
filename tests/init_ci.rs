use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn simit() -> Command {
    Command::new(env!("CARGO_BIN_EXE_simit"))
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

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

fn assert_yaml_parses(text: &str) {
    serde_yaml::from_str::<serde_yaml::Value>(text).unwrap();
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

#[test]
fn generates_forgejo_nix_workflows() {
    let temp = init_package(true);

    let status = simit()
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo", "--runtime", "nix"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: codeberg-small"));
    assert!(ci.contains("uses: https://github.com/cachix/install-nix-action@v31"));
    assert!(ci.contains("run: nix flake check"));
    assert!(ci.contains("run: nix develop -c cargo clippy --all-targets -- --deny warnings"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(publish.contains("tags:"));
    assert!(publish.contains("grep -Eq '^[0-9]+\\.[0-9]+\\.[0-9]+$'"));
    assert!(publish.contains("nix develop -c cargo metadata --no-deps --format-version 1"));
    assert!(publish.contains("CRATES_IO_API_TOKEN: ${{ secrets.CRATES_IO_API_TOKEN }}"));
    assert!(publish.contains("CRATES_IO_API_TOKEN is required"));
    assert!(publish.contains("export CARGO_REGISTRY_TOKEN="));
}

#[test]
fn generates_github_plain_cargo_workflows() {
    let temp = init_package(true);

    let status = simit()
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "github"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
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

    let status = simit()
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".forgejo/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: codeberg-small"));
    assert!(ci.contains("container: rust:alpine"));
    assert!(ci.contains("run: apk add --no-cache git build-base"));
    assert!(ci.contains("run: rustup component add clippy rustfmt"));
    assert!(ci.contains("run: cargo test --all-features"));
    assert!(!ci.contains("uses: https://github.com/cachix/install-nix-action@v31"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("simit changelog release <version>"));
    assert!(publish.contains("runs-on: codeberg-small"));
    assert!(publish.contains("container: rust:alpine"));
    assert!(publish.contains("cargo metadata --no-deps --format-version 1"));
}

#[test]
fn forgejo_runner_override_applies_to_all_jobs() {
    let temp = init_package(true);

    let status = simit()
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

    let status = simit()
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

    let status = simit()
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

    let status = simit()
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

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let check_status = simit()
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn check_fails_when_workflows_differ() {
    let temp = init_package(true);

    let write_status = simit()
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

    let output = simit()
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

    let status = simit()
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

    let output = simit()
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

    let status = simit()
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

    let status = simit()
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

    let check_status = simit()
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

    let output = simit()
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

    let output = simit()
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

    let status = simit()
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
fn github_chocolatey_flag_errors_when_config_is_absent() {
    let temp = init_package(false);

    let output = simit()
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

    let status = simit()
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

    let status = simit()
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

    let output = simit()
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

    let status = simit()
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
    let second_status = simit()
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

    let status = simit()
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
    assert!(workflow.contains("runs-on: codeberg-small"));
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
fn forgejo_windows_packagers_require_windows_runner() {
    let temp = init_package(false);
    write_chocolatey_config(temp.path());

    let output = simit()
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo", "--with-chocolatey"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--windows-runner is required for Forgejo"));
    assert!(stderr.contains("register a Windows runner"));
}

#[test]
fn chocolatey_implies_artifacts_even_when_artifacts_false() {
    let temp = init_package(false);
    write_chocolatey_config(temp.path());

    let output = simit()
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

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo", "--with-deny"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(temp.path().join("deny.toml"), "[licenses]\n").unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "forgejo", "--with-deny", "--check"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("CI workflows are not up to date"));
    assert!(stderr.contains("deny.toml differs"));
}

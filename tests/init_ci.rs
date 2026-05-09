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

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
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
    assert!(ci.contains("toolchain: 1.85"));
    assert!(ci.contains("run: cargo test --all-features"));
    assert!(ci.contains("run: cargo package --allow-dirty"));

    let publish = read(&temp.path().join(".github/workflows/publish-crate.yaml"));
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
    assert!(ci.contains("container: rust:1.85-alpine"));
    assert!(ci.contains("run: apk add --no-cache git build-base"));
    assert!(ci.contains("run: rustup component add clippy rustfmt"));
    assert!(ci.contains("run: cargo test --all-features"));
    assert!(!ci.contains("uses: https://github.com/cachix/install-nix-action@v31"));

    let publish = read(&temp.path().join(".forgejo/workflows/publish-crate.yaml"));
    assert!(publish.contains("runs-on: codeberg-small"));
    assert!(publish.contains("container: rust:1.85-alpine"));
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

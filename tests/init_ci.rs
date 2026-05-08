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
        .args(["init-ci", "--platform", "forgejo"])
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
    assert!(publish.contains("nix develop -c cargo pkgid"));
    assert!(publish.contains("CRATES_IO_API_TOKEN: ${{ secrets.CRATES_IO_API_TOKEN }}"));
    assert!(publish.contains("CRATES_IO_API_TOKEN is required"));
    assert!(publish.contains("export CARGO_REGISTRY_TOKEN="));
}

#[test]
fn generates_github_plain_cargo_workflows() {
    let temp = init_package(false);

    let status = simit()
        .current_dir(temp.path())
        .args(["init-ci", "--platform", "github"])
        .status()
        .unwrap();
    assert!(status.success());

    let ci = read(&temp.path().join(".github/workflows/ci.yaml"));
    assert!(ci.contains("runs-on: ubuntu-latest"));
    assert!(ci.contains("uses: dtolnay/rust-toolchain@stable"));
    assert!(ci.contains("run: cargo fmt --check"));
    assert!(ci.contains("run: cargo package --allow-dirty"));

    let publish = read(&temp.path().join(".github/workflows/publish-crate.yaml"));
    assert!(publish.contains("run: cargo publish --dry-run"));
    assert!(publish.contains("cargo pkgid"));
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
}

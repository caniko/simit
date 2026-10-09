use std::fs;

use tempfile::TempDir;

mod common;

const HARBOR_URL: &str = "git+https://github.com/caniko/harbor.git?ref=feat/harbor-monorepo-components&rev=7d99eb50c52d0a941e2996b97c469b32a7657ef4";

fn rust_project() -> TempDir {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\nrust-version = \"1.85\"\n").unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    root
}

fn generate(root: &TempDir, extra: &[&str]) -> String {
    let output = common::simit()
        .current_dir(root.path())
        .args(["init", "flake", "--scope", "full"])
        .args(extra)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read_to_string(root.path().join("flake.nix")).unwrap()
}

fn assert_shared_input(flake: &str) {
    assert!(flake.contains(HARBOR_URL), "missing qualified Harbor input");
    assert!(!flake.contains("caniko/harbor-rs"));
    assert!(!flake.contains("caniko/harbor-py"));
    assert!(!flake.contains("rs-harbor.lib"));
    assert!(!flake.contains("py-harbor.lib"));
}

#[test]
fn native_rust_flake_uses_the_qualified_namespaced_harbor_input() {
    let root = rust_project();
    let flake = generate(&root, &[]);
    assert_shared_input(&flake);
    assert!(flake.contains("harbor.lib.rust.mkToolchain"));
    assert!(flake.contains("harbor.lib.rust.mkCross"));
    assert!(flake.contains("crane.follows = \"harbor/crane\""));
    assert!(flake.contains("toolchainFile = ./nix/rust-toolchain-msrv.toml;"));
    assert!(!flake.contains("builtins.toFile"));
    let msrv = fs::read_to_string(root.path().join("nix/rust-toolchain-msrv.toml")).unwrap();
    assert!(msrv.contains("channel = \"1.85.0\""));
    assert!(msrv.contains("profile = \"minimal\""));
    assert!(
        common::simit()
            .current_dir(root.path())
            .args(["init", "flake", "--check"])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(flake, generate(&root, &[]), "generation is not idempotent");
}

#[test]
fn cross_rust_flake_uses_the_same_harbor_input_and_namespaced_cross_api() {
    let root = rust_project();
    let flake = generate(
        &root,
        &["--cross", "--target", "native", "--target", "windows"],
    );
    assert_shared_input(&flake);
    assert!(flake.contains("harbor.lib.rust.mkCrossPackages"));
    assert!(flake.contains("harbor.lib.rust.mkDevShells"));
}

#[test]
fn namespaced_flake_accepts_inherited_hook_bindings_after_treefmt() {
    let root = rust_project();
    let flake = generate(&root, &[])
        .replace(
            "inherit rustToolchain;",
            "inherit (toolchain) rustToolchain;",
        )
        .replace(
            "shellHook = pre-commit-check.shellHook;",
            "inherit (pre-commit-check) shellHook;",
        );
    fs::write(root.path().join("flake.nix"), flake).unwrap();
    let output = common::simit()
        .current_dir(root.path())
        .args(["init", "flake", "--check"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn python_flake_uses_the_same_harbor_input_and_namespaced_python_api() {
    let root = TempDir::new().unwrap();
    fs::write(
        root.path().join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\nrequires-python = \">=3.12\"\n",
    )
    .unwrap();
    fs::write(root.path().join("uv.lock"), "").unwrap();
    let flake = generate(&root, &[]);
    assert_shared_input(&flake);
    assert!(flake.contains("py = harbor.lib.python"));
    assert!(
        common::simit()
            .current_dir(root.path())
            .args(["init", "flake", "--scope", "full", "--check"])
            .status()
            .unwrap()
            .success()
    );
}

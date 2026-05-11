use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn simit() -> Command {
    Command::new(env!("CARGO_BIN_EXE_simit"))
}

fn init_package() -> TempDir {
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
    fs::write(root.join("README.md"), "# Demo\n").unwrap();
    fs::write(root.join("settings.yaml"), "name: demo\n").unwrap();

    temp
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

fn patchable_flake() -> &'static str {
    r#"{
  description = "Rust project";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    crane,
    flake-utils,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [(import rust-overlay)];
      };

      rustToolchain = pkgs.rust-bin.stable.latest.default.override {
        extensions = ["rustfmt" "clippy"];
      };
      craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
      src = craneLib.cleanCargoSource ./.;
      commonArgs = {
        inherit src;
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      package = craneLib.buildPackage (commonArgs // {inherit cargoArtifacts;});
    in {
      packages.default = package;
      checks = {
        default = package;
      };
      devShells.default = craneLib.devShell {
        checks = self.checks.${system};
        packages = with pkgs; [
          cargo-nextest
          rust-analyzer
        ];
      };
    });
}
"#
}

#[test]
fn writes_flake_and_detected_hook_files() {
    let temp = init_package();

    let status = simit()
        .current_dir(temp.path())
        .args(["init-flake"])
        .status()
        .unwrap();
    assert!(status.success());

    assert!(temp.path().join("flake.nix").exists());

    let treefmt = read(&temp.path().join("nix/treefmt.nix"));
    assert!(treefmt.contains("programs.rustfmt = {"));
    assert!(treefmt.contains("edition = \"2021\""));
    assert!(treefmt.contains("pkgs.rust-bin.nightly.latest.default.override"));
    assert!(treefmt.contains("extensions = [\"rustfmt\"]"));
    assert!(treefmt.contains("programs.alejandra.enable = true"));
    assert!(treefmt.contains("programs.taplo.enable = true"));
    assert!(treefmt.contains("programs.prettier"));
    assert!(treefmt.contains("\"*.md\""));
    assert!(treefmt.contains("\"*.yaml\""));

    let hooks = read(&temp.path().join("nix/pre-commit.nix"));
    assert!(hooks.contains("cargo fmt --all -- --check"));
    assert!(hooks.contains("cargo clippy --all-targets --all-features -- --deny warnings"));
    assert!(hooks.contains("cargo audit"));
    assert!(hooks.contains("pkgs.cargo-audit"));
    assert!(hooks.contains(
        "nix --extra-experimental-features 'nix-command flakes' flake check --cores 0 --max-jobs auto --no-update-lock-file"
    ));
}

#[test]
fn patches_existing_anchorable_flake() {
    let temp = init_package();
    fs::write(temp.path().join("flake.nix"), patchable_flake()).unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init-flake"])
        .status()
        .unwrap();
    assert!(status.success());

    let flake = read(&temp.path().join("flake.nix"));
    assert!(flake.contains("treefmt-nix.url = \"github:numtide/treefmt-nix\""));
    assert!(flake.contains("git-hooks.url = \"github:cachix/git-hooks.nix\""));
    assert!(flake.contains("treefmtEval = treefmt-nix.lib.evalModule pkgs"));
    assert!(flake.contains("pre-commit-check = git-hooks.lib.${system}.run"));
    assert!(flake.contains("formatter = treefmtEval.config.build.wrapper"));
    assert!(flake.contains("formatting = treefmtEval.config.build.check self"));
    assert!(flake.contains("pre-commit-check.enabledPackages"));
    assert!(flake.contains("shellHook = pre-commit-check.shellHook"));
}

#[test]
fn refuses_unpatchable_existing_flake_without_writing_hooks() {
    let temp = init_package();
    fs::write(temp.path().join("flake.nix"), "{}\n").unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init-flake"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cannot patch flake.nix"));
    assert!(stderr.contains("run `simit init-flake --print`"));
    assert!(!temp.path().join("nix/treefmt.nix").exists());
}

#[test]
fn detects_uv_python_hooks() {
    let temp = init_package();
    fs::write(
        temp.path().join("pyproject.toml"),
        r#"[project]
name = "demo"
version = "0.1.0"

[tool.uv]
"#,
    )
    .unwrap();
    fs::write(temp.path().join("uv.lock"), "").unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init-flake"])
        .status()
        .unwrap();
    assert!(status.success());

    let hooks = read(&temp.path().join("nix/pre-commit.nix"));
    assert!(hooks.contains("uv run ruff format --check ."));
    assert!(hooks.contains("uv run mypy ."));
}

#[test]
fn print_outputs_without_writing() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args(["init-flake", "--print"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--- flake.nix"));
    assert!(stdout.contains("--- nix/treefmt.nix"));
    assert!(stdout.contains("--- nix/pre-commit.nix"));
    assert!(stdout.contains("--- existing flake patching note"));
    assert!(stdout.contains("treefmt-nix.url = \"github:numtide/treefmt-nix\""));
    assert!(!temp.path().join("flake.nix").exists());
}

#[test]
fn check_succeeds_when_flake_and_hooks_are_current() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init-flake"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let check_status = simit()
        .current_dir(temp.path())
        .args(["init-flake", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn check_fails_when_hook_files_differ() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init-flake"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(temp.path().join("nix/treefmt.nix"), "{}\n").unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init-flake", "--check"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("flake and hook files are not up to date"));
    assert!(stderr.contains("nix/treefmt.nix differs"));
}

#[test]
fn check_diff_includes_stale_hook_file_diff() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init-flake"])
        .status()
        .unwrap();
    assert!(write_status.success());
    fs::write(temp.path().join("nix/pre-commit.nix"), "{}\n").unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init-flake", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--- nix/pre-commit.nix"));
    assert!(stderr.contains("+++ nix/pre-commit.nix"));
}

#[test]
fn check_and_print_are_mutually_exclusive() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args(["init-flake", "--check", "--print"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("init-flake accepts only one of --check or --print"));
}

#[test]
fn diff_requires_check() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args(["init-flake", "--diff"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("init-flake --diff requires --check"));
}

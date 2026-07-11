use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
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

fn init_python_project() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::write(
        root.join("pyproject.toml"),
        r#"[project]
name = "py-demo"
version = "0.1.0"
requires-python = "==3.13.*"

[project.scripts]
py-demo = "py_demo:main"
pyd = "py_demo:main"

[project.optional-dependencies]
cpu = ["pytest"]
nvidia = ["torch"]

[dependency-groups]
dev = ["mypy", "pytest", "ruff"]

[tool.uv]
conflicts = [
  [{ extra = "cpu" }, { extra = "nvidia" }],
]
"#,
    )
    .unwrap();
    fs::write(root.join("uv.lock"), "").unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/py_demo.py"), "def main(): pass\n").unwrap();
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

fn custom_rs_harbor_flake() -> &'static str {
    r#"{
  description = "Memory-aware admission gate for Rust work pipelines";

  inputs = {
    rs-harbor.url = "git+https://codeberg.org/caniko/rs-harbor.git";

    nixpkgs.follows = "rs-harbor/nixpkgs";
    rust-overlay.follows = "rs-harbor/rust-overlay";
    crane.follows = "rs-harbor/crane";
    flake-utils.follows = "rs-harbor/flake-utils";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    rs-harbor,
    flake-utils,
    rust-overlay,
    treefmt-nix,
    git-hooks,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [(import rust-overlay)];
      };
      toolchain = rs-harbor.lib.mkToolchain {inherit pkgs;};
      inherit (toolchain) craneLib;
      src = craneLib.cleanCargoSource ./.;
      commonArgs = {
        inherit src;
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      package = craneLib.buildPackage (commonArgs // {inherit cargoArtifacts;});
      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
      pre-commit-check = git-hooks.lib.${system}.run {
        src = ./.;
        hooks = import ./nix/pre-commit.nix {
          inherit pkgs;
          treefmtWrapper = treefmtEval.config.build.wrapper;
          rustToolchain = toolchain.rustToolchain;
        };
      };
    in {
      packages = {
        default = package;
      };
      formatter = treefmtEval.config.build.wrapper;
      checks = {
        default = package;
        formatting = treefmtEval.config.build.check self;
      };
      devShells.default = craneLib.devShell {
        checks = self.checks.${system};
        packages = with pkgs; [
          cargo-nextest
          pre-commit
          rust-analyzer
        ] ++ pre-commit-check.enabledPackages;
        shellHook = ''
          ${pre-commit-check.shellHook}
          echo "Documentation: cd docs && mdbook serve"
        '';
      };
    });
}
"#
}

fn custom_skillnet_flake_config() -> &'static str {
    r#"[flake]
scope = "full"
mode = "custom"
toolchain_binding = "toolchain.rustToolchain"
crane_lib_binding = "craneLib"
package_binding = "package"

[flake.expected_outputs]
packages = ["default", "docs", "site"]
checks = ["default", "formatting", "clippy", "fmt", "nextest", "doc", "audit", "deny", "hm-module"]
top_level = ["hmModules"]
"#
}

fn custom_skillnet_flake() -> &'static str {
    r#"{
  description = "skillnet AI skill mirror manager";

  inputs = {
    rs-harbor.url = "git+https://codeberg.org/caniko/rs-harbor.git";

    nixpkgs.follows = "rs-harbor/nixpkgs";
    rust-overlay.follows = "rs-harbor/rust-overlay";
    crane.follows = "rs-harbor/crane";
    flake-utils.follows = "rs-harbor/flake-utils";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    rs-harbor,
    flake-utils,
    rust-overlay,
    treefmt-nix,
    git-hooks,
    ...
  }: let
    hmModule = import ./nix/hm-module.nix;
  in
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [(import rust-overlay)];
      };
      toolchain = rs-harbor.lib.mkToolchain {inherit pkgs;};
      inherit (toolchain) craneLib;
      src = craneLib.cleanCargoSource ./.;
      commonArgs = {
        inherit src;
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      package = craneLib.buildPackage (commonArgs // {inherit cargoArtifacts;});
      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
      pre-commit-check = git-hooks.lib.${system}.run {
        src = ./.;
        hooks = import ./nix/pre-commit.nix {
          inherit pkgs;
          treefmtWrapper = treefmtEval.config.build.wrapper;
          rustToolchain = toolchain.rustToolchain;
        };
      };
      clippyCheck = craneLib.cargoClippy commonArgs;
      fmtCheck = craneLib.cargoFmt {inherit src;};
      nextestCheck = craneLib.cargoNextest commonArgs;
      docCheck = craneLib.cargoDoc commonArgs;
      auditCheck = craneLib.cargoAudit {inherit src;};
      denyCheck = craneLib.cargoDeny {inherit src;};
      docs = pkgs.stdenv.mkDerivation {
        pname = "skillnet-docs";
        inherit (package) version;
      };
      hmModuleTest = pkgs.runCommand "hm-module" {} "touch $out";
    in {
      packages = {
        default = package;
        docs = docs;
        site = docs;
      };

      formatter = treefmtEval.config.build.wrapper;

      checks = {
        default = package;
        formatting = treefmtEval.config.build.check self;
        clippy = clippyCheck;
        fmt = fmtCheck;
        nextest = nextestCheck;
        doc = docCheck;
        audit = auditCheck;
        deny = denyCheck;
        hm-module = hmModuleTest;
      };

      devShells.default = craneLib.devShell {
        checks = self.checks.${system};
        packages = with pkgs; [
          cargo-nextest
          pre-commit
          rust-analyzer
        ] ++ pre-commit-check.enabledPackages;
        shellHook = pre-commit-check.shellHook;
      };
      devShells.docs = rs-harbor.lib.mkDocsShell {
        inherit pkgs;
        inherit (toolchain) craneLib;
        cross = rs-harbor.lib.mkCross {inherit pkgs system;};
        packages = with pkgs; [
          mdbook
          pre-commit
          rust-analyzer
        ] ++ pre-commit-check.enabledPackages;
        extraShellHook = pre-commit-check.shellHook;
      };
    })
    // {
      hmModules.default = hmModule;
    };
}
"#
}

#[test]
fn default_scope_writes_only_detected_hook_file() {
    let temp = init_package();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(status.success());

    assert!(!temp.path().join("flake.nix").exists());
    assert!(!temp.path().join("nix/treefmt.nix").exists());

    let hooks = read(&temp.path().join("nix/pre-commit.nix"));
    assert!(hooks.contains("cargo fmt --all -- --check"));
    assert!(hooks.contains("cargo clippy --all-targets --all-features -- --deny warnings"));
    assert!(hooks.contains("cargo-msrv"));
    assert!(hooks.contains("cargo check MSRV"));
    assert!(hooks.contains("pkgs.rust-bin.stable.\"1.85.0\".default"));
    assert!(hooks.contains("cargo check --workspace --all-features"));
    assert!(hooks.contains("stages = [\"pre-push\" \"manual\"]"));
    assert!(hooks.contains("cargo audit"));
    assert!(hooks.contains("pkgs.cargo-audit"));
    assert!(hooks.contains(
        "nix --extra-experimental-features 'nix-command flakes' flake check --cores 0 --max-jobs auto --no-update-lock-file"
    ));
}

#[test]
fn full_scope_writes_flake_and_formatter_files() {
    let temp = init_package();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(status.success());

    assert!(temp.path().join("flake.nix").exists());

    let flake = read(&temp.path().join("flake.nix"));
    assert!(flake.contains("cargo-audit"));
    assert!(flake.contains("cargo-deny"));
    assert!(flake.contains("cargo-about"));
    assert!(flake.contains("cargo-sbom"));
    assert!(flake.contains("apps.local-check-fast"));
    assert!(flake.contains("apps.local-check-release"));

    let treefmt = read(&temp.path().join("nix/treefmt.nix"));
    assert!(treefmt.contains("programs.rustfmt = {"));
    assert!(treefmt.contains("edition = \"2024\""));
    assert!(treefmt.contains("pkgs.rust-bin.nightly.latest.default.override"));
    assert!(treefmt.contains("extensions = [\"rustfmt\"]"));
    assert!(treefmt.contains("programs.alejandra.enable = true"));
    assert!(treefmt.contains("programs.taplo.enable = true"));
    assert!(treefmt.contains("programs.prettier"));
    assert!(treefmt.contains("\"*.md\""));
    assert!(treefmt.contains("\"*.yaml\""));
}

#[test]
fn deny_config_adds_deny_shell_tool_and_hook() {
    let temp = init_package();
    fs::write(temp.path().join("simit.toml"), "[ci]\nwith_deny = true\n").unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(status.success());

    let flake = read(&temp.path().join("flake.nix"));
    assert!(flake.contains("cargo-audit"));
    assert!(flake.contains("cargo-deny"));

    let hooks = read(&temp.path().join("nix/pre-commit.nix"));
    assert!(hooks.contains("cargo-deny"));
    assert!(hooks.contains("cargo deny check bans licenses sources"));
    assert!(hooks.contains("pkgs.cargo-deny"));
}

#[test]
fn existing_deny_policy_adds_deny_shell_tool_and_hook() {
    let temp = init_package();
    fs::write(temp.path().join("deny.toml"), "[licenses]\n").unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(status.success());

    let flake = read(&temp.path().join("flake.nix"));
    assert!(flake.contains("cargo-deny"));

    let hooks = read(&temp.path().join("nix/pre-commit.nix"));
    assert!(hooks.contains("cargo deny check bans licenses sources"));
}

#[test]
fn check_detects_missing_audit_shell_tool() {
    let temp = init_package();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(status.success());

    let flake_path = temp.path().join("flake.nix");
    let flake = read(&flake_path).replace("          cargo-audit\n", "");
    fs::write(&flake_path, flake).unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("flake.nix is missing generated hook wiring"));
    assert!(stderr.contains("cargo-audit"));
}

#[test]
fn cross_flake_includes_audit_shell_tools() {
    let temp = init_package();
    fs::write(temp.path().join("deny.toml"), "[licenses]\n").unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--cross"])
        .status()
        .unwrap();
    assert!(status.success());

    let flake = read(&temp.path().join("flake.nix"));
    assert!(flake.contains("cargo-audit"));
    assert!(flake.contains("cargo-deny"));
}

#[test]
fn generated_msrv_hook_uses_highest_workspace_rust_version() {
    let temp = init_package();
    let root = temp.path();
    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["low", "high"]
resolver = "3"
"#,
    )
    .unwrap();
    for (member, rust_version) in [("low", "1.85"), ("high", "1.88")] {
        let dir = root.join(member);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            format!(
                r#"[package]
name = "{member}"
version = "0.1.0"
edition = "2024"
rust-version = "{rust_version}"
"#
            ),
        )
        .unwrap();
        fs::write(dir.join("src/lib.rs"), "").unwrap();
    }

    let status = simit()
        .current_dir(root)
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(status.success());

    let hooks = read(&root.join("nix/pre-commit.nix"));
    assert!(hooks.contains("pkgs.rust-bin.stable.\"1.88.0\".default"));
    assert!(!hooks.contains("pkgs.rust-bin.stable.\"1.85.0\".default"));
}

#[test]
fn patches_existing_anchorable_flake() {
    let temp = init_package();
    fs::write(temp.path().join("flake.nix"), patchable_flake()).unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full"])
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
        .args(["init", "flake", "--scope", "full"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cannot patch flake.nix"));
    assert!(stderr.contains("run `simit init flake --print`"));
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
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(status.success());

    let hooks = read(&temp.path().join("nix/pre-commit.nix"));
    assert!(hooks.contains("uv run ruff format --check ."));
    assert!(hooks.contains("uv run mypy ."));
}

#[test]
fn pure_uv_python_project_generates_py_harbor_flake() {
    let temp = init_python_project();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(status.success());

    let flake = read(&temp.path().join("flake.nix"));
    assert!(flake.contains("py-harbor"));
    assert!(flake.contains("py-harbor.lib"));
    assert!(flake.contains("mkUvDevShell"));
    assert!(flake.contains("mkUvCheckEnv"));
    assert!(flake.contains("mkUvAppPackage"));
    assert!(flake.contains("py-demo-cpu"));
    assert!(flake.contains("\"py-demo\""));
    assert!(flake.contains("\"pyd\""));

    let hooks = read(&temp.path().join("nix/pre-commit.nix"));
    assert!(hooks.contains("uv-ruff-format"));
    assert!(hooks.contains("uv-mypy"));

    let check_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn python_component_selection_excludes_rust_and_unselected_hooks() {
    let temp = init_python_project();
    fs::create_dir_all(temp.path().join("tests/fixtures/demo/src")).unwrap();
    fs::write(
        temp.path().join("tests/fixtures/demo/Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("tests/fixtures/demo/src/lib.rs"),
        "pub fn fixture() {}\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        "[flake]\ncomponents = [\"treefmt\", \"nix-flake-check\", \"uv-ruff-format\"]\n",
    )
    .unwrap();

    let status = simit()
        .current_dir(temp.path())
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(status.success());

    let hooks = read(&temp.path().join("nix/pre-commit.nix"));
    assert!(hooks.contains("treefmt"));
    assert!(hooks.contains("nix-flake-check"));
    assert!(hooks.contains("uv-ruff-format"));
    assert!(!hooks.contains("uv-mypy"));
    assert!(!hooks.contains("cargo-"));
    assert!(!hooks.contains("rustToolchain"));
}

#[test]
fn custom_py_harbor_flake_checks_expected_outputs() {
    let temp = init_python_project();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[flake]
scope = "full"
mode = "custom"
backend = "py-harbor"

[flake.expected_outputs]
packages = ["py-demo-cpu"]
apps = ["py-demo-cpu"]
dev_shells = ["default"]
checks = ["offline-tests", "typecheck"]
"#,
    )
    .unwrap();
    fs::write(
        temp.path().join("flake.nix"),
        r#"{
  inputs = {
    py-harbor.url = "git+https://codeberg.org/caniko/py-harbor.git";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    git-hooks.url = "github:cachix/git-hooks.nix";
  };
  outputs = { self, py-harbor, treefmt-nix, git-hooks, ... }:
    let
      py = py-harbor.lib;
      mkChecks = system: {
        offline-tests = {};
        typecheck = {};
      };
    in {
      packages.x86_64-linux.py-demo-cpu = {};
      apps.x86_64-linux.py-demo-cpu = {};
      devShells.x86_64-linux.default = {};
      checks.x86_64-linux = mkChecks "x86_64-linux";
      formatter.x86_64-linux =
        let
          pkgs = py.mkPkgs { system = "x86_64-linux"; };
          treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
          pre-commit-check = git-hooks.lib.x86_64-linux.run {
            src = ./.;
            hooks = import ./nix/pre-commit.nix {
              inherit pkgs;
              treefmtWrapper = treefmtEval.config.build.wrapper;
            };
          };
        in treefmtEval.config.build.wrapper;
      checks.x86_64-linux.formatting =
        let
          pkgs = py.mkPkgs { system = "x86_64-linux"; };
          treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
        in treefmtEval.config.build.check self;
      devShells.x86_64-linux.hooks = {
        packages = [] ++ pre-commit-check.enabledPackages;
        shellHook = pre-commit-check.shellHook;
      };
    };
}
"#,
    )
    .unwrap();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let check_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn print_outputs_without_writing() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--print"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("--- flake.nix"));
    assert!(!stdout.contains("--- nix/treefmt.nix"));
    assert!(stdout.contains("--- nix/pre-commit.nix"));
    assert!(!stdout.contains("--- existing flake patching note"));
    assert!(!stdout.contains("treefmt-nix.url = \"github:numtide/treefmt-nix\""));
    assert!(!temp.path().join("flake.nix").exists());
}

#[test]
fn full_scope_print_outputs_without_writing() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full", "--print"])
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
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let check_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn hooks_only_check_ignores_project_owned_flake() {
    let temp = init_package();
    fs::write(temp.path().join("flake.nix"), custom_rs_harbor_flake()).unwrap();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(write_status.success());
    assert_eq!(
        read(&temp.path().join("flake.nix")),
        custom_rs_harbor_flake()
    );
    assert!(!temp.path().join("nix/treefmt.nix").exists());

    fs::write(temp.path().join("flake.nix"), "{ custom = \"owned\"; }\n").unwrap();
    let check_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check", "--diff"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn existing_generated_flake_defaults_to_full_scope() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(temp.path().join("nix/treefmt.nix"), "{}\n").unwrap();
    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("flake and hook files are not up to date"));
    assert!(stderr.contains("nix/treefmt.nix is missing generated formatter wiring"));
}

#[test]
fn explicit_full_scope_reemits_full_flake_after_hooks_only_adoption() {
    let temp = init_package();

    let hooks_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(hooks_status.success());
    assert!(!temp.path().join("flake.nix").exists());

    let full_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(full_status.success());
    assert!(temp.path().join("flake.nix").exists());
    assert!(temp.path().join("nix/treefmt.nix").exists());
}

#[test]
fn check_accepts_custom_rs_harbor_flake_with_generated_hook_wiring() {
    let temp = init_package();
    fs::write(temp.path().join("flake.nix"), custom_rs_harbor_flake()).unwrap();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(temp.path().join("flake.nix"), custom_rs_harbor_flake()).unwrap();
    let check_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn custom_mode_accepts_skillnet_rs_harbor_flake() {
    let temp = init_package();
    fs::write(
        temp.path().join("simit.toml"),
        custom_skillnet_flake_config(),
    )
    .unwrap();
    fs::write(temp.path().join("flake.nix"), custom_skillnet_flake()).unwrap();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let flake = read(&temp.path().join("flake.nix"));
    assert_eq!(flake, custom_skillnet_flake());
    assert!(temp.path().join("nix/treefmt.nix").exists());
    assert!(temp.path().join("nix/pre-commit.nix").exists());

    let check_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn custom_mode_reports_missing_owned_wiring_without_template_diff() {
    let temp = init_package();
    fs::write(
        temp.path().join("simit.toml"),
        custom_skillnet_flake_config(),
    )
    .unwrap();
    let flake = custom_skillnet_flake().replace(
        "pre-commit-check = git-hooks.lib.${system}.run",
        "project-hooks = git-hooks.lib.${system}.run",
    );
    fs::write(temp.path().join("flake.nix"), flake).unwrap();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("flake.nix custom mode: missing pre-commit-check binding"));
    assert!(!stderr.contains("+++ flake.nix"));
    assert!(!stderr.contains("description = \"Rust project\""));
}

#[test]
fn custom_mode_requires_docs_shell_when_docs_outputs_are_advertised() {
    let temp = init_package();
    fs::write(
        temp.path().join("simit.toml"),
        custom_skillnet_flake_config(),
    )
    .unwrap();
    let flake = custom_skillnet_flake().replace(
        r#"      devShells.docs = rs-harbor.lib.mkDocsShell {
        inherit pkgs;
        inherit (toolchain) craneLib;
        cross = rs-harbor.lib.mkCross {inherit pkgs system;};
        packages = with pkgs; [
          mdbook
          pre-commit
          rust-analyzer
        ] ++ pre-commit-check.enabledPackages;
        extraShellHook = pre-commit-check.shellHook;
      };
"#,
        "",
    );
    fs::write(temp.path().join("flake.nix"), flake).unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("flake.nix custom mode: missing devShells.docs")
    );
}

#[test]
fn check_accepts_semantically_current_custom_hook_files() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let mut flake = read(&temp.path().join("flake.nix"));
    flake = flake.replace("inherit pkgs;", "inherit pkgs rustToolchain;");
    flake = flake.replace("          inherit rustToolchain;\n", "");
    fs::write(temp.path().join("flake.nix"), flake).unwrap();
    fs::write(
        temp.path().join("nix/treefmt.nix"),
        r#"{pkgs, ...}: {
  projectRootFile = "flake.nix";
  programs.rustfmt = {
    enable = true;
    edition = "2024";
  };
  programs.alejandra.enable = true;
  programs.taplo.enable = true;
  programs.prettier = {
    enable = true;
    package = pkgs.prettier;
    includes = [
      "*.md"
      "*.markdown"
      "*.yaml"
      "*.yml"
    ];
  };
}
"#,
    )
    .unwrap();
    fs::write(
        temp.path().join("nix/pre-commit.nix"),
        r#"{
  pkgs,
  rustToolchain ? null,
  ...
}: {
  cargo-fmt = {
    enable = true;
    entry = "cargo fmt --all -- --check";
    pass_filenames = false;
  };
  cargo-clippy = {
    enable = true;
    entry = "cargo clippy --workspace --all-targets --all-features -- --deny warnings";
    pass_filenames = false;
  };
  cargo-msrv = {
    enable = true;
    name = "cargo check MSRV";
    entry = "${pkgs.rust-bin.stable."1.85.0".default}/bin/cargo check --workspace --all-features";
    stages = ["pre-push" "manual"];
  };
  cargo-audit = {
    enable = true;
    entry = "cargo audit --deny warnings";
  };
  nix-flake-check = {
    enable = true;
    entry = "nix flake check";
  };
}
"#,
    )
    .unwrap();

    let check_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check"])
        .status()
        .unwrap();
    assert!(check_status.success());
}

#[test]
fn check_fails_when_hook_files_differ() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--scope", "full"])
        .status()
        .unwrap();
    assert!(write_status.success());

    fs::write(temp.path().join("nix/treefmt.nix"), "{}\n").unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("flake and hook files are not up to date"));
    assert!(stderr.contains("nix/treefmt.nix is missing generated formatter wiring"));
}

#[test]
fn check_diff_includes_stale_hook_file_diff() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(write_status.success());
    fs::write(temp.path().join("nix/pre-commit.nix"), "{}\n").unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--- nix/pre-commit.nix"));
    assert!(stderr.contains("+++ nix/pre-commit.nix"));
}

#[test]
fn check_diff_reports_removed_pre_commit_hook_notes() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let hooks_path = temp.path().join("nix/pre-commit.nix");
    fs::write(
        &hooks_path,
        r#"{
  custom-check = {
    enable = true;
  };
}
"#,
    )
    .unwrap();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("note: pre-commit hook 'custom-check' will be removed"));
    assert!(stderr.contains("--- nix/pre-commit.nix"));
    assert!(stderr.contains("+++ nix/pre-commit.nix"));
}

#[test]
fn check_diff_emits_no_removed_hook_notes_when_hooks_match() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("note: removing pre-commit hook"));
}

#[test]
fn cross_print_emits_multi_target_flake_with_contract() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--print", "--cross"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--- flake.nix"));
    assert!(stdout.contains("rs-harbor.url = \"git+https://codeberg.org/caniko/rs-harbor.git\";"));
    assert!(stdout.contains("rs-harbor.lib.mkCrossPackages {"));
    assert!(stdout.contains(
        "targets = [\"native\" \"aarch64-linux\" \"windows\" \"darwin-x86_64\" \"darwin-aarch64\"];"
    ));
    assert!(stdout.contains("default = crossPackages.${pname};"));
    assert!(stdout.contains("rs-harbor.lib.mkDevShells {"));
    assert!(stdout.contains("canix:lPzPzKrmYqW5Rxa5r0uQWvCqD3S5nx0h2eCy7XD5JM8="));
    assert!(stdout.contains("cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="));
    assert!(!stdout.contains("uqr0"));
    // Print never writes.
    assert!(!temp.path().join("flake.nix").exists());
}

#[test]
fn cross_print_honors_target_subset() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args([
            "init", "flake", "--print", "--cross", "--target", "windows", "--target", "native",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("targets = [\"native\" \"windows\"];"));
    assert!(!stdout.contains("darwin-x86_64"));
    assert!(!stdout.contains("aarch64-linux"));
}

#[test]
fn cross_write_then_check_is_clean_and_detects_drift() {
    let temp = init_package();

    let write_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--cross"])
        .status()
        .unwrap();
    assert!(write_status.success());

    let flake = read(&temp.path().join("flake.nix"));
    assert!(flake.contains("rs-harbor.lib.mkCrossPackages {"));
    assert!(flake.contains(
        "targets = [\"native\" \"aarch64-linux\" \"windows\" \"darwin-x86_64\" \"darwin-aarch64\"];"
    ));

    // A freshly generated cross flake drift-checks clean.
    let check_status = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check", "--cross"])
        .status()
        .unwrap();
    assert!(check_status.success());

    // Mutating the cross flake is detected even though it still carries the
    // generated hook wiring.
    let drifted = flake.replace(
        "targets = [\"native\" \"aarch64-linux\" \"windows\" \"darwin-x86_64\" \"darwin-aarch64\"];",
        "targets = [\"native\"];",
    );
    fs::write(temp.path().join("flake.nix"), drifted).unwrap();
    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check", "--cross"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("flake.nix does not match the generated cross flake"));
}

#[test]
fn target_requires_cross() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--print", "--target", "native"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--cross"));
}

#[test]
fn check_and_print_are_mutually_exclusive() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--check", "--print"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("init flake accepts only one of --check or --print"));
}

#[test]
fn diff_requires_check() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .args(["init", "flake", "--diff"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("init flake --diff requires --check"));
}

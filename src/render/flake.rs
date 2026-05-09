use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::project::{GeneratedFile, Languages};

const TREEFMT_INPUT: &str = "    treefmt-nix.url = \"github:numtide/treefmt-nix\";\n";
const GIT_HOOKS_INPUT: &str = "    git-hooks.url = \"github:cachix/git-hooks.nix\";\n";
const TREEFMT_OUTPUT: &str = "    treefmt-nix,\n";
const GIT_HOOKS_OUTPUT: &str = "    git-hooks,\n";
const HOOK_BINDINGS: &str = r#"      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
      pre-commit-check = git-hooks.lib.${system}.run {
        src = ./.;
        hooks = import ./nix/pre-commit.nix {
          inherit pkgs;
          treefmtWrapper = treefmtEval.config.build.wrapper;
          inherit rustToolchain;
        };
      };
"#;
const FORMATTER_OUTPUT: &str = "      formatter = treefmtEval.config.build.wrapper;\n";
const FORMATTING_CHECK: &str = "        formatting = treefmtEval.config.build.check self;\n";
const PRE_COMMIT_PACKAGE: &str = "          pre-commit\n";
const PRE_COMMIT_ENABLED_PACKAGES: &str = "        ] ++ pre-commit-check.enabledPackages;\n";
const SHELL_HOOK: &str = "        shellHook = pre-commit-check.shellHook;\n";

pub fn files(languages: &Languages) -> Vec<GeneratedFile> {
    vec![
        GeneratedFile {
            relative_path: PathBuf::from("flake.nix"),
            content: template(),
        },
        GeneratedFile {
            relative_path: PathBuf::from("nix/treefmt.nix"),
            content: treefmt_nix(languages),
        },
        GeneratedFile {
            relative_path: PathBuf::from("nix/pre-commit.nix"),
            content: pre_commit_nix(languages),
        },
    ]
}

pub fn print_files(files: &[GeneratedFile]) {
    for file in files {
        println!("--- {}", file.relative_path.display());
        print!("{}", file.content);
    }
}

pub fn print_existing_flake_note() {
    println!("--- existing flake patching note");
    println!(
        "Existing flake.nix files are patched only when simit can find safe anchors. If patching fails, apply the printed wiring manually."
    );
}

pub fn patch_existing(content: &str) -> Result<String> {
    if has_required_wiring(content) {
        return Ok(content.to_owned());
    }

    let mut patched = content.to_owned();

    ensure_after_any_missing(
        &mut patched,
        TREEFMT_INPUT,
        &["treefmt-nix.url", "treefmt-nix = {"],
        "    flake-utils.url = \"github:numtide/flake-utils\";\n",
        "inputs.flake-utils.url",
        "treefmt-nix input must be inserted near flake inputs",
    )?;
    ensure_after_any_missing(
        &mut patched,
        GIT_HOOKS_INPUT,
        &["git-hooks.url", "git-hooks = {"],
        TREEFMT_INPUT,
        "inputs.treefmt-nix.url",
        "git-hooks input must be inserted after treefmt-nix",
    )?;
    ensure_after(
        &mut patched,
        TREEFMT_OUTPUT,
        "    flake-utils,\n",
        "outputs.flake-utils",
        "treefmt-nix must be added to outputs arguments",
    )?;
    ensure_after(
        &mut patched,
        GIT_HOOKS_OUTPUT,
        TREEFMT_OUTPUT,
        "outputs.treefmt-nix",
        "git-hooks must be added to outputs arguments",
    )?;
    ensure_after_statement(
        &mut patched,
        HOOK_BINDINGS,
        "package = craneLib.buildPackage",
        "package binding",
        "hook bindings must be inserted after the package binding in the system let",
    )?;
    ensure_after(
        &mut patched,
        FORMATTER_OUTPUT,
        "      packages.default = package;\n",
        "packages.default output",
        "formatter output must be inserted beside package outputs",
    )?;
    ensure_after(
        &mut patched,
        FORMATTING_CHECK,
        "      checks = {\n",
        "checks attrset",
        "formatting check must be inserted in checks",
    )?;
    ensure_dev_shell_packages(&mut patched)?;
    ensure_after(
        &mut patched,
        SHELL_HOOK,
        PRE_COMMIT_ENABLED_PACKAGES,
        "pre-commit enabledPackages",
        "dev shell must install the generated pre-commit shellHook",
    )?;

    Ok(patched)
}

pub fn has_required_wiring(content: &str) -> bool {
    let has_treefmt_input =
        content.contains("treefmt-nix.url") || content.contains("treefmt-nix = {");
    let has_git_hooks_input =
        content.contains("git-hooks.url") || content.contains("git-hooks = {");
    has_treefmt_input
        && has_git_hooks_input
        && [
            TREEFMT_OUTPUT,
            GIT_HOOKS_OUTPUT,
            "treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);",
            "pre-commit-check = git-hooks.lib.${system}.run",
            "hooks = import ./nix/pre-commit.nix",
            "treefmtWrapper = treefmtEval.config.build.wrapper;",
            "formatter = treefmtEval.config.build.wrapper;",
            "formatting = treefmtEval.config.build.check self;",
            "pre-commit",
            "pre-commit-check.enabledPackages",
            "shellHook = pre-commit-check.shellHook;",
        ]
        .iter()
        .all(|snippet| content.contains(snippet))
}

pub fn patch_error(anchor: &str, reason: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "cannot patch flake.nix: missing or ambiguous anchor `{anchor}`; {reason}; run `simit init-flake --print` to get the generated template and apply the wiring manually"
    )
}

fn ensure_after(
    content: &mut String,
    snippet: &str,
    anchor: &str,
    anchor_name: &str,
    reason: &str,
) -> Result<()> {
    if content.contains(snippet.trim_end()) {
        return Ok(());
    }

    let index = unique_anchor(content, anchor, anchor_name, reason)? + anchor.len();
    content.insert_str(index, snippet);
    Ok(())
}

fn ensure_after_any_missing(
    content: &mut String,
    snippet: &str,
    existing_snippets: &[&str],
    anchor: &str,
    anchor_name: &str,
    reason: &str,
) -> Result<()> {
    if existing_snippets
        .iter()
        .any(|existing| content.contains(existing))
    {
        return Ok(());
    }
    ensure_after(content, snippet, anchor, anchor_name, reason)
}

fn ensure_after_statement(
    content: &mut String,
    snippet: &str,
    anchor: &str,
    anchor_name: &str,
    reason: &str,
) -> Result<()> {
    if content.contains(snippet.trim_end()) {
        return Ok(());
    }

    let start = unique_anchor(content, anchor, anchor_name, reason)?;
    let Some(offset) = content[start..].find(";\n") else {
        bail!(patch_error(anchor_name, reason));
    };
    content.insert_str(start + offset + 2, snippet);
    Ok(())
}

fn ensure_dev_shell_packages(content: &mut String) -> Result<()> {
    if !content.contains(PRE_COMMIT_PACKAGE) {
        ensure_after(
            content,
            PRE_COMMIT_PACKAGE,
            "          cargo-nextest\n",
            "devShell cargo-nextest package",
            "pre-commit package must be inserted into devShell packages",
        )?;
    }

    if content.contains(PRE_COMMIT_ENABLED_PACKAGES.trim_end()) {
        return Ok(());
    }

    ensure_replace(
        content,
        "        ];\n",
        PRE_COMMIT_ENABLED_PACKAGES,
        "devShell packages closing bracket",
        "pre-commit enabled packages must be appended to devShell packages",
    )
}

fn ensure_replace(
    content: &mut String,
    old: &str,
    new: &str,
    anchor_name: &str,
    reason: &str,
) -> Result<()> {
    let index = unique_anchor(content, old, anchor_name, reason)?;
    content.replace_range(index..index + old.len(), new);
    Ok(())
}

fn unique_anchor(content: &str, anchor: &str, anchor_name: &str, reason: &str) -> Result<usize> {
    let mut matches = content.match_indices(anchor);
    let Some((index, _)) = matches.next() else {
        bail!(patch_error(anchor_name, reason));
    };
    if matches.next().is_some() {
        bail!(patch_error(anchor_name, reason));
    }
    Ok(index)
}

fn template() -> String {
    r#"{
  description = "Rust project";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    git-hooks.url = "github:cachix/git-hooks.nix";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    crane,
    flake-utils,
    treefmt-nix,
    git-hooks,
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
      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
      pre-commit-check = git-hooks.lib.${system}.run {
        src = ./.;
        hooks = import ./nix/pre-commit.nix {
          inherit pkgs;
          treefmtWrapper = treefmtEval.config.build.wrapper;
          inherit rustToolchain;
        };
      };
    in {
      packages.default = package;
      formatter = treefmtEval.config.build.wrapper;
      checks = {
        default = package;
        formatting = treefmtEval.config.build.check self;
        clippy = craneLib.cargoClippy (commonArgs
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets --all-features -- --deny warnings";
          });
        fmt = craneLib.cargoFmt {inherit src;};
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
    });
}
"#
    .to_owned()
}

fn treefmt_nix(languages: &Languages) -> String {
    let mut content = String::new();
    content.push_str("{pkgs, ...}: {\n");
    content.push_str("  projectRootFile = \"flake.nix\";\n");

    if languages.rust {
        content.push_str("\n  programs.rustfmt.enable = true;\n");
    }
    if languages.nix {
        content.push_str("\n  programs.alejandra.enable = true;\n");
    }
    if languages.toml {
        content.push_str("\n  programs.taplo.enable = true;\n");
    }
    if languages.yaml || languages.markdown {
        content.push_str("\n  programs.prettier = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    package = pkgs.prettier;\n");
        content.push_str("    includes = [\n");
        if languages.markdown {
            content.push_str("      \"*.md\"\n");
            content.push_str("      \"*.markdown\"\n");
        }
        if languages.yaml {
            content.push_str("      \"*.yaml\"\n");
            content.push_str("      \"*.yml\"\n");
        }
        content.push_str("    ];\n");
        content.push_str("  };\n");
    }

    content.push_str("}\n");
    content
}

fn pre_commit_nix(languages: &Languages) -> String {
    let mut content = String::new();
    content.push_str("{\n");
    content.push_str("  pkgs,\n");
    content.push_str("  treefmtWrapper,\n");
    content.push_str("  rustToolchain ? null,\n");
    content.push_str("}: {\n");
    content.push_str("  treefmt = {\n");
    content.push_str("    enable = true;\n");
    content.push_str("    name = \"treefmt\";\n");
    content.push_str("    entry = \"${treefmtWrapper}/bin/treefmt --fail-on-change\";\n");
    content.push_str("    pass_filenames = false;\n");
    content.push_str("  };\n");

    if languages.rust {
        content.push_str("\n  cargo-fmt = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"cargo fmt\";\n");
        content.push_str("    entry = \"cargo fmt --all -- --check\";\n");
        content.push_str(
            "    extraPackages = pkgs.lib.optional (rustToolchain != null) rustToolchain;\n",
        );
        content.push_str("    pass_filenames = false;\n");
        content.push_str("  };\n");
        content.push_str("\n  cargo-clippy = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"cargo clippy\";\n");
        content.push_str(
            "    entry = \"cargo clippy --all-targets --all-features -- --deny warnings\";\n",
        );
        content.push_str(
            "    extraPackages = pkgs.lib.optional (rustToolchain != null) rustToolchain;\n",
        );
        content.push_str("    pass_filenames = false;\n");
        content.push_str("  };\n");
        content.push_str("\n  cargo-audit = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"cargo audit\";\n");
        content.push_str("    entry = \"cargo audit\";\n");
        content.push_str("    extraPackages = pkgs.lib.optional (rustToolchain != null) rustToolchain ++ [pkgs.cargo-audit];\n");
        content.push_str("    pass_filenames = false;\n");
        content.push_str("  };\n");
    }

    if languages.nix {
        content.push_str("\n  nix-flake-check = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"nix flake check\";\n");
        content.push_str(
            "    entry = \"nix --extra-experimental-features 'nix-command flakes' flake check --cores 0 --max-jobs auto --no-update-lock-file\";\n",
        );
        content.push_str("    extraPackages = [pkgs.nix];\n");
        content.push_str("    pass_filenames = false;\n");
        content.push_str("    stages = [\"manual\"];\n");
        content.push_str("  };\n");
    }

    if languages.uv_python {
        content.push_str("\n  uv-ruff-format = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"uv ruff format\";\n");
        content.push_str("    entry = \"uv run ruff format --check .\";\n");
        content.push_str("    extraPackages = [pkgs.uv];\n");
        content.push_str("    pass_filenames = false;\n");
        content.push_str("  };\n");
        content.push_str("\n  uv-mypy = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"uv mypy\";\n");
        content.push_str("    entry = \"uv run mypy .\";\n");
        content.push_str("    extraPackages = [pkgs.uv];\n");
        content.push_str("    pass_filenames = false;\n");
        content.push_str("  };\n");
    }

    content.push_str("}\n");
    content
}

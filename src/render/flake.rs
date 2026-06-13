use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::cli::FlakeTargetArg;
use crate::config::FlakeConfig;
use crate::project::{GeneratedFile, Languages};

/// Current canix Attic public key. Do NOT replace with the stale `uqr0...` key.
pub const CANIX_CACHE_KEY: &str = "canix:lPzPzKrmYqW5Rxa5r0uQWvCqD3S5nx0h2eCy7XD5JM8=";
/// Public binary cache served by the canix Attic instance.
pub const CANIX_CACHE_URL: &str = "https://attic.candee.baby/canix";
/// Upstream cache.nixos.org public key, advertised alongside the canix cache.
pub const NIXOS_CACHE_KEY: &str = "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";

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
const CARGO_AUDIT_PACKAGE: &str = "          cargo-audit\n";
const CARGO_DENY_PACKAGE: &str = "          cargo-deny\n";
const RELEASE_DEV_SHELL_PACKAGES: &str = r#"          cargo-about
          cargo-audit
          cargo-cyclonedx
          cargo-deny
          cargo-llvm-cov
          cargo-sbom
          cosign
          jq
          minisign
          nodejs
          rpm
          debootstrap
          util-linux
          reprepro
          taplo
"#;
const PRE_COMMIT_ENABLED_PACKAGES: &str = "        ] ++ pre-commit-check.enabledPackages;\n";
const SHELL_HOOK: &str = "        shellHook = pre-commit-check.shellHook;\n";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuditTools {
    pub audit: bool,
    pub deny: bool,
}

pub fn files(
    languages: &Languages,
    rust_edition: &str,
    rust_version: Option<&str>,
    cross_targets: Option<&[FlakeTargetArg]>,
    audit_tools: AuditTools,
) -> Vec<GeneratedFile> {
    let flake_content = match cross_targets {
        Some(targets) => cross_template(targets, audit_tools),
        None => template(audit_tools),
    };
    vec![
        GeneratedFile {
            relative_path: PathBuf::from("flake.nix"),
            content: flake_content,
        },
        GeneratedFile {
            relative_path: PathBuf::from("nix/treefmt.nix"),
            content: treefmt_nix(languages, rust_edition),
        },
        GeneratedFile {
            relative_path: PathBuf::from("nix/pre-commit.nix"),
            content: pre_commit_nix(languages, rust_version, audit_tools),
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

pub fn patch_existing(content: &str, audit_tools: AuditTools) -> Result<String> {
    if has_required_wiring_with_audit_tools(content, audit_tools) {
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
    ensure_dev_shell_packages(&mut patched, audit_tools)?;
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
    has_required_wiring_with_audit_tools(
        content,
        AuditTools {
            audit: true,
            deny: false,
        },
    )
}

pub fn has_required_wiring_with_audit_tools(content: &str, audit_tools: AuditTools) -> bool {
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
            "formatter = treefmtEval.config.build.wrapper;",
            "formatting = treefmtEval.config.build.check self;",
            "pre-commit",
            "pre-commit-check.enabledPackages",
        ]
        .iter()
        .all(|snippet| content.contains(snippet))
        && has_rust_toolchain_hook_package(content)
        && has_treefmt_wrapper_argument(content)
        && has_pre_commit_shell_hook(content)
        && (!audit_tools.audit || content.contains("cargo-audit"))
        && (!audit_tools.deny || content.contains("cargo-deny"))
}

pub fn custom_wiring_mismatches(content: &str, config: &FlakeConfig) -> Vec<String> {
    let mut missing = Vec::new();
    if !(content.contains("treefmt-nix.url") || content.contains("treefmt-nix = {")) {
        missing.push("flake.nix custom mode: missing treefmt-nix input".to_owned());
    }
    if !(content.contains("git-hooks.url") || content.contains("git-hooks = {")) {
        missing.push("flake.nix custom mode: missing git-hooks input".to_owned());
    }
    if !content
        .contains("treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);")
    {
        missing.push(
            "flake.nix custom mode: missing treefmtEval import of ./nix/treefmt.nix".to_owned(),
        );
    }
    if !content.contains("pre-commit-check = git-hooks.lib.${system}.run") {
        missing.push("flake.nix custom mode: missing pre-commit-check binding".to_owned());
    }
    if !content.contains("hooks = import ./nix/pre-commit.nix") {
        missing.push(
            "flake.nix custom mode: missing pre-commit hook import of ./nix/pre-commit.nix"
                .to_owned(),
        );
    }
    if !has_treefmt_wrapper_argument(content) {
        missing.push(
            "flake.nix custom mode: missing treefmtWrapper argument for pre-commit hooks"
                .to_owned(),
        );
    }
    if !contains_binding(content, &config.toolchain_binding) {
        missing.push(format!(
            "flake.nix custom mode: missing configured toolchain binding `{}`",
            config.toolchain_binding
        ));
    }
    if !contains_binding(content, &config.crane_lib_binding) {
        missing.push(format!(
            "flake.nix custom mode: missing configured crane lib binding `{}`",
            config.crane_lib_binding
        ));
    }
    if !contains_binding(content, &config.package_binding) {
        missing.push(format!(
            "flake.nix custom mode: missing configured package binding `{}`",
            config.package_binding
        ));
    }
    if config.formatter_output && !content.contains("formatter = treefmtEval.config.build.wrapper;")
    {
        missing.push("flake.nix custom mode: missing formatter output".to_owned());
    }
    if config.formatting_check
        && !content.contains("formatting = treefmtEval.config.build.check self;")
    {
        missing.push("flake.nix custom mode: missing formatting check".to_owned());
    }
    if config.pre_commit_shell_hook {
        if !content.contains("pre-commit-check.enabledPackages") {
            missing.push(
                "flake.nix custom mode: dev shell missing pre-commit-check.enabledPackages"
                    .to_owned(),
            );
        }
        if !has_pre_commit_shell_hook(content) {
            missing.push(
                "flake.nix custom mode: dev shell missing pre-commit-check.shellHook".to_owned(),
            );
        }
    }
    for package in &config.expected_outputs.packages {
        if !contains_attr_assignment(content, package) {
            missing.push(format!(
                "flake.nix custom mode: missing expected package output `{package}`"
            ));
        }
    }
    for check in &config.expected_outputs.checks {
        if !contains_attr_assignment(content, check) {
            missing.push(format!(
                "flake.nix custom mode: missing expected check output `{check}`"
            ));
        }
    }
    for top_level in &config.expected_outputs.top_level {
        if !contains_attr_assignment(content, top_level)
            && !content.contains(&format!("{top_level}."))
        {
            missing.push(format!(
                "flake.nix custom mode: missing expected top-level output `{top_level}`"
            ));
        }
    }
    missing
}

fn contains_binding(content: &str, binding: &str) -> bool {
    if binding.contains('.') {
        content.contains(binding)
    } else {
        contains_attr_assignment(content, binding)
            || content.contains(&format!("inherit {binding}"))
            || content.contains(&format!(" {binding};"))
    }
}

fn contains_attr_assignment(content: &str, name: &str) -> bool {
    content.contains(&format!("{name} ="))
        || content.contains(&format!("{name}="))
        || content.contains(&format!("inherit {name}"))
}

pub fn has_required_treefmt(content: &str, languages: &Languages, rust_edition: &str) -> bool {
    content.contains("projectRootFile = \"flake.nix\";")
        && (!languages.rust
            || (content.contains("programs.rustfmt")
                && content.contains("enable = true;")
                && content.contains(&format!("edition = \"{rust_edition}\";"))))
        && (!languages.nix || content.contains("programs.alejandra.enable = true;"))
        && (!languages.toml || content.contains("programs.taplo.enable = true;"))
        && (!(languages.yaml || languages.markdown) || content.contains("programs.prettier"))
        && (!languages.markdown || content.contains("\"*.md\""))
        && (!languages.yaml || content.contains("\"*.yaml\""))
}

pub fn has_required_pre_commit(
    content: &str,
    languages: &Languages,
    rust_version: Option<&str>,
    audit_tools: AuditTools,
) -> bool {
    (!languages.rust
        || (content.contains("cargo-fmt")
            && content.contains("cargo fmt --all -- --check")
            && content.contains("cargo-clippy")
            && content.contains("cargo clippy")
            && content.contains("--all-targets")
            && content.contains("--all-features")
            && content.contains("--deny warnings")
            && content.contains("cargo-audit")
            && content.contains("cargo audit")))
        && (!audit_tools.deny
            || (content.contains("cargo-deny")
                && content.contains("cargo deny check bans licenses sources")
                && content.contains("pkgs.cargo-deny")))
        && (!languages.nix
            || (content.contains("nix-flake-check") && content.contains("flake check")))
        && rust_version.is_none_or(|version| {
            let toolchain_version = rust_overlay_version(version);
            content.contains("cargo-msrv")
                && content.contains("cargo check MSRV")
                && content.contains(&format!(
                    "pkgs.rust-bin.stable.\"{toolchain_version}\".default"
                ))
                && content.contains("cargo check --workspace --all-features")
                && content.contains("\"pre-push\"")
                && content.contains("\"manual\"")
        })
}

pub fn patch_error(anchor: &str, reason: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "cannot patch flake.nix: missing or ambiguous anchor `{anchor}`; {reason}; run `simit init flake --print` to get the generated template and apply the wiring manually"
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

fn ensure_dev_shell_packages(content: &mut String, audit_tools: AuditTools) -> Result<()> {
    if audit_tools.audit {
        ensure_explicit_dev_shell_package(
            content,
            CARGO_AUDIT_PACKAGE,
            "devShell cargo audit package",
            "cargo-audit must be available in simit-managed Rust dev shells",
        )?;
    }
    if audit_tools.deny {
        ensure_explicit_dev_shell_package(
            content,
            CARGO_DENY_PACKAGE,
            "devShell cargo deny package",
            "cargo-deny must be available when dependency policy checks are enabled",
        )?;
    }

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

fn ensure_explicit_dev_shell_package(
    content: &mut String,
    package: &str,
    anchor_name: &str,
    reason: &str,
) -> Result<()> {
    if content.contains(package.trim_end()) {
        return Ok(());
    }

    ensure_after(
        content,
        package,
        "        packages = with pkgs; [\n",
        anchor_name,
        reason,
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

fn template(audit_tools: AuditTools) -> String {
    let mut content = r#"{
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
          cargo-about
          cargo-audit
          cargo-cyclonedx
          cargo-deny
          cargo-llvm-cov
          cargo-sbom
          cargo-nextest
          cosign
          jq
          minisign
          nodejs
          pre-commit
          rpm
          debootstrap
          util-linux
          reprepro
          rust-analyzer
          taplo
        ] ++ pre-commit-check.enabledPackages;
        shellHook = pre-commit-check.shellHook;
      };
      apps.local-check-fast = {
        type = "app";
        program = let
          script = pkgs.writeShellApplication {
            name = "local-check-fast";
            runtimeInputs = with pkgs; [
              cargo-deny
              git
              jq
              rustToolchain
            ];
            text = ''
              set -euo pipefail
              cargo test --workspace --all-features
              cargo clippy --workspace --all-targets --all-features -- --deny warnings
              cargo deny check bans licenses sources
              cargo package --workspace --allow-dirty --list >/dev/null
            '';
          };
        in "${script}/bin/local-check-fast";
      };
      apps.local-check-release = {
        type = "app";
        program = let
          script = pkgs.writeShellApplication {
            name = "local-check-release";
            runtimeInputs = with pkgs; [
              cargo-about
              cargo-cyclonedx
              cargo-deny
              cargo-sbom
              cosign
              jq
              minisign
              rustToolchain
            ];
            text = ''
              set -euo pipefail
              version="''${1:-}"
              if [ -z "$version" ]; then
                echo "usage: local-check-release <version>" >&2
                exit 2
              fi
              ${self.apps.${system}.local-check-fast.program}
              mkdir -p release
              if [ -f about-template.hbs ]; then
                cargo about generate --output-file release/THIRD_PARTY_LICENSES.html about-template.hbs
              else
                echo "warning: about-template.hbs not found; skipping cargo-about report" >&2
              fi
              cargo sbom --output-format cyclone_dx_json_1_5 > "release/''${version}.cdx.json"
              cargo sbom --output-format spdx_json_2_3 > "release/''${version}.spdx.json"
              if [ -n "''${COSIGN_PRIVATE_KEY:-}" ]; then
                echo "COSIGN_PRIVATE_KEY present; local release parity will not sign or upload" >&2
              else
                echo "warning: keyless Sigstore and COSIGN_PRIVATE_KEY unavailable locally; skipping local cosign signing" >&2
              fi
              echo "local release parity dry-run passed for ''${version}; no external publish was attempted"
            '';
          };
        in "${script}/bin/local-check-release";
      };
    });
}
"#
    .to_owned();
    insert_template_audit_packages(&mut content, audit_tools);
    content
}

/// Render the `targets = [ "native" ... ];` body shared by the mkCrossPackages
/// call. Targets keep canonical ordering regardless of CLI order.
fn cross_target_list(targets: &[FlakeTargetArg]) -> String {
    let mut ordered: Vec<FlakeTargetArg> = FlakeTargetArg::all()
        .into_iter()
        .filter(|target| targets.contains(target))
        .collect();
    if ordered.is_empty() {
        ordered.push(FlakeTargetArg::Native);
    }
    ordered
        .iter()
        .map(|target| format!("\"{}\"", target.key()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Nix expression for `packages.default`. Prefer the native attr; otherwise
/// fall back to the first requested target's output attr.
fn cross_default_attr(targets: &[FlakeTargetArg]) -> String {
    let ordered: Vec<FlakeTargetArg> = FlakeTargetArg::all()
        .into_iter()
        .filter(|target| targets.contains(target))
        .collect();
    // The native target's output attr is bare "${pname}"; everything else is
    // suffixed. Default to native when present or when no targets are selected.
    match ordered
        .iter()
        .copied()
        .find(|target| *target != FlakeTargetArg::Native)
    {
        Some(first_non_native) if !ordered.contains(&FlakeTargetArg::Native) => {
            format!("crossPackages.\"${{pname}}-{}\"", first_non_native.key())
        }
        _ => "crossPackages.${pname}".to_owned(),
    }
}

/// Render an rs-harbor-based multi-target flake that builds the requested cross
/// targets through `rs-harbor.lib.mkCrossPackages`.
pub fn cross_template(targets: &[FlakeTargetArg], audit_tools: AuditTools) -> String {
    let target_list = cross_target_list(targets);
    let default_attr = cross_default_attr(targets);
    let mut content = format!(
        r#"{{
  description = "Rust project";

  # Advertise the canix Attic cache so cross builds substitute prebuilt
  # toolchains and cross artifacts instead of rebuilding them locally.
  nixConfig = {{
    extra-substituters = ["{canix_url}"];
    extra-trusted-public-keys = [
      "{canix_key}"
      "{nixos_key}"
    ];
  }};

  inputs = {{
    rs-harbor.url = "git+https://codeberg.org/caniko/rs-harbor.git";

    nixpkgs.follows = "rs-harbor/nixpkgs";
    rust-overlay.follows = "rs-harbor/rust-overlay";
    crane.follows = "rs-harbor/crane";
    flake-utils.follows = "rs-harbor/flake-utils";

    treefmt-nix = {{
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    }};
    git-hooks = {{
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    }};
  }};

  outputs = {{
    self,
    nixpkgs,
    rs-harbor,
    rust-overlay,
    crane,
    flake-utils,
    treefmt-nix,
    git-hooks,
    ...
  }}:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {{
        inherit system;
        overlays = [(import rust-overlay)];
      }};

      toolchain = rs-harbor.lib.mkToolchain {{inherit pkgs;}};
      inherit (toolchain) craneLib;
      rustToolchain = toolchain.rustToolchain;
      cross = rs-harbor.lib.mkCross {{inherit pkgs system;}};

      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      pname = cargoToml.package.name or cargoToml.workspace.package.name;

      src = craneLib.cleanCargoSource ./.;
      commonArgs = {{
        inherit src;
        strictDeps = true;
      }};

      crossPackages = rs-harbor.lib.mkCrossPackages {{
        inherit pkgs cross pname commonArgs;
        inherit (toolchain) craneLib;
        targets = [{target_list}];
      }};

      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
      pre-commit-check = git-hooks.lib.${{system}}.run {{
        src = ./.;
        hooks = import ./nix/pre-commit.nix {{
          inherit pkgs;
          treefmtWrapper = treefmtEval.config.build.wrapper;
          inherit rustToolchain;
        }};
      }};
    in {{
      packages =
        crossPackages
        // {{
          default = {default_attr};
        }};
      formatter = treefmtEval.config.build.wrapper;
      checks = {{
        default = {default_attr};
        formatting = treefmtEval.config.build.check self;
        clippy = craneLib.cargoClippy (commonArgs
          // {{
            cargoArtifacts = craneLib.buildDepsOnly commonArgs;
            cargoClippyExtraArgs = "--all-targets --all-features -- --deny warnings";
          }});
        fmt = craneLib.cargoFmt {{inherit src;}};
      }};
      devShells = rs-harbor.lib.mkDevShells {{
        inherit pkgs cross;
        inherit (toolchain) craneLib;
        packages = with pkgs; [
          cargo-about
          cargo-audit
          cargo-cyclonedx
          cargo-deny
          cargo-llvm-cov
          cargo-sbom
          cargo-nextest
          cosign
          jq
          minisign
          nodejs
          pre-commit
          rpm
          debootstrap
          util-linux
          reprepro
          rust-analyzer
          taplo
        ] ++ pre-commit-check.enabledPackages;
        extraShellHook = pre-commit-check.shellHook;
      }};
      apps.local-check-fast = {{
        type = "app";
        program = let
          script = pkgs.writeShellApplication {{
            name = "local-check-fast";
            runtimeInputs = with pkgs; [
              cargo-deny
              git
              jq
              rustToolchain
            ];
            text = ''
              set -euo pipefail
              cargo test --workspace --all-features
              cargo clippy --workspace --all-targets --all-features -- --deny warnings
              cargo deny check bans licenses sources
              cargo package --workspace --allow-dirty --list >/dev/null
            '';
          }};
        in "${{script}}/bin/local-check-fast";
      }};
      apps.local-check-release = {{
        type = "app";
        program = let
          script = pkgs.writeShellApplication {{
            name = "local-check-release";
            runtimeInputs = with pkgs; [
              cargo-about
              cargo-cyclonedx
              cargo-deny
              cargo-sbom
              cosign
              jq
              minisign
              rustToolchain
            ];
            text = ''
              set -euo pipefail
              version="''${{1:-}}"
              if [ -z "$version" ]; then
                echo "usage: local-check-release <version>" >&2
                exit 2
              fi
              ${{self.apps.${{system}}.local-check-fast.program}}
              mkdir -p release
              if [ -f about-template.hbs ]; then
                cargo about generate --output-file release/THIRD_PARTY_LICENSES.html about-template.hbs
              else
                echo "warning: about-template.hbs not found; skipping cargo-about report" >&2
              fi
              cargo sbom --output-format cyclone_dx_json_1_5 > "release/''${{version}}.cdx.json"
              cargo sbom --output-format spdx_json_2_3 > "release/''${{version}}.spdx.json"
              if [ -n "''${{COSIGN_PRIVATE_KEY:-}}" ]; then
                echo "COSIGN_PRIVATE_KEY present; local release parity will not sign or upload" >&2
              else
                echo "warning: keyless Sigstore and COSIGN_PRIVATE_KEY unavailable locally; skipping local cosign signing" >&2
              fi
              echo "local release parity dry-run passed for ''${{version}}; no external publish was attempted"
            '';
          }};
        in "${{script}}/bin/local-check-release";
      }};
    }});
}}
"#,
        canix_url = CANIX_CACHE_URL,
        canix_key = CANIX_CACHE_KEY,
        nixos_key = NIXOS_CACHE_KEY,
        target_list = target_list,
        default_attr = default_attr,
    );
    insert_template_audit_packages(&mut content, audit_tools);
    content
}

fn insert_template_audit_packages(content: &mut String, audit_tools: AuditTools) {
    if !content.contains("cargo-about") {
        content.insert_str(
            dev_shell_packages_start(content),
            RELEASE_DEV_SHELL_PACKAGES,
        );
    }
    if audit_tools.deny && !content.contains(CARGO_DENY_PACKAGE.trim_end()) {
        content.insert_str(dev_shell_packages_start(content), CARGO_DENY_PACKAGE);
    }
    if audit_tools.audit && !content.contains(CARGO_AUDIT_PACKAGE.trim_end()) {
        content.insert_str(dev_shell_packages_start(content), CARGO_AUDIT_PACKAGE);
    }
}

fn dev_shell_packages_start(content: &str) -> usize {
    let anchor = "        packages = with pkgs; [\n";
    content
        .find(anchor)
        .map(|index| index + anchor.len())
        .expect("generated flake template has dev shell packages")
}

fn treefmt_nix(languages: &Languages, rust_edition: &str) -> String {
    let mut content = String::new();
    content.push_str("{pkgs, ...}: {\n");
    content.push_str("  projectRootFile = \"flake.nix\";\n");

    if languages.rust {
        content.push_str("\n  programs.rustfmt = {\n");
        content.push_str("    enable = true;\n");
        content.push_str(&format!("    edition = \"{rust_edition}\";\n"));
        content.push_str("    package = pkgs.rust-bin.nightly.latest.default.override {\n");
        content.push_str("      extensions = [\"rustfmt\"];\n");
        content.push_str("    };\n");
        content.push_str("  };\n");
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

fn has_rust_toolchain_hook_package(content: &str) -> bool {
    content.contains("inherit rustToolchain;")
        || content.contains("inherit pkgs rustToolchain;")
        || content.contains("rustToolchain = toolchain.rustToolchain;")
}

fn has_treefmt_wrapper_argument(content: &str) -> bool {
    content.contains("treefmtWrapper = treefmtEval.config.build.wrapper;")
}

fn has_pre_commit_shell_hook(content: &str) -> bool {
    content.contains("shellHook =") && content.contains("pre-commit-check.shellHook")
}

fn pre_commit_nix(
    languages: &Languages,
    rust_version: Option<&str>,
    audit_tools: AuditTools,
) -> String {
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
        if let Some(rust_version) = rust_version {
            let toolchain_version = rust_overlay_version(rust_version);
            content.push_str("\n  cargo-msrv = {\n");
            content.push_str("    enable = true;\n");
            content.push_str("    name = \"cargo check MSRV\";\n");
            content.push_str(&format!(
                "    entry = \"${{pkgs.rust-bin.stable.\"{toolchain_version}\".default}}/bin/cargo check --workspace --all-features\";\n"
            ));
            content.push_str(&format!(
                "    extraPackages = [pkgs.rust-bin.stable.\"{toolchain_version}\".default];\n"
            ));
            content.push_str("    pass_filenames = false;\n");
            content.push_str("    stages = [\"pre-push\" \"manual\"];\n");
            content.push_str("  };\n");
        }
        content.push_str("\n  cargo-audit = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"cargo audit\";\n");
        content.push_str("    entry = \"cargo audit\";\n");
        content.push_str("    extraPackages = pkgs.lib.optional (rustToolchain != null) rustToolchain ++ [pkgs.cargo-audit];\n");
        content.push_str("    pass_filenames = false;\n");
        content.push_str("  };\n");
        if audit_tools.deny {
            content.push_str("\n  cargo-deny = {\n");
            content.push_str("    enable = true;\n");
            content.push_str("    name = \"cargo deny\";\n");
            content.push_str("    entry = \"cargo deny check bans licenses sources\";\n");
            content.push_str("    extraPackages = pkgs.lib.optional (rustToolchain != null) rustToolchain ++ [pkgs.cargo-deny];\n");
            content.push_str("    pass_filenames = false;\n");
            content.push_str("  };\n");
        }
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

fn rust_overlay_version(version: &str) -> String {
    match version.matches('.').count() {
        0 => format!("{version}.0.0"),
        1 => format!("{version}.0"),
        _ => version.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::FlakeTargetArg;

    #[test]
    fn single_target_template_is_unchanged_by_cross_support() {
        let flake = template(AuditTools {
            audit: true,
            deny: false,
        });
        // The single-target path must keep its fixed crane build and must not
        // mention rs-harbor or the cross helper.
        assert!(flake.contains("package = craneLib.buildPackage"));
        assert!(flake.contains("packages.default = package;"));
        assert!(!flake.contains("mkCrossPackages"));
        assert!(!flake.contains("rs-harbor"));
        assert!(flake.contains("cargo-about"));
        assert!(flake.contains("cargo-audit"));
        assert!(flake.contains("cargo-deny"));
        assert!(flake.contains("cargo-sbom"));
        assert!(flake.contains("apps.local-check-fast"));
        assert!(flake.contains("apps.local-check-release"));
        assert!(flake.contains("no external publish was attempted"));
    }

    #[test]
    fn cross_template_with_all_targets_pins_the_shared_contract() {
        let flake = cross_template(
            &FlakeTargetArg::all(),
            AuditTools {
                audit: true,
                deny: true,
            },
        );

        // rs-harbor input and follows wiring.
        assert!(
            flake.contains("rs-harbor.url = \"git+https://codeberg.org/caniko/rs-harbor.git\";")
        );
        assert!(flake.contains("nixpkgs.follows = \"rs-harbor/nixpkgs\";"));
        assert!(flake.contains("rust-overlay.follows = \"rs-harbor/rust-overlay\";"));
        assert!(flake.contains("crane.follows = \"rs-harbor/crane\";"));
        assert!(flake.contains("flake-utils.follows = \"rs-harbor/flake-utils\";"));

        // Toolchain + cross + mkCrossPackages call with the exact argument set.
        assert!(flake.contains("toolchain = rs-harbor.lib.mkToolchain {inherit pkgs;};"));
        assert!(flake.contains("cross = rs-harbor.lib.mkCross {inherit pkgs system;};"));
        assert!(flake.contains("rs-harbor.lib.mkCrossPackages {"));
        assert!(flake.contains("inherit pkgs cross pname commonArgs;"));
        assert!(flake.contains("inherit (toolchain) craneLib;"));

        // Full canonical target list.
        assert!(flake.contains(
            "targets = [\"native\" \"aarch64-linux\" \"windows\" \"darwin-x86_64\" \"darwin-aarch64\"];"
        ));

        // Native package is the default.
        assert!(flake.contains("default = crossPackages.${pname};"));

        // Dev shells via rs-harbor and treefmt/git-hooks wiring preserved.
        assert!(flake.contains("rs-harbor.lib.mkDevShells {"));
        assert!(flake.contains("cargo-audit"));
        assert!(flake.contains("cargo-deny"));
        assert!(flake.contains("cargo-about"));
        assert!(flake.contains("cargo-sbom"));
        assert!(flake.contains("apps.local-check-fast"));
        assert!(flake.contains("apps.local-check-release"));
        assert!(flake.contains("no external publish was attempted"));
        assert!(
            flake.contains(
                "treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);"
            )
        );

        // Correct, current canix key plus cache.nixos.org key; not the stale key.
        assert!(flake.contains(CANIX_CACHE_KEY));
        assert!(flake.contains("canix:lPzPzKrmYqW5Rxa5r0uQWvCqD3S5nx0h2eCy7XD5JM8="));
        assert!(flake.contains(NIXOS_CACHE_KEY));
        assert!(flake.contains("https://attic.candee.baby/canix"));
        assert!(!flake.contains("uqr0"));

        // The cross flake still wires treefmt + pre-commit hooks through the
        // rs-harbor dev shell (cross-mode drift is exact-match, so this uses
        // the mkDevShells `extraShellHook` argument rather than the bare
        // `shellHook` the single-target path emits).
        assert!(flake.contains("pre-commit-check.enabledPackages"));
        assert!(flake.contains("extraShellHook = pre-commit-check.shellHook;"));
    }

    #[test]
    fn cross_template_respects_requested_subset_and_ordering() {
        // CLI order is windows-then-native, but canonical order must win.
        let flake = cross_template(
            &[FlakeTargetArg::Windows, FlakeTargetArg::Native],
            AuditTools {
                audit: true,
                deny: false,
            },
        );
        assert!(flake.contains("targets = [\"native\" \"windows\"];"));
        // Native present -> default is the native attr.
        assert!(flake.contains("default = crossPackages.${pname};"));
        assert!(!flake.contains("darwin"));
        assert!(!flake.contains("aarch64-linux"));
    }

    #[test]
    fn cross_template_default_falls_back_to_first_non_native_target() {
        let flake = cross_template(
            &[FlakeTargetArg::Windows, FlakeTargetArg::Aarch64Linux],
            AuditTools {
                audit: true,
                deny: false,
            },
        );
        assert!(flake.contains("targets = [\"aarch64-linux\" \"windows\"];"));
        // No native target -> default is the first canonical non-native attr.
        assert!(flake.contains("default = crossPackages.\"${pname}-aarch64-linux\";"));
    }
}

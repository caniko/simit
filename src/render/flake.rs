use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::cli::FlakeTargetArg;
use crate::config::{FlakeBackend, FlakeComponent, FlakeConfig};
use crate::project::{GeneratedFile, Languages};
use crate::python;

/// Current canix Attic public key. Do NOT replace with the stale `uqr0...` key.
pub const CANIX_CACHE_KEY: &str = "canix:lPzPzKrmYqW5Rxa5r0uQWvCqD3S5nx0h2eCy7XD5JM8=";
/// Public binary cache served by the canix Attic instance.
pub const CANIX_CACHE_URL: &str = "https://attic.candee.baby/canix";
/// Upstream cache.nixos.org public key, advertised alongside the canix cache.
pub const NIXOS_CACHE_KEY: &str = "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";
/// Pinned rs-harbor revision providing the fleet-wide Rust cache contract.
pub const RS_HARBOR_REV: &str = "b40cd4c4fdf6133962f67bd68a48bfd5d554d47f";

const TREEFMT_INPUT: &str = "    treefmt-nix.url = \"github:numtide/treefmt-nix\";\n";
const GIT_HOOKS_INPUT: &str = "    git-hooks.url = \"github:cachix/git-hooks.nix\";\n";
const TREEFMT_OUTPUT: &str = "    treefmt-nix,\n";
const GIT_HOOKS_OUTPUT: &str = "    git-hooks,\n";
const HOOK_BINDINGS: &str = r#"      fmtToolchain = rs-harbor.lib.mkToolchain {inherit pkgs; toolchainProfile = "nightly";};
      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix { rustfmtPackage = fmtToolchain.rustToolchain; });
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
          file
          gnutar
          gzip
          jq
          minisign
          nodejs
          rpm
          debootstrap
          util-linux
          unzip
          zip
          reprepro
           taplo
"#;
const MATURIN_PACKAGE: &str = "          maturin\n";
const PRE_COMMIT_ENABLED_PACKAGES: &str = "        ] ++ pre-commit-check.enabledPackages;\n";
const SHELL_HOOK: &str = "        shellHook = pre-commit-check.shellHook;\n";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuditTools {
    pub audit: bool,
    pub deny: bool,
    pub pyo3: bool,
}

pub fn files(
    languages: &Languages,
    rust_edition: &str,
    rust_version: Option<&str>,
    cross_targets: Option<&[FlakeTargetArg]>,
    audit_tools: AuditTools,
) -> Vec<GeneratedFile> {
    files_with_components(
        languages,
        rust_edition,
        rust_version,
        cross_targets,
        audit_tools,
        &[],
    )
}

pub fn files_with_components(
    languages: &Languages,
    rust_edition: &str,
    rust_version: Option<&str>,
    cross_targets: Option<&[FlakeTargetArg]>,
    audit_tools: AuditTools,
    components: &[FlakeComponent],
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
            content: pre_commit_nix(languages, rust_version, audit_tools, components),
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

pub fn python_files(
    languages: &Languages,
    project: &python::Project,
    components: &[FlakeComponent],
) -> Vec<GeneratedFile> {
    vec![
        GeneratedFile {
            relative_path: PathBuf::from("flake.nix"),
            content: python_template(project),
        },
        GeneratedFile {
            relative_path: PathBuf::from("nix/treefmt.nix"),
            content: treefmt_nix(languages, "2024"),
        },
        GeneratedFile {
            relative_path: PathBuf::from("nix/pre-commit.nix"),
            content: pre_commit_nix(languages, None, AuditTools::default(), components),
        },
    ]
}

pub fn generic_files(languages: &Languages, components: &[FlakeComponent]) -> Vec<GeneratedFile> {
    vec![
        GeneratedFile {
            relative_path: PathBuf::from("flake.nix"),
            content: String::new(),
        },
        GeneratedFile {
            relative_path: PathBuf::from("nix/treefmt.nix"),
            content: treefmt_nix(languages, "2024"),
        },
        GeneratedFile {
            relative_path: PathBuf::from("nix/pre-commit.nix"),
            content: pre_commit_nix(languages, None, AuditTools::default(), components),
        },
    ]
}

/// Bare treefmtEval call shape predating the pinned rustfmtPackage contract.
const BARE_TREEFMT_CALL: &str = "(import ./nix/treefmt.nix)";
/// Pinned call shape threading the harbor nightly rustfmt into the module.
const PINNED_TREEFMT_CALL: &str =
    "(import ./nix/treefmt.nix { rustfmtPackage = fmtToolchain.rustToolchain; })";
/// Binding the pinned call depends on.
const FMT_TOOLCHAIN_BINDING: &str =
    "fmtToolchain = rs-harbor.lib.mkToolchain {inherit pkgs; toolchainProfile = \"nightly\";};";

/// Migrate an old-shape treefmtEval call site to the pinned rustfmtPackage
/// contract, inserting the fmtToolchain binding it depends on. Both edits
/// happen in memory; callers must only write the result when this returns
/// Ok, so a failure can never leave a half-migrated flake on disk.
///
/// The binding is inserted with the surrounding indentation immediately
/// before the first treefmtEval line, so canonically generated flakes keep
/// matching HOOK_BINDINGS verbatim and later ensures stay idempotent.
fn migrate_treefmt_call(patched: &mut String) -> Result<()> {
    if !patched.contains(BARE_TREEFMT_CALL) {
        return Ok(());
    }
    if !patched.contains("fmtToolchain =") {
        let call_pos = patched
            .find("treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix)")
            .ok_or_else(|| {
                patch_error(
                    "treefmtEval call",
                    "expected a treefmtEval call site importing ./nix/treefmt.nix to anchor the fmtToolchain binding",
                )
            })?;
        let line_start = patched[..call_pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let indent: String = patched[line_start..call_pos]
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .collect();
        let binding = format!("{indent}{FMT_TOOLCHAIN_BINDING}\n");
        patched.insert_str(line_start, &binding);
    }
    *patched = patched.replace(BARE_TREEFMT_CALL, PINNED_TREEFMT_CALL);
    Ok(())
}

pub fn patch_existing(
    content: &str,
    audit_tools: AuditTools,
    needs_rustfmt_package: bool,
) -> Result<String> {
    let mut patched = content.to_owned();
    // Migrate the call site before checking wiring: the regenerated module
    // requires rustfmtPackage for Rust projects, so an untouched old flake
    // paired with a fresh module would break Nix evaluation.
    if needs_rustfmt_package {
        migrate_treefmt_call(&mut patched)?;
    }
    // Inserting HOOK_BINDINGS wholesale when a treefmtEval binding already
    // exists would duplicate the binding and break evaluation. A present
    // treefmtEval without its pre-commit-check companion is a hand-modified
    // state simit cannot complete safely: refuse with instructions instead.
    // Both checks are line-anchored so renamed bindings (e.g.
    // `dropped-pre-commit-check =`) do not count as present. This runs
    // before the early return so no path can write a lone treefmtEval.
    let has_treefmt_eval = patched
        .lines()
        .any(|line| line.trim_start().starts_with("treefmtEval ="));
    let has_pre_commit_check = patched
        .lines()
        .any(|line| line.trim_start().starts_with("pre-commit-check ="));
    if has_treefmt_eval && !has_pre_commit_check {
        bail!(patch_error(
            "pre-commit-check binding",
            "flake.nix defines treefmtEval but no pre-commit-check block; add the pre-commit-check binding from `simit init flake --print` next to treefmtEval",
        ));
    }
    if has_required_wiring_with_audit_tools(&patched, audit_tools) {
        return Ok(patched);
    }

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
    // Inserting HOOK_BINDINGS wholesale when a treefmtEval binding already
    // exists would duplicate the binding and break evaluation; the lone
    // treefmtEval case was already refused above.
    if !patched
        .lines()
        .any(|line| line.trim_start().starts_with("treefmtEval ="))
    {
        ensure_after_statement(
            &mut patched,
            HOOK_BINDINGS,
            "package = craneLib.buildPackage",
            "package binding",
            "hook bindings must be inserted after the package binding in the system let",
        )?;
    }
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
            pyo3: false,
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
            // Prefix match: new generations pass rustfmtPackage explicitly,
            // older ones call the module bare. Both shapes are wired.
            "treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix",
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

pub fn custom_wiring_mismatches(
    content: &str,
    config: &FlakeConfig,
    require_docs_shell: bool,
) -> Vec<String> {
    let mut missing = Vec::new();
    if !(content.contains("treefmt-nix.url")
        || content.contains("treefmt-nix = {")
        || content.contains("treefmt-nix.follows"))
    {
        missing.push("flake.nix custom mode: missing treefmt-nix input".to_owned());
    }
    if !(content.contains("git-hooks.url")
        || content.contains("git-hooks = {")
        || content.contains("git-hooks.follows"))
    {
        missing.push("flake.nix custom mode: missing git-hooks input".to_owned());
    }
    if !content
        .contains("treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix")
    {
        missing.push(
            "flake.nix custom mode: missing treefmtEval import of ./nix/treefmt.nix".to_owned(),
        );
    }
    if !has_pre_commit_check_binding(content) {
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
    match config.backend {
        FlakeBackend::RustCrane => {
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
        }
        FlakeBackend::PyHarbor => {
            if !content.contains("py-harbor") {
                missing
                    .push("flake.nix custom mode: missing py-harbor input or binding".to_owned());
            }
            if !content.contains("py-harbor.lib") {
                missing.push("flake.nix custom mode: missing py-harbor.lib usage".to_owned());
            }
        }
        FlakeBackend::Generic => {}
    }
    if config.formatter_output && !has_formatter_output(content) {
        missing.push("flake.nix custom mode: missing formatter output".to_owned());
    }
    if config.formatting_check && !has_formatting_check(content) {
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
    if require_docs_shell && !has_docs_shell(content) {
        missing.push("flake.nix custom mode: missing devShells.docs".to_owned());
    }
    for package in &config.expected_outputs.packages {
        if !contains_attr_assignment(content, package) {
            missing.push(format!(
                "flake.nix custom mode: missing expected package output `{package}`"
            ));
        }
    }
    for app in &config.expected_outputs.apps {
        if !contains_attr_assignment(content, app) {
            missing.push(format!(
                "flake.nix custom mode: missing expected app output `{app}`"
            ));
        }
    }
    for shell in &config.expected_outputs.dev_shells {
        if !contains_attr_assignment(content, shell) {
            missing.push(format!(
                "flake.nix custom mode: missing expected dev shell output `{shell}`"
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

fn has_docs_shell(content: &str) -> bool {
    if content.contains("docs = pkgs.mkShell")
        || content.contains("docs = py.mkUvDevShell")
        || content.contains("docs = craneLib.devShell")
    {
        return true;
    }

    let mut in_dev_shells = false;
    let mut dev_shell_depth = 0i32;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("devShells.docs") {
            return true;
        }

        if !in_dev_shells {
            if trimmed.starts_with("devShells =") {
                in_dev_shells = true;
                dev_shell_depth = brace_delta(trimmed);
                if trimmed.contains("docs =") {
                    return true;
                }
                continue;
            }
            continue;
        }

        if trimmed.starts_with("docs =") {
            return true;
        }

        dev_shell_depth += brace_delta(trimmed);
        if dev_shell_depth <= 0 {
            in_dev_shells = false;
        }
    }

    false
}

fn brace_delta(line: &str) -> i32 {
    let opens = line.chars().filter(|&ch| ch == '{').count() as i32;
    let closes = line.chars().filter(|&ch| ch == '}').count() as i32;
    opens - closes
}

/// Does a flake call-site pass rustfmtPackage into nix/treefmt.nix?
pub fn flake_passes_rustfmt_package(flake_content: &str) -> bool {
    flake_content.contains("rustfmtPackage")
        && flake_content.contains("import ./nix/treefmt.nix")
}

/// Does a nix/treefmt.nix module declare the rustfmtPackage parameter?
pub fn treefmt_module_wants_rustfmt_package(module_content: &str) -> bool {
    module_content.contains("{rustfmtPackage}")
}

/// Flake call-site and module signature must agree where disagreement breaks
/// Nix evaluation. Returns an actionable error message for broken pairs, or
/// None when the pair evaluates. A new-shape call into an old ellipsis
/// module (`{pkgs, ...}:`) is a working pair — the extra argument is
/// ignored — so it passes here; unpinned rustfmt is tracked as drift, not
/// failure. The reverse (old call into a module that requires the
/// parameter) always breaks evaluation, as does passing the argument to a
/// module whose header has no `...` to accept it.
pub fn treefmt_call_module_mismatch(
    flake_content: &str,
    module_content: &str,
) -> Option<String> {
    let call_passes = flake_passes_rustfmt_package(flake_content);
    let module_wants = treefmt_module_wants_rustfmt_package(module_content);
    match (call_passes, module_wants) {
        (true, true) | (false, false) => None,
        (true, false) => {
            let accepts_extra = module_content
                .lines()
                .next()
                .is_some_and(|header| header.contains("..."));
            if accepts_extra {
                None
            } else {
                Some(
                    "flake.nix passes rustfmtPackage but nix/treefmt.nix cannot accept it (no `...` in its parameters); regenerate nix/treefmt.nix with `simit init flake` (full scope) or remove the argument from the treefmtEval call"
                        .to_owned(),
                )
            }
        }
        (false, true) => Some(
            "nix/treefmt.nix requires rustfmtPackage but flake.nix does not pass it; add `fmtToolchain = rs-harbor.lib.mkToolchain {inherit pkgs; toolchainProfile = \"nightly\";};` and call `(import ./nix/treefmt.nix { rustfmtPackage = fmtToolchain.rustToolchain; })`, or regenerate with `simit init flake`"
                .to_owned(),
        ),
    }
}

pub fn has_required_treefmt(content: &str, languages: &Languages, rust_edition: &str) -> bool {
    content.contains("projectRootFile = \"flake.nix\";")
        && (!languages.rust
            || (content.contains("programs.rustfmt")
                && content.contains("enable = true;")
                && content.contains(&format!("edition = \"{rust_edition}\";"))))
        && (!languages.nix || content.contains("programs.alejandra.enable = true;"))
        && (!languages.toml || content.contains("programs.taplo.enable = true;"))
        && (!(languages.yaml || languages.markdown || languages.javascript)
            || content.contains("programs.prettier"))
        && (!languages.markdown || content.contains("\"*.md\""))
        && (!languages.yaml || content.contains("\"*.yaml\""))
        && (!languages.javascript || content.contains("\"*.ts\"") || content.contains("\"*.js\""))
        && (!languages.tex || content.contains("latexindent"))
}

pub fn has_required_pre_commit(
    content: &str,
    languages: &Languages,
    rust_version: Option<&str>,
    audit_tools: AuditTools,
) -> bool {
    has_required_pre_commit_with_components(content, languages, rust_version, audit_tools, &[])
}

pub fn has_required_pre_commit_with_components(
    content: &str,
    languages: &Languages,
    rust_version: Option<&str>,
    audit_tools: AuditTools,
    components: &[FlakeComponent],
) -> bool {
    let selected = |component: FlakeComponent, detected: bool| {
        if components.is_empty() {
            detected
        } else {
            components.contains(&component)
        }
    };
    let has_treefmt = content.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("treefmt =") || line.starts_with("treefmt=")
    });

    (!selected(FlakeComponent::Treefmt, true)
        || (has_treefmt && content.contains("treefmtWrapper")))
        && (!selected(FlakeComponent::CargoFmt, languages.rust)
            || (content.contains("cargo-fmt") && content.contains("cargo fmt --all -- --check")))
        && (!selected(FlakeComponent::CargoClippy, languages.rust)
            || (content.contains("cargo-clippy")
                && content.contains("cargo clippy")
                && content.contains("--all-targets")
                && content.contains("--all-features")
                && content.contains("--deny warnings")))
        && (!selected(FlakeComponent::CargoAudit, languages.rust)
            || (content.contains("cargo-audit") && content.contains("cargo audit")))
        && (!selected(
            FlakeComponent::CargoDeny,
            languages.rust && audit_tools.deny,
        ) || (content.contains("cargo-deny")
            && content.contains("cargo deny check bans licenses sources")
            && content.contains("pkgs.cargo-deny")))
        && (!selected(FlakeComponent::NixFlakeCheck, languages.nix)
            || (content.contains("nix-flake-check") && content.contains("flake check")))
        && (!selected(FlakeComponent::UvRuffFormat, languages.uv_python)
            || (content.contains("uv-ruff-format")
                && content.contains("uv run ruff format --check .")))
        && (!selected(FlakeComponent::UvMypy, languages.uv_python)
            || (content.contains("uv-mypy") && content.contains("uv run mypy .")))
        && (!(selected(
            FlakeComponent::CargoMsrv,
            languages.rust && rust_version.is_some(),
        ) && rust_version.is_some())
            || rust_version.is_some_and(|version| {
                let toolchain_version = rust_overlay_version(version);
                content.contains("cargo-msrv")
                    && content.contains("cargo check MSRV")
                    && content.contains(&format!(
                        "pkgs.rust-bin.stable.\"{toolchain_version}\".default"
                    ))
                    && content.contains("cargo check --workspace --all-features")
                    && content.contains("\"pre-push\"")
                    && content.contains("\"manual\"")
            }))
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
    rs-harbor.url = "git+https://github.com/caniko/harbor-rs.git?ref=trunk&rev=a3e5f76326f0f02de230cb2fba66fa3c1c7171cb";
    nixpkgs.follows = "rs-harbor/nixpkgs";
    rust-overlay.follows = "rs-harbor/rust-overlay";
    crane.follows = "rs-harbor/crane";
    flake-utils.url = "github:numtide/flake-utils";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    git-hooks.url = "github:cachix/git-hooks.nix";
  };

  outputs = {
    self,
    rs-harbor,
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

      toolchain = rs-harbor.lib.mkToolchain {inherit pkgs;};
      inherit (toolchain) craneLib rustToolchain;
      fmtToolchain = rs-harbor.lib.mkToolchain {inherit pkgs; toolchainProfile = "nightly";};
      src = craneLib.cleanCargoSource ./.;
      commonArgs = {
        inherit src;
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      package = craneLib.buildPackage (commonArgs // {inherit cargoArtifacts;});
      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix { rustfmtPackage = fmtToolchain.rustToolchain; });
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
          file
          gnutar
          gzip
          jq
          minisign
          nodejs
          pre-commit
          rpm
          util-linux
          unzip
          zip
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
        meta.description = "Run fast local validation checks";
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
              repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
              cd "$repo"
              ${self.apps.${system}.local-check-fast.program}
              manifest="release/artifacts.json"
              mkdir -p release
              jq -n --arg version "$version" \
                '{version: $version, artifacts: [], skipped: [], generated_by: "simit local-check-release"}' \
                > "$manifest.tmp"
              mv "$manifest.tmp" "$manifest"
              manifest_add_file() {
                path="$1"
                producer="$2"
                [ -f "$path" ] || return 0
                sha256="$(sha256sum "$path" | awk '{print $1}')"
                jq --arg path "$path" --arg sha256 "$sha256" --arg producer "$producer" \
                  '.artifacts += [{path: $path, sha256: $sha256, producer: $producer}]' \
                  "$manifest" > "$manifest.tmp"
                mv "$manifest.tmp" "$manifest"
              }
              manifest_skip() {
                name="$1"
                reason="$2"
                jq --arg name "$name" --arg reason "$reason" \
                  '.skipped += [{name: $name, reason: $reason}]' \
                  "$manifest" > "$manifest.tmp"
                mv "$manifest.tmp" "$manifest"
              }
              if [ -f about-template.hbs ]; then
                cargo about generate --output-file release/THIRD_PARTY_LICENSES.html about-template.hbs
                manifest_add_file release/THIRD_PARTY_LICENSES.html cargo-about
              else
                echo "warning: about-template.hbs not found; skipping cargo-about report" >&2
                manifest_skip cargo-about "about-template.hbs not found"
              fi
              cargo sbom --output-format cyclone_dx_json_1_5 > "release/''${version}.cdx.json"
              cargo sbom --output-format spdx_json_2_3 > "release/''${version}.spdx.json"
              manifest_add_file "release/''${version}.cdx.json" cargo-sbom-cyclonedx
              manifest_add_file "release/''${version}.spdx.json" cargo-sbom-spdx
              if [ -n "''${COSIGN_PRIVATE_KEY:-}" ]; then
                echo "COSIGN_PRIVATE_KEY present; local release parity will not sign or upload" >&2
              else
                echo "warning: keyless Sigstore and COSIGN_PRIVATE_KEY unavailable locally; skipping local cosign signing" >&2
                manifest_skip cosign "keyless Sigstore and COSIGN_PRIVATE_KEY unavailable locally"
              fi
              if [ -x scripts/release-local-check.sh ]; then
                bash scripts/release-local-check.sh "$version"
              fi
              echo "local release parity dry-run passed for ''${version}; no external publish was attempted"
            '';
          };
        in "${script}/bin/local-check-release";
        meta.description = "Run local release parity checks without publishing";
      };
      apps.local-release-deploy = {
        type = "app";
        program = let
          script = pkgs.writeShellApplication {
            name = "local-release-deploy";
            runtimeInputs = with pkgs; [
              git
              jq
            ];
            text = ''
              set -euo pipefail
              version="''${1:-}"
              publish_flag="''${2:-}"
              publish_version="''${3:-}"
              if [ -z "$version" ] || [ "$publish_flag" != "--publish" ] || [ "$publish_version" != "$version" ]; then
                echo "usage: local-release-deploy <version> --publish <version>" >&2
                echo "refusing to publish without an explicit matching confirmation" >&2
                exit 2
              fi
              repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
              cd "$repo"
              ${self.apps.${system}.local-check-release.program} "$version"
              if ! jq -e --arg version "$version" '.version == $version' release/artifacts.json >/dev/null; then
                echo "release/artifacts.json is missing or does not match version $version" >&2
                exit 1
              fi
              if [ -x scripts/local-release-deploy.sh ]; then
                SIMIT_LOCAL_RELEASE_CHECK_DONE=1 exec bash scripts/local-release-deploy.sh "$version" --publish "$version"
              fi
              echo "local-release-deploy has no project publisher hook at scripts/local-release-deploy.sh" >&2
              echo "Homebrew-capable hooks must build Darwin tarballs and gate tap pushes on HOMEBREW_TAP_TOKEN" >&2
              echo "local-check-release must remain non-publishing: no brew bump, git push, upload, or cargo publish" >&2
              exit 2
            '';
          };
        in "${script}/bin/local-release-deploy";
        meta.description = "Run the guarded local release deployment hook";
      };
    });
}

"#
    .to_owned();
    content = content.replace("a3e5f76326f0f02de230cb2fba66fa3c1c7171cb", RS_HARBOR_REV);
    insert_template_audit_packages(&mut content, audit_tools);
    content
}

fn python_template(project: &python::Project) -> String {
    let package_name = format!("{}-cpu", project.name);
    let env_name = format!("{}-cpu-env", project.name);
    let scripts = if project.scripts.is_empty() {
        format!("            \"{}\"\n", project.name)
    } else {
        project
            .scripts
            .iter()
            .map(|script| format!("            \"{script}\"\n"))
            .collect::<String>()
    };
    let dev_group = if project.dependency_groups.iter().any(|group| group == "dev") {
        "dev"
    } else {
        "default"
    };
    let cpu_extra = if project.optional_extras.iter().any(|extra| extra == "cpu") {
        "cpu"
    } else {
        "default"
    };
    let python_comment = project
        .requires_python
        .as_deref()
        .map(|requires| format!("  # Python requirement from pyproject.toml: {requires}\n"))
        .unwrap_or_default();
    let first_script = project
        .scripts
        .first()
        .map(String::as_str)
        .unwrap_or(project.name.as_str());

    format!(
        r#"{{
  description = "{name}: Python uv project";
{python_comment}
  inputs = {{
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    py-harbor = {{
      url = "git+https://github.com/caniko/harbor-py.git";
      inputs.nixpkgs.follows = "nixpkgs";
    }};

    treefmt-nix.url = "github:numtide/treefmt-nix";
    git-hooks.url = "github:cachix/git-hooks.nix";
  }};

  outputs = {{
    self,
    nixpkgs,
    py-harbor,
    treefmt-nix,
    git-hooks,
    ...
  }}:
    let
      py = py-harbor.lib;

      mkDevShells =
        system:
        let
          pkgs = py.mkPkgs {{ inherit system; }};
          treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
          pre-commit-check = git-hooks.lib.${{system}}.run {{
            src = ./.;
            hooks = import ./nix/pre-commit.nix {{
              inherit pkgs;
              treefmtWrapper = treefmtEval.config.build.wrapper;
            }};
          }};
        in
        {{
          default = py.mkUvDevShell {{
            inherit pkgs;
            uvExtra = "{cpu_extra}";
            devGroup = "{dev_group}";
            extraPackages = pre-commit-check.enabledPackages;
            shellHookSuffix = pre-commit-check.shellHook;
          }};
        }};

      mkPythonPackage =
        system:
        let
          pkgs = py.mkPkgs {{ inherit system; }};
          python = pkgs.python313;
        in
        py.mkUvAppPackage {{
          inherit pkgs python;
          name = "{package_name}";
          envName = "{env_name}";
          workspaceRoot = ./.;
          dependencies = {{
            {name} = [ "{cpu_extra}" ];
          }};
          scripts = [
{scripts}          ];
        }};

      mkPythonCheckEnv =
        system:
        let
          pkgs = py.mkPkgs {{ inherit system; }};
          python = pkgs.python313;
        in
        py.mkUvCheckEnv {{
          inherit pkgs python;
          name = "{env_name}-check";
          workspaceRoot = ./.;
          dependencies = {{
            {name} = [
              "{cpu_extra}"
              "{dev_group}"
            ];
          }};
        }};

      mkChecks =
        system:
        let
          pkgs = py.mkPkgs {{ inherit system; }};
          treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
          checkEnv = mkPythonCheckEnv system;
          package = self.packages.${{system}}.{package_name};
        in
        {{
          flake-eval = pkgs.runCommand "{name}-flake-eval" {{ }} ''
            test -x ${{package}}/bin/{first_script}
            mkdir -p $out
            echo ok > $out/result
          '';
          formatting = treefmtEval.config.build.check self;
          offline-tests = pkgs.runCommand "{name}-offline-tests" {{ }} ''
            export HOME=$TMPDIR/home
            export XDG_CACHE_HOME=$TMPDIR/cache
            mkdir -p "$HOME" "$XDG_CACHE_HOME" "$out"
            cd ${{./.}}
            ${{checkEnv}}/bin/python -m pytest -p no:cacheprovider
            echo ok > $out/result
          '';
          typecheck = pkgs.runCommand "{name}-typecheck" {{ }} ''
            export HOME=$TMPDIR/home
            export XDG_CACHE_HOME=$TMPDIR/cache
            mkdir -p "$HOME" "$XDG_CACHE_HOME" "$out"
            cd ${{./.}}
            ${{checkEnv}}/bin/python -m mypy .
            echo ok > $out/result
          '';
          uv-format = pkgs.runCommand "{name}-uv-format" {{ }} ''
            export HOME=$TMPDIR/home
            export XDG_CACHE_HOME=$TMPDIR/cache
            export UV_NO_SYNC=1
            mkdir -p "$HOME" "$XDG_CACHE_HOME" "$out"
            cd ${{./.}}
            ${{checkEnv}}/bin/uv run --no-sync ruff format --check .
            echo ok > $out/result
          '';
        }};

    in
    {{
      devShells = py.forAllSystems mkDevShells;

      packages = py.forPackageSystems (
        system:
        let
          package = mkPythonPackage system;
        in
        {{
          {package_name} = package;
          default = package;
        }}
      );

      apps = py.forPackageSystems (
        system:
        let
          package = self.packages.${{system}}.{package_name};
        in
        {{
          {package_name} = {{
            type = "app";
            program = "${{package}}/bin/{first_script}";
          }};
          default = self.apps.${{system}}.{package_name};
        }}
      );

      formatter = py.forAllSystems (
        system:
        let
          pkgs = py.mkPkgs {{ inherit system; }};
          treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
        in
        treefmtEval.config.build.wrapper
      );

      checks = py.forPackageSystems mkChecks;
    }};
}}
"#,
        name = project.name,
        python_comment = python_comment,
        cpu_extra = cpu_extra,
        dev_group = dev_group,
        package_name = package_name,
        env_name = env_name,
        scripts = scripts,
        first_script = first_script,
    )
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
    rs-harbor.url = "git+https://github.com/caniko/harbor-rs.git?ref=trunk&rev=a3e5f76326f0f02de230cb2fba66fa3c1c7171cb";

    nixpkgs.follows = "rs-harbor/nixpkgs";
    rust-overlay.follows = "rs-harbor/rust-overlay";
    crane.follows = "rs-harbor/crane";
    flake-utils.url = "github:numtide/flake-utils";

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
      inherit (toolchain) craneLib buildCache;
      rustToolchain = toolchain.rustToolchain;
      fmtToolchain = rs-harbor.lib.mkToolchain {{inherit pkgs; toolchainProfile = "nightly";}};
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
        inherit buildCache;
        targets = [{target_list}];
      }};

      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix {{ rustfmtPackage = fmtToolchain.rustToolchain; }});
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
          file
          gnutar
          gzip
          jq
          minisign
          nodejs
          pre-commit
          rpm
          util-linux
          unzip
          zip
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
        meta.description = "Run fast local validation checks";
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
              repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
              cd "$repo"
              ${{self.apps.${{system}}.local-check-fast.program}}
              manifest="release/artifacts.json"
              mkdir -p release
              jq -n --arg version "$version" \
                '{{version: $version, artifacts: [], skipped: [], generated_by: "simit local-check-release"}}' \
                > "$manifest.tmp"
              mv "$manifest.tmp" "$manifest"
              manifest_add_file() {{
                path="$1"
                producer="$2"
                [ -f "$path" ] || return 0
                sha256="$(sha256sum "$path" | awk '{{print $1}}')"
                jq --arg path "$path" --arg sha256 "$sha256" --arg producer "$producer" \
                  '.artifacts += [{{path: $path, sha256: $sha256, producer: $producer}}]' \
                  "$manifest" > "$manifest.tmp"
                mv "$manifest.tmp" "$manifest"
              }}
              manifest_skip() {{
                name="$1"
                reason="$2"
                jq --arg name "$name" --arg reason "$reason" \
                  '.skipped += [{{name: $name, reason: $reason}}]' \
                  "$manifest" > "$manifest.tmp"
                mv "$manifest.tmp" "$manifest"
              }}
              if [ -f about-template.hbs ]; then
                cargo about generate --output-file release/THIRD_PARTY_LICENSES.html about-template.hbs
                manifest_add_file release/THIRD_PARTY_LICENSES.html cargo-about
              else
                echo "warning: about-template.hbs not found; skipping cargo-about report" >&2
                manifest_skip cargo-about "about-template.hbs not found"
              fi
              cargo sbom --output-format cyclone_dx_json_1_5 > "release/''${{version}}.cdx.json"
              cargo sbom --output-format spdx_json_2_3 > "release/''${{version}}.spdx.json"
              manifest_add_file "release/''${{version}}.cdx.json" cargo-sbom-cyclonedx
              manifest_add_file "release/''${{version}}.spdx.json" cargo-sbom-spdx
              if [ -n "''${{COSIGN_PRIVATE_KEY:-}}" ]; then
                echo "COSIGN_PRIVATE_KEY present; local release parity will not sign or upload" >&2
              else
                echo "warning: keyless Sigstore and COSIGN_PRIVATE_KEY unavailable locally; skipping local cosign signing" >&2
                manifest_skip cosign "keyless Sigstore and COSIGN_PRIVATE_KEY unavailable locally"
              fi
              if [ -x scripts/release-local-check.sh ]; then
                bash scripts/release-local-check.sh "$version"
              fi
              echo "local release parity dry-run passed for ''${{version}}; no external publish was attempted"
            '';
          }};
        in "${{script}}/bin/local-check-release";
        meta.description = "Run local release parity checks without publishing";
      }};
      apps.local-release-deploy = {{
        type = "app";
        program = let
          script = pkgs.writeShellApplication {{
            name = "local-release-deploy";
            runtimeInputs = with pkgs; [
              git
              jq
            ];
            text = ''
              set -euo pipefail
              version="''${{1:-}}"
              publish_flag="''${{2:-}}"
              publish_version="''${{3:-}}"
              if [ -z "$version" ] || [ "$publish_flag" != "--publish" ] || [ "$publish_version" != "$version" ]; then
                echo "usage: local-release-deploy <version> --publish <version>" >&2
                echo "refusing to publish without an explicit matching confirmation" >&2
                exit 2
              fi
              repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
              cd "$repo"
              ${{self.apps.${{system}}.local-check-release.program}} "$version"
              if ! jq -e --arg version "$version" '.version == $version' release/artifacts.json >/dev/null; then
                echo "release/artifacts.json is missing or does not match version $version" >&2
                exit 1
              fi
              if [ -x scripts/local-release-deploy.sh ]; then
                SIMIT_LOCAL_RELEASE_CHECK_DONE=1 exec bash scripts/local-release-deploy.sh "$version" --publish "$version"
              fi
              echo "local-release-deploy has no project publisher hook at scripts/local-release-deploy.sh" >&2
              echo "Homebrew-capable hooks must build Darwin tarballs and gate tap pushes on HOMEBREW_TAP_TOKEN" >&2
              echo "local-check-release must remain non-publishing: no brew bump, git push, upload, or cargo publish" >&2
              exit 2
            '';
          }};
        in "${{script}}/bin/local-release-deploy";
        meta.description = "Run the guarded local release deployment hook";
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
    content = content.replace("a3e5f76326f0f02de230cb2fba66fa3c1c7171cb", RS_HARBOR_REV);
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
    if audit_tools.pyo3 && !content.contains(MATURIN_PACKAGE.trim_end()) {
        content.insert_str(dev_shell_packages_start(content), MATURIN_PACKAGE);
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
    // rustfmtPackage comes from the harbor pinned nightly profile via the
    // generated flake (fmtToolchain); never float nightly.latest here, or
    // fleet formatting diverges per project overlay. Python-only modules
    // stay directly importable: no Rust parameter without Rust formatters.
    if languages.rust {
        content.push_str("{rustfmtPackage}: {pkgs, ...}: {\n");
    } else {
        content.push_str("{pkgs, ...}: {\n");
    }
    content.push_str("  projectRootFile = \"flake.nix\";\n");

    if languages.rust {
        content.push_str("\n  programs.rustfmt = {\n");
        content.push_str("    enable = true;\n");
        content.push_str(&format!("    edition = \"{rust_edition}\";\n"));
        content.push_str("    package = rustfmtPackage;\n");
        content.push_str("  };\n");
    }
    if languages.nix {
        content.push_str("\n  programs.alejandra.enable = true;\n");
    }
    if languages.toml {
        content.push_str("\n  programs.taplo.enable = true;\n");
    }
    if languages.yaml || languages.markdown || languages.javascript {
        content.push_str("\n  programs.prettier = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    package = pkgs.prettier;\n");
        content.push_str("    excludes = [\n");
        content.push_str("      \".crow/**\"\n");
        content.push_str("    ];\n");
        content.push_str("    includes = [\n");
        if languages.markdown {
            content.push_str("      \"*.md\"\n");
            content.push_str("      \"*.markdown\"\n");
        }
        if languages.yaml {
            content.push_str("      \"*.yaml\"\n");
            content.push_str("      \"*.yml\"\n");
        }
        if languages.javascript {
            content.push_str("      \"*.js\"\n");
            content.push_str("      \"*.jsx\"\n");
            content.push_str("      \"*.mjs\"\n");
            content.push_str("      \"*.cjs\"\n");
            content.push_str("      \"*.ts\"\n");
            content.push_str("      \"*.tsx\"\n");
            content.push_str("      \"*.json\"\n");
        }
        content.push_str("    ];\n");
        content.push_str("  };\n");
    }

    if languages.tex {
        content.push_str("\n  settings.formatter.latexindent = {\n");
        content.push_str("    command = \"${pkgs.latexindent}/bin/latexindent\";\n");
        content.push_str("    options = [\"-w\" \"-s\"];\n");
        content.push_str("    includes = [\n");
        content.push_str("      \"*.tex\"\n");
        content.push_str("      \"*.sty\"\n");
        content.push_str("      \"*.cls\"\n");
        content.push_str("      \"*.bib\"\n");
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

fn has_pre_commit_check_binding(content: &str) -> bool {
    content.contains("pre-commit-check = git-hooks.lib.${system}.run")
        || ((content.contains("pre-commit-check =") || content.contains("pre-commit-check="))
            && content.contains("git-hooks.lib")
            && content.contains(".run"))
}

fn has_formatter_output(content: &str) -> bool {
    (content.contains("formatter =") || content.contains("formatter."))
        && content.contains("treefmtEval.config.build.wrapper")
}

fn has_formatting_check(content: &str) -> bool {
    (content.contains("formatting =") || content.contains(".formatting ="))
        && content.contains("treefmtEval.config.build.check self")
}

fn has_pre_commit_shell_hook(content: &str) -> bool {
    (content.contains("shellHook =")
        || content.contains("shellHookSuffix =")
        || content.contains("extraShellHook ="))
        && content.contains("pre-commit-check.shellHook")
}

fn pre_commit_nix(
    languages: &Languages,
    rust_version: Option<&str>,
    audit_tools: AuditTools,
    components: &[FlakeComponent],
) -> String {
    let selected = |component: FlakeComponent, detected: bool| {
        if components.is_empty() {
            detected
        } else {
            components.contains(&component)
        }
    };
    let has_rust_component = [
        FlakeComponent::CargoFmt,
        FlakeComponent::CargoClippy,
        FlakeComponent::CargoMsrv,
        FlakeComponent::CargoAudit,
        FlakeComponent::CargoDeny,
    ]
    .into_iter()
    .any(|component| selected(component, languages.rust));
    let mut content = String::new();
    content.push_str("{\n");
    content.push_str("  pkgs,\n");
    if selected(FlakeComponent::Treefmt, true) {
        content.push_str("  treefmtWrapper,\n");
    }
    if has_rust_component {
        content.push_str("  rustToolchain ? null,\n");
    }
    content.push_str("}: {\n");
    if selected(FlakeComponent::Treefmt, true) {
        content.push_str("  treefmt = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"treefmt\";\n");
        content.push_str("    package = treefmtWrapper;\n");
        content.push_str("    entry = \"${treefmtWrapper}/bin/treefmt --fail-on-change\";\n");
        content.push_str("    pass_filenames = false;\n");
        content.push_str("  };\n");
    }

    if selected(FlakeComponent::CargoFmt, languages.rust) {
        content.push_str("\n  cargo-fmt = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"cargo fmt\";\n");
        content.push_str("    entry = \"cargo fmt --all -- --check\";\n");
        content.push_str(
            "    extraPackages = pkgs.lib.optional (rustToolchain != null) rustToolchain;\n",
        );
        content.push_str("    pass_filenames = false;\n");
        content.push_str("  };\n");
    }
    if selected(FlakeComponent::CargoClippy, languages.rust) {
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
    }
    if selected(
        FlakeComponent::CargoMsrv,
        languages.rust && rust_version.is_some(),
    ) {
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
    }
    if selected(FlakeComponent::CargoAudit, languages.rust) {
        content.push_str("\n  cargo-audit = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"cargo audit\";\n");
        content.push_str("    entry = \"cargo audit\";\n");
        content.push_str("    extraPackages = pkgs.lib.optional (rustToolchain != null) rustToolchain ++ [pkgs.cargo-audit];\n");
        content.push_str("    pass_filenames = false;\n");
        content.push_str("  };\n");
    }
    if selected(
        FlakeComponent::CargoDeny,
        languages.rust && audit_tools.deny,
    ) {
        content.push_str("\n  cargo-deny = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"cargo deny\";\n");
        content.push_str("    entry = \"cargo deny check bans licenses sources\";\n");
        content.push_str("    extraPackages = pkgs.lib.optional (rustToolchain != null) rustToolchain ++ [pkgs.cargo-deny];\n");
        content.push_str("    pass_filenames = false;\n");
        content.push_str("  };\n");
    }

    if selected(FlakeComponent::NixFlakeCheck, languages.nix) {
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

    if selected(FlakeComponent::UvRuffFormat, languages.uv_python) {
        content.push_str("\n  uv-ruff-format = {\n");
        content.push_str("    enable = true;\n");
        content.push_str("    name = \"uv ruff format\";\n");
        content.push_str("    entry = \"uv run ruff format --check .\";\n");
        content.push_str("    extraPackages = [pkgs.uv];\n");
        content.push_str("    pass_filenames = false;\n");
        content.push_str("  };\n");
    }
    if selected(FlakeComponent::UvMypy, languages.uv_python) {
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
    use crate::project::Languages;

    #[test]
    fn treefmt_module_takes_rustfmt_package_only_for_rust() {
        let rust = treefmt_nix(
            &Languages {
                rust: true,
                nix: true,
                ..Languages::default()
            },
            "2024",
        );
        assert!(rust.starts_with("{rustfmtPackage}: {pkgs, ...}: {"));
        assert!(rust.contains("package = rustfmtPackage;"));
        assert!(!rust.contains("nightly.latest"));

        let python_only = treefmt_nix(
            &Languages {
                nix: true,
                toml: true,
                ..Languages::default()
            },
            "2024",
        );
        assert!(python_only.starts_with("{pkgs, ...}: {"));
        assert!(!python_only.contains("rustfmtPackage"));
    }

    #[test]
    fn treefmt_call_module_mismatch_catches_breakage_not_drift() {
        let new_call = "treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix { rustfmtPackage = fmtToolchain.rustToolchain; });";
        let old_call = "treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);";
        let new_module = "{rustfmtPackage}: {pkgs, ...}: {\n";
        let old_module = "{pkgs, ...}: {\n";
        let rigid_module = "{pkgs}: {\n";
        assert!(treefmt_call_module_mismatch(new_call, new_module).is_none());
        assert!(treefmt_call_module_mismatch(old_call, old_module).is_none());
        // New call into an ellipsis module evaluates (argument ignored).
        assert!(treefmt_call_module_mismatch(new_call, old_module).is_none());
        // Old call into a module requiring the parameter always breaks.
        assert!(treefmt_call_module_mismatch(old_call, new_module).is_some());
        // ...unless the module cannot accept extra arguments at all.
        assert!(treefmt_call_module_mismatch(new_call, rigid_module).is_some());
    }

    const OLD_WIRED_FLAKE: &str = r#"{
  description = "demo";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    git-hooks.url = "github:cachix/git-hooks.nix";
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    rs-harbor,
    treefmt-nix,
    git-hooks,
    ...
  }:
    {
      packages.default = package;
      formatter = treefmtEval.config.build.wrapper;
      checks = {
        default = package;
        formatting = treefmtEval.config.build.check self;
      };
      devShells.default = {
        packages = [ pre-commit ] ++ pre-commit-check.enabledPackages;
        shellHook = pre-commit-check.shellHook;
      };
      legacyPackages = let
        pkgs = import nixpkgs { inherit system; };
        toolchain = rs-harbor.lib.mkToolchain {inherit pkgs;};
        inherit (toolchain) craneLib rustToolchain;
        src = craneLib.cleanCargoSource ./.;
        commonArgs = { inherit src; strictDeps = true; };
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
        inherit package;
      };
    };
}
"#;

    const NEW_MODULE: &str = "{rustfmtPackage}: {pkgs, ...}: {\n";

    #[test]
    fn migrate_treefmt_call_upgrades_bare_call_site() {
        let mut patched = OLD_WIRED_FLAKE.to_owned();
        migrate_treefmt_call(&mut patched).unwrap();
        assert!(patched.contains(
            "(import ./nix/treefmt.nix { rustfmtPackage = fmtToolchain.rustToolchain; })"
        ));
        assert!(!patched.contains("(import ./nix/treefmt.nix)"));
        assert_eq!(patched.matches("treefmtEval =").count(), 1);
        assert_eq!(patched.matches("fmtToolchain =").count(), 1);
        // The binding lands on the line immediately before its use with
        // matching indentation, so canonically generated flakes keep
        // matching HOOK_BINDINGS verbatim downstream.
        let lines: Vec<&str> = patched.lines().collect();
        let binding_line = lines
            .iter()
            .position(|line| line.trim_start().starts_with("fmtToolchain ="))
            .expect("migrated flake has a fmtToolchain binding");
        let call_line = lines
            .iter()
            .position(|line| line.contains("rustfmtPackage = fmtToolchain.rustToolchain"))
            .expect("migrated flake has a pinned call site");
        assert_eq!(binding_line + 1, call_line);
        fn indent_of(line: &str) -> &str {
            &line[..line.len() - line.trim_start().len()]
        }
        assert_eq!(
            indent_of(lines[binding_line]),
            indent_of(lines[call_line])
        );
        // Idempotent: a second pass changes nothing.
        let mut twice = patched.clone();
        migrate_treefmt_call(&mut twice).unwrap();
        assert_eq!(patched, twice);
        // The migrated flake agrees with the fresh module shape.
        assert!(treefmt_call_module_mismatch(&patched, NEW_MODULE).is_none());
    }

    #[test]
    fn migrate_treefmt_call_leaves_new_shape_alone() {
        let mut patched = OLD_WIRED_FLAKE
            .replace("(import ./nix/treefmt.nix)", "(import ./nix/treefmt.nix { rustfmtPackage = fmtToolchain.rustToolchain; })")
            .replace(
                "inherit (toolchain) craneLib rustToolchain;",
                "inherit (toolchain) craneLib rustToolchain;\n        fmtToolchain = rs-harbor.lib.mkToolchain {inherit pkgs; toolchainProfile = \"nightly\";};",
            );
        let before = patched.clone();
        migrate_treefmt_call(&mut patched).unwrap();
        assert_eq!(patched, before);
    }

    #[test]
    fn patch_existing_migrates_old_wired_flake_for_rust() {
        let patched =
            patch_existing(OLD_WIRED_FLAKE, AuditTools::default(), true).unwrap();
        assert!(treefmt_call_module_mismatch(&patched, NEW_MODULE).is_none());
        assert_eq!(patched.matches("treefmtEval =").count(), 1);
    }

    #[test]
    fn patch_existing_leaves_bare_call_alone_without_rust() {
        let patched =
            patch_existing(OLD_WIRED_FLAKE, AuditTools::default(), false).unwrap();
        assert!(patched.contains("(import ./nix/treefmt.nix)"));
        assert!(!patched.contains("fmtToolchain ="));
    }

    #[test]
    fn patch_existing_refuses_migration_without_anchor() {
        // Bare call present but no treefmtEval binding line to anchor the
        // fmtToolchain insert (e.g. renamed binding): refuse loudly instead
        // of writing a half-migrated flake.
        let odd = "{\n  x = (import ./nix/treefmt.nix);\n}\n";
        let err = patch_existing(odd, AuditTools::default(), true).unwrap_err();
        assert!(err.to_string().contains("fmtToolchain"));
    }

    #[test]
    fn patch_existing_refuses_treefmt_eval_without_pre_commit() {
        // Inserting a second treefmtEval binding would break evaluation;
        // a lone treefmtEval without its pre-commit companion is refused.
        let mut lone = OLD_WIRED_FLAKE.to_owned();
        lone = lone.replace("        pre-commit-check = git-hooks.lib.${system}.run {", "        other = 1;\n        dropped-pre-commit-check = git-hooks.lib.${system}.run {");
        let err = patch_existing(&lone, AuditTools::default(), true).unwrap_err();
        assert!(err.to_string().contains("pre-commit-check"));
    }

    #[test]
    fn single_target_template_is_unchanged_by_cross_support() {
        let flake = template(AuditTools {
            audit: true,
            deny: false,
            pyo3: false,
        });
        // Every generated Rust flake consumes the canonical rs-harbor cache
        // contract, while the single-target path remains non-cross.
        assert!(flake.contains("package = craneLib.buildPackage"));
        assert!(flake.contains("packages.default = package;"));
        assert!(!flake.contains("mkCrossPackages"));
        assert!(flake.contains("toolchain = rs-harbor.lib.mkToolchain {inherit pkgs;};"));
        assert!(flake.contains("package = craneLib.buildPackage"));
        assert!(!flake.contains("rs-harbor.lib.mkBuildCachePolicy"));
        assert!(flake.contains("cargo-about"));
        assert!(flake.contains("cargo-audit"));
        assert!(flake.contains("cargo-deny"));
        assert!(flake.contains("cargo-sbom"));
        assert!(flake.contains("file"));
        assert!(flake.contains("gnutar"));
        assert!(flake.contains("gzip"));
        assert!(flake.contains("zip"));
        assert!(flake.contains("apps.local-check-fast"));
        assert!(flake.contains("apps.local-check-release"));
        assert!(flake.contains("apps.local-release-deploy"));
        assert!(flake.contains("meta.description = \"Run fast local validation checks\";"));
        assert!(flake.contains(
            "meta.description = \"Run local release parity checks without publishing\";"
        ));
        assert!(
            flake.contains("meta.description = \"Run the guarded local release deployment hook\";")
        );
        assert!(flake.contains("no external publish was attempted"));
        assert!(flake.contains("release/artifacts.json"));
        assert!(flake.contains("generated_by: \"simit local-check-release\""));
        assert!(flake.contains("scripts/release-local-check.sh"));
        assert!(flake.contains("scripts/local-release-deploy.sh"));
        assert!(flake.contains("local-release-deploy <version> --publish <version>"));
        assert!(flake.contains("project publisher hook"));
        assert!(flake.contains("Homebrew-capable hooks must build Darwin tarballs"));
        assert!(flake.contains("gate tap pushes on HOMEBREW_TAP_TOKEN"));
        assert!(flake.contains("local-check-release must remain non-publishing"));
        assert!(!flake.contains("nix run '.#rs-harbor' -- brew bump"));
        assert!(!flake.contains("debootstrap"));
        assert!(!flake.contains("copr-cli build"));
        assert!(!flake.contains("choco push"));
        assert!(!flake.contains("cargo publish -p"));
    }

    /// Bidirectional rs-harbor ↔ simit contract test.
    ///
    /// This test verifies that the cross-compilation flake template calls
    /// rs-harbor's `mkDevShells` with the parameters rs-harbor now expects
    /// (packages list, extraShellHook, checks).  The rs-harbor side of the
    /// contract is enforced by rs-harbor's own `checks.nix`:
    ///
    ///   - `mkDevShells-accepts-simit-parameters` — mkDevShells accepts
    ///     the full package set simit passes
    ///   - `mkDevShells-audit-tools-in-path` — cargo-audit, cargo-deny
    ///     resolve and land on PATH
    ///
    /// If this test fails, simit's generated cross-template expects an API
    /// shape that rs-harbor's mkDevShells no longer supports.
    #[test]
    fn cross_template_with_all_targets_pins_the_shared_contract() {
        let flake = cross_template(
            &FlakeTargetArg::all(),
            AuditTools {
                audit: true,
                deny: true,
                pyo3: false,
            },
        );

        // rs-harbor input and follows wiring.
        assert!(flake.contains(&format!(
            "rs-harbor.url = \"git+https://github.com/caniko/harbor-rs.git?ref=trunk&rev={RS_HARBOR_REV}\";"
        )));
        assert!(flake.contains("nixpkgs.follows = \"rs-harbor/nixpkgs\";"));
        assert!(flake.contains("rust-overlay.follows = \"rs-harbor/rust-overlay\";"));
        assert!(flake.contains("crane.follows = \"rs-harbor/crane\";"));
        assert!(flake.contains("flake-utils.url = \"github:numtide/flake-utils\";"));

        // Toolchain + cross + mkCrossPackages call with the exact argument set.
        assert!(flake.contains("toolchain = rs-harbor.lib.mkToolchain {inherit pkgs;};"));
        assert!(flake.contains("cross = rs-harbor.lib.mkCross {inherit pkgs system;};"));
        assert!(flake.contains("rs-harbor.lib.mkCrossPackages {"));
        assert!(flake.contains("inherit pkgs cross pname commonArgs;"));
        assert!(flake.contains("inherit (toolchain) craneLib buildCache;"));

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
        assert!(flake.contains("file"));
        assert!(flake.contains("gnutar"));
        assert!(flake.contains("gzip"));
        assert!(flake.contains("zip"));
        assert!(flake.contains("apps.local-check-fast"));
        assert!(flake.contains("apps.local-check-release"));
        assert!(flake.contains("apps.local-release-deploy"));
        assert!(flake.contains("meta.description = \"Run fast local validation checks\";"));
        assert!(flake.contains(
            "meta.description = \"Run local release parity checks without publishing\";"
        ));
        assert!(
            flake.contains("meta.description = \"Run the guarded local release deployment hook\";")
        );
        assert!(flake.contains("no external publish was attempted"));
        assert!(flake.contains("release/artifacts.json"));
        assert!(flake.contains("generated_by: \"simit local-check-release\""));
        assert!(flake.contains("scripts/release-local-check.sh"));
        assert!(flake.contains("scripts/local-release-deploy.sh"));
        assert!(flake.contains("local-release-deploy <version> --publish <version>"));
        assert!(flake.contains("project publisher hook"));
        assert!(flake.contains("Homebrew-capable hooks must build Darwin tarballs"));
        assert!(flake.contains("gate tap pushes on HOMEBREW_TAP_TOKEN"));
        assert!(flake.contains("local-check-release must remain non-publishing"));
        assert!(!flake.contains("nix run '.#rs-harbor' -- brew bump"));
        assert!(!flake.contains("debootstrap"));
        assert!(!flake.contains("copr-cli build"));
        assert!(!flake.contains("choco push"));
        assert!(!flake.contains("cargo publish -p"));
        assert!(flake.contains(
            "treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix { rustfmtPackage = fmtToolchain.rustToolchain; });"
        ));
        assert!(flake.contains(
            "fmtToolchain = rs-harbor.lib.mkToolchain {inherit pkgs; toolchainProfile = \"nightly\";};"
        ));

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
                pyo3: false,
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
                pyo3: false,
            },
        );
        assert!(flake.contains("targets = [\"aarch64-linux\" \"windows\"];"));
        // No native target -> default is the first canonical non-native attr.
        assert!(flake.contains("default = crossPackages.\"${pname}-aarch64-linux\";"));
    }

    #[test]
    fn generic_treefmt_wires_javascript_and_tex() {
        let languages = Languages {
            nix: true,
            javascript: true,
            tex: true,
            ..Languages::default()
        };
        let content = treefmt_nix(&languages, "2024");
        assert!(content.contains("programs.alejandra.enable = true;"));
        assert!(content.contains("\"*.ts\""));
        assert!(content.contains("\"*.json\""));
        assert!(content.contains("latexindent"));
        assert!(has_required_treefmt(&content, &languages, "2024"));
    }

    #[test]
    fn semantic_pre_commit_check_respects_selected_components() {
        let languages = Languages {
            nix: true,
            uv_python: true,
            ..Languages::default()
        };
        let components = [FlakeComponent::Treefmt, FlakeComponent::NixFlakeCheck];
        let content = pre_commit_nix(&languages, None, AuditTools::default(), &components);

        assert!(has_required_pre_commit_with_components(
            &content,
            &languages,
            None,
            AuditTools::default(),
            &components,
        ));
        assert!(!content.contains("uv-ruff-format"));
        assert!(!content.contains("uv-mypy"));

        let missing_treefmt = content.replace("  treefmt = {", "  removed-treefmt = {");
        assert!(!has_required_pre_commit_with_components(
            &missing_treefmt,
            &languages,
            None,
            AuditTools::default(),
            &components,
        ));
    }
}

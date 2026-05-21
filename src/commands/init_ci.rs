use anyhow::{Result, bail};

use crate::cargo;
use crate::cli::{HomebrewOverridesArgs, InitCiCommand, Platform, Runtime, RuntimeChoice};
use crate::config::{ProjectConfig, ResolvedHomebrew};
use crate::project;
use crate::render::ci::{self, CiOptions, HomebrewOptions, HomebrewPlatformSet};

pub fn run(command: InitCiCommand) -> Result<()> {
    validate_runner(command.runner.as_deref())?;

    let metadata = cargo::metadata_for_current_dir()?;
    let package = cargo::select_packages(&metadata, &[], false)?
        .into_iter()
        .next()
        .expect("single package selected");
    let workspace_root = metadata.workspace_root.as_std_path();
    if command.with_homebrew && command.platform != Platform::Forgejo {
        bail!("Homebrew tap publish is forgejo-only for now");
    }
    let runtime = resolve_runtime(command.runtime, workspace_root)?;
    if command.with_homebrew && runtime != Runtime::Nix {
        bail!("Homebrew tap publish requires --runtime nix");
    }
    let self_check = metadata
        .packages
        .iter()
        .any(|package| package.name == "simit");
    let with_artifacts = command.with_artifacts || command.with_homebrew;
    if command.with_homebrew && !command.with_artifacts {
        eprintln!("--with-homebrew implies --with-artifacts; enabling it.");
    }
    let homebrew = if command.with_homebrew {
        let cfg = ProjectConfig::load(workspace_root)?;
        Some(homebrew_options(&cfg, &command.homebrew, &package)?)
    } else {
        None
    };
    let options = CiOptions {
        with_nextest: command.with_nextest,
        with_msrv: command.with_msrv,
        with_audit: command.with_audit,
        with_deny: command.with_deny,
        with_docs: command.with_docs,
        with_artifacts,
        homebrew,
    };
    let files = ci::files(
        command.platform,
        runtime,
        &package,
        self_check,
        command.runner.as_deref(),
        options,
    )?;

    if command.check {
        project::check_generated_files(
            workspace_root,
            &files,
            &format!(
                "CI workflows are not up to date; run `simit init-ci --platform {}`",
                command.platform.as_str()
            ),
            command.diff,
        )
    } else {
        project::write_generated_files(workspace_root, &files)
    }
}

fn homebrew_options(
    cfg: &ProjectConfig,
    args: &HomebrewOverridesArgs,
    package: &cargo::Package,
) -> Result<HomebrewOptions> {
    let resolved = cfg.resolve_homebrew(args.as_overrides(), package)?;
    let tap_url = normalize_tap_url(&resolved.tap_url)?;
    validate_download_repo(&resolved.download_repo)?;
    let platforms = homebrew_platforms(&resolved);
    Ok(HomebrewOptions {
        name: resolved.name,
        binaries: resolved.binaries,
        tap_url,
        description: resolved.description,
        homepage: resolved.homepage,
        license: resolved.license,
        archive_pattern: resolved.archive_pattern,
        download_repo: resolved.download_repo,
        platforms,
    })
}

fn normalize_tap_url(value: &str) -> Result<String> {
    if value.is_empty() {
        bail!("--homebrew-tap cannot be empty");
    }
    let with_scheme = if value.starts_with("https://") || value.starts_with("http://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    let (_, path) = with_scheme
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("--homebrew-tap must include a repository path"))?;
    if path.trim_matches('/').is_empty() {
        bail!("--homebrew-tap must include a repository path");
    }
    if with_scheme.ends_with(".git") {
        Ok(with_scheme)
    } else {
        Ok(format!("{with_scheme}.git"))
    }
}

fn validate_download_repo(value: &str) -> Result<()> {
    if value.split('/').count() != 2 || value.split('/').any(str::is_empty) {
        bail!("--homebrew-download-repo must be OWNER/REPO");
    }
    Ok(())
}

fn homebrew_platforms(resolved: &ResolvedHomebrew) -> HomebrewPlatformSet {
    HomebrewPlatformSet {
        darwin_arm: resolved.platforms.darwin_arm,
        darwin_intel: resolved.platforms.darwin_intel,
        linux_arm: resolved.platforms.linux_arm,
        linux_intel: resolved.platforms.linux_intel,
    }
}

pub(crate) fn resolve_runtime(
    choice: RuntimeChoice,
    workspace_root: &std::path::Path,
) -> Result<Runtime> {
    match choice {
        RuntimeChoice::Auto | RuntimeChoice::Cargo => Ok(Runtime::Cargo),
        RuntimeChoice::Nix => {
            if !workspace_root.join("flake.nix").exists() {
                bail!("--runtime nix requires flake.nix at the workspace root");
            }
            Ok(Runtime::Nix)
        }
    }
}

pub(crate) fn validate_runner(value: Option<&str>) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_empty() {
        bail!("--runner cannot be empty");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("--runner may only contain ASCII letters, digits, '.', '_', and '-'");
    }
    Ok(())
}

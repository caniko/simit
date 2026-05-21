use anyhow::{Result, bail};

use crate::cargo;
use crate::cli::{InitCiCommand, Platform, Runtime, RuntimeChoice};
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
        Some(homebrew_options(&command, &package)?)
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

fn homebrew_options(command: &InitCiCommand, package: &cargo::Package) -> Result<HomebrewOptions> {
    let tap_url = normalize_tap_url(
        command
            .homebrew_tap
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("--with-homebrew requires --homebrew-tap"))?,
    )?;
    let description = required_homebrew_value(
        "--homebrew-description",
        command.homebrew_description.as_deref(),
    )?;
    if description.chars().count() > 80 {
        bail!("--homebrew-description must be 80 characters or fewer");
    }
    let homepage =
        required_homebrew_value("--homebrew-homepage", command.homebrew_homepage.as_deref())?;
    let download_repo = required_homebrew_value(
        "--homebrew-download-repo",
        command.homebrew_download_repo.as_deref(),
    )?;
    validate_download_repo(&download_repo)?;
    let license = match command.homebrew_license.as_deref() {
        Some(value) if !value.is_empty() => value.to_owned(),
        Some(_) => bail!("--homebrew-license cannot be empty"),
        None => package.license.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "--with-homebrew requires --homebrew-license when package.license is missing"
            )
        })?,
    };
    let platforms = homebrew_platforms(&command.homebrew_no_platform)?;
    if !platforms.darwin_arm
        && !platforms.darwin_intel
        && !platforms.linux_arm
        && !platforms.linux_intel
    {
        bail!("at least one Homebrew platform must be enabled");
    }

    Ok(HomebrewOptions {
        name: package.name.clone(),
        binaries: command.homebrew_binary.clone(),
        tap_url,
        description,
        homepage,
        license,
        archive_pattern: command.homebrew_archive_pattern.clone(),
        download_repo,
        platforms,
    })
}

fn required_homebrew_value(flag: &str, value: Option<&str>) -> Result<String> {
    match value {
        Some(value) if !value.is_empty() => Ok(value.to_owned()),
        _ => bail!("--with-homebrew requires {flag}"),
    }
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

fn homebrew_platforms(disabled: &[String]) -> Result<HomebrewPlatformSet> {
    let mut platforms = HomebrewPlatformSet::default();
    for key in disabled {
        match key.as_str() {
            "darwin_arm" => platforms.darwin_arm = false,
            "darwin_intel" => platforms.darwin_intel = false,
            "linux_arm" => platforms.linux_arm = false,
            "linux_intel" => platforms.linux_intel = false,
            _ => bail!(
                "--homebrew-no-platform must be one of darwin_arm, darwin_intel, linux_arm, linux_intel"
            ),
        }
    }
    Ok(platforms)
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

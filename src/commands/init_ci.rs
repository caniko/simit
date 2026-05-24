use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::{
    ChocolateyOverridesArgs, HomebrewOverridesArgs, InitCiCommand, Platform, Runtime,
    RuntimeChoice, ScoopOverridesArgs,
};
use crate::config::{ProjectConfig, ResolvedChocolatey, ResolvedHomebrew, ResolvedScoop};
use crate::project;
use crate::registry::{self, FeatureStatus};
use crate::release_trust::{self, TrustOverrides};
use crate::render::ci::{
    self, ChocolateyOptions, CiOptions, HomebrewOptions, HomebrewPlatformSet, OMNIX_REF_DEFAULT,
    OmCiMode, ScoopOptions, SelfCheckOptions,
};
use crate::user_config::{UserConfig, validate_runner_label};

pub fn run(command: InitCiCommand) -> Result<()> {
    validate_runner(command.runner.as_deref())?;
    validate_runner(command.windows_runner.as_deref())?;

    let metadata = cargo::metadata_for_current_dir()?;
    let packages = cargo::select_packages(&metadata, &command.packages, command.workspace)?;
    let workspace_root = metadata.workspace_root.as_std_path();
    if command.with_homebrew && command.platform != Platform::Forgejo {
        bail!("Homebrew tap publish is forgejo-only for now");
    }
    let runtime = resolve_runtime(command.runtime, workspace_root)?;
    if command.with_homebrew && runtime != Runtime::Nix {
        bail!("Homebrew tap publish requires --runtime nix");
    }
    // Chocolatey and Scoop are allowed with both runtimes: phase 3 will build
    // Windows artifacts on Windows runners, independent of the Linux CI runtime.
    let self_check = metadata
        .packages
        .iter()
        .any(|package| package.name == "simit");
    let windows_packagers = command.with_chocolatey || command.with_scoop;
    let with_artifacts = command.with_artifacts || command.with_homebrew || windows_packagers;
    if command.with_homebrew && !command.with_artifacts {
        eprintln!("--with-homebrew implies --with-artifacts; enabling it.");
    }
    if command.with_chocolatey && !command.with_artifacts {
        eprintln!("--with-chocolatey implies --with-artifacts; enabling it.");
    }
    if command.with_scoop && !command.with_artifacts {
        eprintln!("--with-scoop implies --with-artifacts; enabling it.");
    }
    let explicit_runners_cover_required =
        runner_overrides_cover_required_runners(&command, windows_packagers);
    let cfg = ProjectConfig::load(workspace_root)?;
    let om_ci_requested =
        command.with_om_ci || command.om_ci_augment || cfg.ci.om_ci || cfg.ci.om_ci_augment;
    let augment = command.om_ci_augment || cfg.ci.om_ci_augment;
    let om_ci = match (om_ci_requested, augment) {
        (false, _) => OmCiMode::Off,
        (true, true) => OmCiMode::Augment,
        (true, false) => OmCiMode::Replace,
    };
    if om_ci != OmCiMode::Off && runtime != Runtime::Nix {
        bail!("--with-om-ci requires --runtime nix");
    }
    if command.omnix_ref.is_some() && om_ci == OmCiMode::Off {
        eprintln!("--omnix-ref is ignored unless --with-om-ci or --om-ci-augment is enabled.");
    }
    let user_config = UserConfig::load().or_else(|err| {
        if command.platform == Platform::Github || explicit_runners_cover_required {
            Ok(UserConfig::default())
        } else {
            Err(err)
        }
    })?;
    let omnix_ref = command
        .omnix_ref
        .clone()
        .or_else(|| cfg.ci.omnix_ref.clone())
        .or_else(|| user_config.ci.tools.omnix.r#ref.clone())
        .unwrap_or_else(|| OMNIX_REF_DEFAULT.to_owned());
    let options = CiOptions {
        with_nextest: command.with_nextest,
        with_msrv: command.with_msrv,
        with_audit: command.with_audit,
        with_deny: command.with_deny,
        with_docs: command.with_docs,
        with_artifacts,
        om_ci,
        omnix_ref,
        release_smoke_command: command
            .release_smoke_command
            .clone()
            .or_else(|| cfg.release.smoke.command.clone()),
        extra_setup: cfg.ci.extra_setup.clone(),
        extra_env: cfg
            .ci
            .extra_env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        required_secrets: cfg.ci.required_secrets.clone(),
        package_scoped: false,
        homebrew: None,
        chocolatey: None,
        scoop: None,
    };
    let runners = user_config.resolve_ci_runners(
        command.platform,
        runtime,
        command.runner.as_deref(),
        command.windows_runner.as_deref(),
        windows_packagers,
    )?;
    let multi_package_workspace = metadata.workspace_members.len() > 1;
    let mut files = Vec::new();
    for package in &packages {
        let homebrew = if command.with_homebrew {
            Some(homebrew_options(&cfg, &command.homebrew, package)?)
        } else {
            None
        };
        let chocolatey = if command.with_chocolatey {
            Some(chocolatey_options(&cfg, &command.chocolatey, package)?)
        } else {
            None
        };
        let scoop = if command.with_scoop {
            Some(scoop_options(&cfg, &command.scoop, package)?)
        } else {
            None
        };
        let package_options = CiOptions {
            homebrew,
            chocolatey,
            scoop,
            package_scoped: multi_package_workspace,
            ..options.clone()
        };
        files.extend(ci::files(
            command.platform,
            runtime,
            package,
            multi_package_workspace.then_some(package.name.as_str()),
            SelfCheckOptions {
                enabled: self_check,
                runner_override: command.runner.as_deref(),
                windows_runner_override: command.windows_runner.as_deref(),
                packages: &command.packages,
                workspace: command.workspace,
            },
            &runners,
            package_options,
        )?);
    }
    let trust_overrides = TrustOverrides {
        key: command.maintainer_key,
        trust_root: command.maintainers_gpg,
    };
    files.push(release_trust::generated_file(
        workspace_root,
        &cfg,
        &trust_overrides,
        command.check,
    )?);

    if command.check {
        check_generated_ci_files(
            workspace_root,
            &files,
            command.platform,
            &format!(
                "CI workflows are not up to date; run `simit init ci --platform {}`",
                command.platform.as_str()
            ),
            command.diff,
        )
    } else {
        project::write_generated_files(workspace_root, &files)?;
        registry::touch_current_project_or_warn([("ci", FeatureStatus::Managed)]);
        Ok(())
    }
}

fn check_generated_ci_files(
    workspace_root: &Path,
    files: &[project::GeneratedFile],
    platform: Platform,
    message: &str,
    show_diff: bool,
) -> Result<()> {
    project::check_generated_files(workspace_root, files, message, show_diff)?;

    let expected = files
        .iter()
        .map(|file| file.relative_path.clone())
        .collect::<BTreeSet<_>>();
    let extras = extra_generated_workflows(workspace_root, platform, &expected)?;
    if extras.is_empty() {
        Ok(())
    } else {
        let details = extras
            .into_iter()
            .map(|path| format!("{} is extra", path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        bail!("{message}:\n{details}");
    }
}

fn extra_generated_workflows(
    workspace_root: &Path,
    platform: Platform,
    expected: &BTreeSet<PathBuf>,
) -> Result<Vec<PathBuf>> {
    let workflow_dir = PathBuf::from(platform.workflow_dir());
    let absolute_dir = workspace_root.join(&workflow_dir);
    let entries = match fs::read_dir(&absolute_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", absolute_dir.display())),
    };

    let mut extras = Vec::new();
    for entry in entries {
        let entry =
            entry.with_context(|| format!("reading entry in {}", absolute_dir.display()))?;
        if !entry
            .file_type()
            .with_context(|| format!("reading file type for {}", entry.path().display()))?
            .is_file()
        {
            continue;
        }
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if extension != "yaml" && extension != "yml" {
            continue;
        }
        let relative_path = workflow_dir.join(entry.file_name());
        if expected.contains(&relative_path) {
            continue;
        }
        let content =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        if content.contains(ci::GENERATED_WORKFLOW_MARKER) {
            extras.push(relative_path);
        }
    }
    extras.sort();
    Ok(extras)
}

fn runner_overrides_cover_required_runners(
    command: &InitCiCommand,
    windows_packagers: bool,
) -> bool {
    command.runner.is_some() && (!windows_packagers || command.windows_runner.is_some())
}

fn chocolatey_options(
    cfg: &ProjectConfig,
    args: &ChocolateyOverridesArgs,
    package: &cargo::Package,
) -> Result<ChocolateyOptions> {
    let resolved = cfg.resolve_chocolatey(args.as_overrides(), package)?;
    validate_download_repo_for("--choco-download-repo", &resolved.download_repo)?;
    Ok(chocolatey_options_from_resolved(resolved))
}

fn chocolatey_options_from_resolved(resolved: ResolvedChocolatey) -> ChocolateyOptions {
    ChocolateyOptions {
        name: resolved.name,
        id: resolved.id,
        title: resolved.title,
        authors: resolved.authors,
        description: resolved.description,
        project_url: resolved.project_url,
        license_url: resolved.license_url,
        tags: resolved.tags,
        release_notes_url: resolved.release_notes_url,
        download_repo: resolved.download_repo,
        archive_pattern: resolved.archive_pattern,
        push_source: resolved.push.source,
    }
}

fn scoop_options(
    cfg: &ProjectConfig,
    args: &ScoopOverridesArgs,
    package: &cargo::Package,
) -> Result<ScoopOptions> {
    let resolved = cfg.resolve_scoop(args.as_overrides(), package)?;
    validate_download_repo_for("--scoop-download-repo", &resolved.download_repo)?;
    Ok(scoop_options_from_resolved(resolved))
}

fn scoop_options_from_resolved(resolved: ResolvedScoop) -> ScoopOptions {
    ScoopOptions {
        name: resolved.name,
        bucket_url: resolved.bucket_url,
        description: resolved.description,
        homepage: resolved.homepage,
        license: resolved.license,
        download_repo: resolved.download_repo,
        archive_pattern: resolved.archive_pattern,
        binaries: resolved.binaries,
        x64: resolved.architectures.x64,
        arm64: resolved.architectures.arm64,
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
    validate_download_repo_for("--homebrew-download-repo", value)
}

fn validate_download_repo_for(flag: &str, value: &str) -> Result<()> {
    if value.split('/').count() != 2 || value.split('/').any(str::is_empty) {
        bail!("{flag} must be OWNER/REPO");
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
    value.map(validate_runner_label).transpose()?;
    Ok(())
}

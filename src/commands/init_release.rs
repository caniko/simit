use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::cargo;
use crate::cli::InitReleaseCommand;
use crate::cli::{CiProvider, Platform, Runtime};
use crate::commands::scaffold::{CheckPrintMode, print_next_steps};
use crate::commands::upgrade;
use crate::config::{
    AptOverrides, AurOverrides, ChocolateyOverrides, CoprOverrides, HomebrewOverrides,
    ProjectConfig, ScoopOverrides,
};
use crate::project;
use crate::registry::{self, FeatureStatus};
use crate::render::release_workflow::{self, ReleaseWorkflowInputs};
use crate::user_config::{ResolvedRunner, UserConfig};

pub fn run(command: InitReleaseCommand) -> Result<()> {
    let mode = CheckPrintMode::parse("init release", command.check, command.print, command.diff)?;

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let package = cargo::representative_package(&metadata, command.package.as_deref())?;
    let cfg = ProjectConfig::load(workspace_root)?;

    let platform = command
        .platform
        .or(match (&cfg.release.github, &cfg.release.codeberg) {
            (Some(_), None) => Some(Platform::Github),
            (None, Some(_)) => Some(Platform::Forgejo),
            _ => cfg.ci.platform,
        })
        .unwrap_or(Platform::Forgejo);
    let provider = command.ci_provider.unwrap_or_else(|| {
        if platform == Platform::Github {
            CiProvider::Actions
        } else {
            cfg.ci.provider.unwrap_or(CiProvider::Actions)
        }
    });
    crate::ci_resolution::validate_ci_capability(provider, platform)?;
    let release = cfg.resolve_release_target(platform)?;
    let aur = cfg
        .aur
        .as_ref()
        .map(|_| cfg.resolve_aur_for_platform(AurOverrides::default(), &package, platform))
        .transpose()?;
    let copr = cfg
        .copr
        .as_ref()
        .map(|_| cfg.resolve_copr_for_platform(CoprOverrides::default(), &package, platform))
        .transpose()?;
    let apt = cfg
        .apt
        .as_ref()
        .map(|_| cfg.resolve_apt(AptOverrides::default(), &package))
        .transpose()?;
    let homebrew = cfg
        .homebrew
        .as_ref()
        .map(|_| cfg.resolve_homebrew(HomebrewOverrides::default(), &package))
        .transpose()?;
    let scoop = cfg
        .scoop
        .as_ref()
        .map(|_| cfg.resolve_scoop(ScoopOverrides::default(), &package))
        .transpose()?;
    let chocolatey = cfg
        .chocolatey
        .as_ref()
        .map(|_| cfg.resolve_chocolatey(ChocolateyOverrides::default(), &package))
        .transpose()?;

    let (runner, preinstalled_nix) = if provider == CiProvider::Crow {
        (
            cfg.release
                .artifacts
                .runner
                .clone()
                .or_else(|| cfg.ci.runner.clone())
                .unwrap_or_else(|| "crow-default".to_owned()),
            false,
        )
    } else {
        resolve_release_runner(&cfg, platform)?
    };

    let inputs = ReleaseWorkflowInputs {
        platform,
        runner: &runner,
        preinstalled_nix,
        publish_enforcement: cfg.release.publish.enforcement,
        publish: &cfg.release.publish,
        artifacts: &cfg.release.artifacts,
        prebuild: cfg.prebuild.as_ref(),
        smoke_command: cfg.release.smoke.command.as_deref(),
        release: release.as_ref(),
        attic: cfg.release.attic.as_ref(),
        aur: aur.as_ref(),
        copr: copr.as_ref(),
        apt: apt.as_ref(),
        homebrew: homebrew.as_ref(),
        scoop: scoop.as_ref(),
        chocolatey: chocolatey.as_ref(),
        windows_signing: cfg.release.windows_signing.as_ref(),
        flatpak: cfg.flatpak.as_ref(),
        winget: cfg.winget.as_ref(),
        announce: cfg.release.announce.as_ref(),
    };
    let workflow = if provider == CiProvider::Crow {
        crate::render::crow::release_file(cfg.ci.crow.format, &cfg.ci.crow, &inputs)?
    } else {
        release_workflow::file(&inputs)
    };
    let workflow_path = workflow.relative_path.to_string_lossy().into_owned();
    let obsolete = obsolete_release_files(workspace_root, &workflow.relative_path)?;
    let files = [workflow];

    match mode {
        CheckPrintMode::Print => {
            print!("{}", files[0].content);
            Ok(())
        }
        CheckPrintMode::Check { diff } => project::reconcile_generated_files(
            workspace_root,
            &files,
            &obsolete,
            "release workflow is not up to date; run `simit init release`",
            true,
            diff,
        )
        .and_then(|_| upgrade::update_readme_badges_if_present(workspace_root, true, diff)),
        CheckPrintMode::Write => {
            project::reconcile_generated_files(
                workspace_root,
                &files,
                &obsolete,
                "release workflow",
                false,
                false,
            )?;
            upgrade::update_readme_badges_if_present(workspace_root, false, false)?;
            print_next_steps(
                workspace_root,
                "Generated release workflow",
                &[
                    format!("git add {workflow_path}"),
                    "configure the secrets listed at the top of the workflow".to_owned(),
                    "push a signed tag like 0.1.0 to trigger it".to_owned(),
                ],
            );
            registry::touch_current_project_or_warn([("ci", FeatureStatus::Managed)]);
            Ok(())
        }
    }
}

fn obsolete_release_files(
    workspace_root: &std::path::Path,
    expected: &std::path::Path,
) -> Result<Vec<PathBuf>> {
    let mut obsolete = Vec::new();
    for directory in [".github/workflows", ".forgejo/workflows", ".crow"] {
        let root = workspace_root.join(directory);
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("reading {}", root.display())),
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("reading entry in {}", root.display()))?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            if !matches!(
                name.to_str(),
                Some("release.yml" | "release.yaml" | "release.jsonnet")
            ) {
                continue;
            }
            let relative = PathBuf::from(directory).join(name);
            if relative == expected {
                continue;
            }
            let content = fs::read_to_string(entry.path())
                .with_context(|| format!("reading {}", entry.path().display()))?;
            if content.contains("Generated by simit") {
                obsolete.push(relative);
            }
        }
    }
    obsolete.sort();
    Ok(obsolete)
}

fn resolve_release_runner(cfg: &ProjectConfig, platform: Platform) -> Result<(String, bool)> {
    if let Some(runner) = cfg
        .release
        .artifacts
        .runner
        .clone()
        .or_else(|| cfg.ci.runner.clone())
    {
        let preinstalled_nix = match UserConfig::load() {
            Ok(user_config) if platform == Platform::Forgejo => {
                user_config.explicit_label_is_trusted_forgejo_nix_runner(&runner)?
            }
            Err(_) => false,
            Ok(_) => false,
        };
        return Ok((runner, preinstalled_nix));
    }

    let user_config = UserConfig::load()?;
    let runners = user_config.resolve_ci_runners(platform, Runtime::Nix, None, None, false)?;
    Ok((runs_on(&runners.release), platform == Platform::Forgejo))
}

fn runs_on(runner: &ResolvedRunner) -> String {
    if runner.labels.len() == 1 {
        return runner.labels[0].clone();
    }

    let labels = runner
        .labels
        .iter()
        .map(|label| format!("\"{}\"", label.replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{labels}]")
}

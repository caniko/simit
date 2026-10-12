use anyhow::{Result, bail};

use crate::cargo;
use crate::ci_resolution::CiBackend;
use crate::cli::InitReleaseCommand;
use crate::cli::{CiProvider, Platform, Runtime};
use crate::commands::scaffold::{CheckPrintMode, print_next_steps};
use crate::commands::upgrade;
use crate::config::ProjectConfig;
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
    // Reject Crow + non-Forgejo pairs; Crow release workflows only render for
    // Forgejo. Actions works on every platform.
    CiBackend::from_parts(provider, platform)?;
    if platform == Platform::Gitlab {
        bail!(
            "release workflows are not supported for GitLab; GitLab CI is nix-only and has no Actions-style release workflow"
        );
    }
    let release = cfg.resolve_release_target(platform)?;
    let targets =
        crate::packaging_common::resolve_release_platform_targets(&cfg, &package, platform)?;

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
        tag_prefix: cfg.release.tag_prefix,
        notes_source: cfg.release.notes.source,
        required_gates: &cfg.ci.required_gates,
        runner: &runner,
        preinstalled_nix,
        publish_enforcement: cfg.release.publish.enforcement,
        publish: &cfg.release.publish,
        artifacts: &cfg.release.artifacts,
        prebuild: cfg.prebuild.as_ref(),
        smoke_command: cfg.release.smoke.command.as_deref(),
        release: release.as_ref(),
        attic: cfg.release.attic.as_ref(),
        aur: targets.aur.as_ref(),
        copr: targets.copr.as_ref(),
        apt: targets.apt.as_ref(),
        homebrew: targets.homebrew.as_ref(),
        scoop: targets.scoop.as_ref(),
        chocolatey: targets.chocolatey.as_ref(),
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
    let plan = project::GeneratedPlan {
        files: vec![workflow],
        message: "release workflow is not up to date; run `simit init release`",
        owns_name: is_release_workflow_name,
    };

    match mode {
        CheckPrintMode::Print => {
            print!("{}", plan.files[0].content);
            Ok(())
        }
        CheckPrintMode::Check { diff } => plan
            .check(workspace_root, diff)
            .and_then(|_| upgrade::update_readme_badges_if_present(workspace_root, true, diff)),
        CheckPrintMode::Write => {
            plan.write(workspace_root)?;
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

fn is_release_workflow_name(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some("release.yml" | "release.yaml" | "release.jsonnet")
    )
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

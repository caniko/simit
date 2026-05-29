use anyhow::Result;

use crate::cargo;
use crate::cli::InitReleaseCommand;
use crate::commands::scaffold::{ArtifactCheck, CheckPrintMode, print_next_steps};
use crate::config::{
    AptOverrides, AurOverrides, ChocolateyOverrides, CoprOverrides, HomebrewOverrides,
    ProjectConfig, ScoopOverrides,
};
use crate::registry::{self, FeatureStatus};
use crate::render::release_workflow::{self, ReleaseWorkflowInputs};

const WORKFLOW_PATH: &str = ".forgejo/workflows/release.yml";

pub fn run(command: InitReleaseCommand) -> Result<()> {
    let mode = CheckPrintMode::parse("init release", command.check, command.print, command.diff)?;

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let package = cargo::representative_package(&metadata, command.package.as_deref())?;
    let cfg = ProjectConfig::load(workspace_root)?;

    let codeberg = cfg.resolve_codeberg_release()?;
    let aur = cfg
        .aur
        .as_ref()
        .map(|_| cfg.resolve_aur(AurOverrides::default(), &package))
        .transpose()?;
    let copr = cfg
        .copr
        .as_ref()
        .map(|_| cfg.resolve_copr(CoprOverrides::default(), &package))
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

    let runner = cfg
        .release
        .artifacts
        .runner
        .clone()
        .or_else(|| cfg.ci.runner.clone())
        .unwrap_or_else(|| "atlas".to_owned());

    let inputs = ReleaseWorkflowInputs {
        runner: &runner,
        artifacts: &cfg.release.artifacts,
        smoke_command: cfg.release.smoke.command.as_deref(),
        codeberg: codeberg.as_ref(),
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
    let content = release_workflow::render(&inputs);
    let path = workspace_root.join(WORKFLOW_PATH);

    match mode {
        CheckPrintMode::Print => {
            print!("{content}");
            Ok(())
        }
        CheckPrintMode::Check { diff } => ArtifactCheck {
            label: "release workflow",
            path: &path,
            expected: &content,
            remediation: "run `simit init release`",
        }
        .verify(diff),
        CheckPrintMode::Write => {
            let parent = path.parent().expect("workflow path has a parent");
            std::fs::create_dir_all(parent)?;
            std::fs::write(&path, &content)?;
            print_next_steps(
                workspace_root,
                "Generated release workflow",
                &[
                    format!("git add {WORKFLOW_PATH}"),
                    "configure the secrets listed at the top of the workflow".to_owned(),
                    "push a signed tag like 0.1.0 to trigger it".to_owned(),
                ],
            );
            registry::touch_current_project_or_warn([("ci", FeatureStatus::Managed)]);
            Ok(())
        }
    }
}

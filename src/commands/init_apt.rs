use anyhow::Result;

use crate::cargo;
use crate::cli::InitAptCommand;
use crate::commands::apt::{self, DISTRIBUTIONS_PATH};
use crate::commands::scaffold::{ArtifactCheck, CheckPrintMode, print_next_steps};
use crate::config::ProjectConfig;
use crate::registry::{self, FeatureStatus};
use crate::render::apt_conf;

pub fn run(command: InitAptCommand) -> Result<()> {
    let mode = CheckPrintMode::parse("init apt", command.check, command.print, command.diff)?;

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let package = cargo::representative_package(&metadata, command.package.as_deref())?;
    let cfg = ProjectConfig::load(workspace_root)?;
    let resolved = cfg.resolve_apt(command.apt.as_overrides(), &package)?;
    let distributions = apt_conf::render_distributions(&resolved);

    match mode {
        CheckPrintMode::Print => {
            print!("{distributions}");
            return Ok(());
        }
        CheckPrintMode::Check { diff } => {
            ArtifactCheck {
                label: "apt distributions config",
                path: &workspace_root.join(DISTRIBUTIONS_PATH),
                expected: &distributions,
                remediation: "run `simit init apt`",
            }
            .verify(diff)?;
            return Ok(());
        }
        CheckPrintMode::Write => {}
    }

    apt::write_distributions(workspace_root, &resolved)?;
    print_next_steps(
        workspace_root,
        "Initialised apt repository config",
        &[
            "commit dist/apt/conf/distributions and dist/apt/key.gpg.asc (public key)".to_owned(),
            "publish on release via the generated workflow (needs apt repo gpg + ssh secrets)"
                .to_owned(),
        ],
    );
    registry::touch_current_project_or_warn([("apt", FeatureStatus::Managed)]);
    Ok(())
}

use anyhow::Result;

use crate::cargo;
use crate::cli::InitCoprCommand;
use crate::commands::copr;
use crate::commands::scaffold::{ArtifactCheck, CheckPrintMode, print_next_steps, shell_word};
use crate::commands::upgrade;
use crate::config::ProjectConfig;
use crate::registry::{self, FeatureStatus};

pub fn run(command: InitCoprCommand) -> Result<()> {
    let mode = CheckPrintMode::parse("init copr", command.check, command.print, command.diff)?;

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let cfg = ProjectConfig::load(workspace_root)?;
    let package = cargo::representative_package(
        &metadata,
        command
            .package
            .as_deref()
            .or(cfg.primary_package.as_deref()),
    )?;
    let resolved = cfg.resolve_copr(command.copr.as_overrides(), &package)?;
    let rendered = copr::render_files(&resolved, &package.version);

    match mode {
        CheckPrintMode::Print => {
            println!("==> {}", resolved.spec_path);
            print!("{}", rendered.spec);
            println!("==> {}", resolved.makefile_path);
            print!("{}", rendered.makefile);
            return Ok(());
        }
        CheckPrintMode::Check { diff } => {
            let remediation = "run `simit init copr`";
            ArtifactCheck {
                label: "COPR spec",
                path: &workspace_root.join(&resolved.spec_path),
                expected: &rendered.spec,
                remediation,
            }
            .verify(diff)?;
            ArtifactCheck {
                label: "COPR Makefile",
                path: &workspace_root.join(&resolved.makefile_path),
                expected: &rendered.makefile,
                remediation,
            }
            .verify(diff)?;
            upgrade::update_readme_badges_if_present(workspace_root, true, diff)?;
            return Ok(());
        }
        CheckPrintMode::Write => {}
    }

    copr::write_files(workspace_root, &resolved, &rendered)?;
    upgrade::update_readme_badges_if_present(workspace_root, false, false)?;
    print_next_steps(
        workspace_root,
        "Initialised COPR packaging",
        &[
            format!(
                "git add {} {}",
                shell_word(&resolved.spec_path),
                shell_word(&resolved.makefile_path)
            ),
            "copr-cli build <owner>/<project> <srpm>".to_owned(),
        ],
    );
    registry::touch_current_project_or_warn([("copr", FeatureStatus::Managed)]);
    Ok(())
}

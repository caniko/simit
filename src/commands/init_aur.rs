use anyhow::Result;

use crate::cargo;
use crate::cli::InitAurCommand;
use crate::commands::aur;
use crate::commands::scaffold::{ArtifactCheck, CheckPrintMode, print_next_steps};
use crate::config::ProjectConfig;
use crate::registry::{self, FeatureStatus};
use crate::render::pkgbuild;

pub fn run(command: InitAurCommand) -> Result<()> {
    let mode = CheckPrintMode::parse("init aur", command.check, command.print, command.diff)?;

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let package = cargo::representative_package(&metadata, command.package.as_deref())?;
    let cfg = ProjectConfig::load(workspace_root)?;
    let resolved = cfg.resolve_aur(command.aur.as_overrides(), &package)?;
    let flavors = pkgbuild::render(&resolved, &package.version);

    match mode {
        CheckPrintMode::Print => {
            for flavor in &flavors {
                println!("==> dist/aur/{}/PKGBUILD", flavor.pkgname);
                print!("{}", flavor.pkgbuild);
            }
            return Ok(());
        }
        CheckPrintMode::Check { diff } => {
            for flavor in &flavors {
                ArtifactCheck {
                    label: "AUR PKGBUILD",
                    path: &aur::pkgbuild_path(workspace_root, flavor),
                    expected: &flavor.pkgbuild,
                    remediation: "run `simit init aur`",
                }
                .verify(diff)?;
            }
            return Ok(());
        }
        CheckPrintMode::Write => {}
    }

    aur::write_flavors(workspace_root, &flavors)?;
    let pkgnames = flavors
        .iter()
        .map(|flavor| flavor.pkgname.clone())
        .collect::<Vec<_>>()
        .join(" ");
    print_next_steps(
        workspace_root,
        "Initialised AUR PKGBUILDs",
        &[
            "git add dist/aur".to_owned(),
            format!("# flavors: {pkgnames}"),
            "publish on release via the generated workflow (needs AUR_SSH_KEY)".to_owned(),
        ],
    );
    registry::touch_current_project_or_warn([("aur", FeatureStatus::Managed)]);
    Ok(())
}

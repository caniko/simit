use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::cli::InitChocolateyCommand;
use crate::commands::chocolatey;
use crate::commands::scaffold::{ArtifactCheck, CheckPrintMode, print_next_steps, shell_word};
use crate::registry::{self, FeatureStatus};
use crate::render::chocolatey_nuspec::{RenderedPackage, Sha256Set};

pub fn run(command: InitChocolateyCommand) -> Result<()> {
    let mode = CheckPrintMode::parse(
        "init chocolatey",
        command.check,
        command.print,
        command.diff,
    )?;

    let (resolved, package_version) = chocolatey::resolve(command.chocolatey.as_overrides())?;
    chocolatey::validate_resolved(&resolved)?;
    let package = chocolatey::render_package(
        &resolved,
        &package_version,
        false,
        &Sha256Set::all_no_check(),
    );

    let target = command.target.as_std_path();
    match mode {
        CheckPrintMode::Print => {
            chocolatey::print_package(&package);
            return Ok(());
        }
        CheckPrintMode::Check { diff } => {
            return verify_package(target, &package, diff);
        }
        CheckPrintMode::Write => {}
    }

    chocolatey::write_package(target, &package)?;
    print_chocolatey_next_steps(target, &resolved.id);
    registry::touch_current_project_or_warn([("chocolatey", FeatureStatus::Managed)]);
    Ok(())
}

fn verify_package(target: &Path, package: &RenderedPackage, show_diff: bool) -> Result<()> {
    let expected = [
        (target.join(&package.nuspec_name), package.nuspec.as_str()),
        (
            target.join("tools").join("chocolateyInstall.ps1"),
            package.install_script.as_str(),
        ),
        (
            target.join("tools").join("chocolateyUninstall.ps1"),
            package.uninstall_script.as_str(),
        ),
    ];

    for (path, text) in expected {
        ArtifactCheck {
            label: "Chocolatey package",
            path: &path,
            expected: text,
            remediation: &format!(
                "run `simit init chocolatey --target {}`",
                package_root(&path).display()
            ),
        }
        .verify(show_diff)?;
    }
    Ok(())
}

fn package_root(path: &Path) -> PathBuf {
    if path.file_name().and_then(|name| name.to_str()) == Some("chocolateyInstall.ps1")
        || path.file_name().and_then(|name| name.to_str()) == Some("chocolateyUninstall.ps1")
    {
        path.parent()
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    } else {
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }
}

fn print_chocolatey_next_steps(target: &Path, id: &str) {
    print_next_steps(
        target,
        "Initialised Chocolatey package",
        &[
            format!("choco pack {}", shell_word(&target.display().to_string())),
            format!(
                "simit dist chocolatey bump --version <version> --package-dir {} --archive x64=<archive.zip>",
                shell_word(&target.display().to_string())
            ),
        ],
    );
    println!();
    println!("Package id: {id}");
}

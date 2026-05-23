use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::cargo;
use crate::cli::InitHomebrewTapCommand;
use crate::commands::scaffold::{
    ArtifactCheck, CheckPrintMode, WriteArtifact, bootstrap_repo, prepare_target, print_next_steps,
    shell_word,
};
use crate::config::{ProjectConfig, ResolvedHomebrew};
use crate::registry::{self, FeatureStatus};
use crate::render::homebrew_formula::{self, Sha256Set};

pub fn run(command: InitHomebrewTapCommand) -> Result<()> {
    let mode = CheckPrintMode::parse(
        "init homebrew-tap",
        command.check,
        command.print,
        command.diff,
    )?;

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let package = cargo::select_packages(&metadata, &[], false)?
        .into_iter()
        .next()
        .expect("single package selected");
    let cfg = ProjectConfig::load(workspace_root)?;
    let resolved = cfg.resolve_homebrew(command.homebrew.as_overrides(), &package)?;
    let formula_text =
        homebrew_formula::render(&resolved, &package.version, &Sha256Set::all_no_check());

    let target = command.target.as_std_path();
    let formula_path = formula_path(target, &resolved.name);

    match mode {
        CheckPrintMode::Print => {
            print!("{formula_text}");
            return Ok(());
        }
        CheckPrintMode::Check { diff } => {
            return ArtifactCheck {
                label: "Homebrew formula",
                path: &formula_path,
                expected: &formula_text,
                remediation: &format!(
                    "run `simit init homebrew-tap --target {}`",
                    target.display()
                ),
            }
            .verify(diff);
        }
        CheckPrintMode::Write => {}
    }

    prepare_target(target, command.no_git)?;
    WriteArtifact {
        path: &formula_path,
        contents: &formula_text,
    }
    .commit()?;

    if !command.no_git {
        bootstrap_repo(
            target,
            &resolved.tap_url,
            "trunk",
            &format!("Formula/{}.rb", resolved.name),
        )?;
    }

    print_homebrew_next_steps(target, &resolved);
    registry::touch_current_project_or_warn([("homebrew", FeatureStatus::Managed)]);
    Ok(())
}

fn formula_path(target: &Path, name: &str) -> PathBuf {
    target.join("Formula").join(format!("{name}.rb"))
}

fn print_homebrew_next_steps(target: &Path, resolved: &ResolvedHomebrew) {
    print_next_steps(
        target,
        "Initialised tap",
        &[
            format!(
                "git -C {} commit -m {}",
                shell_word(&target.display().to_string()),
                shell_word(&format!("Initial {} formula", resolved.name))
            ),
            format!(
                "git -C {} push -u origin trunk",
                shell_word(&target.display().to_string())
            ),
        ],
    );
    println!();
    println!(
        "Use your tap repo's default branch instead of trunk if it already uses another convention."
    );
}

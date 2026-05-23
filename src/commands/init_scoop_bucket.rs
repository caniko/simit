use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::cargo;
use crate::cli::InitScoopBucketCommand;
use crate::commands::scaffold::{
    ArtifactCheck, CheckPrintMode, WriteArtifact, bootstrap_repo, prepare_target, print_next_steps,
    shell_word,
};
use crate::config::{ProjectConfig, ResolvedScoop};
use crate::registry::{self, FeatureStatus};
use crate::render::scoop_manifest::{self, ScoopChecksums};

pub fn run(command: InitScoopBucketCommand) -> Result<()> {
    let mode = CheckPrintMode::parse(
        "init scoop-bucket",
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
    let resolved = cfg.resolve_scoop(command.scoop.as_overrides(), &package)?;
    let manifest_text = scoop_manifest::render(
        &resolved,
        &package.version,
        &ScoopChecksums::all_placeholder(),
    );

    let target = command.target.as_std_path();
    let manifest_path = manifest_path(target, &resolved.name);

    match mode {
        CheckPrintMode::Print => {
            print!("{manifest_text}");
            return Ok(());
        }
        CheckPrintMode::Check { diff } => {
            return ArtifactCheck {
                label: "Scoop manifest",
                path: &manifest_path,
                expected: &manifest_text,
                remediation: &format!(
                    "run `simit init scoop-bucket --target {}`",
                    target.display()
                ),
            }
            .verify(diff);
        }
        CheckPrintMode::Write => {}
    }

    prepare_target(target, command.no_git)?;
    WriteArtifact {
        path: &manifest_path,
        contents: &manifest_text,
    }
    .commit()?;

    if !command.no_git {
        bootstrap_repo(
            target,
            &resolved.bucket_url,
            "trunk",
            &format!("bucket/{}.json", resolved.name),
        )?;
    }

    print_scoop_next_steps(target, &resolved);
    registry::touch_current_project_or_warn([("scoop", FeatureStatus::Managed)]);
    Ok(())
}

fn manifest_path(target: &Path, name: &str) -> PathBuf {
    target.join("bucket").join(format!("{name}.json"))
}

fn print_scoop_next_steps(target: &Path, resolved: &ResolvedScoop) {
    print_next_steps(
        target,
        "Initialised Scoop bucket",
        &[
            format!(
                "git -C {} commit -m {}",
                shell_word(&target.display().to_string()),
                shell_word(&format!("Initial {} manifest", resolved.name))
            ),
            format!(
                "git -C {} push -u origin trunk",
                shell_word(&target.display().to_string())
            ),
        ],
    );
    println!();
    println!(
        "Use your bucket repo's default branch instead of trunk if it already uses another convention."
    );
}

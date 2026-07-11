use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::cargo;
use crate::cli::InitGitignoreCommand;
use crate::commands::scaffold::{ArtifactCheck, CheckPrintMode};
use crate::project;
use crate::python;
use crate::render::gitignore;

pub fn run(command: InitGitignoreCommand) -> Result<()> {
    let mode = CheckPrintMode::parse("init gitignore", command.check, command.print, command.diff)?;
    let workspace_root = workspace_root()?;
    let languages = project::detect_languages(&workspace_root)?;
    let rendered = gitignore::file(&languages);
    let path = workspace_root.join(&rendered.relative_path);

    match mode {
        CheckPrintMode::Print => {
            println!("--- {}", rendered.relative_path.display());
            print!("{}", rendered.content);
            Ok(())
        }
        CheckPrintMode::Check { diff } => ArtifactCheck {
            label: ".gitignore",
            path: &path,
            expected: &rendered.content,
            remediation: "run simit init gitignore",
        }
        .verify(diff),
        CheckPrintMode::Write => {
            std::fs::write(&path, &rendered.content)
                .with_context(|| format!("writing {}", path.display()))?;
            println!("Generated {}.", path.display());
            Ok(())
        }
    }
}

fn workspace_root() -> Result<PathBuf> {
    let current_dir = std::env::current_dir().context("reading current directory")?;

    if cargo::find_manifest(&current_dir).is_ok() {
        return Ok(cargo::metadata_for_current_dir()?
            .workspace_root
            .into_std_path_buf());
    }

    if let Ok(root) = python::find_project_root(&current_dir) {
        return Ok(root);
    }

    Ok(current_dir)
}

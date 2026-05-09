use anyhow::{Result, bail};

use crate::cargo;
use crate::cli::InitHooksCommand;
use crate::project;
use crate::render::hooks;

pub fn run(command: InitHooksCommand) -> Result<()> {
    if command.check && command.print {
        bail!("init-hooks accepts only one of --check or --print");
    }
    if command.diff && !command.check {
        bail!("init-hooks --diff requires --check");
    }

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let languages = project::detect_languages(workspace_root)?;
    let files = hooks::files(&languages);

    if command.print {
        hooks::print_files(&files);
        hooks::print_flake_snippet();
        Ok(())
    } else if command.check {
        project::check_generated_files(
            workspace_root,
            &files,
            "hook files are not up to date; run `simit init-hooks`",
            command.diff,
        )
    } else {
        project::write_generated_files(workspace_root, &files)?;
        hooks::print_flake_snippet();
        Ok(())
    }
}

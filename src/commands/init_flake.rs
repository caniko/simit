use anyhow::{Result, bail};

use crate::cargo;
use crate::cli::InitFlakeCommand;
use crate::project;
use crate::render::flake;

pub fn run(command: InitFlakeCommand) -> Result<()> {
    if command.check && command.print {
        bail!("init-flake accepts only one of --check or --print");
    }

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let files = flake::files();

    if command.print {
        flake::print_files(&files);
        return Ok(());
    }

    if command.check {
        return project::check_generated_files(
            workspace_root,
            &files,
            "flake.nix is not up to date; run `simit init-flake`",
            false,
        );
    }

    if workspace_root.join("flake.nix").exists() {
        bail!(
            "flake.nix already exists; run `simit init-flake --print` and apply the template manually"
        );
    }

    project::write_generated_files(workspace_root, &files)
}

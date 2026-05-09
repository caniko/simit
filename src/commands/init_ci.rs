use anyhow::{Result, bail};

use crate::cargo;
use crate::cli::{InitCiCommand, Runtime, RuntimeChoice};
use crate::project;
use crate::render::ci::{self, CiOptions};

pub fn run(command: InitCiCommand) -> Result<()> {
    validate_runner(command.runner.as_deref())?;

    let metadata = cargo::metadata_for_current_dir()?;
    let package = cargo::select_packages(&metadata, &[], false)?
        .into_iter()
        .next()
        .expect("single package selected");
    let workspace_root = metadata.workspace_root.as_std_path();
    let runtime = resolve_runtime(command.runtime, workspace_root)?;
    let self_check = metadata
        .packages
        .iter()
        .any(|package| package.name == "simit");
    let options = CiOptions {
        with_nextest: command.with_nextest,
        with_msrv: command.with_msrv,
        with_audit: command.with_audit,
        with_deny: command.with_deny,
        with_docs: command.with_docs,
        with_artifacts: command.with_artifacts,
    };
    let files = ci::files(
        command.platform,
        runtime,
        &package,
        self_check,
        command.runner.as_deref(),
        options,
    )?;

    if command.check {
        project::check_generated_files(
            workspace_root,
            &files,
            &format!(
                "CI workflows are not up to date; run `simit init-ci --platform {}`",
                command.platform.as_str()
            ),
            command.diff,
        )
    } else {
        project::write_generated_files(workspace_root, &files)
    }
}

pub(crate) fn resolve_runtime(
    choice: RuntimeChoice,
    workspace_root: &std::path::Path,
) -> Result<Runtime> {
    match choice {
        RuntimeChoice::Auto | RuntimeChoice::Cargo => Ok(Runtime::Cargo),
        RuntimeChoice::Nix => {
            if !workspace_root.join("flake.nix").exists() {
                bail!("--runtime nix requires flake.nix at the workspace root");
            }
            Ok(Runtime::Nix)
        }
    }
}

pub(crate) fn validate_runner(value: Option<&str>) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_empty() {
        bail!("--runner cannot be empty");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("--runner may only contain ASCII letters, digits, '.', '_', and '-'");
    }
    Ok(())
}

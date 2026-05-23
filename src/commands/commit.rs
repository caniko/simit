use anyhow::Result;

use crate::cargo::{self, BumpSpec};
use crate::cli::CommitCommand;
use crate::git;
use crate::registry;

pub fn run(command: CommitCommand) -> Result<()> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let packages = cargo::select_packages(&metadata, &command.packages, command.workspace)?;
    let bump = BumpSpec::new(command.bump, command.pre)?;
    let plans = cargo::plan_versions(packages, &bump)?;
    let new_version = cargo::common_new_version(&plans)?;
    let create_tag = !command.no_tag;
    let sign_tag = !command.no_sign;

    if command.dry_run {
        print_dry_run(&plans, create_tag, sign_tag, &command.git_args);
        return Ok(());
    }

    git::commit_preflight(workspace_root, &plans, create_tag, sign_tag, &new_version)?;
    for plan in &plans {
        cargo::update_manifest_version(
            plan.package.manifest_path.as_std_path(),
            &plan.new_version,
        )?;
    }
    let version_paths = git::version_paths(workspace_root, &plans);
    if workspace_root.join("Cargo.lock").exists() {
        cargo::update_lockfile(workspace_root, &plans)?;
    }
    git::stage_paths(workspace_root, &version_paths)?;
    git::commit(workspace_root, &command.git_args)?;
    if create_tag {
        git::tag(workspace_root, &new_version, sign_tag)?;
    }

    registry::refresh_current_project_or_warn();
    Ok(())
}

fn print_dry_run(
    plans: &[cargo::VersionPlan],
    create_tag: bool,
    sign_tag: bool,
    git_args: &[std::ffi::OsString],
) {
    println!("simit commit dry-run");
    for plan in plans {
        println!(
            "package {}: {} -> {}",
            plan.package.name, plan.old_version, plan.new_version
        );
        println!("would update {}", plan.package.manifest_path);
    }
    println!("would update Cargo.lock if present");
    println!("would run git commit with {:?}", git_args);
    if create_tag {
        let version = &plans[0].new_version;
        if sign_tag {
            println!("would create signed tag {version}");
        } else {
            println!("would create unsigned tag {version}");
        }
    } else {
        println!("would not create a tag");
    }
}

use std::ffi::OsString;

use anyhow::Result;

use crate::cargo::{self, BumpSpec};
use crate::cli::ReleaseCommand;
use crate::git;
use crate::render::changelog;

pub fn run(command: ReleaseCommand) -> Result<()> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let packages = cargo::select_packages(&metadata, &command.packages, command.workspace)?;
    let bump = BumpSpec::new(command.bump, command.pre)?;
    let plans = cargo::plan_versions(packages, &bump)?;
    let new_version = cargo::common_new_version(&plans)?;
    let create_tag = !command.no_tag;
    let sign_tag = !command.no_sign;
    let git_args = vec![
        OsString::from("-m"),
        OsString::from(command.message.clone()),
    ];

    if command.dry_run {
        println!("simit release dry-run");
        for plan in &plans {
            println!(
                "package {}: {} -> {}",
                plan.package.name, plan.old_version, plan.new_version
            );
        }
        println!("would run cargo test and cargo clippy");
        println!("would update CHANGELOG.md");
        println!("would run git commit with {:?}", git_args);
        if create_tag {
            println!("would create tag {new_version}");
        }
        return Ok(());
    }

    git::release_preflight(workspace_root, create_tag, sign_tag, &new_version)?;
    let changelog_update =
        changelog::planned_update(workspace_root, &new_version, &command.message)?;
    git::run_project_checks(workspace_root)?;
    for plan in &plans {
        cargo::update_manifest_version(
            plan.package.manifest_path.as_std_path(),
            &plan.new_version,
        )?;
    }
    if workspace_root.join("Cargo.lock").exists() {
        cargo::update_lockfile(workspace_root, &plans)?;
    }
    changelog::write_update(workspace_root, &changelog_update)?;

    let mut paths = git::version_paths(workspace_root, &plans);
    paths.push(workspace_root.join("CHANGELOG.md"));
    paths.sort();
    paths.dedup();
    git::stage_paths(workspace_root, &paths)?;
    git::commit(workspace_root, &git_args)?;
    if create_tag {
        git::tag(workspace_root, &new_version, sign_tag)?;
    }

    Ok(())
}

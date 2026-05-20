use std::ffi::OsString;

use anyhow::Result;

use crate::cargo::{self, BumpSpec};
use crate::changelog;
use crate::cli::ReleaseCommand;
use crate::git;

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
    let changelog_path = workspace_root.join(changelog::DEFAULT_PATH);

    if command.dry_run {
        println!("simit release dry-run");
        for plan in &plans {
            println!(
                "package {}: {} -> {}",
                plan.package.name, plan.old_version, plan.new_version
            );
        }
        println!("would run cargo test and cargo clippy");
        if !command.no_changelog && changelog_path.exists() {
            println!("would promote CHANGELOG.md [Unreleased] to {new_version}");
        }
        println!("would run git commit with {:?}", git_args);
        if create_tag {
            println!("would create tag {new_version}");
        }
        return Ok(());
    }

    git::release_preflight(workspace_root, create_tag, sign_tag, &new_version)?;
    let changelog_update = if !command.no_changelog && changelog_path.exists() {
        Some(changelog::release_content(
            &std::fs::read_to_string(&changelog_path)?,
            &new_version,
            changelog::today_utc(),
            None,
            &changelog_path,
            Some(workspace_root),
        )?)
    } else {
        None
    };
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
    if let Some(changelog_update) = changelog_update {
        std::fs::write(&changelog_path, changelog_update)?;
    }

    let mut paths = git::version_paths(workspace_root, &plans);
    if changelog_path.exists() && !command.no_changelog {
        paths.push(changelog_path);
    }
    paths.sort();
    paths.dedup();
    git::stage_paths(workspace_root, &paths)?;
    git::commit(workspace_root, &git_args)?;
    if create_tag {
        git::tag(workspace_root, &new_version, sign_tag)?;
    }

    Ok(())
}

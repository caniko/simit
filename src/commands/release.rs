use std::ffi::OsString;

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;

use crate::cargo::{self, BumpSpec, Package};
use crate::changelog;
use crate::cli::{ReleaseAction, ReleaseCommand};
use crate::git;

pub fn run(command: ReleaseCommand) -> Result<()> {
    if command.action == ReleaseAction::SyncUp {
        return sync_up(command);
    }

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let packages = cargo::select_packages(&metadata, &command.packages, command.workspace)?;
    if command.push {
        bail!("--push is only valid with `simit release sync-up`");
    }
    if command.remote != "origin" {
        bail!("--remote is only valid with `simit release sync-up`");
    }
    let bump = BumpSpec::new(
        command.action.bump_kind().expect("release bump action"),
        command.pre,
    )?;
    let plans = cargo::plan_versions(packages, &bump)?;
    let new_version = cargo::common_new_version(&plans)?;
    let create_tag = !command.no_tag;
    let sign_tag = !command.no_sign;
    let message = command
        .message
        .clone()
        .ok_or_else(|| anyhow!("release commit message is required; pass -m <message>"))?;
    let git_args = vec![OsString::from("-m"), OsString::from(message)];
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

fn sync_up(command: ReleaseCommand) -> Result<()> {
    if command.no_tag {
        bail!("--no-tag is not valid with `simit release sync-up`");
    }
    if command.pre.is_some() {
        bail!("--pre is not valid with `simit release sync-up`");
    }
    if command.message.is_some() {
        bail!("-m/--message is not valid with `simit release sync-up`");
    }
    if command.no_changelog {
        bail!("--no-changelog is not valid with `simit release sync-up`");
    }

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let packages = cargo::select_packages(&metadata, &command.packages, command.workspace)?;
    let version = common_current_version(&packages)?;
    let sign_tag = !command.no_sign;
    git::tag_ref_object(workspace_root, &version)?;
    let old_tag_target = git::tag_target_commit(workspace_root, &version)?;
    let head = git::head_commit(workspace_root)?;

    if command.dry_run {
        println!("simit release sync-up dry-run");
        println!("version: {version}");
        println!("tag {version} currently points to {old_tag_target}");
        println!("would run cargo test and cargo clippy");
        if old_tag_target == head {
            println!("tag {version} already points to HEAD");
        } else {
            println!("would move tag {version} to HEAD {head}");
        }
        if command.push {
            println!("would push tag {version} to {}", command.remote);
        } else {
            println!("would not push tag {version}");
        }
        return Ok(());
    }

    git::sync_up_preflight(workspace_root, sign_tag)?;
    git::run_project_checks(workspace_root)?;

    if old_tag_target == head {
        println!("tag {version} already points to HEAD");
    } else {
        git::move_tag(workspace_root, &version, sign_tag)?;
        println!("moved tag {version} to HEAD {head}");
    }

    if command.push {
        let expected_remote_object =
            git::remote_tag_ref_object(workspace_root, &command.remote, &version)?;
        git::push_moved_tag(
            workspace_root,
            &command.remote,
            &version,
            &expected_remote_object,
        )?;
    }

    Ok(())
}

fn common_current_version(packages: &[Package]) -> Result<Version> {
    let Some(first) = packages.first() else {
        bail!("no packages selected");
    };
    let first_version = Version::parse(&first.version)
        .with_context(|| format!("parsing version {}", first.version))?;

    let mut divergent = Vec::new();
    for package in packages.iter().skip(1) {
        let version = Version::parse(&package.version)
            .with_context(|| format!("parsing version {}", package.version))?;
        if version != first_version {
            divergent.push(format!("{} -> {}", package.name, package.version));
        }
    }

    if !divergent.is_empty() {
        let mut details = vec![format!("{} -> {}", first.name, first.version)];
        details.extend(divergent);
        bail!(
            "selected packages do not have one current release version:\n{}",
            details.join("\n")
        );
    }

    Ok(first_version)
}

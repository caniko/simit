use std::ffi::OsString;

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;

use crate::cargo::{self, BumpSpec, Package};
use crate::changelog;
use crate::cli::{ReleaseAction, ReleaseCommand, ReleaseSecretsAction, ReleaseTrustAction};
use crate::config::ProjectConfig;
use crate::git;
use crate::registry;
use crate::release_trust::{self, TrustOverrides};

pub fn run(command: ReleaseCommand) -> Result<()> {
    if command.action == ReleaseAction::Verify {
        return crate::commands::release_verify::run(command);
    }
    if command.action == ReleaseAction::Plan {
        return crate::commands::release_plan::run(command);
    }
    if command.action == ReleaseAction::Trust {
        return trust(command);
    }
    if command.action == ReleaseAction::Secrets {
        return secrets(command);
    }
    if command.trust_action.is_some() || command.trust_key.is_some() || command.trust_root.is_some()
    {
        bail!("release trust arguments are only valid with `simit release trust`");
    }
    reject_secrets_flags(&command)?;
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
    reject_verify_flags(&command)?;
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
            changelog::today_utc()?,
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

    registry::refresh_current_project_or_warn();
    Ok(())
}

fn secrets(command: ReleaseCommand) -> Result<()> {
    if !command.packages.is_empty() {
        bail!("--package is not valid with `simit release secrets`");
    }
    if command.workspace {
        bail!("--workspace is not valid with `simit release secrets`");
    }
    if command.no_tag {
        bail!("--no-tag is not valid with `simit release secrets`");
    }
    if command.no_sign {
        bail!("--no-sign is not valid with `simit release secrets`");
    }
    if command.dry_run {
        bail!("--dry-run is not valid with `simit release secrets`");
    }
    if command.pre.is_some() {
        bail!("--pre is not valid with `simit release secrets`");
    }
    if command.message.is_some() {
        bail!("-m/--message is not valid with `simit release secrets`");
    }
    if command.no_changelog {
        bail!("--no-changelog is not valid with `simit release secrets`");
    }
    if command.push {
        bail!("--push is not valid with `simit release secrets`");
    }
    if command.remote != "origin" {
        bail!("--remote is not valid with `simit release secrets`");
    }
    if command.trust_key.is_some() || command.trust_root.is_some() {
        bail!("release trust arguments are only valid with `simit release trust`");
    }
    let contract_action = command.secrets_action == Some(ReleaseSecretsAction::Contract)
        || command.trust_action == Some(ReleaseTrustAction::Contract);
    if !(contract_action && command.json) {
        reject_verify_flags(&command)?;
    }

    let action = match (command.secrets_action, command.trust_action) {
        (Some(action), None) => action,
        (None, Some(ReleaseTrustAction::Init)) => ReleaseSecretsAction::Init,
        (None, Some(ReleaseTrustAction::Check)) => ReleaseSecretsAction::Check,
        (None, Some(ReleaseTrustAction::InspectMinisignInput)) => {
            ReleaseSecretsAction::InspectMinisignInput
        }
        (None, Some(ReleaseTrustAction::Contract)) => ReleaseSecretsAction::Contract,
        (None, Some(ReleaseTrustAction::Status)) => {
            bail!("release secrets action must be init, check, contract, or inspect-minisign-input")
        }
        (None, None) => {
            bail!(
                "release secrets action is required: init, check, contract, or inspect-minisign-input"
            )
        }
        (Some(_), Some(_)) => bail!("release secrets action specified more than once"),
    };
    match action {
        ReleaseSecretsAction::Init => crate::commands::release_secrets::init(command),
        ReleaseSecretsAction::Check => crate::commands::release_secrets::check(command),
        ReleaseSecretsAction::Contract => crate::commands::release_secrets::contract(command),
        ReleaseSecretsAction::InspectMinisignInput => {
            crate::commands::release_secrets::inspect_minisign_input(command)
        }
    }
}

fn trust(command: ReleaseCommand) -> Result<()> {
    if !command.packages.is_empty() {
        bail!("--package is not valid with `simit release trust`");
    }
    if command.workspace {
        bail!("--workspace is not valid with `simit release trust`");
    }
    if command.no_tag {
        bail!("--no-tag is not valid with `simit release trust`");
    }
    if command.no_sign {
        bail!("--no-sign is not valid with `simit release trust`");
    }
    if command.dry_run {
        bail!("--dry-run is not valid with `simit release trust`");
    }
    if command.pre.is_some() {
        bail!("--pre is not valid with `simit release trust`");
    }
    if command.message.is_some() {
        bail!("-m/--message is not valid with `simit release trust`");
    }
    if command.no_changelog {
        bail!("--no-changelog is not valid with `simit release trust`");
    }
    if command.push {
        bail!("--push is not valid with `simit release trust`");
    }
    if command.remote != "origin" {
        bail!("--remote is not valid with `simit release trust`");
    }
    reject_verify_flags(&command)?;

    let action = command
        .trust_action
        .ok_or_else(|| anyhow!("release trust action is required: status, init, or check"))?;
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let config = ProjectConfig::load(workspace_root)?;
    let overrides = TrustOverrides {
        key: command.trust_key,
        trust_root: command.trust_root,
    };

    match action {
        ReleaseTrustAction::Status => release_trust::status(workspace_root, &config, &overrides),
        ReleaseTrustAction::Init => release_trust::init(workspace_root, &config, &overrides),
        ReleaseTrustAction::Check => release_trust::check(workspace_root, &config, &overrides),
        ReleaseTrustAction::InspectMinisignInput | ReleaseTrustAction::Contract => {
            bail!("release trust action must be status, init, or check")
        }
    }
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
    reject_verify_flags(&command)?;

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

    registry::refresh_current_project_or_warn();
    Ok(())
}

fn reject_verify_flags(command: &ReleaseCommand) -> Result<()> {
    if command.json {
        bail!("--json is only valid with `simit release verify`");
    }
    if command.verify_version.is_some() {
        bail!("--version is only valid with `simit release verify`");
    }
    if command.push_target.is_some() {
        bail!("--push-target is only valid with `simit release verify`");
    }
    Ok(())
}

fn reject_secrets_flags(command: &ReleaseCommand) -> Result<()> {
    if command.secrets_action.is_some()
        || command.secrets_repo.is_some()
        || command.secrets_token_file.is_some()
        || !command.assumed_account_secrets.is_empty()
        || command.minisign_secret_key_file.is_some()
        || command.minisign_password_file.is_some()
        || command.rotate_minisign
        || command.minisign_public_key.as_str() != "keys/minisign.pub"
        || command.secrets_api_base != "https://codeberg.org/api/v1"
    {
        bail!("release secrets arguments are only valid with `simit release secrets`");
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

use std::fs;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::{Platform, WingetAction, WingetCommand, WingetSubmitArgs};
use crate::config::{ProjectConfig, ResolvedWinget, WingetConfig};
use crate::packaging_common::{validate_download_repo, validate_version};
use crate::registry::{self, FeatureStatus};

pub fn run(command: WingetCommand) -> Result<()> {
    match command.action {
        WingetAction::Submit(args) => submit(args),
    }
}

/// Submit the release zip to winget-pkgs by running the same GitHub REST
/// script as the release workflow step (needs `bash`, `curl` and `jq`).
pub(crate) fn submit(args: WingetSubmitArgs) -> Result<()> {
    validate_version(&args.version)?;
    let (resolved, platform, tag_prefix) = resolve(&args)?;
    validate_download_repo("winget.download_repo", &resolved.download_repo)?;
    let url = args
        .url
        .clone()
        .unwrap_or_else(|| release_url(&resolved, &args.version, platform, tag_prefix));
    let token_env = args
        .token_env
        .as_deref()
        .unwrap_or(resolved.token_secret.as_str());
    if token_env.is_empty() {
        bail!("Winget token environment variable name is empty");
    }
    if args.dry_run {
        println!(
            "winget: would submit {} {} ({url}) to microsoft/winget-pkgs through the GitHub API with ${token_env}",
            resolved.package_id, args.version
        );
        return Ok(());
    }
    let token = std::env::var(token_env)
        .with_context(|| format!("reading Winget GitHub token from ${token_env}"))?;
    if token.is_empty() {
        bail!("Winget token environment variable ${token_env} is empty");
    }

    let work_dir = args
        .work_dir
        .as_ref()
        .map(|path| path.as_std_path().to_path_buf())
        .unwrap_or_else(|| "target/simit-winget".into());
    let release_dir = work_dir.join("release");
    fs::create_dir_all(&release_dir)
        .with_context(|| format!("creating {}", release_dir.display()))?;
    let zip_name = url.rsplit('/').next().unwrap_or_default();
    if zip_name.is_empty() {
        bail!("cannot derive the installer file name from {url}");
    }
    // The manifests carry the installer's SHA-256, so fetch the zip.
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(release_dir.join(zip_name))
        .arg(&url)
        .status()
        .context("downloading the installer with curl")?;
    if !status.success() {
        bail!("downloading {url} failed");
    }
    let script = work_dir.join("winget-submit.sh");
    fs::write(
        &script,
        crate::render::release_workflow::winget_submit_script(&resolved, platform),
    )
    .with_context(|| format!("writing {}", script.display()))?;
    // The token travels in the environment; the script hands it to curl via a header file.
    let status = Command::new("bash")
        .arg("winget-submit.sh")
        .current_dir(&work_dir)
        .env("VERSION", &args.version)
        .env("ZIP_NAME", zip_name)
        .env("ZIP_URL", &url)
        .env("WINGET_PAT", token)
        .env_remove("PUBLISH_WINGET_STATE")
        .status()
        .context("running bash")?;
    if !status.success() {
        bail!("winget submission failed");
    }
    registry::touch_current_project_or_warn([("winget", FeatureStatus::Managed)]);
    Ok(())
}

fn resolve(
    args: &WingetSubmitArgs,
) -> Result<(ResolvedWinget, Platform, crate::release_identity::TagPrefix)> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let mut cfg = ProjectConfig::load(workspace_root)?;
    let package = cargo::representative_package(&metadata, cfg.primary_package.as_deref())?;
    let mut winget = cfg.winget.clone().unwrap_or(WingetConfig {
        package_id: String::new(),
        download_repo: String::new(),
        zip_archive: String::new(),
        token_secret: "WINGET_PAT".to_owned(),
        publisher: None,
        package_name: None,
        license: None,
        license_url: None,
        short_description: None,
        homepage: None,
        binary: None,
        command: None,
        dependencies: Vec::new(),
    });
    for (flag, value, slot, name) in [
        (
            "--package-id",
            &args.package_id,
            &mut winget.package_id,
            "package_id",
        ),
        (
            "--download-repo",
            &args.download_repo,
            &mut winget.download_repo,
            "download_repo",
        ),
        (
            "--zip-archive",
            &args.zip_archive,
            &mut winget.zip_archive,
            "zip_archive",
        ),
    ] {
        if let Some(value) = value {
            slot.clone_from(value);
        }
        if slot.is_empty() {
            bail!("winget.{name} not set; provide {flag} or [winget].{name}");
        }
    }
    if let Some(env) = &args.token_env {
        winget.token_secret.clone_from(env);
    }
    cfg.winget = Some(winget);
    Ok((
        cfg.resolve_winget(&package)?,
        crate::ci_resolution::release_backend(&cfg, None, None)?.platform(),
        cfg.release.tag_prefix,
    ))
}

fn release_url(
    resolved: &ResolvedWinget,
    version: &str,
    platform: Platform,
    prefix: crate::release_identity::TagPrefix,
) -> String {
    let zip = resolved.zip_archive.replace("{version}", version);
    platform.release_download_url(
        &resolved.download_repo,
        &format!("{}{version}", prefix.as_str()),
        &zip,
    )
}

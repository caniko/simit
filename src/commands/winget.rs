use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::{Platform, WingetAction, WingetCommand, WingetSubmitArgs};
use crate::config::{ProjectConfig, WingetConfig};
use crate::registry::{self, FeatureStatus};

pub fn run(command: WingetCommand) -> Result<()> {
    match command.action {
        WingetAction::Submit(args) => submit(args),
    }
}

pub(crate) fn submit(args: WingetSubmitArgs) -> Result<()> {
    validate_version(&args.version)?;
    let (resolved, platform) = resolve(&args)?;
    validate_download_repo(&resolved.download_repo)?;
    let url = args
        .url
        .clone()
        .unwrap_or_else(|| release_url(&resolved, &args.version, platform));
    let token_env = args
        .token_env
        .as_deref()
        .unwrap_or(resolved.token_secret.as_str());
    if token_env.is_empty() {
        bail!("Winget token environment variable name is empty");
    }
    let token = std::env::var(token_env)
        .with_context(|| format!("reading Winget GitHub token from ${token_env}"))?;
    if token.is_empty() {
        bail!("Winget token environment variable ${token_env} is empty");
    }

    let wingetcreate = resolve_wingetcreate(&args)?;
    let command_display = format!(
        "{} {} update {} --version {} --urls {}|x64 --token ${} --submit",
        args.wine,
        wingetcreate.display(),
        resolved.package_id,
        args.version,
        url,
        token_env
    );
    if args.dry_run {
        println!("{command_display}");
        return Ok(());
    }

    let mut command = Command::new(&args.wine);
    command
        .arg(&wingetcreate)
        .arg("update")
        .arg(&resolved.package_id)
        .arg("--version")
        .arg(&args.version)
        .arg("--urls")
        .arg(format!("{url}|x64"))
        .arg("--token")
        .arg(token)
        .arg("--submit")
        .env("WINEDEBUG", "-all")
        .env("TERM", "xterm");
    if args.wingetcreate.is_none() {
        let wineprefix = wingetcreate
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("wine");
        command.env("WINEPREFIX", wineprefix);
    }
    let status = command
        .status()
        .with_context(|| format!("running {}", args.wine))?;
    if !status.success() {
        bail!("wingetcreate update failed");
    }
    registry::touch_current_project_or_warn([("winget", FeatureStatus::Managed)]);
    Ok(())
}

fn resolve_wingetcreate(args: &WingetSubmitArgs) -> Result<PathBuf> {
    if let Some(path) = &args.wingetcreate {
        return Ok(path.as_std_path().to_path_buf());
    }
    let work_dir = args
        .work_dir
        .as_ref()
        .map(|path| path.as_std_path().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("target/simit-winget"));
    if args.dry_run {
        return Ok(work_dir.join("wingetcreate.exe"));
    }
    fs::create_dir_all(&work_dir).with_context(|| format!("creating {}", work_dir.display()))?;
    let wingetcreate = work_dir.join("wingetcreate.exe");
    if wingetcreate.is_file() {
        return Ok(wingetcreate);
    }
    let url = match &args.wingetcreate_url {
        Some(url) => url.clone(),
        None => latest_wingetcreate_url()?,
    };
    let status = Command::new("curl")
        .arg("-fsSL")
        .arg("-o")
        .arg(&wingetcreate)
        .arg(&url)
        .status()
        .with_context(|| "downloading wingetcreate.exe with curl")?;
    if !status.success() {
        bail!("downloading wingetcreate.exe failed");
    }
    Ok(wingetcreate)
}

fn latest_wingetcreate_url() -> Result<String> {
    let output = Command::new("curl")
        .arg("-fsSL")
        .arg("https://api.github.com/repos/microsoft/winget-create/releases/latest")
        .output()
        .with_context(|| "fetching latest winget-create release with curl")?;
    if !output.status.success() {
        bail!("fetching latest winget-create release failed");
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("parsing winget-create release JSON")?;
    json.get("assets")
        .and_then(|assets| assets.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                (asset.get("name").and_then(|name| name.as_str()) == Some("wingetcreate.exe"))
                    .then(|| {
                        asset
                            .get("browser_download_url")
                            .and_then(|url| url.as_str())
                    })
                    .flatten()
            })
        })
        .map(str::to_owned)
        .context("latest winget-create release did not contain wingetcreate.exe")
}

fn resolve(args: &WingetSubmitArgs) -> Result<(WingetConfig, Platform)> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let cfg = ProjectConfig::load(workspace_root)?;
    let configured = cfg.winget.as_ref();
    let package_id = args
        .package_id
        .clone()
        .or_else(|| configured.map(|winget| winget.package_id.clone()))
        .context("winget.package_id not set; provide --package-id or [winget].package_id")?;
    let download_repo = args
        .download_repo
        .clone()
        .or_else(|| configured.map(|winget| winget.download_repo.clone()))
        .context(
            "winget.download_repo not set; provide --download-repo or [winget].download_repo",
        )?;
    let zip_archive = args
        .zip_archive
        .clone()
        .or_else(|| configured.map(|winget| winget.zip_archive.clone()))
        .context("winget.zip_archive not set; provide --zip-archive or [winget].zip_archive")?;
    let token_secret = args
        .token_env
        .clone()
        .or_else(|| configured.map(|winget| winget.token_secret.clone()))
        .unwrap_or_else(|| "WINGET_PAT".to_owned());
    Ok((
        WingetConfig {
            package_id,
            download_repo,
            zip_archive,
            token_secret,
        },
        cfg.ci.platform.unwrap_or(Platform::Forgejo),
    ))
}

fn release_url(resolved: &WingetConfig, version: &str, platform: Platform) -> String {
    let zip = resolved.zip_archive.replace("{version}", version);
    platform.release_download_url(&resolved.download_repo, version, &zip)
}

fn validate_version(version: &str) -> Result<()> {
    if version.starts_with('v')
        || version.is_empty()
        || !version
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '+' | '~' | '_' | '-'))
    {
        bail!("version must match [0-9A-Za-z.+~_-]+ without a leading v, got: {version}");
    }
    Ok(())
}

fn validate_download_repo(value: &str) -> Result<()> {
    if value.split('/').count() != 2 || value.split('/').any(str::is_empty) {
        bail!("winget.download_repo must be OWNER/REPO");
    }
    Ok(())
}

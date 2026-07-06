use std::collections::BTreeMap;

use crate::cargo;
use crate::cli::{
    ChocolateyBumpArgs, ScoopBumpArgs, WindowsAction, WindowsCommand, WindowsPublishArgs,
    WingetSubmitArgs,
};
use crate::config::ProjectConfig;
use anyhow::{Context, Result, bail};

pub fn run(command: WindowsCommand) -> Result<()> {
    match command.action {
        WindowsAction::Publish(args) => publish(args),
    }
}

fn publish(args: WindowsPublishArgs) -> Result<()> {
    validate_version(&args.version)?;
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let cfg = ProjectConfig::load(workspace_root)?;
    let archives = parse_archives(&args.archive)?;
    let x64 = archives
        .get("x64")
        .context("missing --archive x64=PATH_OR_URL")?;

    let selected = SelectedChannels {
        chocolatey: args.all
            || args.chocolatey
            || no_channel_flags(&args) && cfg.chocolatey.is_some(),
        scoop: args.all || args.scoop || no_channel_flags(&args) && cfg.scoop.is_some(),
        winget: args.all || args.winget || no_channel_flags(&args) && cfg.winget.is_some(),
    };
    if !selected.any() {
        bail!("no Windows channels selected or configured");
    }
    if (selected.chocolatey || selected.scoop) && looks_like_url(x64) {
        bail!("Chocolatey and Scoop require a local x64 archive path for checksum computation");
    }

    if selected.chocolatey {
        let package_dir = args.work_dir.join("chocolatey-package");
        crate::commands::chocolatey::bump(ChocolateyBumpArgs {
            version: args.version.clone(),
            package_dir,
            archive: vec![format!("x64={x64}")],
            push: !args.dry_run,
            push_source: args.choco_push_source.clone(),
            api_key_env: None,
            force_resubmit: args.force_resubmit,
            dry_run: args.dry_run,
            chocolatey: Default::default(),
        })?;
    }
    if selected.scoop {
        crate::commands::scoop::bump(ScoopBumpArgs {
            version: args.version.clone(),
            bucket: None,
            bucket_url: args.scoop_bucket_url.clone(),
            bucket_token_env: args.scoop_bucket_token_env.clone(),
            work_dir: Some(args.work_dir.join("scoop-bucket")),
            archive: vec![format!("x64={x64}")],
            push: !args.dry_run,
            commit_message: None,
            dry_run: args.dry_run,
            scoop: Default::default(),
        })?;
    }
    if selected.winget {
        let url = looks_like_url(x64).then(|| x64.clone());
        crate::commands::winget::submit(WingetSubmitArgs {
            version: args.version,
            package_id: None,
            download_repo: None,
            zip_archive: None,
            url,
            token_env: None,
            wingetcreate: None,
            wingetcreate_url: None,
            work_dir: Some(args.work_dir.join("winget")),
            wine: "wine".to_owned(),
            dry_run: args.dry_run,
        })?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct SelectedChannels {
    chocolatey: bool,
    scoop: bool,
    winget: bool,
}

impl SelectedChannels {
    fn any(self) -> bool {
        self.chocolatey || self.scoop || self.winget
    }
}

fn no_channel_flags(args: &WindowsPublishArgs) -> bool {
    !args.all && !args.chocolatey && !args.scoop && !args.winget
}

fn parse_archives(values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut archives = BTreeMap::new();
    for value in values {
        let (arch, path) = value
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--archive must be ARCH=PATH_OR_URL"))?;
        if arch.is_empty() || path.is_empty() {
            bail!("--archive must be ARCH=PATH_OR_URL");
        }
        if archives.insert(arch.to_owned(), path.to_owned()).is_some() {
            bail!("duplicate archive for {arch}");
        }
    }
    Ok(archives)
}

fn looks_like_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
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

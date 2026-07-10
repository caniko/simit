use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::{ChocolateyAction, ChocolateyBumpArgs, ChocolateyCommand, ChocolateyRenderArgs};
use crate::config::{ProjectConfig, ResolvedChocolatey};
use crate::registry::{self, FeatureStatus};
use crate::render::chocolatey_nuspec::{
    self, Architecture, ChocolateyRenderOptions, RenderedPackage, Sha256Set,
};
use crate::sha256;

pub fn run(command: ChocolateyCommand) -> Result<()> {
    match command.action {
        ChocolateyAction::Render(args) => render(args),
        ChocolateyAction::Bump(args) => bump(args),
    }
}

fn render(args: ChocolateyRenderArgs) -> Result<()> {
    let (resolved, package_version) = resolve(args.chocolatey.as_overrides())?;
    let version = args.version.as_deref().unwrap_or(&package_version);
    validate_version(version)?;
    validate_resolved(&resolved)?;
    validate_download_repo(&resolved.download_repo)?;

    let package = render_package(&resolved, version, false, &Sha256Set::all_no_check());
    if let Some(output_dir) = args.output_dir {
        write_package(output_dir.as_std_path(), &package)?;
    } else {
        print_package(&package);
    }
    Ok(())
}

pub(crate) fn bump(args: ChocolateyBumpArgs) -> Result<()> {
    validate_version(&args.version)?;
    let mut overrides = args.chocolatey.as_overrides();
    if let Some(source) = args.push_source.as_deref() {
        overrides.push_source = Some(source);
    }
    let (resolved, _) = resolve(overrides)?;
    validate_resolved(&resolved)?;
    validate_download_repo(&resolved.download_repo)?;

    let archives = parse_archives(&args.archive)?;
    let include_x86 = archives.contains_key(&Architecture::X86);
    let sha256s = sha256_set(&archives)?;
    let package = render_package(&resolved, &args.version, include_x86, &sha256s);
    let package_dir = args.package_dir.as_std_path();
    if args.dry_run {
        println!(
            "would write Chocolatey package {} {} to {}",
            resolved.id,
            args.version,
            package_dir.display()
        );
        if args.push {
            println!(
                "would push Chocolatey package to {} using ${}",
                resolved.push.source,
                args.api_key_env
                    .as_deref()
                    .unwrap_or(resolved.api_key_env.as_str())
            );
        }
        return Ok(());
    }
    write_package(package_dir, &package)?;

    if args.push {
        let api_key_env = args
            .api_key_env
            .as_deref()
            .or(Some(resolved.api_key_env.as_str()))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "--push requires --api-key-env ENV naming the Chocolatey API key variable"
                )
            })?;
        let api_key = std::env::var(api_key_env)
            .with_context(|| format!("reading Chocolatey API key from ${api_key_env}"))?;
        if api_key.is_empty() {
            bail!("Chocolatey API key environment variable ${api_key_env} is empty");
        }
        if !args.force_resubmit && package_version_exists(&resolved.id, &args.version)? {
            println!(
                "already-current: Chocolatey {} {}",
                resolved.id, args.version
            );
            registry::touch_current_project_or_warn([("chocolatey", FeatureStatus::Managed)]);
            return Ok(());
        }
        pack_and_push(package_dir, &resolved.push.source, &api_key)?;
    }
    registry::touch_current_project_or_warn([("chocolatey", FeatureStatus::Managed)]);
    Ok(())
}

pub(crate) fn resolve(
    overrides: crate::config::ChocolateyOverrides<'_>,
) -> Result<(ResolvedChocolatey, String)> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let package = cargo::representative_package(&metadata, None)?;
    let cfg = ProjectConfig::load(workspace_root)?;
    let resolved = cfg.resolve_chocolatey(overrides, &package)?;
    Ok((resolved, package.version))
}

pub(crate) fn render_package(
    resolved: &ResolvedChocolatey,
    version: &str,
    include_x86: bool,
    sha256s: &Sha256Set,
) -> RenderedPackage {
    chocolatey_nuspec::render_package(
        &ChocolateyRenderOptions {
            resolved,
            version,
            include_x86,
        },
        sha256s,
    )
}

pub(crate) fn write_package(target: &Path, package: &RenderedPackage) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    let tools = target.join("tools");
    fs::create_dir_all(&tools).with_context(|| format!("creating {}", tools.display()))?;
    fs::write(target.join(&package.nuspec_name), &package.nuspec)
        .with_context(|| format!("writing {}", target.join(&package.nuspec_name).display()))?;
    fs::write(tools.join("chocolateyInstall.ps1"), &package.install_script)
        .with_context(|| format!("writing {}", tools.join("chocolateyInstall.ps1").display()))?;
    fs::write(
        tools.join("chocolateyUninstall.ps1"),
        &package.uninstall_script,
    )
    .with_context(|| {
        format!(
            "writing {}",
            tools.join("chocolateyUninstall.ps1").display()
        )
    })?;
    Ok(())
}

pub(crate) fn print_package(package: &RenderedPackage) {
    println!("==> {}", package.nuspec_name);
    print!("{}", package.nuspec);
    println!("==> tools/chocolateyInstall.ps1");
    print!("{}", package.install_script);
    println!("==> tools/chocolateyUninstall.ps1");
    print!("{}", package.uninstall_script);
}

fn parse_archives(values: &[String]) -> Result<BTreeMap<Architecture, PathBuf>> {
    let mut archives = BTreeMap::new();
    for value in values {
        let (arch, path) = value
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--archive must be ARCH=PATH"))?;
        let arch = Architecture::parse(arch).map_err(anyhow::Error::msg)?;
        if path.is_empty() {
            bail!("archive path for {} must not be empty", arch.key());
        }
        let path = PathBuf::from(path);
        if !path.is_file() {
            bail!("archive path does not exist: {}", path.display());
        }
        if archives.insert(arch, path).is_some() {
            bail!("duplicate archive for {}", arch.key());
        }
    }
    Ok(archives)
}

fn sha256_set(archives: &BTreeMap<Architecture, PathBuf>) -> Result<Sha256Set> {
    let x64 = archives
        .get(&Architecture::X64)
        .ok_or_else(|| anyhow::anyhow!("missing --archive for x64"))?;
    let mut sha256s = Sha256Set::default();
    sha256s.set(Architecture::X64, sha256::sha256_of_file(x64)?);
    if let Some(x86) = archives.get(&Architecture::X86) {
        sha256s.set(Architecture::X86, sha256::sha256_of_file(x86)?);
    }
    Ok(sha256s)
}

fn pack_and_push(package_dir: &Path, source: &str, api_key: &str) -> Result<()> {
    let package_dir = package_dir
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", package_dir.display()))?;
    let nuspec = package_nuspec(&package_dir)?;
    let pack = Command::new("choco")
        .arg("pack")
        .arg(&nuspec)
        .arg("--output-directory")
        .arg(&package_dir)
        .current_dir(&package_dir)
        .status()
        .with_context(choco_missing_context)?;
    if !pack.success() {
        bail!("choco pack failed");
    }

    let nupkg = newest_nupkg(&package_dir)?;
    let push = Command::new("choco")
        .arg("push")
        .arg(&nupkg)
        .arg("--source")
        .arg(source)
        .arg("--api-key")
        .arg(api_key)
        .current_dir(&package_dir)
        .status()
        .with_context(choco_missing_context)?;
    if !push.success() {
        bail!("choco push failed; Chocolatey rejects duplicate version pushes");
    }
    Ok(())
}

pub(crate) fn package_version_exists(id: &str, version: &str) -> Result<bool> {
    let filter = format!(
        "Id eq '{}' and Version eq '{}'",
        odata_quote(id),
        odata_quote(version)
    );
    let url = format!(
        "https://community.chocolatey.org/api/v2/Packages()?%24filter={}",
        percent_encode(&filter)
    );
    let output = Command::new("curl")
        .arg("-fsSL")
        .arg(&url)
        .output()
        .with_context(|| "checking Chocolatey package version with curl")?;
    if !output.status.success() {
        bail!("Chocolatey package version check failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).contains("<entry>"))
}

fn odata_quote(value: &str) -> String {
    value.replace('\'', "''")
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn package_nuspec(package_dir: &Path) -> Result<PathBuf> {
    let mut nuspecs = fs::read_dir(package_dir)
        .with_context(|| format!("reading {}", package_dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|ext| ext.to_str()) == Some("nuspec")).then_some(path)
        })
        .collect::<Vec<_>>();
    nuspecs.sort();
    match nuspecs.as_slice() {
        [nuspec] => nuspec
            .canonicalize()
            .with_context(|| format!("canonicalizing {}", nuspec.display())),
        [] => bail!("no .nuspec file found in {}", package_dir.display()),
        _ => bail!("multiple .nuspec files found in {}", package_dir.display()),
    }
}

fn newest_nupkg(package_dir: &Path) -> Result<PathBuf> {
    let mut packages = fs::read_dir(package_dir)
        .with_context(|| format!("reading {}", package_dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|ext| ext.to_str()) == Some("nupkg")).then_some(path)
        })
        .collect::<Vec<_>>();
    packages.sort();
    packages
        .pop()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "choco pack did not create a .nupkg in {}",
                package_dir.display()
            )
        })?
        .canonicalize()
        .with_context(|| format!("canonicalizing newest .nupkg in {}", package_dir.display()))
}

fn choco_missing_context() -> String {
    "running choco; install Chocolatey and ensure `choco` is on PATH for --push".to_owned()
}

pub(crate) fn validate_resolved(resolved: &ResolvedChocolatey) -> Result<()> {
    if resolved.authors.as_deref().is_none_or(str::is_empty) {
        bail!(
            "chocolatey.authors not set: provide it via --choco-authors, simit project config [chocolatey].authors, or Cargo.toml package.authors"
        );
    }
    Ok(())
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
        bail!("chocolatey.download_repo must be OWNER/REPO");
    }
    Ok(())
}

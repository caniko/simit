use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use camino::Utf8PathBuf;

use crate::config::ProjectConfig;
use crate::project::GeneratedFile;

#[derive(Debug, Clone, Default)]
pub struct TrustOverrides {
    pub key: Option<String>,
    pub trust_root: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone)]
pub struct TrustResolution {
    pub key: String,
    pub trust_root: PathBuf,
    pub public_key: String,
    pub source: TrustSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustSource {
    Environment,
    Override,
    ProjectConfig,
    GitConfig,
}

impl TrustSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Environment => "SIMIT_MAINTAINERS_GPG",
            Self::Override => "CLI override",
            Self::ProjectConfig => "project config",
            Self::GitConfig => "git config user.signingkey",
        }
    }
}

pub fn generated_file(
    workspace_root: &Path,
    config: &ProjectConfig,
    overrides: &TrustOverrides,
    check: bool,
) -> Result<GeneratedFile> {
    let trust_root = trust_root_path(config, overrides)?;
    let content = match resolve(workspace_root, config, overrides) {
        Ok(resolved) => resolved.public_key,
        Err(err) if check && workspace_root.join(&trust_root).exists() => {
            let content = read_and_validate_existing(workspace_root, &trust_root)?;
            eprintln!(
                "simit: using existing {} for check because local signing key is unavailable: {err:#}",
                trust_root.display()
            );
            content
        }
        Err(err) => return Err(err),
    };

    Ok(GeneratedFile {
        relative_path: trust_root,
        content,
    })
}

pub fn resolve(
    workspace_root: &Path,
    config: &ProjectConfig,
    overrides: &TrustOverrides,
) -> Result<TrustResolution> {
    let trust_root = trust_root_path(config, overrides)?;

    if let Some(path) = env::var_os("SIMIT_MAINTAINERS_GPG") {
        let path = Path::new(&path);
        let public_key =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        validate_public_key_text(&public_key, "SIMIT_MAINTAINERS_GPG")?;
        return Ok(TrustResolution {
            key: "environment".to_owned(),
            trust_root,
            public_key,
            source: TrustSource::Environment,
        });
    }

    let (key, source) = if let Some(key) = &overrides.key {
        (key.clone(), TrustSource::Override)
    } else if let Some(key) = &config.release.signing.key {
        (key.clone(), TrustSource::ProjectConfig)
    } else if let Some(key) = git_signing_key(workspace_root)? {
        (key, TrustSource::GitConfig)
    } else if config.release.signing.required {
        bail!(
            "release signing key not configured; set [release.signing].key in simit.toml or Cargo metadata, run `git config user.signingkey <fingerprint>`, or pass --key/--maintainer-key"
        );
    } else {
        bail!("release signing is disabled by project config");
    };

    let public_key = export_public_key(&key)?;
    Ok(TrustResolution {
        key,
        trust_root,
        public_key,
        source,
    })
}

pub fn init(
    workspace_root: &Path,
    config: &ProjectConfig,
    overrides: &TrustOverrides,
) -> Result<()> {
    let resolved = resolve(workspace_root, config, overrides)?;
    let path = workspace_root.join(&resolved.trust_root);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("trust-root path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    fs::write(&path, resolved.public_key).with_context(|| format!("writing {}", path.display()))?;
    println!(
        "wrote {} from {}",
        resolved.trust_root.display(),
        resolved.source.label()
    );
    Ok(())
}

pub fn check(
    workspace_root: &Path,
    config: &ProjectConfig,
    overrides: &TrustOverrides,
) -> Result<()> {
    let trust_root = trust_root_path(config, overrides)?;
    let existing = read_and_validate_existing(workspace_root, &trust_root)?;

    match resolve(workspace_root, config, overrides) {
        Ok(resolved) if existing == resolved.public_key => {
            println!(
                "{} matches {}",
                trust_root.display(),
                resolved.source.label()
            );
            Ok(())
        }
        Ok(_) => bail!(
            "{} does not match the configured release signing key; run `simit release trust init`",
            trust_root.display()
        ),
        Err(err) => {
            println!("{} is present and parseable", trust_root.display());
            println!("could not compare against local signing key: {err:#}");
            Ok(())
        }
    }
}

pub fn check_quiet(
    workspace_root: &Path,
    config: &ProjectConfig,
    overrides: &TrustOverrides,
) -> Result<()> {
    let trust_root = trust_root_path(config, overrides)?;
    let existing = read_and_validate_existing(workspace_root, &trust_root)?;

    match resolve(workspace_root, config, overrides) {
        Ok(resolved) if existing == resolved.public_key => Ok(()),
        Ok(_) => bail!(
            "{} does not match the configured release signing key; run `simit release trust init`",
            trust_root.display()
        ),
        Err(_) => Ok(()),
    }
}

pub fn status(
    workspace_root: &Path,
    config: &ProjectConfig,
    overrides: &TrustOverrides,
) -> Result<()> {
    let trust_root = trust_root_path(config, overrides)?;
    println!("trust root: {}", trust_root.display());

    match resolve(workspace_root, config, overrides) {
        Ok(resolved) => {
            println!("signing key: {}", resolved.key);
            println!("source: {}", resolved.source.label());
        }
        Err(err) => {
            println!("signing key: unavailable");
            println!("reason: {err:#}");
        }
    }

    let path = workspace_root.join(&trust_root);
    if path.exists() {
        validate_public_key_file(&path)?;
        println!("trust root state: present");
    } else {
        println!("trust root state: missing");
    }

    Ok(())
}

fn trust_root_path(config: &ProjectConfig, overrides: &TrustOverrides) -> Result<PathBuf> {
    let trust_root = overrides
        .trust_root
        .as_ref()
        .map(|path| PathBuf::from(path.as_str()))
        .unwrap_or_else(|| PathBuf::from(&config.release.signing.trust_root));
    if trust_root.is_absolute() {
        bail!(
            "release trust-root path must be repository-relative: {}",
            trust_root.display()
        );
    }
    Ok(trust_root)
}

fn read_and_validate_existing(workspace_root: &Path, trust_root: &Path) -> Result<String> {
    let path = workspace_root.join(trust_root);
    let content =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    validate_public_key_text(&content, &path.display().to_string())?;
    Ok(content)
}

fn validate_public_key_file(path: &Path) -> Result<()> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    validate_public_key_text(&content, &path.display().to_string())
}

fn validate_public_key_text(content: &str, label: &str) -> Result<()> {
    if content.trim().is_empty() {
        bail!("{label} is empty");
    }
    if !content.contains("-----BEGIN PGP PUBLIC KEY BLOCK-----") {
        bail!("{label} is not an armored OpenPGP public key");
    }
    let gpg_home = temporary_gpg_home()?;
    let mut child = Command::new("gpg")
        .args(["--import-options", "show-only", "--import"])
        .env("GNUPGHOME", &gpg_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("validating OpenPGP public key {label}"))?;
    {
        let stdin = child.stdin.as_mut().expect("stdin was piped");
        stdin
            .write_all(content.as_bytes())
            .with_context(|| format!("feeding OpenPGP public key {label} to gpg"))?;
    }
    drop(child.stdin.take());
    let status = child
        .wait()
        .with_context(|| format!("validating OpenPGP public key {label}"))?;
    let _ = fs::remove_dir_all(&gpg_home);
    if !status.success() {
        bail!("{label} is not parseable by gpg");
    }
    Ok(())
}

fn temporary_gpg_home() -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?
        .as_nanos();
    let path = env::temp_dir().join(format!("simit-gpg-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path).with_context(|| format!("creating {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("setting permissions on {}", path.display()))?;
    }
    Ok(path)
}

fn git_signing_key(workspace_root: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .current_dir(workspace_root)
        .args(["config", "--get", "user.signingkey"])
        .output()
        .context("checking git signing key")?;
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }
    let key = String::from_utf8(output.stdout)
        .context("git user.signingkey was not valid UTF-8")?
        .trim()
        .to_owned();
    Ok((!key.is_empty()).then_some(key))
}

fn export_public_key(key: &str) -> Result<String> {
    let output = Command::new("gpg")
        .args(["--armor", "--export", key])
        .output()
        .with_context(|| format!("exporting OpenPGP public key {key}"))?;
    if !output.status.success() || output.stdout.is_empty() {
        bail!("could not export OpenPGP public key {key} with `gpg --armor --export {key}`");
    }
    let content = String::from_utf8(output.stdout)
        .with_context(|| format!("exported OpenPGP public key {key} was not valid UTF-8"))?;
    validate_public_key_text(&content, key)?;
    Ok(content)
}

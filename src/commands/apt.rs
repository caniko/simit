use std::env;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::{
    AptAction, AptBuildArgs, AptCommand, AptPublishArgs, AptRenderArgs, AptVerifyArgs,
};
use crate::commands::scaffold::run_git;
use crate::config::{AptOverrides, ProjectConfig, ResolvedApt};
use crate::git;
use crate::render::apt_conf;

pub const DISTRIBUTIONS_PATH: &str = "dist/apt/conf/distributions";

pub fn run(command: AptCommand) -> Result<()> {
    match command.action {
        AptAction::Render(args) => render(args),
        AptAction::Build(args) => build(args),
        AptAction::Publish(args) => publish(args),
        AptAction::Verify(args) => verify(args),
    }
}

fn render(args: AptRenderArgs) -> Result<()> {
    let resolved = resolve(args.apt.as_overrides(), args.package.as_deref())?;
    print!("{}", apt_conf::render_distributions(&resolved));
    Ok(())
}

fn build(args: AptBuildArgs) -> Result<()> {
    validate_version(&args.version)?;
    let resolved = resolve(args.apt.as_overrides(), args.package.as_deref())?;
    let architecture = host_architecture(&resolved)?;
    let release_dir = args.release_dir.as_std_path();
    fs::create_dir_all(release_dir)
        .with_context(|| format!("creating release directory {}", release_dir.display()))?;

    for package in &resolved.packages {
        let (cargo_package, deb_name) = parse_package_spec(package)?;
        let output = release_dir.join(format!("{deb_name}_{}_{}.deb", args.version, architecture));
        let mut command = Command::new("cargo");
        command
            .args(["deb", "--locked", "--package", cargo_package, "--output"])
            .arg(&output);
        run_command(
            &mut command,
            &format!("building Debian package {cargo_package}"),
        )?;
        validate_deb(&output, deb_name, &args.version, architecture)?;
        println!("built: {}", output.display());
    }
    Ok(())
}

fn publish(args: AptPublishArgs) -> Result<()> {
    validate_version(&args.version)?;
    let resolved = resolve(args.apt.as_overrides(), args.package.as_deref())?;
    let release_dir = args.release_dir.as_std_path();
    let debs = release_debs(release_dir, &resolved, &args.version)?;
    if args.dry_run {
        println!(
            "would publish {} Debian package(s) for {} to {}",
            debs.len(),
            args.version,
            args.repo
                .as_deref()
                .map_or_else(|| resolved.repo_url.clone(), ToString::to_string)
        );
        return Ok(());
    }

    let workspace_root = cargo::metadata_for_current_dir()?.workspace_root;
    let public_key = workspace_root.as_std_path().join("dist/apt/key.gpg.asc");
    if !public_key.is_file() {
        bail!(
            "missing {}; required producer: export the apt signing public key; validation: test -s {}",
            public_key.display(),
            public_key.display()
        );
    }

    let temp = TempDir::new("simit-apt")?;
    let ssh_command = prepare_ssh(&temp, &resolved)?;
    let repo = if let Some(repo) = args.repo {
        let repo = repo.as_std_path().to_path_buf();
        ensure_repo_checkout(&repo)?;
        repo
    } else {
        if args.work_dir.exists() {
            fs::remove_dir_all(args.work_dir.as_std_path()).with_context(|| {
                format!("removing previous apt work directory {}", args.work_dir)
            })?;
        }
        if let Some(parent) = args.work_dir.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating apt work directory parent {parent}"))?;
        }
        let mut command = Command::new("git");
        command
            .args(["clone", "--depth", "1", "--branch", &resolved.branch])
            .arg(&resolved.repo_url)
            .arg(args.work_dir.as_std_path());
        if let Some(ssh_command) = &ssh_command {
            command.env("GIT_SSH_COMMAND", ssh_command);
        }
        run_command(&mut command, "cloning apt repository")?;
        args.work_dir.as_std_path().to_path_buf()
    };

    let dirty = git::output(&repo, &["status", "--porcelain"])?;
    if !dirty.trim().is_empty() {
        bail!(
            "apt repository {} has unrelated working-tree changes; publish into a clean checkout",
            repo.display()
        );
    }

    let gpg = prepare_gpg(&temp, &resolved)?;
    let conf = repo.join("conf/distributions");
    if !conf.is_file() {
        bail!(
            "missing {}; run `simit init apt-repo --target {}` first",
            conf.display(),
            repo.display()
        );
    }
    let mut reprepro = Command::new("reprepro");
    reprepro
        .env("GNUPGHOME", &gpg.home)
        .current_dir(&repo)
        .args([
            "-b",
            ".",
            "--keepunreferencedfiles",
            "includedeb",
            &resolved.distribution,
        ]);
    for deb in &debs {
        reprepro.arg(deb);
    }
    run_command(&mut reprepro, "updating apt repository with reprepro")?;
    fs::copy(&public_key, repo.join("key.gpg.asc"))
        .with_context(|| format!("publishing {}", public_key.display()))?;

    run_git(
        &repo,
        &[
            "add",
            "--",
            "conf/distributions",
            "dists",
            "pool",
            "key.gpg.asc",
        ],
    )?;
    let staged = git::output(
        &repo,
        &[
            "status",
            "--porcelain",
            "--",
            "conf/distributions",
            "dists",
            "pool",
            "key.gpg.asc",
        ],
    )?;
    if staged.trim().is_empty() {
        println!(
            "apt: no changes for {}; repository is current",
            args.version
        );
        return Ok(());
    }
    run_git(
        &repo,
        &[
            "-c",
            "user.name=release bot",
            "-c",
            "user.email=release-bot@localhost",
            "commit",
            "-m",
            &format!("apt: publish {}", args.version),
        ],
    )?;
    if args.push {
        let mut push = Command::new("git");
        push.current_dir(&repo).args([
            "push",
            "origin",
            &format!("HEAD:refs/heads/{}", resolved.branch),
        ]);
        if let Some(ssh_command) = &ssh_command {
            push.env("GIT_SSH_COMMAND", ssh_command);
        }
        run_command(&mut push, "pushing apt repository")?;
    }
    Ok(())
}

fn verify(args: AptVerifyArgs) -> Result<()> {
    let resolved = resolve(args.apt.as_overrides(), None)?;
    ensure_repo_checkout(args.repo.as_std_path())?;
    let repo = args.repo.as_std_path();
    let release = repo
        .join("dists")
        .join(&resolved.distribution)
        .join("Release");
    if !release.is_file() {
        bail!("missing apt Release file {}", release.display());
    }
    if !repo.join("key.gpg.asc").is_file() {
        bail!(
            "missing apt public key {}",
            repo.join("key.gpg.asc").display()
        );
    }
    for architecture in resolved.architectures.split_whitespace() {
        for component in resolved.components.split_whitespace() {
            let package_index = repo
                .join("dists")
                .join(&resolved.distribution)
                .join(component)
                .join(format!("binary-{architecture}/Packages"));
            if !package_index.is_file() {
                bail!("missing apt package index {}", package_index.display());
            }
            if let Some(version) = &args.version {
                let expected = format!("Version: {version}-1");
                let contents = fs::read_to_string(&package_index)
                    .with_context(|| format!("reading {}", package_index.display()))?;
                if !contents.lines().any(|line| line.trim() == expected) {
                    bail!(
                        "apt package index {} does not contain {}",
                        package_index.display(),
                        expected
                    );
                }
            }
        }
    }
    println!("verified apt repository {}", repo.display());
    Ok(())
}

pub(crate) fn resolve(overrides: AptOverrides<'_>, package: Option<&str>) -> Result<ResolvedApt> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let pkg = cargo::representative_package(&metadata, package)?;
    let cfg = ProjectConfig::load(workspace_root)?;
    cfg.resolve_apt(overrides, &pkg)
}

pub(crate) fn write_distributions(workspace_root: &Path, resolved: &ResolvedApt) -> Result<()> {
    let path = workspace_root.join(DISTRIBUTIONS_PATH);
    let parent = path.parent().expect("distributions path has a parent");
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    fs::write(&path, apt_conf::render_distributions(resolved))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn release_debs(release_dir: &Path, apt: &ResolvedApt, version: &str) -> Result<Vec<PathBuf>> {
    if !release_dir.is_dir() {
        bail!(
            "release directory does not exist: {}",
            release_dir.display()
        );
    }
    let architecture = host_architecture(apt)?;
    let mut debs = Vec::new();
    for package in &apt.packages {
        let (_, deb_name) = parse_package_spec(package)?;
        let path = release_dir.join(format!("{deb_name}_{version}_{architecture}.deb"));
        if !path.is_file() {
            bail!(
                "missing Debian package {}; run `simit dist apt build --version {version}` first",
                path.display()
            );
        }
        validate_deb(&path, deb_name, version, architecture)?;
        debs.push(path);
    }
    Ok(debs)
}

fn parse_package_spec(spec: &str) -> Result<(&str, &str)> {
    let (cargo_package, deb_name) = spec.split_once('=').map_or((spec, spec), |(a, b)| (a, b));
    if cargo_package.is_empty() || deb_name.is_empty() || !valid_deb_name(deb_name) {
        bail!("apt package must be CARGO_PACKAGE=DEB_NAME with a valid Debian name, got `{spec}`");
    }
    Ok((cargo_package, deb_name))
}

fn valid_deb_name(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
}

fn host_architecture(apt: &ResolvedApt) -> Result<&'static str> {
    let architecture = match env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => bail!("unsupported Debian package architecture {other}"),
    };
    if !apt
        .architectures
        .split_whitespace()
        .any(|value| value == architecture)
    {
        bail!(
            "host architecture {architecture} is not declared by [apt].architectures = `{}`",
            apt.architectures
        );
    }
    Ok(architecture)
}

fn validate_version(version: &str) -> Result<()> {
    if version.is_empty()
        || version.starts_with('v')
        || !version
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '+' | '~' | '_' | '-'))
    {
        bail!("version must match [0-9A-Za-z.+~_-]+ without a leading v, got: {version}");
    }
    Ok(())
}

fn validate_deb(path: &Path, deb_name: &str, version: &str, architecture: &str) -> Result<()> {
    let output = command_output(
        Command::new("dpkg-deb").args(["--field"]).arg(path).args([
            "Package",
            "Version",
            "Architecture",
        ]),
        &format!("reading Debian metadata from {}", path.display()),
    )?;
    let fields = output
        .stdout
        .iter()
        .map(|line| {
            line.split_once(':')
                .map_or(line.as_str(), |(_, value)| value.trim())
        })
        .collect::<Vec<_>>();
    let expected_version = format!("{version}-1");
    if fields.as_slice() != [deb_name, expected_version.as_str(), architecture] {
        bail!(
            "Debian metadata mismatch for {}: expected Package={} Version={} Architecture={}, got {}",
            path.display(),
            deb_name,
            expected_version,
            architecture,
            fields.join(", ")
        );
    }
    Ok(())
}

fn ensure_repo_checkout(repo: &Path) -> Result<()> {
    if !repo.is_dir() || !repo.join(".git").is_dir() {
        bail!(
            "apt repository checkout is not a git repository: {}",
            repo.display()
        );
    }
    Ok(())
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let base = env::temp_dir();
        for attempt in 0..100u32 {
            let path = base.join(format!("{prefix}-{}-{stamp}-{attempt}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    set_private_dir(&path)?;
                    return Ok(Self { path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    return Err(err).with_context(|| format!("creating {}", path.display()));
                }
            }
        }
        bail!("could not allocate a temporary directory for apt publication")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct GpgContext {
    home: PathBuf,
}

fn prepare_gpg(temp: &TempDir, apt: &ResolvedApt) -> Result<GpgContext> {
    let key = configured_or_standard_env(&apt.gpg_key_secret, "APT_REPO_GPG_KEY")
        .with_context(|| format!("environment variable ${} is unset", apt.gpg_key_secret))?;
    if key.is_empty() {
        bail!("environment variable ${} is empty", apt.gpg_key_secret);
    }
    let key_id = configured_or_standard_env(&apt.gpg_key_id_secret, "APT_REPO_GPG_KEY_ID")
        .with_context(|| format!("environment variable ${} is unset", apt.gpg_key_id_secret))?;
    if key_id.is_empty() {
        bail!("environment variable ${} is empty", apt.gpg_key_id_secret);
    }
    let passphrase =
        configured_or_standard_env(&apt.gpg_passphrase_secret, "APT_REPO_GPG_PASSPHRASE")
            .unwrap_or_default();
    let home = temp.path.join("gpg");
    fs::create_dir(&home).with_context(|| format!("creating {}", home.display()))?;
    set_private_dir(&home)?;
    let key_file = write_private(&temp.path.join("apt-key.asc"), key.as_bytes())?;
    let pass_file = write_private(&temp.path.join("apt-passphrase"), passphrase.as_bytes())?;
    let mut import = Command::new("gpg");
    import
        .env("GNUPGHOME", &home)
        .args([
            "--batch",
            "--pinentry-mode",
            "loopback",
            "--passphrase-file",
        ])
        .arg(&pass_file)
        .args(["--import"])
        .arg(&key_file);
    run_command(&mut import, "importing apt signing key")?;
    let mut trust = Command::new("gpg");
    trust
        .env("GNUPGHOME", &home)
        .args(["--batch", "--import-ownertrust"])
        .stdin(Stdio::piped());
    let mut child = trust.spawn().context("starting gpg ownertrust import")?;
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        writeln!(stdin, "{key_id}:6:").context("writing gpg ownertrust")?;
    }
    let status = child.wait().context("waiting for gpg ownertrust import")?;
    if !status.success() {
        bail!("gpg ownertrust import failed");
    }
    Ok(GpgContext { home })
}

fn prepare_ssh(temp: &TempDir, apt: &ResolvedApt) -> Result<Option<String>> {
    let Ok(key) = configured_or_standard_env(&apt.ssh_key_secret, "APT_REPO_SSH_KEY") else {
        return Ok(None);
    };
    if key.is_empty() {
        return Ok(None);
    }
    let key_path = write_private(&temp.path.join("apt-ssh-key"), key.as_bytes())?;
    Ok(Some(format!(
        "ssh -i {} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new",
        key_path.display()
    )))
}

fn configured_or_standard_env(configured: &str, standard: &str) -> Result<String> {
    match env::var(standard) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Ok(env::var(configured)?),
    }
}

fn write_private(path: &Path, contents: &[u8]) -> Result<PathBuf> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .with_context(|| format!("creating private temporary file {}", path.display()))?;
    use std::io::Write;
    file.write_all(contents)
        .with_context(|| format!("writing private temporary file {}", path.display()))?;
    set_private(path)?;
    Ok(path.to_path_buf())
}

#[cfg(unix)]
fn set_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting permissions on {}", path.display()))
}

#[cfg(unix)]
fn set_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("restricting permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn set_private_dir(_path: &Path) -> Result<()> {
    Ok(())
}

fn run_command(command: &mut Command, description: &str) -> Result<()> {
    let status = command.status().with_context(|| description.to_owned())?;
    if !status.success() {
        bail!("{description} failed with {status}");
    }
    Ok(())
}

fn command_output(command: &mut Command, description: &str) -> Result<PackageOutput> {
    let output = command.output().with_context(|| description.to_owned())?;
    if !output.status.success() {
        bail!(
            "{description} failed:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(PackageOutput {
        stdout: String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect(),
    })
}

struct PackageOutput {
    stdout: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_spec_defaults_deb_name_to_cargo_name() {
        assert_eq!(parse_package_spec("modde").unwrap(), ("modde", "modde"));
        assert_eq!(
            parse_package_spec("modde-ui=modde-ui").unwrap(),
            ("modde-ui", "modde-ui")
        );
    }

    #[test]
    fn package_spec_rejects_path_like_names() {
        assert!(parse_package_spec("modde=../modde").is_err());
    }
}

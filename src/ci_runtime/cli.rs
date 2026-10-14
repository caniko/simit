//! Thin CLI adapters for versioned CI runtime contracts.
use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use std::path::PathBuf;
#[path = "cli_git.rs"]
mod git;

/// CI execution commands. Receipts are evidence, never standalone acceptance.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Execute every enrolled Nix member on one worker and retain per-member evidence.
    NixGroup(GroupArgs),
    /// Capture enrolled original GitHub producers with bounded read-only observation.
    ObserveOriginal(ObserveArgs),
    /// Revalidate exact uploaded custody, provider digest, members and retention.
    VerifyOriginalRetention(RetentionArgs),
}

/// Source-enrolled custody capture inputs.
#[derive(Debug, Args)]
pub struct ObserveArgs {
    /// Versioned source-owned JSON enrollment with exact required job names.
    #[arg(long)]
    pub spec: PathBuf,
    /// Fresh evidence directory; original packets are never reused or overwritten.
    #[arg(long)]
    pub results: PathBuf,
}

/// Independently supplied inputs for native uploaded-custody verification.
#[derive(Debug, Args)]
pub struct RetentionArgs {
    /// Versioned source-owned JSON enrollment.
    #[arg(long)]
    pub spec: PathBuf,
    /// Original local captured packet.
    #[arg(long)]
    pub results: PathBuf,
    /// Native upload action artifact id.
    #[arg(long)]
    pub artifact_id: u64,
    /// Native upload action SHA-256 digest.
    #[arg(long)]
    pub artifact_digest: String,
}

#[cfg(windows)]
#[path = "cli_windows.rs"]
mod windows;

struct CliCancellation {
    token: super::cancellation::Cancellation,
}
#[cfg(windows)]
impl Drop for CliCancellation {
    fn drop(&mut self) {
        windows::finished();
    }
}

fn cancellation() -> Result<CliCancellation> {
    let cancellation = super::cancellation::Cancellation::default();
    #[cfg(unix)]
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, cancellation.0.clone())?;
    }
    #[cfg(windows)]
    windows::register(cancellation.clone())?;
    Ok(CliCancellation {
        token: cancellation,
    })
}

fn observer_inputs(
    path: &std::path::Path,
) -> Result<(
    super::observer::Spec,
    super::observer::Identity,
    super::observer::GitHubTransport,
)> {
    let raw = super::group::read_bytes(path, super::observer::MAX_SPEC_BYTES)?;
    let spec: super::observer::Spec = serde_json::from_slice(&raw)?;
    let identity = super::observer::Identity::from_environment(&spec)?;
    let merge = if identity.event == "pull_request" {
        let event: serde_json::Value = serde_json::from_str(&identity.event_json)?;
        let base = event["pull_request"]["base"]["sha"]
            .as_str()
            .context("missing PR base revision")?
            .to_owned();
        Some((
            std::env::var("GITHUB_SHA").context("missing protected PR merge revision")?,
            base,
        ))
    } else {
        None
    };
    let committed = verify_source_spec(
        path,
        &raw,
        &identity.head,
        merge
            .as_ref()
            .map(|(sha, base)| (sha.as_str(), base.as_str())),
    )?;
    // The committed blob is policy, even when configured checkout/clean filters
    // change its representation. Never deserialize the transformed policy.
    let spec: super::observer::Spec = serde_json::from_slice(&committed)?;
    identity.validate(&spec)?;
    let token = observer_token(
        std::env::var("GH_TOKEN").ok(),
        std::env::var("GITHUB_TOKEN").ok(),
    )?;
    Ok((
        spec,
        identity,
        super::observer::GitHubTransport::new(token)?,
    ))
}

fn observer_token(primary: Option<String>, fallback: Option<String>) -> Result<String> {
    primary
        .filter(|token| !token.is_empty())
        .or_else(|| fallback.filter(|token| !token.is_empty()))
        .context("missing GitHub token environment variable")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_primary_token_uses_nonempty_fallback_without_exposing_credentials() {
        assert_eq!(
            observer_token(Some(String::new()), Some("inert-fallback".into())).unwrap(),
            "inert-fallback"
        );
        assert_eq!(
            observer_token(Some("inert-primary".into()), Some("inert-fallback".into())).unwrap(),
            "inert-primary"
        );
        assert!(observer_token(Some(String::new()), Some(String::new())).is_err());
        assert!(observer_token(None, None).is_err());
    }
}

fn source_probe(
    root: &std::path::Path,
    name: &str,
    command: &mut std::process::Command,
    deadline: std::time::Instant,
) -> Result<String> {
    Ok(String::from_utf8(source_probe_bytes(
        root,
        name,
        command,
        deadline,
        1024 * 1024,
    )?)?
    .trim_end_matches(['\r', '\n'])
    .to_owned())
}

fn source_probe_bytes(
    root: &std::path::Path,
    name: &str,
    command: &mut std::process::Command,
    deadline: std::time::Instant,
    maximum: u64,
) -> Result<Vec<u8>> {
    let output = root.join(name);
    let errors = root.join(format!("{name}.errors"));
    let remaining = deadline
        .checked_duration_since(std::time::Instant::now())
        .context("source-owned observer spec probe deadline expired")?;
    // Source probes never need the observer credential or a network fetch.
    command
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env("GIT_NO_LAZY_FETCH", "1");
    command
        .env_remove("GIT_EXEC_PATH")
        .env_remove("DEVELOPER_DIR")
        .env_remove("SDKROOT")
        .env_remove("TOOLCHAINS");
    let status = super::process::run_probe(
        command,
        &output,
        &errors,
        &super::cancellation::Cancellation::default(),
        remaining,
        maximum,
    )?;
    ensure!(
        status == 0,
        "source-owned observer spec Git probe failed ({name})"
    );
    super::group::read_bytes(&output, maximum)
}

fn verify_source_spec(
    path: &std::path::Path,
    raw: &[u8],
    head: &str,
    merge: Option<(&str, &str)>,
) -> Result<Vec<u8>> {
    let probe = tempfile::tempdir()?;
    let git = git::executable()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let actual_head = source_probe(
        probe.path(),
        "head",
        std::process::Command::new(&git)
            .arg("--no-replace-objects")
            .args(["rev-parse", "HEAD"]),
        deadline,
    )?;
    ensure!(
        actual_head == head,
        "observer checkout differs from exact source head"
    );
    if let Some((merge_sha, base)) = merge {
        ensure!(
            merge_sha.len() == 40
                && merge_sha
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "invalid protected PR merge revision"
        );
        let merge = source_probe_bytes(
            probe.path(),
            "merge",
            std::process::Command::new(&git)
                .arg("--no-replace-objects")
                .args(["cat-file", "commit", merge_sha]),
            deadline,
            65536,
        )?;
        let merge = std::str::from_utf8(&merge)?;
        let parents = merge
            .split("\n\n")
            .next()
            .unwrap_or("")
            .lines()
            .filter_map(|line| line.strip_prefix("parent "))
            .collect::<Vec<_>>();
        ensure!(
            parents.len() == 2 && parents[1] == head && parents[0] == base,
            "PR source head/base differs from protected merge revision"
        );
    }
    let repository = source_probe(
        probe.path(),
        "root",
        std::process::Command::new(&git)
            .arg("--no-replace-objects")
            .args(["rev-parse", "--show-toplevel"]),
        deadline,
    )?;
    let repository = std::fs::canonicalize(repository)?;
    let path = std::fs::canonicalize(path)?;
    let relative = path
        .strip_prefix(&repository)
        .context("source-owned observer spec must be inside the checkout")?;
    let relative = relative
        .components()
        .map(|part| {
            part.as_os_str()
                .to_str()
                .context("source-owned observer spec path must be UTF-8")
        })
        .collect::<Result<Vec<_>>>()?
        .join("/");
    let committed = source_probe(
        probe.path(),
        "committed",
        std::process::Command::new(&git)
            .arg("--no-replace-objects")
            .current_dir(&repository)
            .args(["rev-parse", "--verify", &format!("{head}:{relative}")]),
        deadline,
    )?;
    let blob = source_probe_bytes(
        probe.path(),
        "blob",
        std::process::Command::new(&git)
            .arg("--no-replace-objects")
            .current_dir(&repository)
            .args(["cat-file", "blob", &committed]),
        deadline,
        super::observer::MAX_SPEC_BYTES,
    )?;
    // Compare the immutable loaded bytes without executing repository-defined
    // clean filters. Only built-in CRLF/LF equivalence is admitted; policy is
    // always parsed from the bounded committed blob, never converted input.
    ensure!(
        raw == blob || json_newlines(raw) == json_newlines(&blob),
        "source-owned observer spec differs from committed blob"
    );
    Ok(blob)
}

fn json_newlines(raw: &[u8]) -> Vec<u8> {
    raw.iter()
        .enumerate()
        .filter_map(|(index, &byte)| {
            (byte != b'\r' || raw.get(index + 1) != Some(&b'\n')).then_some(byte)
        })
        .collect()
}

/// Group execution input locations.
#[derive(Debug, Args)]
pub struct GroupArgs {
    /// Versioned JSON spec; omitted to use generated workflow environment lists.
    #[arg(long)]
    pub spec: Option<PathBuf>,
    /// Evidence root (must exist and have no previous receipt).
    #[arg(long)]
    pub results: Option<PathBuf>,
}

/// Dispatch through the same public library used by native consumers.
pub fn run(command: Command) -> Result<()> {
    match command {
        Command::ObserveOriginal(args) => {
            let (spec, identity, mut transport) = observer_inputs(&args.spec)?;
            let cancellation = cancellation()?;
            super::observer::observe(
                &spec,
                &identity,
                &args.results,
                &mut transport,
                &cancellation.token,
            )?;
            Ok(())
        }
        Command::VerifyOriginalRetention(args) => {
            let (spec, identity, mut transport) = observer_inputs(&args.spec)?;
            let cancellation = cancellation()?;
            let receipt = super::observer::verify_retention(
                &spec,
                &identity,
                &args.results,
                args.artifact_id,
                &args.artifact_digest,
                &mut transport,
                &cancellation.token,
            )?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
            Ok(())
        }
        Command::NixGroup(args) => {
            let cancellation = super::cancellation::Cancellation::default();
            // This CLI exits after its one command. Retain handlers for that entire
            // lifetime; unregistering signal-hook actions does not restore defaults.
            // The library never installs these process-global handlers.
            #[cfg(unix)]
            for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
                signal_hook::flag::register(signal, cancellation.0.clone())?;
            }
            let spec = match args.spec {
                Some(path) => super::group::read_json(&path, super::group::MAX_SPEC_BYTES)?,
                None => super::group::GroupSpec::from_environment()?,
            };
            let root = args
                .results
                .or_else(|| std::env::var_os("SIMIT_NIX_BUILD_RESULTS").map(PathBuf::from));
            let root = root.filter(|path| !path.as_os_str().is_empty()).ok_or_else(|| {
                anyhow::anyhow!(
                    "a nonempty results directory is required via --results or SIMIT_NIX_BUILD_RESULTS"
                )
            })?;
            let source = super::group::Source::from_environment()?;
            let receipt = super::group::execute_with(
                &spec,
                &source,
                &std::env::current_dir()?,
                &root,
                &mut super::process::CommandRunner,
                &cancellation,
            )?;
            ensure!(
                receipt.success == Some(true),
                "one or more grouped Nix members failed; see retained evidence"
            );
            Ok(())
        }
    }
}

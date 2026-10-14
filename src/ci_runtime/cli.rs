//! Thin CLI adapters for versioned CI runtime contracts.
use anyhow::{Result, ensure};
use clap::{Args, Subcommand};
use std::path::PathBuf;

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

fn cancellation() -> Result<super::cancellation::Cancellation> {
    let cancellation = super::cancellation::Cancellation::default();
    #[cfg(unix)]
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, cancellation.0.clone())?;
    }
    Ok(cancellation)
}

fn observer_inputs(
    path: &std::path::Path,
) -> Result<(
    super::observer::Spec,
    super::observer::Identity,
    super::observer::GitHubTransport,
)> {
    let spec: super::observer::Spec = super::group::read_json(path, 1024 * 1024)?;
    let identity = super::observer::Identity::from_environment(&spec)?;
    let probe = tempfile::tempdir()?;
    let output = probe.path().join("head");
    let errors = probe.path().join("errors");
    let status = super::process::run_probe(
        std::process::Command::new("git").args(["rev-parse", "HEAD"]),
        &output,
        &errors,
        &super::cancellation::Cancellation::default(),
    )?;
    ensure!(
        status == 0 && std::fs::read_to_string(output)?.trim() == identity.head,
        "observer checkout differs from exact source head"
    );
    let token = std::env::var("GH_TOKEN").or_else(|_| std::env::var("GITHUB_TOKEN"))?;
    Ok((
        spec,
        identity,
        super::observer::GitHubTransport::new(token)?,
    ))
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
            super::observer::observe(
                &spec,
                &identity,
                &args.results,
                &mut transport,
                &cancellation()?,
            )?;
            Ok(())
        }
        Command::VerifyOriginalRetention(args) => {
            let (spec, identity, mut transport) = observer_inputs(&args.spec)?;
            let receipt = super::observer::verify_retention(
                &spec,
                &identity,
                &args.results,
                args.artifact_id,
                &args.artifact_digest,
                &mut transport,
                &cancellation()?,
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

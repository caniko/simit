//! Thin CLI adapters for versioned CI runtime contracts.
use anyhow::{Result, ensure};
use clap::{Args, Subcommand};
use std::path::PathBuf;

/// CI execution commands. Receipts are evidence, never standalone acceptance.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Execute every enrolled Nix member on one worker and retain per-member evidence.
    NixGroup(GroupArgs),
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
            let root = args.results.unwrap_or(PathBuf::from(
                std::env::var_os("SIMIT_NIX_BUILD_RESULTS").unwrap_or_default(),
            ));
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

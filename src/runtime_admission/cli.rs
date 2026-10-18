//! Independent CLI over the same admission library consumed by adapters.
use super::{AdmissionOutcome, AdmissionSpec, Cancellation, admit};
use anyhow::{Result, ensure};
use clap::{Args, Subcommand};
use std::path::PathBuf;

/// Runtime observation commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run one bounded admission invocation; success does not create a durable grant.
    Admit(AdmissionArgs),
}

/// Explicit input and invocation-specific evidence locations.
#[derive(Debug, Args)]
pub struct AdmissionArgs {
    /// Version-1 JSON spec, at most 64 KiB.
    #[arg(long)]
    pub spec: PathBuf,
    /// New private evidence directory; its parent must exist.
    #[arg(long)]
    pub evidence: PathBuf,
}

/// Dispatch without policy, activation or cached success.
pub fn run(command: Command) -> Result<()> {
    let Command::Admit(args) = command;
    let spec = AdmissionSpec::read(&args.spec)?;
    let cancellation = Cancellation::default();
    #[cfg(unix)]
    let _signals = [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM]
        .map(|signal| signal_hook::flag::register(signal, cancellation.0.clone()))
        .into_iter()
        .collect::<std::io::Result<Vec<_>>>()?;
    let receipt = admit(&spec, &args.evidence, &cancellation)?;
    println!("{}", serde_json::to_string(&receipt)?);
    ensure!(
        receipt.outcome == AdmissionOutcome::Admitted,
        "admission failed: {:?}",
        receipt.outcome
    );
    Ok(())
}

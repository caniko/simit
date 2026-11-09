//! `simit ci usage` inputs.

use clap::Args;

/// Read-only CI resource usage report inputs.
#[derive(Debug, Args)]
pub struct UsageArgs {
    /// Forge-qualified repository, repeatable: github:owner/name or crow:owner/name.
    #[arg(long = "repo", value_name = "FORGE:OWNER/NAME", required = true)]
    pub repos: Vec<String>,

    /// Window start (UTC): YYYY-MM-DD or Nd/Nh/Nw relative to now.
    #[arg(long, default_value = "30d", value_name = "DATE|DURATION")]
    pub since: String,

    /// Window end (UTC, exclusive): YYYY-MM-DD (whole day) or Nd/Nh/Nw relative to now.
    #[arg(long, value_name = "DATE|DURATION")]
    pub until: Option<String>,

    /// Emit the documented JSON schema instead of human tables.
    #[arg(long)]
    pub json: bool,

    /// Maximum pathway rows to report.
    #[arg(long, default_value_t = 20)]
    pub top: usize,
}

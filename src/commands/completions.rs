use anyhow::Result;
use clap::CommandFactory;

use crate::cli::{Cli, CompletionsCommand};

#[expect(
    clippy::needless_pass_by_value,
    reason = "Keep the public command entry point's owned argument API"
)]
pub fn run(command: CompletionsCommand) -> Result<()> {
    let mut cli = Cli::command();
    clap_complete::generate(command.shell, &mut cli, "simit", &mut std::io::stdout());
    Ok(())
}

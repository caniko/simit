use anyhow::Result;
use clap::CommandFactory;

use crate::cli::{Cli, CompletionsCommand};

pub fn run(command: CompletionsCommand) -> Result<()> {
    let mut cli = Cli::command();
    clap_complete::generate(command.shell, &mut cli, "simit", &mut std::io::stdout());
    Ok(())
}

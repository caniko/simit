use anyhow::Result;
use clap::CommandFactory;

use crate::cli::{Cli, ManCommand};

pub fn run(_command: ManCommand) -> Result<()> {
    let command = Cli::command();
    let man = clap_mangen::Man::new(command);
    man.render(&mut std::io::stdout())?;
    Ok(())
}

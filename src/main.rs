mod cargo;
mod cli;
mod commands;
mod git;
mod project;
mod render;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Commands};

fn main() {
    if let Err(err) = run() {
        eprintln!("simit: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Commit(command) => commands::commit::run(command),
        Commands::Release(command) => commands::release::run(command),
        Commands::InitCi(command) => commands::init_ci::run(command),
        Commands::InitHooks(command) => commands::init_hooks::run(command),
        Commands::InitFlake(command) => commands::init_flake::run(command),
        Commands::Completions(command) => commands::completions::run(command),
        Commands::Man(command) => commands::man::run(command),
    }
}

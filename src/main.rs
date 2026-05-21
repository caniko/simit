use anyhow::Result;
use clap::Parser;

use simit::cli::{Cli, Commands};
use simit::commands;

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
        Commands::InitHomebrewTap(command) => commands::init_homebrew_tap::run(command),
        Commands::Homebrew(command) => commands::homebrew::run(command),
        Commands::InitFlake(command) => commands::init_flake::run(command),
        Commands::Changelog(command) => commands::changelog::run(command),
        Commands::Completions(command) => commands::completions::run(command),
        Commands::Man(command) => commands::man::run(command),
    }
}

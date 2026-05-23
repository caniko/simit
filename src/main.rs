use anyhow::Result;
use clap::Parser;

use simit::cli::{Cli, Commands, DistAction, InitAction};
use simit::commands;

fn main() {
    if let Err(err) = run() {
        eprintln!("simit: {err:#}");
        let code = err
            .downcast_ref::<simit::commands::projects::CommandExit>()
            .map_or(1, simit::commands::projects::CommandExit::code);
        std::process::exit(code);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Commit(command) => commands::commit::run(command),
        Commands::Release(command) => commands::release::run(command),
        Commands::Init(command) => match command.action {
            InitAction::Ci(command) => commands::init_ci::run(*command),
            InitAction::Flake(command) => commands::init_flake::run(command),
            InitAction::HomebrewTap(command) => commands::init_homebrew_tap::run(command),
            InitAction::Chocolatey(command) => commands::init_chocolatey::run(command),
            InitAction::ScoopBucket(command) => commands::init_scoop_bucket::run(command),
        },
        Commands::Dist(command) => match command.action {
            DistAction::Homebrew(command) => commands::homebrew::run(command),
            DistAction::Chocolatey(command) => commands::chocolatey::run(command),
            DistAction::Scoop(command) => commands::scoop::run(command),
        },
        Commands::Changelog(command) => commands::changelog::run(command),
        Commands::Config(command) => commands::config::run(command),
        Commands::Projects(command) => commands::projects::run(command),
        Commands::Completions(command) => commands::completions::run(command),
        Commands::Man(command) => commands::man::run(command),
    }
}

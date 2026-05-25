use anyhow::Result;
use clap::Parser;

use simit::cli::{Cli, Commands, DistAction, HooksAction, InitAction};
use simit::commands;

fn main() {
    if let Err(err) = run() {
        if let Some(command_exit) = err.downcast_ref::<simit::commands::projects::CommandExit>() {
            eprintln!("simit: {command_exit}");
            std::process::exit(command_exit.code());
        }
        if let Some(verify_exit) = err.downcast_ref::<simit::commands::release_verify::VerifyExit>()
        {
            std::process::exit(verify_exit.code());
        }
        eprintln!("simit: {err:#}");
        let code = 1;
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
        Commands::Hooks(command) => match command.action {
            HooksAction::Install(command) => commands::hooks::install(command),
        },
        Commands::Config(command) => commands::config::run(command),
        Commands::Projects(command) => commands::projects::run(command),
        Commands::Completions(command) => commands::completions::run(command),
        Commands::Man(command) => commands::man::run(command),
    }
}

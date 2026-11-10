use crate::binstall::{self, State};
use crate::cargo;
use crate::cli::{BinstallCommand, BinstallSyncCommand};
use anyhow::{Result, bail};

pub fn run(command: BinstallCommand) -> Result<()> {
    match command.action {
        crate::cli::BinstallAction::Sync(command) => sync(command),
    }
}

fn sync(command: BinstallSyncCommand) -> Result<()> {
    let metadata = cargo::metadata_for_current_dir()?;
    let system = binstall::resolve_system(command.system.as_deref())?;
    let updates = binstall::plan(
        metadata.workspace_root.as_std_path(),
        &metadata,
        &command.package,
        &system,
    )?;
    if updates.is_empty() {
        println!("binstall: no publishable binary crates to sync");
        return Ok(());
    }
    if command.check {
        return check(&updates);
    }
    let mut drifted = 0;
    for update in &updates {
        match binstall::state(update)? {
            State::InSync => {}
            State::Missing => println!(
                "binstall: warning: {} has no rendered metadata; {}",
                update.package,
                binstall::MISSING_HINT
            ),
            State::Drift { diff } => {
                print!("{diff}");
                binstall::apply(update)?;
                println!("binstall: updated {}", update.package);
                drifted += 1;
            }
        }
    }
    if drifted == 0 {
        println!("binstall: {} crate(s) already in sync", updates.len());
    }
    Ok(())
}

fn check(updates: &[binstall::Update]) -> Result<()> {
    let mut failed = Vec::new();
    for (package, state) in binstall::check(updates)? {
        match state {
            State::InSync => {}
            State::Missing => {
                println!(
                    "binstall: {package}: missing metadata; {}",
                    binstall::MISSING_HINT
                );
                failed.push(package);
            }
            State::Drift { diff } => {
                print!("{diff}");
                failed.push(package);
            }
        }
    }
    if !failed.is_empty() {
        bail!(
            "binstall metadata drift in {} package(s): {}; run `simit binstall sync`",
            failed.len(),
            failed.join(", ")
        );
    }
    println!("binstall: metadata in sync");
    Ok(())
}

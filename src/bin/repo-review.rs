//! Compatibility frontend; the engine is owned by Simit's review module.
use clap::Parser;
fn main() {
    simit::review::cli::finish(simit::review::cli::Cli::parse().command);
}

use anyhow::Result;

use crate::cli::{ConfigAction, ConfigCommand};
use crate::user_config::UserConfig;

pub fn run(command: ConfigCommand) -> Result<()> {
    match command.action {
        ConfigAction::Path => {
            println!("{}", UserConfig::path().display());
            Ok(())
        }
        ConfigAction::Show => {
            let config = UserConfig::load()?;
            print!("{}", config.to_toml_pretty()?);
            Ok(())
        }
        ConfigAction::Check => {
            let config = UserConfig::load()?;
            config.validate()?;
            println!("{} is valid", UserConfig::path().display());
            Ok(())
        }
        ConfigAction::Init => {
            let path = UserConfig::write_starter()?;
            println!("wrote {}", path.display());
            Ok(())
        }
    }
}

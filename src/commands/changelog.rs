use anyhow::Result;

use crate::changelog::{self, EntryKind};
use crate::cli::{ChangelogAction, ChangelogCommand, ChangelogEntryKind};
use crate::registry::{self, FeatureStatus};

pub fn run(command: ChangelogCommand) -> Result<()> {
    match command.action {
        ChangelogAction::Init => {
            changelog::init_file(command.file.as_std_path())?;
            registry::touch_current_project_or_warn([("changelog", FeatureStatus::Managed)]);
            Ok(())
        }
        ChangelogAction::Draft { base_ref, dry_run } => {
            changelog::draft_file(command.file.as_std_path(), base_ref.as_deref(), dry_run)?;
            if !dry_run {
                registry::touch_current_project_or_warn([("changelog", FeatureStatus::Managed)]);
            }
            Ok(())
        }
        ChangelogAction::Add { kind, text } => {
            changelog::add_entry_file(command.file.as_std_path(), entry_kind(kind), &text)
        }
        ChangelogAction::Release {
            version,
            date,
            repo_url,
        } => {
            let date = match date {
                Some(value) => changelog::parse_iso_date(&value)?,
                None => changelog::today_utc()?,
            };
            changelog::release_file(
                command.file.as_std_path(),
                &version,
                date,
                repo_url.as_deref(),
                None,
            )?;
            registry::touch_current_project_or_warn([("changelog", FeatureStatus::Managed)]);
            Ok(())
        }
        ChangelogAction::Check => changelog::check_file(command.file.as_std_path()),
        ChangelogAction::Show { version } => {
            let body = changelog::show_file(command.file.as_std_path(), version.as_deref())?;
            print!("{body}");
            Ok(())
        }
    }
}

fn entry_kind(kind: ChangelogEntryKind) -> EntryKind {
    match kind {
        ChangelogEntryKind::Added => EntryKind::Added,
        ChangelogEntryKind::Changed => EntryKind::Changed,
        ChangelogEntryKind::Deprecated => EntryKind::Deprecated,
        ChangelogEntryKind::Removed => EntryKind::Removed,
        ChangelogEntryKind::Fixed => EntryKind::Fixed,
        ChangelogEntryKind::Security => EntryKind::Security,
    }
}

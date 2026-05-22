use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::InitFlakeCommand;
use crate::project::{self, GeneratedFile};
use crate::render::diff::unified_diff;
use crate::render::flake;

pub fn run(command: InitFlakeCommand) -> Result<()> {
    if command.check && command.print {
        bail!("init-flake accepts only one of --check or --print");
    }
    if command.diff && !command.check {
        bail!("init-flake --diff requires --check");
    }

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let mut languages = project::detect_languages(workspace_root)?;
    languages.nix = true;
    let rust_edition = rustfmt_edition(&metadata);
    let files = flake::files(&languages, &rust_edition);

    if command.print {
        flake::print_files(&files);
        flake::print_existing_flake_note();
        return Ok(());
    }

    if command.check {
        return check_files(workspace_root, &files, command.diff);
    }

    let flake_path = workspace_root.join("flake.nix");
    if flake_path.exists() {
        let content = fs::read_to_string(&flake_path)
            .with_context(|| format!("reading {}", flake_path.display()))?;
        let patched = flake::patch_existing(&content)?;
        let mut patched_files = files.clone();
        let flake_file = patched_files
            .iter_mut()
            .find(|file| file.relative_path == Path::new("flake.nix"))
            .expect("flake.nix is generated");
        flake_file.content = patched;
        return project::write_generated_files(workspace_root, &patched_files);
    }

    project::write_generated_files(workspace_root, &files)
}

fn rustfmt_edition(metadata: &cargo::Metadata) -> String {
    metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .filter_map(|package| package.edition.as_deref())
        .max()
        .unwrap_or("2021")
        .to_owned()
}

fn check_files(
    workspace_root: &std::path::Path,
    files: &[GeneratedFile],
    show_diff: bool,
) -> Result<()> {
    let mut mismatches = Vec::new();
    let mut diffs = Vec::new();

    for file in files {
        let path = workspace_root.join(&file.relative_path);
        match fs::read_to_string(&path) {
            Ok(actual) if file.relative_path == Path::new("flake.nix") => {
                if actual == file.content || flake::has_required_wiring(&actual) {
                    continue;
                }
                mismatches.push("flake.nix is missing generated hook wiring".to_owned());
                if show_diff {
                    diffs.push(unified_diff("flake.nix", &actual, &file.content));
                }
            }
            Ok(actual) if actual == file.content => {}
            Ok(actual) => {
                mismatches.push(format!("{} differs", file.relative_path.display()));
                if show_diff {
                    diffs.push(unified_diff(
                        &file.relative_path.display().to_string(),
                        &actual,
                        &file.content,
                    ));
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                mismatches.push(format!("{} is missing", file.relative_path.display()));
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    if mismatches.is_empty() {
        Ok(())
    } else if diffs.is_empty() {
        bail!(
            "flake and hook files are not up to date; run `simit init-flake`:\n{}",
            mismatches.join("\n")
        );
    } else {
        bail!(
            "flake and hook files are not up to date; run `simit init-flake`:\n{}\n{}",
            mismatches.join("\n"),
            diffs.join("\n")
        );
    }
}

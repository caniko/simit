use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::{FlakeScopeArg, InitFlakeCommand};
use crate::config::{FlakeMode, FlakeScope, ProjectConfig};
use crate::project::{self, GeneratedFile, Languages};
use crate::registry::{self, FeatureStatus};
use crate::render::diff::unified_diff;
use crate::render::flake;

const HOOK_REMOVAL_NOTES: &[(&str, &str)] = &[
    // When a generated hook is removed, add its CI-side replacement here so
    // `simit init flake --check --diff` can explain the migration inline.
    (
        "cargo-audit",
        "the equivalent CI check now lives in .forgejo/workflows/ci.yaml::Audit dependencies",
    ),
];

pub fn run(command: InitFlakeCommand) -> Result<()> {
    if command.check && command.print {
        bail!("init flake accepts only one of --check or --print");
    }
    if command.diff && !command.check {
        bail!("init flake --diff requires --check");
    }

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let cfg = ProjectConfig::load(workspace_root)?;
    let mut languages = project::detect_languages(workspace_root)?;
    languages.nix = true;
    let rust_edition = rustfmt_edition(&metadata);
    let rust_version = workspace_rust_version(&metadata);
    let all_files = flake::files(&languages, &rust_edition, rust_version.as_deref());
    let scope = resolve_scope(command.scope, &cfg, workspace_root);
    let files = scoped_files(&all_files, scope);

    if command.print {
        flake::print_files(&files);
        if scope == FlakeScope::Full {
            flake::print_existing_flake_note();
        }
        return Ok(());
    }

    if command.check {
        return check_files(
            workspace_root,
            &files,
            &languages,
            &rust_edition,
            rust_version.as_deref(),
            &cfg,
            scope,
            command.diff,
        );
    }

    if scope == FlakeScope::HooksOnly {
        if workspace_root.join("flake.nix").exists() {
            println!(
                "note: hooks-only scope leaves existing flake.nix untouched; review it before taking project ownership"
            );
        }
        project::write_generated_files(workspace_root, &files)?;
        registry::touch_current_project_or_warn([
            ("flake", FeatureStatus::Managed),
            ("hooks", FeatureStatus::Installed),
        ]);
        return Ok(());
    }

    let flake_path = workspace_root.join("flake.nix");
    if cfg.flake.mode == FlakeMode::Custom {
        if !flake_path.exists() {
            bail!(
                "custom flake mode requires an existing flake.nix; simit will manage hook files but will not generate a canonical flake"
            );
        }
        let hook_files = hook_files(&files);
        project::write_generated_files(workspace_root, &hook_files)?;
        registry::touch_current_project_or_warn([
            ("flake", FeatureStatus::Managed),
            ("hooks", FeatureStatus::Installed),
        ]);
        return Ok(());
    }

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
        project::write_generated_files(workspace_root, &patched_files)?;
        registry::touch_current_project_or_warn([
            ("flake", FeatureStatus::Managed),
            ("hooks", FeatureStatus::Installed),
        ]);
        return Ok(());
    }

    project::write_generated_files(workspace_root, &files)?;
    registry::touch_current_project_or_warn([
        ("flake", FeatureStatus::Managed),
        ("hooks", FeatureStatus::Installed),
    ]);
    Ok(())
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

fn workspace_rust_version(metadata: &cargo::Metadata) -> Option<String> {
    metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .filter_map(|package| package.rust_version.as_deref())
        .filter_map(|version| {
            let normalized = normalize_rust_version(version);
            semver::Version::parse(&normalized)
                .ok()
                .map(|parsed| (parsed, version.to_owned()))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, version)| version)
}

fn normalize_rust_version(version: &str) -> String {
    match version.matches('.').count() {
        0 => format!("{version}.0.0"),
        1 => format!("{version}.0"),
        _ => version.to_owned(),
    }
}

fn resolve_scope(
    cli_scope: Option<FlakeScopeArg>,
    cfg: &ProjectConfig,
    workspace_root: &Path,
) -> FlakeScope {
    if let Some(scope) = cli_scope {
        return match scope {
            FlakeScopeArg::HooksOnly => FlakeScope::HooksOnly,
            FlakeScopeArg::Full => FlakeScope::Full,
        };
    }

    if let Some(scope) = cfg.flake.scope {
        return scope;
    }

    if has_existing_full_scope_adoption(workspace_root, cfg) {
        FlakeScope::Full
    } else {
        FlakeScope::HooksOnly
    }
}

fn has_existing_full_scope_adoption(workspace_root: &Path, cfg: &ProjectConfig) -> bool {
    if cfg.flake.mode == FlakeMode::Custom {
        return false;
    }

    let Ok(content) = fs::read_to_string(workspace_root.join("flake.nix")) else {
        return false;
    };

    workspace_root.join("nix/treefmt.nix").exists()
        && content.contains("crane.url = \"github:ipetkov/crane\"")
        && content.contains("rustToolchain = pkgs.rust-bin.stable.latest.default.override")
        && content.contains("package = craneLib.buildPackage")
}

fn scoped_files(files: &[GeneratedFile], scope: FlakeScope) -> Vec<GeneratedFile> {
    match scope {
        FlakeScope::HooksOnly => files
            .iter()
            .filter(|file| file.relative_path == Path::new("nix/pre-commit.nix"))
            .cloned()
            .collect(),
        FlakeScope::Full => files.to_vec(),
    }
}

fn checked_file_label(scope: FlakeScope) -> &'static str {
    match scope {
        FlakeScope::HooksOnly => "hook files",
        FlakeScope::Full => "flake and hook files",
    }
}

fn scope_arg_hint(scope: FlakeScope) -> &'static str {
    match scope {
        FlakeScope::HooksOnly => "",
        FlakeScope::Full => " --scope full",
    }
}

#[allow(clippy::too_many_arguments)]
fn check_files(
    workspace_root: &std::path::Path,
    files: &[GeneratedFile],
    languages: &Languages,
    rust_edition: &str,
    rust_version: Option<&str>,
    cfg: &ProjectConfig,
    scope: FlakeScope,
    show_diff: bool,
) -> Result<()> {
    let mut mismatches = Vec::new();
    let mut notes = Vec::new();
    let mut diffs = Vec::new();

    for file in files {
        if cfg.flake.mode == FlakeMode::Custom && file.relative_path == Path::new("flake.nix") {
            let path = workspace_root.join(&file.relative_path);
            match fs::read_to_string(&path) {
                Ok(actual) => {
                    let missing = flake::custom_wiring_mismatches(&actual, &cfg.flake);
                    if missing.is_empty() {
                        continue;
                    }
                    mismatches.extend(missing);
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    mismatches.push("flake.nix is missing; custom flake mode requires a project-owned flake.nix".to_owned());
                }
                Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
            }
            continue;
        }

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
            Ok(actual) if file.relative_path == Path::new("nix/treefmt.nix") => {
                if actual == file.content
                    || flake::has_required_treefmt(&actual, languages, rust_edition)
                {
                    continue;
                }
                mismatches.push("nix/treefmt.nix is missing generated formatter wiring".to_owned());
                if show_diff {
                    diffs.push(unified_diff("nix/treefmt.nix", &actual, &file.content));
                }
            }
            Ok(actual) if file.relative_path == Path::new("nix/pre-commit.nix") => {
                if actual == file.content
                    || flake::has_required_pre_commit(&actual, languages, rust_version)
                {
                    continue;
                }
                mismatches.push("nix/pre-commit.nix is missing generated hook wiring".to_owned());
                if show_diff {
                    notes.extend(pre_commit_removal_notes(&actual, &file.content));
                    diffs.push(unified_diff("nix/pre-commit.nix", &actual, &file.content));
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
            "{} are not up to date; run `simit init flake{}`:\n{}",
            checked_file_label(scope),
            scope_arg_hint(scope),
            mismatches.join("\n")
        );
    } else {
        let note_block = if notes.is_empty() {
            String::new()
        } else {
            format!("{}\n", notes.join("\n"))
        };
        bail!(
            "{} are not up to date; run `simit init flake{}`:\n{}\n{}{}",
            checked_file_label(scope),
            scope_arg_hint(scope),
            mismatches.join("\n"),
            note_block,
            diffs.join("\n")
        );
    }
}

fn hook_files(files: &[GeneratedFile]) -> Vec<GeneratedFile> {
    files
        .iter()
        .filter(|file| file.relative_path != Path::new("flake.nix"))
        .cloned()
        .collect()
}

fn pre_commit_removal_notes(actual: &str, generated: &str) -> Vec<String> {
    let existing_hooks = extract_pre_commit_hook_names(actual);
    let generated_hooks = extract_pre_commit_hook_names(generated);

    if existing_hooks.is_empty() {
        return Vec::new();
    }

    existing_hooks
        .difference(&generated_hooks)
        .map(|hook| match hook_removal_note(hook) {
            Some(target) => format!("note: pre-commit hook '{hook}' will be removed; {target}"),
            None => format!("note: pre-commit hook '{hook}' will be removed"),
        })
        .collect()
}

fn hook_removal_note(hook: &str) -> Option<&'static str> {
    HOOK_REMOVAL_NOTES
        .iter()
        .find_map(|(name, note)| (*name == hook).then_some(*note))
}

fn extract_pre_commit_hook_names(content: &str) -> BTreeSet<String> {
    let mut hooks = BTreeSet::new();
    let mut depth = 0i32;

    for line in content.lines() {
        let trimmed = line.trim();
        if depth == 1
            && !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !trimmed.starts_with('{')
            && trimmed != "}"
        {
            let Some((name, remainder)) = trimmed.split_once('=') else {
                update_nix_brace_depth(&mut depth, line);
                continue;
            };
            let name = name.trim();
            if !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
                && remainder.trim_start().starts_with('{')
            {
                hooks.insert(name.to_owned());
            }
        }

        update_nix_brace_depth(&mut depth, line);
    }

    hooks
}

fn update_nix_brace_depth(depth: &mut i32, line: &str) {
    for ch in line.chars() {
        match ch {
            '{' => *depth += 1,
            '}' => *depth -= 1,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_pre_commit_hook_names, pre_commit_removal_notes};

    #[test]
    fn removal_notes_use_specific_mapping_when_available() {
        let actual = r#"{
  cargo-audit = {
    enable = true;
  };
  cargo-fmt = {
    enable = true;
  };
}
"#;
        let generated = r#"{
  cargo-fmt = {
    enable = true;
  };
}
"#;

        let notes = pre_commit_removal_notes(actual, generated);
        assert_eq!(
            notes,
            vec![
                "note: pre-commit hook 'cargo-audit' will be removed; the equivalent CI check now lives in .forgejo/workflows/ci.yaml::Audit dependencies"
            ]
        );
    }

    #[test]
    fn removal_notes_fall_back_for_unknown_hooks() {
        let actual = r#"{
  custom-check = {
    enable = true;
  };
}
"#;

        let notes = pre_commit_removal_notes(actual, "{}\n");
        assert_eq!(
            notes,
            vec!["note: pre-commit hook 'custom-check' will be removed"]
        );
    }

    #[test]
    fn extractor_ignores_nested_attributes() {
        let hooks = extract_pre_commit_hook_names(
            r#"{
  cargo-msrv = {
    stages = {
      pre-push = true;
    };
  };
}
"#,
        );

        assert!(hooks.contains("cargo-msrv"));
        assert!(!hooks.contains("stages"));
        assert!(!hooks.contains("pre-push"));
    }
}

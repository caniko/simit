//! `[package.metadata.binstall]` synchronization with release bundles.
//!
//! `cargo binstall` reads the crate manifest, so the metadata has to live in
//! `Cargo.toml` even though harbor-rs derives it from the release bundle. The
//! flake renders the exact block as `checks.<system>.binstall-metadata*.toml`;
//! this module proves (or restores) that the manifest carries it verbatim.

use std::fmt::Write as _;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::cargo::{Metadata, Package};

/// Shown whenever a crate has binaries but its flake exposes no check.
pub const MISSING_HINT: &str = "expose mkBinstallRelease checks in flake.nix";

const SYSTEMS: [&str; 4] = [
    "x86_64-linux",
    "aarch64-linux",
    "aarch64-darwin",
    "x86_64-darwin",
];

/// One crate whose manifest should carry the bundle's binstall metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub package: String,
    pub manifest: PathBuf,
    /// `None`: the flake exposes no `binstall-metadata` check for this crate.
    pub expected: Option<String>,
}

/// Resolve `--system`, defaulting to the Nix system of this host.
pub fn resolve_system(requested: Option<&str>) -> Result<String> {
    match requested {
        Some(system) => {
            if !SYSTEMS.contains(&system) {
                bail!(
                    "unsupported Nix system {system:?}; use one of {}",
                    SYSTEMS.join(", ")
                );
            }
            Ok(system.to_owned())
        }
        None => {
            let arch = match std::env::consts::ARCH {
                "x86_64" => "x86_64",
                "aarch64" => "aarch64",
                other => bail!("no Nix system mapping for architecture {other}"),
            };
            let os = match std::env::consts::OS {
                "linux" => "linux",
                "macos" => "darwin",
                other => bail!("no Nix system mapping for operating system {other}"),
            };
            Ok(format!("{arch}-{os}"))
        }
    }
}

/// Workspace members that `cargo binstall` could serve from release archives.
pub fn candidates(metadata: &Metadata) -> Vec<&Package> {
    metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .filter(|package| package.publishes_binstall_archive())
        .collect()
}

/// Read the rendered binstall metadata for every selected crate.
pub fn plan(
    root: &Path,
    metadata: &Metadata,
    requested: &[String],
    system: &str,
) -> Result<Vec<Update>> {
    for name in requested {
        if !metadata
            .packages
            .iter()
            .any(|package| &package.name == name)
        {
            bail!("--package {name} is not a workspace member");
        }
    }
    let candidates = candidates(metadata);
    // The unsuffixed check is the single-binary-crate convention; with more
    // than one candidate it cannot say which manifest it describes.
    let generic = candidates.len() == 1;
    let selected: Vec<&Package> = candidates
        .into_iter()
        .filter(|package| requested.is_empty() || requested.contains(&package.name))
        .collect();
    let mut updates = Vec::new();
    for package in selected {
        let mut expected =
            eval_metadata(root, system, &format!("binstall-metadata-{}", package.name))?;
        if expected.is_none() && generic {
            expected = eval_metadata(root, system, "binstall-metadata")?;
        }
        if let Some(text) = &expected {
            expected = Some(canonical(text));
        }
        updates.push(Update {
            package: package.name.clone(),
            manifest: PathBuf::from(package.manifest_path.as_str()),
            expected,
        });
    }
    Ok(updates)
}

/// What a selected manifest currently needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    InSync,
    Drift { diff: String },
    Missing,
}

/// Compare every update with its manifest without writing anything.
pub fn check(updates: &[Update]) -> Result<Vec<(String, State)>> {
    updates
        .iter()
        .map(|update| Ok((update.package.clone(), state(update)?)))
        .collect()
}

/// Compare one update with its manifest without writing anything.
pub fn state(update: &Update) -> Result<State> {
    let current = fs::read_to_string(&update.manifest)
        .with_context(|| format!("reading {}", update.manifest.display()))?;
    let Some(expected) = &update.expected else {
        return Ok(State::Missing);
    };
    let current = canonical(&binstall_blocks(&current));
    if &current == expected {
        Ok(State::InSync)
    } else {
        Ok(State::Drift {
            diff: unified_diff(&current, expected, &update.package),
        })
    }
}

/// Replace the manifest's binstall tables with the rendered block.
pub fn apply(update: &Update) -> Result<()> {
    let Some(expected) = &update.expected else {
        return Ok(());
    };
    let current = fs::read_to_string(&update.manifest)
        .with_context(|| format!("reading {}", update.manifest.display()))?;
    let spans = binstall_spans(&current);
    let mut written = String::with_capacity(current.len() + expected.len());
    let mut last = 0;
    let insert_at = spans.first().map(|span| span.start);
    for span in &spans {
        written.push_str(&current[last..span.start]);
        last = span.end;
        if Some(span.start) == insert_at {
            written.push_str(expected);
        }
    }
    written.push_str(&current[last..]);
    if insert_at.is_none() {
        if !written.ends_with('\n') {
            written.push('\n');
        }
        written.push('\n');
        written.push_str(expected);
    }
    verify(&written, expected)
        .with_context(|| format!("updating {}", update.manifest.display()))?;
    fs::write(&update.manifest, written)
        .with_context(|| format!("writing {}", update.manifest.display()))?;
    Ok(())
}

/// Fail unless the rewritten manifest parses and carries exactly `expected`.
fn verify(text: &str, expected: &str) -> Result<()> {
    let want: serde_json::Value = toml_edit::de::from_str(expected)
        .context("the rendered binstall metadata is not valid TOML")?;
    let got: serde_json::Value =
        toml_edit::de::from_str(text).context("Cargo.toml is not valid TOML after the update")?;
    let want = &want["package"]["metadata"]["binstall"];
    anyhow::ensure!(
        !want.is_null(),
        "the rendered binstall metadata has no [package.metadata.binstall] table"
    );
    anyhow::ensure!(
        got["package"]["metadata"]["binstall"] == *want,
        "rewritten Cargo.toml does not carry the rendered [package.metadata.binstall] block"
    );
    Ok(())
}

fn eval_metadata(root: &Path, system: &str, attr: &str) -> Result<Option<String>> {
    let output = Command::new("nix")
        .current_dir(root)
        .args(["eval", "--raw", &format!(".#checks.{system}.{attr}.toml")])
        .stderr(Stdio::null())
        .output()
        .context("running `nix eval` for the binstall metadata check")?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// Byte ranges of every `[package.metadata.binstall*]` table, in order.
fn binstall_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut current: Option<usize> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if let Some(name) = table_name(line) {
            if is_binstall_table(name) {
                current.get_or_insert(offset);
            } else if let Some(start) = current.take() {
                spans.push(start..offset);
            }
        }
        offset += line.len();
    }
    if let Some(start) = current {
        spans.push(start..text.len());
    }
    spans
}

fn binstall_blocks(text: &str) -> String {
    binstall_spans(text)
        .into_iter()
        .map(|span| &text[span])
        .collect()
}

fn is_binstall_table(name: &str) -> bool {
    name == "package.metadata.binstall" || name.starts_with("package.metadata.binstall.")
}

fn table_name(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("[[")
        .or_else(|| trimmed.strip_prefix('['))?;
    Some(rest[..rest.find(']')?].trim())
}

/// Rust target triples an evaluated `[package.metadata.binstall]` block
/// serves, one per `[package.metadata.binstall.overrides.<triple>]` table.
pub fn archive_targets(text: &str) -> std::collections::BTreeSet<String> {
    text.lines()
        .filter_map(table_name)
        .filter_map(|name| name.strip_prefix("package.metadata.binstall.overrides."))
        .map(str::to_owned)
        .collect()
}

/// Trailing whitespace and blank edges carry no meaning here; compare the
/// block, not the file's spacing around it.
fn canonical(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let line = line.trim_end();
        if out.is_empty() && line.is_empty() {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Whole-block replacement diff; the hunk is the block, so listing every
/// removed and added line is the complete change.
fn unified_diff(current: &str, expected: &str, package: &str) -> String {
    let mut diff = String::new();
    let _ = writeln!(diff, "--- {package}: a/Cargo.toml");
    let _ = writeln!(diff, "+++ {package}: b/Cargo.toml");
    let _ = writeln!(
        diff,
        "@@ -1,{} +1,{} @@",
        current.lines().count(),
        expected.lines().count()
    );
    for line in current.lines() {
        let _ = writeln!(diff, "-{line}");
    }
    for line in expected.lines() {
        let _ = writeln!(diff, "+{line}");
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = "[package.metadata.binstall.overrides.x86_64-unknown-linux-gnu]\npkg-url = \"{ repo }/releases/download/{ version }/a.tar.gz\"\npkg-fmt = \"tgz\"\n";

    #[test]
    fn spans_cover_adjacent_overrides_and_leave_other_tables() {
        let text = format!(
            "[package]\nname = \"a\"\n\n[package.metadata.simit.ci]\nrunner = \"x\"\n\n{BLOCK}[package.metadata.binstall.overrides.aarch64-apple-darwin]\npkg-fmt = \"zip\"\n\n[profile.release]\nlto = true\n"
        );
        let spans = binstall_spans(&text);
        assert_eq!(spans.len(), 1, "{spans:?}");
        let block = binstall_blocks(&text);
        assert!(block.starts_with("[package.metadata.binstall.overrides.x86_64"));
        assert!(block.ends_with("pkg-fmt = \"zip\"\n\n"), "{block:?}");
        assert_eq!(
            canonical(&block),
            canonical(&format!(
                "{BLOCK}[package.metadata.binstall.overrides.aarch64-apple-darwin]\npkg-fmt = \"zip\"\n"
            ))
        );
    }

    #[test]
    fn archive_targets_reads_override_tables() {
        let text = "[package.metadata.binstall.overrides.x86_64-unknown-linux-musl]\npkg-fmt = \"tgz\"\n\n[package.metadata.binstall.overrides.x86_64-pc-windows-gnu]\npkg-fmt = \"zip\"\n\n[package.metadata.simit.ci]\nrunner = \"x\"\n";
        assert_eq!(
            archive_targets(text).into_iter().collect::<Vec<_>>(),
            vec![
                "x86_64-pc-windows-gnu".to_owned(),
                "x86_64-unknown-linux-musl".to_owned()
            ]
        );
        assert!(archive_targets("").is_empty());
    }

    #[test]
    fn apply_replaces_in_place_and_verifies_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("Cargo.toml");
        let original = "[package]\nname = \"a\"\n\n[package.metadata.binstall.overrides.x86_64-unknown-linux-gnu]\npkg-url = \"stale\"\n\n[profile.release]\nlto = true\n";
        fs::write(&manifest, original).unwrap();
        let update = Update {
            package: "a".to_owned(),
            manifest: manifest.clone(),
            expected: Some(canonical(BLOCK)),
        };
        assert!(matches!(
            check(std::slice::from_ref(&update)).unwrap()[0].1,
            State::Drift { .. }
        ));
        apply(&update).unwrap();
        assert!(matches!(check(&[update]).unwrap()[0].1, State::InSync));
        let written = fs::read_to_string(&manifest).unwrap();
        assert!(
            written.contains("[profile.release]\nlto = true\n"),
            "{written}"
        );
        assert!(!written.contains("stale"), "{written}");
    }

    #[test]
    fn apply_appends_when_the_manifest_has_no_binstall_table() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("Cargo.toml");
        fs::write(&manifest, "[package]\nname = \"a\"").unwrap();
        let update = Update {
            package: "a".to_owned(),
            manifest: manifest.clone(),
            expected: Some(canonical(BLOCK)),
        };
        apply(&update).unwrap();
        let written = fs::read_to_string(&manifest).unwrap();
        assert_eq!(
            written,
            format!("[package]\nname = \"a\"\n\n{}", canonical(BLOCK))
        );
        assert!(matches!(check(&[update]).unwrap()[0].1, State::InSync));
    }
}

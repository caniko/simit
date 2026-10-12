//! Signed Git source admission before data-only Cargo reconstruction.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use super::archive::CrateMember;

pub(super) fn validate_tree(source: &Path, tracked: &[u8]) -> Result<BTreeSet<PathBuf>> {
    ensure!(
        tracked.is_empty() || tracked.ends_with(&[0]),
        "invalid signed source index"
    );
    let mut links = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for record in tracked
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let record = std::str::from_utf8(record)?;
        let (metadata, relative) = record
            .split_once('\t')
            .context("invalid signed source record")?;
        let fields: Vec<_> = metadata.split_ascii_whitespace().collect();
        ensure!(
            fields.len() == 3
                && fields[2] == "0"
                && matches!(fields[0], "100644" | "100755" | "120000")
                && matches!(fields[1].len(), 40 | 64)
                && fields[1].bytes().all(|byte| byte.is_ascii_hexdigit()),
            "signed packaging source contains an unsupported entry or submodule"
        );
        let relative = Path::new(relative);
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
                && seen.insert(relative.to_path_buf()),
            "invalid or duplicate signed source path"
        );
        let path = source.join(relative);
        // core.symlinks=false must have produced an inert regular file containing
        // the Git link-target blob. Never follow or read its referenced target.
        ensure!(
            fs::symlink_metadata(&path)?.file_type().is_file(),
            "signed source checkout contains a filesystem link or non-file"
        );
        if fields[0] == "120000" {
            links.insert(path);
        }
    }
    Ok(links)
}

pub(super) fn reject_link_members(
    members: &BTreeMap<String, CrateMember>,
    root: &str,
    source: &Path,
    links: &BTreeSet<PathBuf>,
) -> Result<()> {
    let vcs = members
        .get(&format!("{root}/.cargo_vcs_info.json"))
        .context("crate has no source binding")?;
    let vcs: serde_json::Value = serde_json::from_slice(&vcs.contents)?;
    let package = vcs["path_in_vcs"]
        .as_str()
        .context("crate has no package source path")?;
    ensure!(
        package.is_empty()
            || Path::new(package)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
        "invalid package source path"
    );
    let prefix = format!("{root}/");
    for member in members.keys() {
        let relative = member
            .strip_prefix(&prefix)
            .context("invalid crate member root")?;
        // Cargo copies the original manifest under a generated filename.
        let relative = if relative == "Cargo.toml.orig" {
            "Cargo.toml"
        } else {
            relative
        };
        ensure!(
            !links.contains(&source.join(package).join(relative)),
            "crate member depends on a signed Git symlink"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrelated_symlink_blob_is_inert_source_data() {
        let source = tempfile::tempdir().unwrap();
        let link = source.path().join(".pre-commit-config.yaml");
        let target = b"/outside/must-never-be-read";
        std::fs::write(&link, target).unwrap();
        let tracked =
            b"120000 0609598af200c6de3c32af590bf245ec3a27dfff 0\t.pre-commit-config.yaml\0";
        let links = validate_tree(source.path(), tracked).unwrap();
        assert_eq!(links, BTreeSet::from([link.clone()]));
        assert!(std::fs::symlink_metadata(&link).unwrap().is_file());
        assert_eq!(std::fs::read(link).unwrap(), target);
    }

    #[test]
    fn unsupported_or_unmerged_git_entries_and_unsafe_paths_are_rejected() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("entry"), "data").unwrap();
        let blob = "0609598af200c6de3c32af590bf245ec3a27dfff";
        for record in [
            format!("160000 {blob} 0\tentry\0"),
            format!("040000 {blob} 0\tentry\0"),
            format!("100644 {blob} 1\tentry\0"),
            "100644 invalid 0\tentry\0".to_owned(),
            format!("100644 {blob} 0\t../entry\0"),
            format!("100644 {blob} 0\t/entry\0"),
            format!("100644 {blob} 0\tentry"),
            format!("100644 {blob} 0\tentry\0").repeat(2),
        ] {
            assert!(
                validate_tree(source.path(), record.as_bytes()).is_err(),
                "{record:?}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn filesystem_links_are_rejected_even_when_the_index_advertises_regular_files() {
        let source = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/outside/not-readable", source.path().join("entry")).unwrap();
        for mode in ["100644", "100755", "120000"] {
            let record = format!("{mode} 0609598af200c6de3c32af590bf245ec3a27dfff 0\tentry\0");
            assert!(validate_tree(source.path(), record.as_bytes()).is_err());
        }
    }

    #[test]
    fn reconstructed_archive_may_not_package_git_symlink_blobs() {
        let source = Path::new("/isolated/source");
        let root = "demo-1.0.0";
        let member = |contents: &[u8]| CrateMember {
            contents: contents.to_vec(),
            mode: 0o644,
        };
        let mut members = BTreeMap::from([
            (
                format!("{root}/.cargo_vcs_info.json"),
                member(br#"{"path_in_vcs":"crates/demo"}"#),
            ),
            (format!("{root}/src/lib.rs"), member(b"regular source")),
            (
                format!("{root}/Cargo.toml.orig"),
                member(b"original manifest"),
            ),
        ]);
        reject_link_members(
            &members,
            root,
            source,
            &BTreeSet::from([source.join(".pre-commit-config.yaml")]),
        )
        .unwrap();
        for relative in ["crates/demo/src/lib.rs", "crates/demo/Cargo.toml"] {
            assert!(
                reject_link_members(
                    &members,
                    root,
                    source,
                    &BTreeSet::from([source.join(relative)])
                )
                .is_err()
            );
        }
        members.insert(
            format!("{root}/.pre-commit-config.yaml"),
            member(b"link-target"),
        );
        assert!(
            reject_link_members(
                &members,
                root,
                source,
                &BTreeSet::from([source.join("crates/demo/.pre-commit-config.yaml")])
            )
            .is_err()
        );
        for path in ["../escape", "/escape"] {
            members.insert(
                format!("{root}/.cargo_vcs_info.json"),
                member(
                    serde_json::json!({"path_in_vcs":path})
                        .to_string()
                        .as_bytes(),
                ),
            );
            assert!(reject_link_members(&members, root, source, &BTreeSet::new()).is_err());
        }
    }
}

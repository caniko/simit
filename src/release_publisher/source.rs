//! Signed Git source admission before data-only Cargo reconstruction.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};

use super::archive::CrateMember;
use super::execution::output;

/// Superproject gitlink path to the exact commit admitted at that path.
pub(super) type Gitlinks = BTreeMap<PathBuf, String>;

#[derive(Debug, Default)]
pub(super) struct Submodules {
    pub(super) gitlinks: Gitlinks,
    /// Inert Git symlink blobs inside admitted submodule trees.
    pub(super) links: BTreeSet<PathBuf>,
}

/// The only accepted submodule remote prefix: the superproject's GitHub owner.
pub(super) fn github_owner_prefix(repository: &str) -> Result<String> {
    let (owner, _) = repository
        .split_once('/')
        .context("invalid provider repository")?;
    ensure!(
        !owner.is_empty()
            && owner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid provider repository owner"
    );
    Ok(format!("https://github.com/{owner}/"))
}

/// Exactly `<prefix><repository>[.git]`: no relative, SSH, file, credential,
/// port, query or nested-path forms. Production always passes the HTTPS
/// owner prefix; only in-crate tests pass a `file://` fixture prefix.
fn submodule_url_allowed(url: &str, prefix: &str) -> bool {
    let Some(name) = url.strip_prefix(prefix) else {
        return false;
    };
    let name = name.strip_suffix(".git").unwrap_or(name);
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn normal_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

/// The signed superproject tag is the sole root of trust. Its tree pins every
/// gitlink to an exact commit, so submodule content is admitted transitively
/// and needs no signature of its own. `.gitmodules` is read from that signed
/// tree, never the working tree, and only supplies the fetch location.
pub(super) fn admit_submodules(
    git: &dyn Fn() -> Result<Command>,
    source: &Path,
    revision: &str,
    prefix: &str,
) -> Result<Submodules> {
    let run = |directory: &Path, args: &[&str]| -> Result<Vec<u8>> {
        Ok(output(git()?.arg("-C").arg(directory).args(args))?.stdout)
    };
    let tree = run(source, &["ls-tree", "-r", "-z", "--full-tree", revision])?;
    let mut gitlinks = Gitlinks::new();
    let mut declarations = false;
    for record in tree.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
        let (metadata, path) = std::str::from_utf8(record)?
            .split_once('\t')
            .context("invalid signed tree record")?;
        let fields: Vec<_> = metadata.split(' ').collect();
        if path == ".gitmodules" {
            ensure!(
                metadata.starts_with("100644 blob "),
                "signed .gitmodules must be a regular file"
            );
            declarations = true;
        }
        if fields[0] == "160000" {
            ensure!(
                fields.len() == 3
                    && fields[1] == "commit"
                    && matches!(fields[2].len(), 40 | 64)
                    && fields[2].bytes().all(|byte| byte.is_ascii_hexdigit())
                    && normal_relative(Path::new(path)),
                "invalid signed gitlink"
            );
            gitlinks.insert(PathBuf::from(path), fields[2].to_owned());
        }
    }
    let mut declared = BTreeMap::new();
    if declarations {
        let config = run(
            source,
            &[
                "config",
                "--blob",
                &format!("{revision}:.gitmodules"),
                "--null",
                "--list",
            ],
        )?;
        let mut entries = BTreeMap::<String, (Option<String>, Option<String>)>::new();
        for entry in config.split(|byte| *byte == 0).filter(|e| !e.is_empty()) {
            let (key, value) = std::str::from_utf8(entry)?
                .split_once('\n')
                .context("valueless .gitmodules key")?;
            let (name, variable) = key
                .strip_prefix("submodule.")
                .and_then(|key| key.rsplit_once('.'))
                .context("unsupported .gitmodules section")?;
            let entry = entries.entry(name.to_owned()).or_default();
            let field = match variable {
                "path" => &mut entry.0,
                "url" => &mut entry.1,
                // Never followed: the gitlink alone selects the commit.
                "branch" => continue,
                _ => bail!("unsupported .gitmodules key"),
            };
            ensure!(
                field.replace(value.to_owned()).is_none(),
                "duplicate .gitmodules key"
            );
        }
        for (path, url) in entries.into_values() {
            let (Some(path), Some(url)) = (path, url) else {
                bail!("submodule declaration requires a path and URL");
            };
            ensure!(
                submodule_url_allowed(&url, prefix),
                "submodule URL must be an HTTPS repository of the superproject owner"
            );
            ensure!(
                declared.insert(PathBuf::from(path), url).is_none(),
                "duplicate submodule path declaration"
            );
        }
    }
    ensure!(
        declared.keys().eq(gitlinks.keys()),
        "every signed gitlink requires exactly one .gitmodules declaration"
    );
    let mut links = BTreeSet::new();
    for (path, id) in &gitlinks {
        let directory = source.join(path);
        ensure!(
            fs::symlink_metadata(&directory)?.is_dir()
                && fs::read_dir(&directory)?.next().is_none(),
            "submodule checkout path must be an empty directory"
        );
        output(git()?.arg("init").arg(&directory))?;
        run(&directory, &["config", "core.symlinks", "false"])?;
        run(
            &directory,
            &["fetch", "--no-tags", "--", &declared[path], id],
        )?;
        run(&directory, &["checkout", "--detach", id])?;
        verify_head(
            &run(&directory, &["rev-parse", "--verify", "HEAD^{commit}"])?,
            id,
        )?;
        // Nested gitlinks are unadmitted here, so the existing rules reject them.
        let tracked = run(&directory, &["ls-files", "--stage", "-z"])?;
        links.extend(validate_signed_tree(
            &directory,
            &tracked,
            &Gitlinks::new(),
        )?);
    }
    Ok(Submodules { gitlinks, links })
}

fn verify_head(head: &[u8], id: &str) -> Result<()> {
    ensure!(
        std::str::from_utf8(head)?.trim() == id,
        "submodule checkout differs from its signed gitlink"
    );
    Ok(())
}

pub(super) fn validate_tree(source: &Path, tracked: &[u8]) -> Result<BTreeSet<PathBuf>> {
    validate_signed_tree(source, tracked, &Gitlinks::new())
}

/// Gitlinks are accepted only when `admit_submodules` checked out that exact commit.
pub(super) fn validate_signed_tree(
    source: &Path,
    tracked: &[u8],
    gitlinks: &Gitlinks,
) -> Result<BTreeSet<PathBuf>> {
    ensure!(
        tracked.is_empty() || tracked.ends_with(&[0]),
        "invalid signed source index"
    );
    let mut links = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut admitted = 0;
    for record in tracked
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let record = std::str::from_utf8(record)?;
        let (metadata, relative) = record
            .split_once('\t')
            .context("invalid signed source record")?;
        let fields: Vec<_> = metadata.split_ascii_whitespace().collect();
        let relative = Path::new(relative);
        let gitlink = fields.first() == Some(&"160000")
            && gitlinks.get(relative).map(String::as_str) == fields.get(1).copied();
        ensure!(
            fields.len() == 3
                && fields[2] == "0"
                && (gitlink || matches!(fields[0], "100644" | "100755" | "120000"))
                && matches!(fields[1].len(), 40 | 64)
                && fields[1].bytes().all(|byte| byte.is_ascii_hexdigit()),
            "signed packaging source contains an unsupported entry or submodule"
        );
        ensure!(
            normal_relative(relative) && seen.insert(relative.to_path_buf()),
            "invalid or duplicate signed source path"
        );
        let path = source.join(relative);
        if gitlink {
            // Its tree was validated by admit_submodules at the pinned commit.
            ensure!(
                fs::symlink_metadata(&path)?.is_dir(),
                "admitted submodule checkout is not a directory"
            );
            admitted += 1;
            continue;
        }
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
    ensure!(
        admitted == gitlinks.len(),
        "admitted submodule is absent from the signed source index"
    );
    Ok(links)
}

pub(super) fn reject_link_members(
    members: &BTreeMap<String, CrateMember>,
    root: &str,
    source: &Path,
    links: &BTreeSet<PathBuf>,
    gitlinks: &Gitlinks,
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
        // Cargo records path_in_vcs relative to the package's own repository,
        // which may be any admitted submodule; check every candidate root.
        ensure!(
            std::iter::once(source.to_path_buf())
                .chain(gitlinks.keys().map(|path| source.join(path)))
                .all(|base| !links.contains(&base.join(package).join(relative))),
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
    fn submodule_urls_must_be_https_repositories_of_the_superproject_owner() {
        let prefix = github_owner_prefix("caniko/harbor").unwrap();
        for url in [
            "https://github.com/caniko/harbor-core",
            "https://github.com/caniko/harbor_core.git",
        ] {
            assert!(submodule_url_allowed(url, &prefix), "{url}");
        }
        for url in [
            "https://github.com/someone-else/core",
            "https://gitlab.com/caniko/core",
            "https://github.com.evil/caniko/core",
            "https://token@github.com/caniko/core",
            "https://github.com:443/caniko/core",
            "https://github.com/caniko/core/../other",
            "https://github.com/caniko/core?ref=x",
            "https://github.com/caniko/.git",
            "https://github.com/caniko/",
            "HTTPS://github.com/caniko/core",
            "http://github.com/caniko/core",
            "ssh://git@github.com/caniko/core",
            "git@github.com:caniko/core",
            "git://github.com/caniko/core",
            "file:///srv/caniko/core",
            "../core",
            "./core",
            "core",
        ] {
            assert!(!submodule_url_allowed(url, &prefix), "{url}");
        }
        assert!(github_owner_prefix("caniko").is_err());
    }

    #[test]
    fn submodule_head_must_equal_its_signed_gitlink() {
        let pinned = "a".repeat(40);
        verify_head(format!("{pinned}\n").as_bytes(), &pinned).unwrap();
        assert!(verify_head(format!("{}\n", "b".repeat(40)).as_bytes(), &pinned).is_err());
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
            &Gitlinks::new(),
        )
        .unwrap();
        for relative in ["crates/demo/src/lib.rs", "crates/demo/Cargo.toml"] {
            assert!(
                reject_link_members(
                    &members,
                    root,
                    source,
                    &BTreeSet::from([source.join(relative)]),
                    &Gitlinks::new()
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
                &BTreeSet::from([source.join("crates/demo/.pre-commit-config.yaml")]),
                &Gitlinks::new()
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
            assert!(
                reject_link_members(&members, root, source, &BTreeSet::new(), &Gitlinks::new())
                    .is_err()
            );
        }
    }
}

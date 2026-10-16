//! Hash the actual commit/tree/blob bytes used for source authorization.
use anyhow::{Context, Result, ensure};
use sha1::{Digest, Sha1};
use std::path::Path;

pub(super) fn object(
    probe: &Path,
    git: &Path,
    kind: &str,
    oid: &str,
    name: &str,
    control: &super::SourceControl<'_>,
    maximum: u64,
) -> Result<Vec<u8>> {
    ensure!(
        oid.len() == 40
            && oid
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid source object id"
    );
    let raw = super::source_probe_bytes(
        probe,
        name,
        std::process::Command::new(git)
            .arg("--no-replace-objects")
            .args(["cat-file", kind, oid]),
        control,
        maximum,
    )?;
    let mut hash = Sha1::new();
    hash.update(format!("{kind} {}\0", raw.len()).as_bytes());
    hash.update(&raw);
    ensure!(
        hex::encode(hash.finalize()) == oid,
        "source object integrity mismatch ({kind})"
    );
    Ok(raw)
}

pub(super) fn blob_id(
    probe: &Path,
    git: &Path,
    commit: &[u8],
    path: &str,
    control: &super::SourceControl<'_>,
) -> Result<String> {
    // Only the structural header is ASCII; Git permits non-UTF8 author/message
    // bytes, which have already been hashed as their exact original contents.
    let tree = commit
        .split(|&byte| byte == b'\n')
        .next()
        .and_then(|line| line.strip_prefix(b"tree "))
        .context("source commit missing tree")?;
    let mut tree = std::str::from_utf8(tree)?.to_owned();
    let components = path.split('/').collect::<Vec<_>>();
    ensure!(
        !components.is_empty() && components.len() <= 64,
        "source path depth exceeded"
    );
    for (index, component) in components.iter().enumerate() {
        let raw = object(
            probe,
            git,
            "tree",
            &tree,
            &format!("tree-{index}"),
            control,
            16 * 1024 * 1024,
        )?;
        let mut entries = raw.as_slice();
        let mut selected = None;
        while !entries.is_empty() {
            let space = entries
                .iter()
                .position(|&byte| byte == b' ')
                .context("invalid tree mode")?;
            let zero = entries
                .iter()
                .position(|&byte| byte == 0)
                .context("invalid tree name")?;
            ensure!(
                space < zero && zero + 21 <= entries.len(),
                "invalid tree entry"
            );
            if &entries[space + 1..zero] == component.as_bytes() {
                ensure!(selected.is_none(), "ambiguous source tree entry");
                let final_component = index + 1 == components.len();
                ensure!(
                    if final_component {
                        matches!(&entries[..space], b"100644" | b"100755")
                    } else {
                        &entries[..space] == b"40000"
                    },
                    "source path is not a tracked regular file"
                );
                selected = Some(hex::encode(&entries[zero + 1..zero + 21]));
            }
            entries = &entries[zero + 21..];
        }
        tree = selected.context("source-owned observer spec path missing from checked tree")?;
    }
    Ok(tree)
}

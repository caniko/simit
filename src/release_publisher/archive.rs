//! Read release archives as bounded data; never extract candidate paths.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use anyhow::{Context, Result, ensure};
use flate2::read::GzDecoder;

pub const MAX_CRATE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CONTENT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MEMBER_BYTES: u64 = 32 * 1024 * 1024;
const MAX_MEMBERS: usize = 10_000;

fn read_bounded(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "archive member exceeds byte limit"
    );
    Ok(bytes)
}

pub fn crate_from_provider_zip(zip_bytes: &[u8], crate_name: &str) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))?;
    ensure!(
        archive.len() == 1,
        "release artifact must contain exactly one crate"
    );
    let member = archive.by_index(0)?;
    ensure!(
        member.name() == crate_name
            && member.is_file()
            && member
                .unix_mode()
                .is_none_or(|mode| mode & 0o170000 == 0o100000)
            && member.size() > 0
            && member.size() <= MAX_CRATE_BYTES,
        "release artifact member identity, type or size is invalid"
    );
    read_bounded(member, MAX_CRATE_BYTES)
}

pub fn crate_members(bytes: &[u8], root: &str) -> Result<BTreeMap<String, Vec<u8>>> {
    ensure!(
        bytes.len() as u64 <= MAX_CRATE_BYTES,
        "crate exceeds byte limit"
    );
    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let mut members = BTreeMap::new();
    let mut total = 0_u64;
    for entry in archive.entries()? {
        let entry = entry?;
        ensure!(
            entry.header().entry_type().is_file(),
            "crate contains a non-regular member"
        );
        let path = std::str::from_utf8(&entry.path_bytes())?.to_owned();
        ensure!(
            path.strip_prefix(&format!("{root}/"))
                .is_some_and(|relative| {
                    !relative.is_empty()
                        && !relative.contains('\\')
                        && !relative.bytes().any(|byte| byte.is_ascii_control())
                        && relative
                            .split('/')
                            .all(|part| !part.is_empty() && part != "." && part != "..")
                }),
            "crate member is outside the expected package root"
        );
        ensure!(
            members.len() < MAX_MEMBERS,
            "crate contains too many members"
        );
        let content = read_bounded(entry, MAX_MEMBER_BYTES)?;
        total += content.len() as u64;
        ensure!(
            total <= MAX_CONTENT_BYTES,
            "crate exceeds expanded byte limit"
        );
        ensure!(
            members.insert(path, content).is_none(),
            "crate contains duplicate members"
        );
    }
    ensure!(!members.is_empty(), "crate is empty");
    Ok(members)
}

/// Reconstruct with trusted Cargo's non-executing `package --no-verify` route in
/// an isolated configuration. The signed Git source, not candidate metadata or
/// `.cargo_vcs_info.json`, supplies the expected contents, including normalization.
pub fn verify_reconstructed_crate(
    candidate: &[u8],
    reconstructed: &[u8],
    root: &str,
) -> Result<()> {
    let candidate = crate_members(candidate, root).context("read candidate crate")?;
    let reconstructed = crate_members(reconstructed, root).context("read signed-source crate")?;
    ensure!(
        candidate == reconstructed,
        "crate contents differ from reconstructed signed source"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};

    use super::*;

    fn crate_bytes(entries: &[(&str, &[u8], tar::EntryType)]) -> Vec<u8> {
        let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for (name, content, kind) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(content.len() as u64);
            header.set_entry_type(*kind);
            header.set_cksum();
            tar.append_data(&mut header, name, *content).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn rejects_duplicate_link_and_wrong_root_members() {
        let regular = tar::EntryType::Regular;
        for entries in [
            vec![
                ("demo-1.0.0/a", &b"one"[..], regular),
                ("demo-1.0.0/a", &b"two"[..], regular),
            ],
            vec![("other-1.0.0/a", &b"one"[..], regular)],
            vec![("demo-1.0.0/link", &b""[..], tar::EntryType::Symlink)],
            vec![("demo-1.0.0/link", &b""[..], tar::EntryType::Link)],
        ] {
            assert!(crate_members(&crate_bytes(&entries), "demo-1.0.0").is_err());
        }
    }

    #[test]
    fn rejects_manifest_or_source_replacement_and_extra_files() {
        let regular = tar::EntryType::Regular;
        let expected = crate_bytes(&[("demo-1.0.0/Cargo.toml", b"trusted", regular)]);
        verify_reconstructed_crate(&expected, &expected, "demo-1.0.0").unwrap();
        for candidate in [
            crate_bytes(&[("demo-1.0.0/Cargo.toml", b"replaced", regular)]),
            crate_bytes(&[
                ("demo-1.0.0/Cargo.toml", b"trusted", regular),
                ("demo-1.0.0/build.rs", b"malicious", regular),
            ]),
        ] {
            assert!(verify_reconstructed_crate(&candidate, &expected, "demo-1.0.0").is_err());
        }
    }

    #[test]
    fn provider_zip_admits_only_one_expected_regular_crate() {
        let make = |names: &[&str]| {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for name in names {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(b"crate bytes").unwrap();
            }
            zip.finish().unwrap().into_inner()
        };
        assert_eq!(
            crate_from_provider_zip(&make(&["demo-1.0.0.crate"]), "demo-1.0.0.crate").unwrap(),
            b"crate bytes"
        );
        for names in [
            vec!["../demo-1.0.0.crate"],
            vec!["other.crate"],
            vec!["demo-1.0.0.crate", "extra"],
        ] {
            assert!(crate_from_provider_zip(&make(&names), "demo-1.0.0.crate").is_err());
        }
    }
}

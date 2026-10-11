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

#[derive(Debug, PartialEq, Eq)]
pub struct CrateMember {
    pub contents: Vec<u8>,
    pub mode: u32,
}

pub fn crate_members(bytes: &[u8], root: &str) -> Result<BTreeMap<String, CrateMember>> {
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
        let mode = entry.header().mode()?;
        ensure!(
            mode & !0o777 == 0,
            "crate contains unsupported permission bits"
        );
        let content = read_bounded(entry, MAX_MEMBER_BYTES)?;
        total += content.len() as u64;
        ensure!(
            total <= MAX_CONTENT_BYTES,
            "crate exceeds expanded byte limit"
        );
        ensure!(
            members
                .insert(
                    path,
                    CrateMember {
                        contents: content,
                        mode
                    }
                )
                .is_none(),
            "crate contains duplicate members"
        );
    }
    ensure!(!members.is_empty(), "crate is empty");
    Ok(members)
}

/// Reconstruct with trusted Cargo's non-executing `package --no-verify` route in
/// an isolated configuration. The signed Git source, not candidate metadata or
/// `.cargo_vcs_info.json`, supplies the expected contents. Cargo-generated TOML
/// is compared as typed data so formatter/header changes across Cargo versions
/// do not change admission. All member paths and modes remain exact; original
/// manifests, source, scripts and other files retain byte-for-byte comparison.
pub fn verify_reconstructed_crate(
    candidate: &[u8],
    reconstructed: &[u8],
    root: &str,
) -> Result<()> {
    let candidate = crate_members(candidate, root).context("read candidate crate")?;
    let reconstructed = crate_members(reconstructed, root).context("read signed-source crate")?;
    ensure!(
        candidate.len() == reconstructed.len()
            && candidate.iter().all(|(path, member)| {
                let Some(expected) = reconstructed.get(path) else {
                    return false;
                };
                if member.mode != expected.mode {
                    return false;
                }
                if member.contents == expected.contents {
                    return true;
                }
                let lock = path == &format!("{root}/Cargo.lock");
                if !lock && path != &format!("{root}/Cargo.toml") {
                    return false;
                }
                match (
                    canonical_cargo_toml(&member.contents, lock),
                    canonical_cargo_toml(&expected.contents, lock),
                ) {
                    (Ok(actual), Ok(expected)) => actual == expected,
                    _ => false,
                }
            }),
        "crate contents differ from reconstructed signed source"
    );
    Ok(())
}

fn canonical_cargo_toml(bytes: &[u8], lock: bool) -> Result<serde_json::Value> {
    let mut value: serde_json::Value = toml_edit::de::from_str(std::str::from_utf8(bytes)?)?;
    if lock {
        // Formats 3 and 4 encode the same package graph; only the document
        // format marker is version-dependent. Package versions/checksums and
        // dependency identities are retained in full.
        ensure!(
            matches!(value["version"].as_u64(), Some(3 | 4)),
            "unsupported Cargo lock format"
        );
        value
            .as_object_mut()
            .context("Cargo lock is not a table")?
            .remove("version");
        if let Some(packages) = value["package"].as_array_mut() {
            for package in packages.iter_mut() {
                if let Some(dependencies) = package["dependencies"].as_array_mut() {
                    dependencies.sort_by_key(serde_json::Value::to_string);
                }
            }
            packages.sort_by_key(serde_json::Value::to_string);
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};

    use super::*;

    fn crate_bytes(entries: &[(&str, &[u8], tar::EntryType)]) -> Vec<u8> {
        crate_bytes_with_mode(entries, 0o644)
    }

    fn crate_bytes_with_mode(entries: &[(&str, &[u8], tar::EntryType)], mode: u32) -> Vec<u8> {
        let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for (name, content, kind) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(mode);
            header.set_size(content.len() as u64);
            header.set_entry_type(*kind);
            header.set_cksum();
            tar.append_data(&mut header, name, *content).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn reconstruction_rejects_file_mode_changes_even_when_bytes_match() {
        let entries = [(
            "demo-1.0.0/build.rs",
            &b"trusted bytes"[..],
            tar::EntryType::Regular,
        )];
        let expected = crate_bytes_with_mode(&entries, 0o644);
        for mode in [0o755, 0o600, 0o666, 0o4644] {
            assert!(
                verify_reconstructed_crate(
                    &crate_bytes_with_mode(&entries, mode),
                    &expected,
                    "demo-1.0.0"
                )
                .is_err(),
                "mode {mode:o}"
            );
        }
        let executable = crate_bytes_with_mode(&entries, 0o755);
        verify_reconstructed_crate(&executable, &executable, "demo-1.0.0").unwrap();
    }

    #[test]
    fn cargo_generated_formatting_is_canonical_but_manifest_and_lock_identity_are_preserved() {
        let regular = tar::EntryType::Regular;
        let manifest = b"[package]\nname='demo'\nversion='1.0.0'\n[dependencies.other]\nversion='=2.0.0'\nfeatures=['safe']\n";
        let expected = crate_bytes(&[
            ("demo-1.0.0/Cargo.toml", manifest, regular),
            (
                "demo-1.0.0/Cargo.lock",
                b"version=4\n[[package]]\nname='demo'\nversion='1.0.0'\n",
                regular,
            ),
        ]);
        let reformatted = crate_bytes(&[("demo-1.0.0/Cargo.toml", b"# Cargo generated header\n[package]\nversion = \"1.0.0\"\nname = \"demo\"\n[dependencies.other]\nfeatures = [\n  \"safe\",\n]\nversion = \"=2.0.0\"\n", regular), ("demo-1.0.0/Cargo.lock", b"# Another Cargo version's generated header\nversion = 4\n[[package]]\nversion = \"1.0.0\"\nname = \"demo\"\n", regular)]);
        verify_reconstructed_crate(&reformatted, &expected, "demo-1.0.0").unwrap();
        for changed in [
            crate_bytes(&[("demo-1.0.0/Cargo.toml", b"[package]\nname='demo'\nversion='1.0.0'\n[dependencies.other]\nversion='=9.0.0'\nfeatures=['safe']\n", regular), ("demo-1.0.0/Cargo.lock", b"version=4\n[[package]]\nname='demo'\nversion='1.0.0'\n", regular)]),
            crate_bytes(&[("demo-1.0.0/Cargo.toml", manifest, regular), ("demo-1.0.0/Cargo.lock", b"version=4\n[[package]]\nname='other'\nversion='9.0.0'\n", regular)]),
            crate_bytes_with_mode(&[("demo-1.0.0/Cargo.toml", manifest, regular), ("demo-1.0.0/Cargo.lock", b"version=4\n[[package]]\nname='demo'\nversion='1.0.0'\n", regular)], 0o755),
        ] { assert!(verify_reconstructed_crate(&changed, &expected, "demo-1.0.0").is_err()); }
    }

    #[test]
    fn canonical_lock_preserves_graph_checksums_and_rejects_unknown_formats() {
        let older = b"version=3\n[[package]]\nname='a'\nversion='1.0.0'\nchecksum='trusted'\ndependencies=['z','b']\n[[package]]\nname='b'\nversion='2.0.0'\n";
        let newer = b"version=4\n[[package]]\nname='b'\nversion='2.0.0'\n[[package]]\nname='a'\nversion='1.0.0'\nchecksum='trusted'\ndependencies=['b','z']\n";
        let expected = canonical_cargo_toml(older, true).unwrap();
        assert_eq!(expected, canonical_cargo_toml(newer, true).unwrap());
        let changed = String::from_utf8(newer.to_vec())
            .unwrap()
            .replace("checksum='trusted'", "checksum='changed'");
        assert_ne!(
            expected,
            canonical_cargo_toml(changed.as_bytes(), true).unwrap()
        );
        assert!(canonical_cargo_toml(b"version=5", true).is_err());
    }

    #[test]
    fn original_manifest_bytes_never_use_generated_toml_canonicalization() {
        let regular = tar::EntryType::Regular;
        let expected = crate_bytes(&[(
            "demo-1.0.0/Cargo.toml.orig",
            b"[package]\nname='demo'\n",
            regular,
        )]);
        let changed = crate_bytes(&[(
            "demo-1.0.0/Cargo.toml.orig",
            b"# changed\n[package]\nname='demo'\n",
            regular,
        )]);
        assert!(verify_reconstructed_crate(&changed, &expected, "demo-1.0.0").is_err());
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

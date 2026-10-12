//! Binary grammar contracts independent of the export-stream implementation.
use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};
use simit::ci_runtime::{
    cancellation::Cancellation,
    dependencies::{ArchiveAdmission, admit_archive, validate_export},
};
use std::{
    collections::BTreeSet,
    fs,
    io::{Cursor, Read, Write},
    path::Path,
};

#[path = "fixtures/nix-export-wire.rs"]
mod wire;
use wire::*;
fn admitted(closure: Vec<String>) -> ArchiveAdmission {
    ArchiveAdmission {
        closure,
        outputs: vec![path('0', "input")],
        qualification_outputs: BTreeSet::from([path('2', "qualified")]),
        archive_bytes: 0,
        archive_sha256: String::new(),
        uncompressed_bytes: 0,
        maximum_bytes: 1024 * 1024,
    }
}
fn parse(raw: &[u8], admission: &ArchiveAdmission) -> anyhow::Result<()> {
    validate_export(
        &mut Cursor::new(raw),
        raw.len() as u64,
        admission,
        &Cancellation::default(),
    )
}
fn write_archive(root: &Path, raw: &[u8], admission: &mut ArchiveAdmission) -> std::path::PathBuf {
    let archive = root.join("closure.nar.gz");
    let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
    gzip.write_all(raw).unwrap();
    let gzip = gzip.finish().unwrap();
    fs::write(&archive, &gzip).unwrap();
    admission.archive_bytes = gzip.len() as u64;
    admission.uncompressed_bytes = raw.len() as u64;
    admission.archive_sha256 = hex::encode(Sha256::digest(&gzip));
    archive
}

#[test]
fn entire_record_inventory_and_closed_sorted_references_are_admitted() {
    let root = path('0', "input");
    let other = path('3', "other");
    let admission = admitted(vec![root.clone(), other.clone()]);
    let raw = export(vec![
        record(&other, regular(b"other", false), &[], ""),
        record(
            &root,
            regular(b"input", true),
            &[root.clone(), other],
            &path('1', "input.drv"),
        ),
    ]);
    parse(&raw, &admission).unwrap();
}

#[test]
fn exact_valid_importer_edges_admit_the_complete_stream() {
    let root = path('0', "input");
    let other = path('3', "other");
    let admission = admitted(vec![root.clone(), other.clone()]);
    for node in [
        directory(vec![(vec![b'a'; 255], regular(b"", false))]),
        link(&vec![b'x'; 4095]),
        depth(63),
    ] {
        parse(
            &export(vec![
                record(&root, regular(b"input", false), &[], ""),
                record(&other, node, &[], ""),
            ]),
            &admission,
        )
        .unwrap();
    }
}

#[test]
fn every_truncated_prefix_and_bad_framing_is_denied() {
    let admission = admitted(vec![path('0', "input")]);
    let raw = export(vec![record(
        &admission.outputs[0],
        regular(b"input", false),
        &[],
        "",
    )]);
    for size in 0..raw.len() {
        assert!(parse(&raw[..size], &admission).is_err(), "prefix {size}");
    }
    for invalid in [
        [raw.clone(), vec![1]].concat(),
        [raw.clone(), raw.clone()].concat(),
        [number(2), raw[8..].to_vec()].concat(),
        [number(1), number(u64::MAX)].concat(),
        [raw[..raw.len() - 16].to_vec(), number(1), number(0)].concat(),
    ] {
        assert!(parse(&invalid, &admission).is_err());
    }
    let mut bad_magic = raw.clone();
    let position = raw
        .windows(8)
        .position(|part| part == number(0x4558494e))
        .unwrap();
    bad_magic[position..position + 8].copy_from_slice(&number(0x1234));
    assert!(parse(&bad_magic, &admission).is_err());
    let position = raw
        .windows(8)
        .position(|part| part == b"input\0\0\0")
        .unwrap();
    let mut bad_padding = raw;
    bad_padding[position + 5] = 1;
    assert!(parse(&bad_padding, &admission).is_err());
}

#[test]
fn malformed_paths_inventories_references_and_nar_fields_are_denied() {
    let out = path('0', "input");
    let other = path('3', "other");
    let admission = admitted(vec![out.clone()]);
    for raw in [
        export(vec![]),
        export(vec![
            record(&out, regular(b"", false), &[], ""),
            record(&out, regular(b"", false), &[], ""),
        ]),
        export(vec![record(
            &out,
            regular(b"", false),
            std::slice::from_ref(&other),
            "",
        )]),
        export(vec![record(
            &out,
            regular(b"", false),
            &[out.clone(), out.clone()],
            "",
        )]),
        export(vec![record(
            &(out.clone() + "/child"),
            regular(b"", false),
            &[],
            "",
        )]),
        export(vec![record(
            &path('e', "input"),
            regular(b"", false),
            &[],
            "",
        )]),
        export(vec![record(
            &path('0', ".-invalid"),
            regular(b"", false),
            &[],
            "",
        )]),
        export(vec![record(
            &out,
            regular(b"", false),
            &[],
            "/nix/store/../escape",
        )]),
    ] {
        assert!(parse(&raw, &admission).is_err());
    }
    for node in [
        directory(vec![
            (b"b".to_vec(), regular(b"", false)),
            (b"a".to_vec(), regular(b"", false)),
        ]),
        directory(vec![
            (b"a".to_vec(), regular(b"", false)),
            (b"a".to_vec(), regular(b"", false)),
        ]),
        directory(vec![(vec![], regular(b"", false))]),
        directory(vec![(b"..".to_vec(), regular(b"", false))]),
        directory(vec![(b"a/b".to_vec(), regular(b"", false))]),
        directory(vec![(b"a\0b".to_vec(), regular(b"", false))]),
        link(b""),
        link(b"x\0y"),
        [
            string("("),
            string("type"),
            string("regular"),
            string("contents"),
            number(u64::MAX),
        ]
        .concat(),
        [
            string("("),
            string("type"),
            string("regular"),
            string("executable"),
            string("nonempty"),
        ]
        .concat(),
    ] {
        assert!(parse(&export(vec![record(&out, node, &[], "")]), &admission).is_err());
    }
    let mut invalid = admission.clone();
    invalid.closure.push(out.clone());
    assert!(
        parse(
            &export(vec![record(&out, regular(b"", false), &[], "")]),
            &invalid
        )
        .is_err()
    );
    invalid = admitted(vec![other.clone()]);
    assert!(
        parse(
            &export(vec![record(&other, regular(b"", false), &[], "")]),
            &invalid
        )
        .is_err()
    );
    invalid = admitted(vec![out.clone(), path('2', "qualified")]);
    assert!(
        parse(
            &export(vec![record(&out, regular(b"", false), &[], "")]),
            &invalid
        )
        .is_err()
    );
}

#[test]
fn first_invalid_importer_edge_after_valid_record_has_zero_import_effects() {
    let root = tempfile::tempdir().unwrap();
    let out = path('0', "input");
    let other = path('3', "other");
    for node in [
        directory(vec![(vec![b'a'; 256], regular(b"", false))]),
        link(&vec![b'a'; 4096]),
        depth(64),
    ] {
        let mut admission = admitted(vec![out.clone(), other.clone()]);
        let raw = export(vec![
            record(&out, regular(b"input", false), &[], ""),
            record(&other, node, &[], ""),
        ]);
        let archive = write_archive(root.path(), &raw, &mut admission);
        let mut imports = 0;
        let result = admit_archive(&archive, root.path(), &admission, &Cancellation::default())
            .and_then(|stream| {
                stream.consume(|_| {
                    imports += 1;
                    Ok(())
                })
            });
        assert!(result.is_err());
        assert_eq!(imports, 0);
    }
}

#[test]
fn consistent_checksums_cannot_authorize_extra_paths_or_trailing_data() {
    let root = tempfile::tempdir().unwrap();
    let out = path('0', "input");
    for extra in [path('3', "other"), path('2', "qualified")] {
        let raw = export(vec![
            record(&out, regular(b"", false), &[], ""),
            record(&extra, regular(b"", false), &[], ""),
        ]);
        let mut admission = admitted(vec![out.clone()]);
        let archive = write_archive(root.path(), &raw, &mut admission);
        let mut imports = 0;
        assert!(
            admit_archive(&archive, root.path(), &admission, &Cancellation::default())
                .and_then(|stream| stream.consume(|_| {
                    imports += 1;
                    Ok(())
                }))
                .is_err()
        );
        assert_eq!(imports, 0);
    }
}

#[test]
fn private_unlinked_rewound_same_descriptor_survives_archive_path_replacement() {
    let root = tempfile::tempdir().unwrap();
    let out = path('0', "input");
    let raw = export(vec![record(
        &out,
        directory(vec![(vec![0xff], link(b"../target"))]),
        std::slice::from_ref(&out),
        &path('3', "metadata-only.drv"),
    )]);
    let mut admission = admitted(vec![out]);
    let archive = write_archive(root.path(), &raw, &mut admission);
    let stream =
        admit_archive(&archive, root.path(), &admission, &Cancellation::default()).unwrap();
    fs::write(&archive, "replaced transport").unwrap();
    stream
        .consume(|mut fd| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                assert_eq!(fd.metadata()?.mode() & 0o777, 0o600);
                assert_eq!(fd.metadata()?.nlink(), 0);
            }
            let mut imported = Vec::new();
            fd.read_to_end(&mut imported)?;
            assert_eq!(imported, raw);
            Ok(())
        })
        .unwrap();
}

#[test]
fn checksum_sizes_budget_corrupt_gzip_and_cancellation_deny_before_consumer() {
    let root = tempfile::tempdir().unwrap();
    let out = path('0', "input");
    let raw = export(vec![record(&out, regular(b"input", false), &[], "")]);
    let mut admission = admitted(vec![out]);
    let archive = write_archive(root.path(), &raw, &mut admission);
    let mut invalids = Vec::new();
    let mut wrong = admission.clone();
    wrong.archive_sha256 = "0".repeat(64);
    invalids.push(wrong);
    let mut wrong = admission.clone();
    wrong.archive_bytes += 1;
    invalids.push(wrong);
    let mut wrong = admission.clone();
    wrong.uncompressed_bytes -= 1;
    invalids.push(wrong);
    let mut wrong = admission.clone();
    wrong.maximum_bytes = raw.len() as u64 - 1;
    invalids.push(wrong);
    for wrong in invalids {
        assert!(admit_archive(&archive, root.path(), &wrong, &Cancellation::default()).is_err());
    }
    let cancellation = Cancellation::default();
    cancellation.cancel();
    assert!(admit_archive(&archive, root.path(), &admission, &cancellation).is_err());
    let mut compressed = fs::read(&archive).unwrap();
    compressed.truncate(compressed.len() - 4);
    fs::write(&archive, &compressed).unwrap();
    admission.archive_bytes = compressed.len() as u64;
    admission.archive_sha256 = hex::encode(Sha256::digest(&compressed));
    assert!(admit_archive(&archive, root.path(), &admission, &Cancellation::default()).is_err());
}

#[test]
fn file_payloads_are_streamed_in_bounded_reads() {
    struct Bounded(Cursor<Vec<u8>>);
    impl Read for Bounded {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            assert!(bytes.len() <= 65536);
            self.0.read(bytes)
        }
    }
    let out = path('0', "input");
    let admission = admitted(vec![out.clone()]);
    let raw = export(vec![record(
        &out,
        regular(&vec![b'x'; 3 * 65536 + 1], false),
        &[],
        "",
    )]);
    validate_export(
        &mut Bounded(Cursor::new(raw.clone())),
        raw.len() as u64,
        &admission,
        &Cancellation::default(),
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn real_process_receives_the_fully_validated_rewound_spool_once() {
    use std::process::{Command, Stdio};
    let root = tempfile::tempdir().unwrap();
    let out = path('0', "input");
    let raw = export(vec![record(&out, regular(b"\0binary\xff", true), &[], "")]);
    let mut admission = admitted(vec![out]);
    let archive = write_archive(root.path(), &raw, &mut admission);
    let stream =
        admit_archive(&archive, root.path(), &admission, &Cancellation::default()).unwrap();
    let imported = root.path().join("actual-imported");
    stream
        .consume(|fd| {
            let output = Command::new(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dependency-import.sh"),
            )
            .stdin(Stdio::from(fd))
            .env("DEPENDENCY_IMPORTED", &imported)
            .output()?;
            assert!(output.status.success(), "{output:?}");
            assert_eq!(output.stdout, b"imported validated bytes\n");
            assert_eq!(output.stderr, b"retained import diagnostic\n");
            Ok(())
        })
        .unwrap();
    assert_eq!(fs::read(imported).unwrap(), raw);
}

#[cfg(unix)]
#[test]
fn native_restore_imports_once_then_verifies_and_never_claims_qualification() {
    use simit::ci_runtime::dependencies::StoreTools;
    use std::os::unix::fs::symlink;
    for failure in [
        None,
        Some("import-fail"),
        Some("verify-fail"),
        Some("roots-fail"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let out = path('0', "input");
        let raw = export(vec![record(&out, regular(b"\0binary\xff", true), &[], "")]);
        let mut admission = admitted(vec![out.clone()]);
        let archive = write_archive(root.path(), &raw, &mut admission);
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dependency-store.sh");
        let tools = StoreTools {
            nix_store: root.path().join("nix-store"),
            nix: root.path().join("nix"),
        };
        symlink(&fixture, &tools.nix_store).unwrap();
        symlink(&fixture, &tools.nix).unwrap();
        if let Some(failure) = failure {
            fs::write(root.path().join(failure), "denied").unwrap();
        }
        let stream =
            admit_archive(&archive, root.path(), &admission, &Cancellation::default()).unwrap();
        let result = stream.restore_with_tools(root.path(), &tools);
        assert_eq!(result.is_ok(), failure.is_none(), "{result:?}");
        assert_eq!(fs::read(root.path().join("imported")).unwrap(), raw);
        let effects = fs::read_to_string(root.path().join("effects")).unwrap();
        assert_eq!(
            effects.lines().filter(|line| *line == "--import").count(),
            1
        );
        assert_eq!(
            effects.contains("--verify-path"),
            failure != Some("import-fail")
        );
        assert_eq!(
            effects.contains("path-info"),
            !matches!(failure, Some("import-fail" | "verify-fail"))
        );
        if let Ok(receipt) = result {
            assert_eq!(receipt.restored, vec![out]);
            assert!(!receipt.qualification_execution);
        }
    }
}

#[test]
fn directory_ordering_is_unsigned_bytes_not_utf8_or_signed_chars() {
    let out = path('0', "input");
    let admission = admitted(vec![out.clone()]);
    let entries = vec![
        (b"z".to_vec(), regular(b"", false)),
        (vec![0x80], regular(b"", false)),
        (vec![0xff], regular(b"", false)),
    ];
    parse(
        &export(vec![record(&out, directory(entries.clone()), &[], "")]),
        &admission,
    )
    .unwrap();
    parse(
        &export(vec![record(
            &out,
            directory(entries.into_iter().rev().collect()),
            &[],
            "",
        )]),
        &admission,
    )
    .unwrap_err();
}

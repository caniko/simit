use simit::ci_runtime::log_archive::{
    ArchiveState, Budget, Limits, compare, inspect, inspect_with,
};
use std::{fs::File, io::Write};
use tempfile::TempDir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn archive(
    root: &TempDir,
    name: &str,
    count: usize,
    reverse: bool,
    method: CompressionMethod,
) -> std::path::PathBuf {
    let path = root.path().join(name);
    let mut zip = ZipWriter::new(File::create(&path).unwrap());
    let mut indices = (0..count).collect::<Vec<_>>();
    if reverse {
        indices.reverse();
    }
    for index in indices {
        zip.start_file(
            format!("job/{}_step.txt", index + 1),
            SimpleFileOptions::default().compression_method(method),
        )
        .unwrap();
        writeln!(zip, "native bytes for step {index}").unwrap();
    }
    zip.finish().unwrap();
    path
}

#[test]
fn omission_sequences_are_observations_not_native_custody_loss() {
    let root = TempDir::new().unwrap();
    for (original, subsequent) in [(19, vec![2, 2]), (13, vec![2, 2]), (34, vec![26, 6])] {
        let path = archive(
            &root,
            &format!("original-{original}.zip"),
            original,
            false,
            CompressionMethod::Stored,
        );
        let native = inspect(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        for (index, count) in subsequent.into_iter().enumerate() {
            let fresh = inspect(&archive(
                &root,
                &format!("{original}-{index}.zip"),
                count,
                false,
                CompressionMethod::Stored,
            ))
            .unwrap();
            let observation = compare(&native, &fresh);
            assert_eq!(observation.state, ArchiveState::OmissionOnlyRegression);
            assert_eq!(observation.missing_members.len(), original - count);
            assert!(observation.changed_members.is_empty() && observation.added_members.is_empty());
        }
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

#[test]
fn equal_members_are_equivalent_despite_order_and_compression() {
    let root = TempDir::new().unwrap();
    let first = inspect(&archive(
        &root,
        "stored.zip",
        19,
        false,
        CompressionMethod::Stored,
    ))
    .unwrap();
    let other = inspect(&archive(
        &root,
        "deflated.zip",
        19,
        true,
        CompressionMethod::Deflated,
    ))
    .unwrap();
    assert_ne!(first.sha256, other.sha256);
    assert_eq!(
        compare(&first, &other).state,
        ArchiveState::EquivalentMemberContent
    );
}

#[test]
fn added_changed_and_incompatible_variants_fail_closed() {
    let root = TempDir::new().unwrap();
    let first = inspect(&archive(
        &root,
        "first.zip",
        2,
        false,
        CompressionMethod::Stored,
    ))
    .unwrap();
    let more = inspect(&archive(
        &root,
        "more.zip",
        3,
        false,
        CompressionMethod::Stored,
    ))
    .unwrap();
    assert_eq!(
        compare(&first, &more).state,
        ArchiveState::AddedOrChangedConflict
    );
    let mut changed = more.clone();
    changed.members.get_mut("job/1_step.txt").unwrap().sha256 = "0".repeat(64);
    assert_eq!(
        compare(&first, &changed).changed_members,
        vec!["job/1_step.txt"]
    );
    let incompatible = root.path().join("prefixed.zip");
    let mut bytes = b"foreign prefix".to_vec();
    bytes.extend(
        std::fs::read(archive(
            &root,
            "source.zip",
            2,
            false,
            CompressionMethod::Stored,
        ))
        .unwrap(),
    );
    std::fs::write(&incompatible, bytes).unwrap();
    assert!(inspect(&incompatible).is_err());
}

fn named_archive(root: &TempDir, names: &[&str], zip64: bool) -> std::path::PathBuf {
    let path = root.path().join("names.zip");
    let mut zip = ZipWriter::new(File::create(&path).unwrap());
    for name in names {
        zip.start_file(
            *name,
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .large_file(zip64),
        )
        .unwrap();
        zip.write_all(b"inert content").unwrap();
    }
    zip.finish().unwrap();
    path
}

#[test]
fn names_and_directory_records_are_not_allowed_to_alias() {
    for names in [
        vec!["a.txt", "A.txt"],
        vec!["a/../b.txt"],
        vec!["a//b.txt"],
        vec!["a\\b.txt"],
        vec!["/a.txt"],
        vec!["a.txt", "a.txt/"],
        vec!["é.txt", "e\u{301}.txt"],
        vec!["ß.txt", "ss.txt"],
    ] {
        let root = TempDir::new().unwrap();
        assert!(
            inspect(&named_archive(&root, &names, false)).is_err(),
            "{names:?}"
        );
    }
    let root = TempDir::new().unwrap();
    let path = named_archive(&root, &["a.txt", "b.txt"], false);
    let mut bytes = std::fs::read(&path).unwrap();
    for i in 0..bytes.len() - 5 {
        if bytes[i..i + 5] == *b"b.txt" {
            bytes[i] = b'a';
        }
    }
    std::fs::write(&path, bytes).unwrap();
    assert!(inspect(&path).is_err());
}

#[test]
fn zip64_member_records_are_supported_without_large_allocation() {
    let root = TempDir::new().unwrap();
    let path = named_archive(&root, &["a.txt"], true);
    assert_eq!(inspect(&path).unwrap().members["a.txt"].size, 13);
}

#[test]
fn zip64_directory_footer_and_actual_record_count_are_bounded() {
    let root = TempDir::new().unwrap();
    let path = named_archive(&root, &["a.txt"], true);
    let mut bytes = std::fs::read(&path).unwrap();
    let eocd = bytes.len() - 22;
    let size = u32::from_le_bytes(bytes[eocd + 12..eocd + 16].try_into().unwrap()) as u64;
    let start = u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as u64;
    let mut legacy = bytes.split_off(eocd);
    for range in [8..12, 12..16, 16..20] {
        legacy[range].fill(255);
    }
    bytes.extend(b"PK\x06\x06");
    bytes.extend(44_u64.to_le_bytes());
    bytes.extend(45_u16.to_le_bytes());
    bytes.extend(45_u16.to_le_bytes());
    bytes.extend(0_u32.to_le_bytes());
    bytes.extend(0_u32.to_le_bytes());
    bytes.extend(1_u64.to_le_bytes());
    bytes.extend(1_u64.to_le_bytes());
    bytes.extend(size.to_le_bytes());
    bytes.extend(start.to_le_bytes());
    bytes.extend(b"PK\x06\x07");
    bytes.extend(0_u32.to_le_bytes());
    bytes.extend((eocd as u64).to_le_bytes());
    bytes.extend(1_u32.to_le_bytes());
    bytes.extend(legacy);
    std::fs::write(&path, &bytes).unwrap();
    assert!(inspect(&path).is_ok());
    bytes[eocd + 32] = 0;
    std::fs::write(&path, bytes).unwrap();
    assert!(inspect(&path).is_err());
}

#[test]
fn crc_and_malformed_central_directory_are_rejected() {
    let root = TempDir::new().unwrap();
    let path = named_archive(&root, &["a.txt"], false);
    let original = std::fs::read(&path).unwrap();
    let mut bytes = original.clone();
    let offset = bytes
        .windows(13)
        .position(|b| b == b"inert content")
        .unwrap();
    bytes[offset] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(inspect(&path).is_err());
    let mut bytes = original.clone();
    let offset = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
    bytes[offset + 28] = 255;
    std::fs::write(&path, bytes).unwrap();
    assert!(inspect(&path).is_err());
    let mut bytes = original;
    let offset = bytes.windows(4).position(|b| b == b"PK\x05\x06").unwrap();
    bytes[offset + 10] = 0;
    std::fs::write(&path, bytes).unwrap();
    assert!(inspect(&path).is_err());
}

#[test]
fn local_sizes_crc_and_descriptors_must_match_the_actual_directory() {
    let root = TempDir::new().unwrap();
    let path = named_archive(&root, &["a.txt"], false);
    let original = std::fs::read(&path).unwrap();
    for offset in [14, 18, 22] {
        let mut bytes = original.clone();
        bytes[offset] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        assert!(inspect(&path).is_err(), "local field {offset}");
    }
    let mut bytes = original;
    let central = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
    let crc = bytes[14..18].to_vec();
    let compressed = bytes[18..22].to_vec();
    let expanded = bytes[22..26].to_vec();
    bytes[6] |= 8;
    bytes[central + 8] |= 8;
    bytes[14..26].fill(0);
    let mut descriptor = b"PK\x07\x08".to_vec();
    descriptor.extend(crc);
    descriptor.extend(compressed);
    descriptor.extend(expanded);
    bytes.splice(central..central, descriptor);
    let footer = bytes.len() - 22;
    bytes[footer + 16..footer + 20].copy_from_slice(&((central + 16) as u32).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    assert!(inspect(&path).is_ok());
    bytes[central + 4] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(inspect(&path).is_err());
}

#[test]
fn readback_and_input_variants_cannot_restart_or_evade_the_shared_budget() {
    use simit::ci_runtime::log_custody::persist_with;
    let root = TempDir::new().unwrap();
    let storage = TempDir::new().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(storage.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = named_archive(&root, &["a.txt"], false);
    let native = inspect(&path).unwrap();
    let mut budget = Budget::new(Limits {
        bytes: native.bytes,
        ..Limits::default()
    })
    .unwrap();
    inspect_with(&path, &mut budget).unwrap();
    assert!(persist_with(&path, storage.path(), &native, &mut budget).is_err());
    assert!(
        !storage
            .path()
            .join(format!("{}.zip", native.sha256))
            .exists()
    );
    #[cfg(unix)]
    {
        let symlink = root.path().join("redirect.zip");
        std::os::unix::fs::symlink(&path, &symlink).unwrap();
        assert!(inspect(&symlink).is_err());
        let fifo = root.path().join("fifo.zip");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        assert!(inspect(&fifo).is_err());
    }
}

#[test]
fn actual_byte_member_directory_request_and_time_budgets_are_enforced() {
    let root = TempDir::new().unwrap();
    let path = archive(&root, "bounded.zip", 3, false, CompressionMethod::Stored);
    for limits in [
        Limits {
            bytes: 1,
            ..Limits::default()
        },
        Limits {
            expanded_bytes: 1,
            ..Limits::default()
        },
        Limits {
            members: 2,
            ..Limits::default()
        },
        Limits {
            directory_bytes: 1,
            ..Limits::default()
        },
        Limits {
            seconds: 0,
            ..Limits::default()
        },
    ] {
        assert!(inspect_with(&path, &mut Budget::new(limits).unwrap()).is_err());
    }
    let mut budget = Budget::new(Limits {
        requests: 1,
        ..Limits::default()
    })
    .unwrap();
    budget.request().unwrap();
    assert!(budget.request().is_err());
    assert!(
        Budget::new(Limits {
            members: 10001,
            ..Limits::default()
        })
        .is_err()
    );
}

#[test]
fn declared_small_counts_and_sizes_do_not_bypass_actual_admission() {
    let root = TempDir::new().unwrap();
    let path = archive(
        &root,
        "lying-count.zip",
        3,
        false,
        CompressionMethod::Stored,
    );
    let mut bytes = std::fs::read(&path).unwrap();
    let footer = bytes.len() - 22;
    bytes[footer + 8..footer + 12].copy_from_slice(&[1, 0, 1, 0]);
    std::fs::write(&path, bytes).unwrap();
    assert!(
        inspect_with(
            &path,
            &mut Budget::new(Limits {
                members: 2,
                ..Limits::default()
            })
            .unwrap()
        )
        .is_err()
    );
    let path = archive(
        &root,
        "lying-size.zip",
        1,
        false,
        CompressionMethod::Deflated,
    );
    let mut bytes = std::fs::read(&path).unwrap();
    let directory = bytes
        .windows(4)
        .position(|raw| raw == b"PK\x01\x02")
        .unwrap();
    bytes[22..26].copy_from_slice(&1_u32.to_le_bytes());
    bytes[directory + 24..directory + 28].copy_from_slice(&1_u32.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();
    assert!(
        inspect_with(
            &path,
            &mut Budget::new(Limits {
                expanded_bytes: 1,
                ..Limits::default()
            })
            .unwrap()
        )
        .is_err()
    );
}

#[test]
fn interrupted_or_corrupted_persistence_never_replaces_an_original() {
    use simit::ci_runtime::log_custody::persist;
    let root = TempDir::new().unwrap();
    let storage = TempDir::new().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(storage.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = archive(&root, "native.zip", 19, false, CompressionMethod::Stored);
    let native = inspect(&path).unwrap();
    let retained = persist(&path, storage.path(), &native).unwrap();
    let bytes = std::fs::read(&retained).unwrap();
    let interrupted = storage.path().join("incomplete.tmp");
    std::fs::write(&interrupted, b"partial write").unwrap();
    let mut threads = vec![];
    for _ in 0..4 {
        let path = path.clone();
        let storage = storage.path().to_owned();
        let native = native.clone();
        threads.push(std::thread::spawn(move || {
            persist(&path, &storage, &native)
        }));
    }
    for thread in threads {
        assert_eq!(thread.join().unwrap().unwrap(), retained);
    }
    assert_eq!(std::fs::read(&retained).unwrap(), bytes);
    std::fs::write(&path, b"altered source").unwrap();
    assert!(persist(&path, storage.path(), &native).is_err());
    assert_eq!(std::fs::read(&retained).unwrap(), bytes);
    std::fs::write(&retained, b"altered durable bytes").unwrap();
    assert!(persist(&retained, storage.path(), &native).is_err());
    std::fs::write(&path, &bytes).unwrap();
    assert!(persist(&path, storage.path(), &native).is_err());
    assert_eq!(std::fs::read(&retained).unwrap(), b"altered durable bytes");
    assert!(persist(&retained, &storage.path().join("missing"), &native).is_err());
}

#[test]
fn approved_observer_signature_is_required_for_exact_receipt_bytes() {
    use base64::Engine;
    use simit::ci_runtime::log_custody::{Approval, authenticate};
    let root = TempDir::new().unwrap();
    let raw = include_bytes!("fixtures/native-log-observer-receipt.txt");
    let receipt = root.path().join("receipt.json");
    std::fs::write(&receipt, raw).unwrap();
    let signature = root.path().join("receipt.asc");
    std::fs::write(
        &signature,
        include_bytes!("fixtures/native-log-observer-receipt.txt.asc"),
    )
    .unwrap();
    let encoded = include_str!("fixtures/native-log-observer-key.asc")
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('-') && !line.starts_with('='))
        .collect::<String>();
    let keyring = root.path().join("keyring.gpg");
    std::fs::write(
        &keyring,
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap(),
    )
    .unwrap();
    let mut approval = Approval {
        keyring,
        fingerprint: "0FC1B570BD0A2268C1D2F062F61549E59057035D".into(),
        observer_revision: "fixture-observer-v1".into(),
    };
    authenticate(&receipt, &signature, &approval, &mut Budget::default()).unwrap();
    approval.fingerprint = "0".repeat(40);
    assert!(authenticate(&receipt, &signature, &approval, &mut Budget::default()).is_err());
    approval.fingerprint = "0FC1B570BD0A2268C1D2F062F61549E59057035D".into();
    approval.observer_revision = "wrong-observer".into();
    assert!(authenticate(&receipt, &signature, &approval, &mut Budget::default()).is_err());
    approval.observer_revision = "fixture-observer-v1".into();
    std::fs::write(&receipt, b"altered historical claim").unwrap();
    assert!(authenticate(&receipt, &signature, &approval, &mut Budget::default()).is_err());
    assert!(
        authenticate(
            &receipt,
            &root.path().join("missing-signature"),
            &approval,
            &mut Budget::default()
        )
        .is_err()
    );
}

#[test]
fn native_consumer_retains_diagnostics_without_authorizing_unsigned_or_old_captures() {
    use serde_json::json;
    use simit::ci_runtime::{
        log_custody::Identity,
        log_reconcile::{Manifest, Original},
    };
    let root = TempDir::new().unwrap();
    let original = archive(&root, "original.zip", 19, false, CompressionMethod::Stored);
    let later = archive(&root, "later.zip", 2, false, CompressionMethod::Stored);
    let before = std::fs::read(&original).unwrap();
    let jobs = root.path().join("jobs.json");
    std::fs::write(&jobs,serde_json::to_vec(&json!({"total_count":1,"jobs":[{"id":5,"name":"job","run_id":42,"run_attempt":1,"head_sha":"a".repeat(40),"status":"completed","conclusion":"success","steps":(1..=19).map(|number|json!({"name":"step","number":number,"status":"completed","conclusion":"success"})).collect::<Vec<_>>()}]})).unwrap()).unwrap();
    let manifest = Manifest {
        schema: "simit.native-log-manifest.v1".into(),
        originals: vec![Original {
            identity: Identity {
                repository: "example/project".into(),
                repository_id: 1,
                run: 42,
                attempt: 1,
                source: "a".repeat(40),
                event: "push".into(),
                branch: "trunk".into(),
                workflow: ".github/workflows/ci.yaml".into(),
                workflow_id: 7,
                workflow_revision: "a".repeat(40),
                workflow_blob: "b".repeat(40),
                pull_request: None,
            },
            archive: original.clone(),
            jobs,
            observations: vec![later],
            supporting_records: vec![],
            receipt: None,
            signature: None,
            capture_record: None,
            non_emitting_evidence: vec![],
        }],
    };
    let manifest_path = root.path().join("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let results = root.path().join("results");
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_simit"))
            .env("PATH", "")
            .env("XDG_DATA_HOME", root.path().join("data"))
            .args(["ci", "log-reconcile", "--manifest"])
            .arg(&manifest_path)
            .arg("--results")
            .arg(&results)
            .args(["--contract", "retained-native-v1"])
            .output()
            .unwrap()
    };
    assert!(!run().status.success());
    let raw = std::fs::read(results.join("reconciliation.json")).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(report["accepted"], false);
    assert_eq!(
        report["originals"][0]["evaluations"][0]["custody_provenance"],
        "unverified"
    );
    assert_eq!(
        report["originals"][0]["evaluations"][0]["provider_archive_state"],
        "omission_only_regression"
    );
    assert_eq!(report["originals"][0]["durable_read_back"], true);
    assert!(!run().status.success());
    assert_eq!(
        std::fs::read(results.join("reconciliation.json")).unwrap(),
        raw
    );
    assert_eq!(std::fs::read(&original).unwrap(), before);
}

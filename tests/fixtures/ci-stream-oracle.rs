//! Explicit native Nix oracle; example target excluded from production outputs.
use anyhow::{Context, Result, ensure};
use simit::ci_runtime::{
    cancellation::Cancellation,
    dependencies::{ArchiveAdmission, validate_export},
};
use std::{
    collections::BTreeSet,
    fs,
    io::Cursor,
    path::PathBuf,
    process::{Command, Stdio},
};
#[path = "nix-export-wire.rs"]
mod nix_wire;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    ensure!(
        args.len() == 2,
        "usage: ci-stream-oracle <owned-scratch-directory> <existing-store-path>"
    );
    let root = PathBuf::from(&args[0]);
    ensure!(
        fs::symlink_metadata(&root)?.file_type().is_dir(),
        "oracle root must be an existing directory"
    );
    ensure!(
        fs::read_dir(&root)?.next().is_none(),
        "oracle root must be empty"
    );
    let version = Command::new("nix-store").arg("--version").output()?;
    ensure!(
        version.status.success() && String::from_utf8_lossy(&version.stdout).contains("2.34.8"),
        "oracle requires Nix 2.34.8"
    );
    let fixtures = [
        (
            "name255",
            nix_wire::directory(vec![(vec![b'a'; 255], nix_wire::regular(b"", false))]),
            true,
        ),
        (
            "name256",
            nix_wire::directory(vec![(vec![b'a'; 256], nix_wire::regular(b"", false))]),
            false,
        ),
        ("target4095", nix_wire::link(&vec![b'a'; 4095]), true),
        ("target4096", nix_wire::link(&vec![b'a'; 4096]), false),
        ("depth63", nix_wire::depth(63), true),
        ("depth64", nix_wire::depth(64), false),
    ];
    let out = nix_wire::path('0', "input");
    let other = nix_wire::path('3', "other");
    let admission = ArchiveAdmission {
        closure: vec![out.clone(), other.clone()],
        outputs: vec![out.clone()],
        qualification_outputs: BTreeSet::new(),
        archive_bytes: 0,
        archive_sha256: String::new(),
        uncompressed_bytes: 0,
        maximum_bytes: 1024 * 1024,
    };
    for (name, node, accepted) in fixtures {
        let nar = [nix_wire::string("nix-archive-1"), node.clone()].concat();
        let nar_path = root.join(format!("{name}.nar"));
        fs::write(&nar_path, nar)?;
        let restored = root.join(format!("restore-{name}"));
        ensure!(!restored.exists(), "oracle destination already exists");
        // Restore beneath owned scratch, never nix-store --import or a store add.
        let result = Command::new("nix-store")
            .arg("--restore")
            .arg(&restored)
            .stdin(Stdio::from(fs::File::open(nar_path)?))
            .output()?;
        fs::write(root.join(format!("{name}.stderr")), &result.stderr)?;
        ensure!(
            result.status.success() == accepted,
            "Nix restore disagreement at {name}"
        );
        let raw = nix_wire::export(vec![
            nix_wire::record(&out, nix_wire::regular(b"input", false), &[], ""),
            nix_wire::record(&other, node, &[], ""),
        ]);
        let rust = validate_export(
            &mut Cursor::new(&raw),
            raw.len() as u64,
            &admission,
            &Cancellation::default(),
        );
        ensure!(
            rust.is_ok() == accepted,
            "Rust parser disagreement at {name}"
        );
    }
    let queried = Command::new("nix-store")
        .args(["--query", "--requisites"])
        .arg(&args[1])
        .output()?;
    ensure!(queried.status.success(), "read-only closure query failed");
    let closure: Vec<String> = String::from_utf8(queried.stdout)?
        .lines()
        .map(str::to_owned)
        .collect();
    let export_path = root.join("actual-export.stream");
    let status = Command::new("nix-store")
        .arg("--export")
        .args(&closure)
        .stdout(Stdio::from(
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&export_path)?,
        ))
        .stderr(Stdio::from(
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join("actual-export.stderr"))?,
        ))
        .status()?;
    ensure!(status.success(), "read-only actual Nix export failed");
    let mut export = fs::File::open(export_path)?;
    let size = export.metadata()?.len();
    let actual = ArchiveAdmission {
        closure,
        outputs: vec![args[1].to_str().context("store path encoding")?.to_owned()],
        qualification_outputs: BTreeSet::new(),
        maximum_bytes: 1024 * 1024 * 1024,
        ..admission
    };
    validate_export(&mut export, size, &actual, &Cancellation::default())?;
    let receipt = serde_json::json!({"schema":"simit.nix-stream-oracle.v1", "nix_version":"2.34.8", "importer_edges":6, "actual_export_paths":actual.closure.len(), "actual_export_bytes":size, "canonical_import":false, "qualification_execution":false});
    fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{receipt}");
    Ok(())
}

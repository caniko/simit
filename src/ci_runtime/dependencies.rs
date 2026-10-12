//! Actual export-stream admission, separate from source/run qualification authority.
//! Grammar baseline: Nix `adbf23e8184bbbe8d786aad1f362443b6542e0a4`
//! `src/libstore/export-import.cc`; importer limits: Nix 2.34.8
//! `src/libutil/archive.cc`. Compatibility source: Simit `a987453`.
use super::{
    cancellation::Cancellation,
    process::{CommandRunner, Runner},
};
use anyhow::{Context, Result, ensure};
use flate2::read::MultiGzDecoder;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

const MAX_PATHS: usize = 100_000;
const MAX_NODES: u64 = 1_000_000;
const MAX_REFERENCES: u64 = 1_000_000;
const MAX_DEPTH: usize = 64;
const COMPRESSED_OVERHEAD: u64 = 1024 * 1024;

/// Expected bytes and inventories obtained from separately validated provenance
/// and fresh evaluation. These values alone establish no source/run authority.
#[derive(Clone, Debug)]
pub struct ArchiveAdmission {
    /// Entire manifest closure, without duplicates.
    pub closure: Vec<String>,
    /// Freshly evaluated dependency output roots, all required in the closure.
    pub outputs: Vec<String>,
    /// Freshly evaluated qualification outputs, prohibited from the closure.
    pub qualification_outputs: BTreeSet<String>,
    /// Exact compressed archive length from the admitted manifest.
    pub archive_bytes: u64,
    /// Exact lowercase SHA-256 of the compressed archive.
    pub archive_sha256: String,
    /// Exact decompressed export-stream length from the admitted manifest.
    pub uncompressed_bytes: u64,
    /// Positive transport budget, applied to the complete decompressed stream.
    pub maximum_bytes: u64,
}

impl ArchiveAdmission {
    fn inventory(&self) -> Result<BTreeSet<Vec<u8>>> {
        ensure!(
            !self.closure.is_empty() && self.closure.len() <= MAX_PATHS,
            "invalid admitted export inventory"
        );
        ensure!(
            self.maximum_bytes > 0 && self.maximum_bytes <= u64::MAX - COMPRESSED_OVERHEAD,
            "invalid transport budget"
        );
        ensure!(
            self.closure
                .iter()
                .chain(&self.outputs)
                .chain(&self.qualification_outputs)
                .all(|path| canonical_store_path(path.as_bytes())),
            "invalid admitted export paths"
        );
        let admitted: BTreeSet<_> = self
            .closure
            .iter()
            .map(|path| path.as_bytes().to_vec())
            .collect();
        ensure!(
            admitted.len() == self.closure.len(),
            "duplicate admitted export paths"
        );
        ensure!(
            !self.outputs.is_empty()
                && self
                    .outputs
                    .iter()
                    .all(|path| admitted.contains(path.as_bytes())),
            "export closure omits evaluated roots"
        );
        ensure!(
            self.qualification_outputs
                .iter()
                .all(|path| !admitted.contains(path.as_bytes())),
            "export closure includes qualification outputs"
        );
        Ok(admitted)
    }
}

/// Complete parsed bytes held on a private unlinked descriptor. No pathname
/// reopen, stream writer, or mutable descriptor is exposed before consumption.
pub struct VerifiedArchive {
    stream: File,
    cancellation: Cancellation,
    closure: Vec<String>,
    outputs: Vec<String>,
    digest: String,
}

/// Successful restoration facts, explicitly separate from qualification execution.
#[derive(Debug, serde::Serialize)]
pub struct RestoreReceipt {
    /// Checksum of the admitted archive.
    pub archive_sha256: String,
    /// Evaluated roots restored and verified by current Nix observations.
    pub restored: Vec<String>,
    /// Restoration cannot attest to execution of qualification derivations.
    pub qualification_execution: bool,
}

/// Caller-owned Nix executable selection for hermetic runtime provisioning.
#[derive(Debug)]
pub struct StoreTools {
    /// `nix-store` executable; invoked directly, never through a shell.
    pub nix_store: PathBuf,
    /// `nix` executable for current root observations.
    pub nix: PathBuf,
}
impl Default for StoreTools {
    fn default() -> Self {
        Self {
            nix_store: "nix-store".into(),
            nix: "nix".into(),
        }
    }
}

impl VerifiedArchive {
    /// Import exactly once from the validated private descriptor, then verify
    /// all admitted paths and evaluated roots. Canonical mutation is confined to
    /// the import after complete parsing; failed verification returns no receipt.
    pub fn restore(self, evidence_root: &Path) -> Result<RestoreReceipt> {
        self.restore_with_tools(evidence_root, &StoreTools::default())
    }

    /// Restore using independently provisioned caller-selected Nix tools.
    /// Executable selection is runtime policy, not evidence authority.
    pub fn restore_with_tools(
        self,
        evidence_root: &Path,
        tools: &StoreTools,
    ) -> Result<RestoreReceipt> {
        self.cancellation.check()?;
        ensure!(
            fs::symlink_metadata(evidence_root)?.file_type().is_dir(),
            "import evidence root must be an existing directory"
        );
        let closure = self.closure.clone();
        let outputs = self.outputs.clone();
        let digest = self.digest.clone();
        let cancellation = self.cancellation.clone();
        self.consume(|stream| {
            let mut command = std::process::Command::new(&tools.nix_store);
            command.arg("--import");
            let status = super::process::run_with_input(
                &mut command,
                stream,
                &evidence_root.join("import.stdout"),
                &evidence_root.join("import.log"),
                &cancellation,
            )?;
            ensure!(
                status == 0,
                "dependency import failed ({status}); see retained import log"
            );
            let mut runner = CommandRunner;
            for (index, paths) in closure.chunks(1024).enumerate() {
                cancellation.check()?;
                let mut command = std::process::Command::new(&tools.nix_store);
                command.arg("--verify-path").args(paths);
                let status = runner.run(
                    &mut command,
                    &evidence_root.join(format!("verify-{index}.stdout")),
                    &evidence_root.join(format!("verify-{index}.log")),
                    &cancellation,
                )?;
                ensure!(
                    status == 0,
                    "dependency path verification failed ({status})"
                );
            }
            for (index, paths) in outputs.chunks(1024).enumerate() {
                cancellation.check()?;
                let mut command = std::process::Command::new(&tools.nix);
                command.arg("path-info").args(paths);
                let status = runner.run(
                    &mut command,
                    &evidence_root.join(format!("roots-{index}.stdout")),
                    &evidence_root.join(format!("roots-{index}.log")),
                    &cancellation,
                )?;
                ensure!(status == 0, "dependency root observation failed ({status})");
            }
            cancellation.check()?;
            Ok(RestoreReceipt {
                archive_sha256: digest,
                restored: outputs,
                qualification_execution: false,
            })
        })
    }

    /// Consume the capability once, passing the rewound validated descriptor to
    /// an importer. The caller owns canonical import and subsequent verification;
    /// this capability attests to bytes/inventory, never qualification execution.
    pub fn consume<T>(mut self, importer: impl FnOnce(File) -> Result<T>) -> Result<T> {
        self.cancellation.check()?;
        self.stream.rewind()?;
        importer(self.stream)
    }
}

/// Hash and decompress a pinned regular archive, then parse every record before
/// returning a one-use import capability. The compressed pathname is never reopened.
pub fn admit_archive(
    archive: &Path,
    spool_directory: &Path,
    admission: &ArchiveAdmission,
    cancellation: &Cancellation,
) -> Result<VerifiedArchive> {
    cancellation.check()?;
    admission.inventory()?;
    ensure!(
        fs::symlink_metadata(archive)?.file_type().is_file(),
        "dependency archive must be a regular file, not a symlink"
    );
    ensure!(
        fs::symlink_metadata(spool_directory)?.file_type().is_dir(),
        "spool root must be an existing directory, not a symlink"
    );
    let mut compressed = open_regular(archive)?;
    ensure!(
        compressed.metadata()?.len() == admission.archive_bytes
            && admission.archive_bytes <= admission.maximum_bytes + COMPRESSED_OVERHEAD,
        "invalid compressed dependency archive size"
    );
    ensure!(
        admission.archive_sha256.len() == 64
            && admission
                .archive_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid archive checksum"
    );
    let mut hash = Sha256::new();
    // Snapshot the checksum-bound compressed bytes too: an open archive inode
    // can still be modified by another writer after hashing it.
    let mut compressed_snapshot = private_spool(spool_directory)?;
    let mut buffer = [0u8; 65536];
    let mut compressed_size = 0u64;
    loop {
        cancellation.check()?;
        let count = compressed.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        compressed_size = compressed_size
            .checked_add(count as u64)
            .context("archive size overflow")?;
        ensure!(
            compressed_size <= admission.archive_bytes,
            "dependency archive grew during admission"
        );
        hash.update(&buffer[..count]);
        compressed_snapshot.write_all(&buffer[..count])?;
    }
    ensure!(
        compressed_size == admission.archive_bytes
            && hex::encode(hash.finalize()) == admission.archive_sha256,
        "dependency archive checksum mismatch"
    );
    compressed_snapshot.rewind()?;
    // Never persist or reopen the anonymous descriptor's pathname.
    let mut stream = private_spool(spool_directory)?;
    let mut decoder = MultiGzDecoder::new(compressed_snapshot);
    let mut size = 0u64;
    loop {
        cancellation.check()?;
        let count = decoder.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .context("decompressed size overflow")?;
        ensure!(
            size <= admission.maximum_bytes,
            "decompressed dependency transport exceeds budget"
        );
        stream.write_all(&buffer[..count])?;
    }
    ensure!(
        size == admission.uncompressed_bytes,
        "dependency transport size differs from manifest"
    );
    stream.rewind()?;
    validate_export(&mut stream, size, admission, cancellation)?;
    stream.rewind()?;
    Ok(VerifiedArchive {
        stream,
        cancellation: cancellation.clone(),
        closure: admission.closure.clone(),
        outputs: admission.outputs.clone(),
        digest: admission.archive_sha256.clone(),
    })
}

fn private_spool(root: &Path) -> Result<File> {
    let file = tempfile::tempfile_in(root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // Anonymous tempfile creation may use mode 0666 masked by umask.
        // Set privacy explicitly on the descriptor before writing any bytes.
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        let info = file.metadata()?;
        ensure!(
            info.mode() & 0o777 == 0o600 && info.nlink() == 0,
            "transport spool is not private and unlinked"
        );
    }
    Ok(file)
}

fn open_regular(path: &Path) -> Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            rustix::fs::OFlags::NOFOLLOW.bits() as i32 | rustix::fs::OFlags::NONBLOCK.bits() as i32,
        );
    }
    let file = options.open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "dependency archive descriptor is not a regular file"
    );
    Ok(file)
}

/// Validate the complete strict Nix writer grammar, without extraction or import.
/// This rejects a malformed later record before any canonical store mutation.
pub fn validate_export(
    stream: &mut impl Read,
    size: u64,
    admission: &ArchiveAdmission,
    cancellation: &Cancellation,
) -> Result<()> {
    let admitted = admission.inventory()?;
    ensure!(
        size <= admission.maximum_bytes,
        "export stream exceeds transport budget"
    );
    let mut parser = ExportInventory {
        stream,
        remaining: size,
        nodes: 0,
        references: 0,
        cancellation,
    };
    let mut seen = BTreeSet::new();
    loop {
        let marker = parser.number()?;
        if marker == 0 {
            break;
        }
        ensure!(
            marker == 1 && seen.len() < admitted.len(),
            "invalid or excessive export records"
        );
        parser.expect(b"nix-archive-1")?;
        parser.node(0)?;
        ensure!(parser.number()? == 0x4558494e, "invalid export magic");
        let path = parser.path()?;
        ensure!(
            admitted.contains(&path) && seen.insert(path),
            "export path is unadmitted or duplicated"
        );
        let count = parser.number()?;
        parser.references = parser
            .references
            .checked_add(count)
            .context("export reference count overflow")?;
        ensure!(
            count <= admitted.len() as u64 && parser.references <= MAX_REFERENCES,
            "export reference inventory exceeds its bound"
        );
        let mut previous = Vec::new();
        for _ in 0..count {
            let reference = parser.path()?;
            ensure!(
                admitted.contains(&reference) && reference > previous,
                "export reference escapes closure or is duplicated/unordered"
            );
            previous = reference;
        }
        let deriver = parser.string(255)?;
        ensure!(
            deriver.is_empty() || canonical_store_path(&deriver),
            "invalid export deriver metadata"
        );
        ensure!(
            parser.number()? == 0,
            "unsupported export signature framing"
        );
    }
    ensure!(
        seen == admitted,
        "export stream inventory differs from admitted closure"
    );
    ensure!(
        parser.remaining == 0 && parser.stream.read(&mut [0u8; 1])? == 0,
        "trailing export data"
    );
    cancellation.check()?;
    Ok(())
}

struct ExportInventory<'a, R> {
    stream: &'a mut R,
    remaining: u64,
    nodes: u64,
    references: u64,
    cancellation: &'a Cancellation,
}

impl<R: Read> ExportInventory<'_, R> {
    fn exact(&mut self, raw: &mut [u8]) -> Result<()> {
        self.cancellation.check()?;
        ensure!(
            raw.len() as u64 <= self.remaining,
            "truncated export stream"
        );
        self.stream
            .read_exact(raw)
            .context("truncated export stream")?;
        self.remaining -= raw.len() as u64;
        Ok(())
    }
    fn number(&mut self) -> Result<u64> {
        let mut raw = [0u8; 8];
        self.exact(&mut raw)?;
        Ok(u64::from_le_bytes(raw))
    }
    fn padding(&mut self, size: u64) -> Result<()> {
        let mut raw = [0u8; 7];
        let length = ((8 - size % 8) % 8) as usize;
        self.exact(&mut raw[..length])?;
        ensure!(raw == [0u8; 7], "nonzero export padding");
        Ok(())
    }
    fn string(&mut self, maximum: usize) -> Result<Vec<u8>> {
        let size = self.number()?;
        ensure!(size <= maximum as u64, "export string exceeds its bound");
        let mut raw = vec![0; size as usize];
        self.exact(&mut raw)?;
        self.padding(size)?;
        Ok(raw)
    }
    fn expect(&mut self, token: &[u8]) -> Result<()> {
        ensure!(self.string(32)? == token, "invalid NAR grammar token");
        Ok(())
    }
    fn contents(&mut self) -> Result<()> {
        let size = self.number()?;
        ensure!(size <= self.remaining, "truncated NAR contents");
        let mut remaining = size;
        let mut raw = [0u8; 65536];
        while remaining > 0 {
            let chunk = remaining.min(raw.len() as u64) as usize;
            self.exact(&mut raw[..chunk])?;
            remaining -= chunk as u64;
        }
        self.padding(size)
    }
    fn path(&mut self) -> Result<Vec<u8>> {
        let path = self.string(255)?;
        ensure!(
            canonical_store_path(&path),
            "noncanonical export store path"
        );
        Ok(path)
    }
    fn node(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        ensure!(
            depth < MAX_DEPTH && self.nodes <= MAX_NODES,
            "NAR tree exceeds its bound"
        );
        self.expect(b"(")?;
        self.expect(b"type")?;
        match self.string(32)?.as_slice() {
            b"regular" => {
                let mut field = self.string(32)?;
                if field == b"executable" {
                    self.expect(b"")?;
                    field = self.string(32)?;
                }
                ensure!(field == b"contents", "invalid regular NAR fields");
                self.contents()?;
            }
            b"symlink" => {
                self.expect(b"target")?;
                let target = self.string(4095)?;
                ensure!(
                    !target.is_empty() && !target.contains(&0),
                    "invalid NAR symlink target"
                );
            }
            b"directory" => {
                let mut previous = Vec::new();
                loop {
                    let field = self.string(32)?;
                    if field == b")" {
                        return Ok(());
                    }
                    ensure!(field == b"entry", "invalid directory NAR fields");
                    self.expect(b"(")?;
                    self.expect(b"name")?;
                    let name = self.string(255)?;
                    ensure!(
                        name > previous
                            && name != b"."
                            && name != b".."
                            && !name.contains(&b'/')
                            && !name.contains(&0),
                        "invalid or unordered NAR entry name"
                    );
                    previous = name;
                    self.expect(b"node")?;
                    self.node(depth + 1)?;
                    self.expect(b")")?;
                }
            }
            _ => anyhow::bail!("unsupported NAR object type"),
        }
        self.expect(b")")
    }
}

fn canonical_store_path(raw: &[u8]) -> bool {
    raw.strip_prefix(b"/nix/store/").is_some_and(|rest| {
        raw.len() <= 255
            && rest.len() >= 34
            && rest[32] == b'-'
            && rest[..32]
                .iter()
                .all(|b| b"0123456789abcdfghijklmnpqrsvwxyz".contains(b))
            && rest[33..]
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || b"+._?=-".contains(b))
            && !matches!(&rest[33..], b"." | b"..")
            && !rest[33..].starts_with(b".-")
            && !rest[33..].starts_with(b"..-")
    })
}

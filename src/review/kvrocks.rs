//! Bootstrap the worker-pinned native transport independently of the consumer.
//! This does not start a service or establish socket/sandbox readiness.
use super::{
    artifact,
    contract::{CompilerCache, Plan, runner},
    digest,
    engine::{self, EngineManifest},
    process::{args, nix_args, run},
    write_json,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSource {
    pub engine_repository: String,
    pub engine_revision: String,
    pub nixpkgs_revision: String,
    pub system: String,
    pub installable: String,
    pub version: String,
    pub derivation: String,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapReceipt {
    pub source: NativeSource,
    pub nar_hash: String,
    pub nar_size: u64,
    pub binary_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentity {
    pub user: String,
    pub uid: u32,
    pub group: String,
    pub gid: u32,
}

fn account_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
            && !name.starts_with('-'),
        "invalid worker account name"
    );
    Ok(())
}

fn validate_identity(identity: &WorkerIdentity) -> Result<()> {
    account_name(&identity.user)?;
    account_name(&identity.group)?;
    ensure!(
        identity.uid != 0
            && identity.uid != u32::MAX
            && identity.gid != 0
            && identity.gid != u32::MAX,
        "Kvrocks requires a non-root runner and Nix build group"
    );
    Ok(())
}

fn identity_record(value: &str, fields: usize) -> Result<Vec<&str>> {
    let record = value.strip_suffix('\n').unwrap_or(value);
    ensure!(
        !record.is_empty() && record.len() <= 16 * 1024 && !record.chars().any(char::is_control),
        "expected one bounded account database record"
    );
    let record = record.split(':').collect::<Vec<_>>();
    ensure!(record.len() == fields, "malformed account database record");
    Ok(record)
}

/// Parse exact `getent passwd USER` and `getent group GROUP` observations. The
/// lifecycle must obtain these from the actual worker, not a consumer request.
pub fn parse_worker_identity(
    user: &str,
    group: &str,
    passwd_record: &str,
    group_record: &str,
) -> Result<WorkerIdentity> {
    for name in [user, group] {
        account_name(name)?;
    }
    let passwd = identity_record(passwd_record, 7)?;
    let build_group = identity_record(group_record, 4)?;
    ensure!(
        passwd[0] == user && build_group[0] == group,
        "worker account lookup returned a different identity"
    );
    let uid: u32 = passwd[2].parse().context("invalid runner UID")?;
    let _: u32 = passwd[3].parse().context("invalid runner primary GID")?;
    let gid: u32 = build_group[2].parse().context("invalid Nix build GID")?;
    ensure!(
        uid != 0 && uid != u32::MAX && gid != 0 && gid != u32::MAX,
        "Kvrocks requires a non-root runner and Nix build group"
    );
    Ok(WorkerIdentity {
        user: user.into(),
        uid,
        group: group.into(),
        gid,
    })
}

/// Refuse any pre-existing runtime state, including dangling symlinks. This is
/// an observation only; creation must still be exclusive in the owned lifecycle.
pub fn refuse_existing_runtime(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("inspecting Kvrocks runtime state"),
        Ok(_) => anyhow::bail!(
            "Kvrocks runtime state already exists at {}; preserve its owner",
            path.display()
        ),
    }
}

/// An exclusively created directory retained by its open inode. The privileged
/// lifecycle must place this below a trusted parent (the canonical `/run` route)
/// and stop its owned service before cleanup. This guard never recursively
/// removes state and does not establish service or sandbox readiness.
#[cfg(unix)]
pub struct OwnedRuntime {
    path: std::path::PathBuf,
    directory: fs::File,
    identity: WorkerIdentity,
    initialized: bool,
    cleaned: bool,
}

/// Arm conservative rollback before opening the allocated directory can fail.
/// The lifecycle's trusted parent prevents untrusted creators; the snapshot
/// additionally preserves a replacement path or unexpected nonempty state.
#[cfg(unix)]
struct PendingRuntime<'a> {
    path: &'a Path,
    allocated: fs::Metadata,
    armed: bool,
}

#[cfg(unix)]
impl Drop for PendingRuntime<'_> {
    fn drop(&mut self) {
        use std::os::unix::fs::MetadataExt;

        if self.armed
            && fs::symlink_metadata(self.path).is_ok_and(|current| {
                current.is_dir()
                    && current.dev() == self.allocated.dev()
                    && current.ino() == self.allocated.ino()
            })
        {
            // remove_dir refuses nonempty state; never recursively delete it.
            let _ = fs::remove_dir(self.path);
        }
    }
}

#[cfg(unix)]
impl OwnedRuntime {
    pub fn create(path: &Path, identity: &WorkerIdentity) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt;

        Self::create_with_setup(path, identity, |owned| {
            std::os::unix::fs::chown(path, Some(identity.uid), Some(identity.gid))
                .context("assigning owned runtime identity")?;
            owned
                .directory
                .set_permissions(fs::Permissions::from_mode(0o750))?;
            Ok(())
        })
    }

    fn create_with_setup(
        path: &Path,
        identity: &WorkerIdentity,
        setup: impl FnOnce(&Self) -> Result<()>,
    ) -> Result<Self> {
        Self::create_with_setup_and_open(path, identity, |path| fs::File::open(path), setup)
    }

    fn create_with_setup_and_open(
        path: &Path,
        identity: &WorkerIdentity,
        open: impl FnOnce(&Path) -> std::io::Result<fs::File>,
        setup: impl FnOnce(&Self) -> Result<()>,
    ) -> Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};

        ensure!(
            path.is_absolute() && path.file_name().is_some(),
            "invalid runtime directory"
        );
        ensure!(
            path.components().all(|part| matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )),
            "runtime directory must not contain traversal"
        );
        for ancestor in path.ancestors().skip(1) {
            let metadata = fs::symlink_metadata(ancestor).context("inspecting runtime parent")?;
            ensure!(
                metadata.is_dir(),
                "runtime parent is a symlink or non-directory"
            );
        }
        // create_dir, not create_dir_all: concurrent creators and dangling
        // symlinks both fail without adopting or altering existing state.
        fs::DirBuilder::new()
            .mode(0o750)
            .create(path)
            .context("exclusively allocating Kvrocks runtime directory")?;
        let mut pending = PendingRuntime {
            path,
            allocated: fs::symlink_metadata(path).context("observing allocated runtime inode")?,
            armed: true,
        };
        let directory = open(path).context("retaining owned runtime inode")?;
        let pinned = directory.metadata()?;
        ensure!(
            pinned.is_dir()
                && pinned.dev() == pending.allocated.dev()
                && pinned.ino() == pending.allocated.ino(),
            "runtime allocation was replaced while retaining its inode"
        );
        let mut owned = Self {
            path: path.to_owned(),
            directory,
            identity: identity.clone(),
            initialized: false,
            cleaned: false,
        };
        // Ownership transfers to the file-pinned guard before setup can fail.
        pending.armed = false;
        setup(&owned)?;
        owned.verify_owned_path()?;
        owned.initialized = true;
        Ok(owned)
    }

    fn verify_pinned_path(&self) -> Result<fs::Metadata> {
        use std::os::unix::fs::MetadataExt;

        let pinned = self.directory.metadata()?;
        let current = fs::symlink_metadata(&self.path).context("inspecting owned runtime path")?;
        ensure!(
            current.is_dir() && current.dev() == pinned.dev() && current.ino() == pinned.ino(),
            "runtime path was replaced; preserve foreign state"
        );
        Ok(current)
    }

    fn verify_owned_path(&self) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        let current = self.verify_pinned_path()?;
        ensure!(
            current.uid() == self.identity.uid
                && current.gid() == self.identity.gid
                && current.mode() & 0o7777 == 0o750,
            "runtime path or identity was replaced; preserve foreign state"
        );
        Ok(())
    }

    fn remove_empty(&mut self) -> Result<()> {
        if self.initialized {
            self.verify_owned_path()?;
        } else {
            // Failed chown/chmod cannot establish the intended final identity.
            // Roll back only the allocated inode and only if it is still empty.
            self.verify_pinned_path()?;
        }
        // An unexpected socket, database file or foreign sentinel prevents
        // removal. The service owner must separately account for its artifacts.
        fs::remove_dir(&self.path).context("removing only the owned empty runtime directory")?;
        self.cleaned = true;
        Ok(())
    }

    pub fn cleanup(mut self) -> Result<()> {
        self.remove_empty()
    }
}

#[cfg(unix)]
impl Drop for OwnedRuntime {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = self.remove_empty();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServicePlan {
    pub bootstrap: BootstrapReceipt,
    pub identity: WorkerIdentity,
    pub run_identity: String,
    pub unit: String,
    pub state_directory: String,
    pub socket: String,
    pub sandbox_paths: Vec<String>,
    pub startup_seconds: u32,
    pub shutdown_seconds: u32,
    pub memory_max_bytes: u64,
    pub cpu_quota_percent: u32,
    pub max_db_gib: u32,
    pub config: String,
    pub systemd_args: Vec<String>,
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Freeze the source-owned bounded service configuration. This does not allocate
/// runtime state, start a unit or establish actual socket/sandbox readiness.
pub fn service_plan(
    bootstrap: &BootstrapReceipt,
    identity: &WorkerIdentity,
    worker_temp: &Path,
    run_identity: &str,
) -> Result<ServicePlan> {
    validate_identity(identity)?;
    ensure!(
        sha256(run_identity),
        "invalid immutable service run identity"
    );
    ensure!(
        sha256(&bootstrap.binary_sha256),
        "invalid native tool binary digest"
    );
    ensure!(
        bootstrap.nar_size > 0
            && bootstrap.nar_hash.len() <= 80
            && (bootstrap.nar_hash.starts_with("sha256:")
                || bootstrap.nar_hash.starts_with("sha256-")),
        "invalid native tool NAR identity"
    );
    let source = &bootstrap.source;
    let manifest = EngineManifest {
        schema_version: 1,
        repository: source.engine_repository.clone(),
        revision: source.engine_revision.clone(),
        nixpkgs_revision: source.nixpkgs_revision.clone(),
    };
    let frozen = freeze_source(
        &manifest,
        &source.system,
        &serde_json::json!({
            "pname": "kvrocks", "version": source.version, "system": source.system,
            "outputs": ["out"], "drvPath": source.derivation, "outPath": source.output,
        }),
    )?;
    ensure!(
        &frozen == source,
        "native tool source differs from frozen worker pin"
    );
    let temp = worker_temp
        .to_str()
        .context("worker temporary path is not UTF-8")?;
    ensure!(
        worker_temp.is_absolute()
            && worker_temp.file_name().is_some()
            && worker_temp.components().all(|part| matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            ))
            && temp
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b)),
        "invalid worker temporary directory"
    );
    let state_directory = worker_temp
        .join(format!("simit-kvrocks-{run_identity}"))
        .to_str()
        .context("service state path is not UTF-8")?
        .to_owned();
    let unit = format!("simit-kvrocks-{run_identity}.service");
    let socket = "/run/redis-sccache/redis.sock".to_owned();
    let startup_seconds = 30;
    let shutdown_seconds = 30;
    let memory_max_bytes = 2 * 1024 * 1024 * 1024;
    let cpu_quota_percent = 200;
    let max_db_gib = 4;
    // An absent bind directive follows the pinned Kvrocks Unix-only route.
    // RestrictAddressFamilies also denies network sockets; actual listeners
    // must independently be verified before admitting a consumer build.
    let config = format!(
        "port 16666\nunixsocket {socket}\nunixsocketperm 660\ndir {state_directory}\ndaemonize no\nsupervised no\nworkers 2\nmax-db-size {max_db_gib}\nrocksdb.block_cache_size 512\nrocksdb.max_background_jobs 2\nrocksdb.max_write_buffer_number 2\nrocksdb.write_buffer_size 64\n"
    );
    let mut systemd_args = args(&[
        "--no-block",
        "--service-type=exec",
        &format!("--unit={unit}"),
        &format!("--property=User={}", identity.user),
        &format!("--property=Group={}", identity.group),
        "--property=RuntimeDirectory=redis-sccache",
        "--property=RuntimeDirectoryMode=0750",
        "--property=RuntimeDirectoryPreserve=yes",
        &format!("--property=MemoryMax={memory_max_bytes}"),
        &format!("--property=CPUQuota={cpu_quota_percent}%"),
        "--property=KillMode=control-group",
        &format!("--property=TimeoutStartSec={startup_seconds}s"),
        &format!("--property=TimeoutStopSec={shutdown_seconds}s"),
        "--property=SendSIGKILL=yes",
        "--property=KillSignal=SIGTERM",
        "--property=FinalKillSignal=SIGKILL",
        "--property=Restart=no",
        "--property=RestrictAddressFamilies=AF_UNIX",
        "--property=NoNewPrivileges=yes",
        "--property=TasksMax=64",
        "--property=LimitNOFILE=8192",
    ]);
    systemd_args.extend(args(&[
        "--",
        &format!("{}/bin/kvrocks", source.output),
        "-c",
        &format!("{state_directory}/kvrocks.conf"),
    ]));
    Ok(ServicePlan {
        bootstrap: bootstrap.clone(),
        identity: identity.clone(),
        run_identity: run_identity.into(),
        unit,
        state_directory,
        socket: socket.clone(),
        sandbox_paths: vec![socket],
        startup_seconds,
        shutdown_seconds,
        memory_max_bytes,
        cpu_quota_percent,
        max_db_gib,
        config,
        systemd_args,
    })
}

/// Verify the socket and immediate runtime directory without following their
/// symlinks. These metadata checks do not establish process ownership or sandbox
/// admission; those require the lifecycle's independent receipts.
#[cfg(unix)]
pub fn verify_socket(socket: &Path, identity: &WorkerIdentity) -> Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    ensure!(socket.is_absolute(), "socket path must be absolute");
    let parent = socket.parent().context("socket has no runtime directory")?;
    let directory = fs::symlink_metadata(parent).context("inspecting runtime directory")?;
    ensure!(
        directory.is_dir()
            && directory.uid() == identity.uid
            && directory.gid() == identity.gid
            && directory.mode() & 0o7777 == 0o750,
        "Kvrocks runtime directory identity/mode differs from the worker"
    );
    let metadata = fs::symlink_metadata(socket).context("inspecting Kvrocks socket")?;
    ensure!(
        metadata.file_type().is_socket()
            && metadata.uid() == identity.uid
            && metadata.gid() == identity.gid
            && metadata.mode() & 0o7777 == 0o660,
        "Kvrocks socket type/identity/mode differs from the worker"
    );
    Ok(())
}

/// The direct controller tool pin owns this package, never the target's lock.
pub fn tool_reference(manifest: &EngineManifest, system: &str) -> Result<String> {
    ensure!(manifest.schema_version == 1, "unsupported engine manifest");
    super::contract::repository(&manifest.repository)?;
    super::contract::sha(&manifest.revision)?;
    super::contract::sha(&manifest.nixpkgs_revision)?;
    runner(system)?;
    ensure!(system.ends_with("-linux"), "Kvrocks requires native Linux");
    Ok(format!(
        "github:NixOS/nixpkgs/{}#legacyPackages.{system}.kvrocks",
        manifest.nixpkgs_revision
    ))
}

/// Freeze the evaluated native derivation before realization. This is also the
/// validation boundary for retained package discovery in later lifecycle receipts.
pub fn freeze_source(
    manifest: &EngineManifest,
    system: &str,
    package: &Value,
) -> Result<NativeSource> {
    let installable = tool_reference(manifest, system)?;
    ensure!(
        package["pname"] == "kvrocks"
            && package["system"] == system
            && package["outputs"] == serde_json::json!(["out"]),
        "Kvrocks bootstrap package/platform/output mismatch"
    );
    let version = package["version"]
        .as_str()
        .context("missing tool version")?;
    ensure!(
        !version.is_empty()
            && version.len() <= 64
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_+".contains(&b)),
        "invalid tool version"
    );
    let derivation = package["drvPath"]
        .as_str()
        .context("missing tool derivation")?;
    let output = package["outPath"].as_str().context("missing tool output")?;
    artifact::store_path(derivation)?;
    artifact::store_path(output)?;
    ensure!(
        derivation.ends_with(".drv") && !output.ends_with(".drv"),
        "Kvrocks bootstrap requires frozen derivation and runtime output"
    );
    Ok(NativeSource {
        engine_repository: manifest.repository.clone(),
        engine_revision: manifest.revision.clone(),
        nixpkgs_revision: manifest.nixpkgs_revision.clone(),
        system: system.into(),
        installable,
        version: version.into(),
        derivation: derivation.into(),
        output: output.into(),
    })
}

/// Realize only the native worker tool, with the same bounded no-IFD/no-builder
/// policy as the engine. The lifecycle owner calls this before target evaluation.
pub fn bootstrap(plan: &Plan, system: &str, out: &Path) -> Result<BootstrapReceipt> {
    plan.validate()?;
    ensure!(
        plan.request.compiler_cache == Some(CompilerCache::KvrocksV1)
            && plan
                .request
                .systems
                .iter()
                .any(|requested| requested == system),
        "Kvrocks bootstrap requires an explicit requested platform/cache profile"
    );
    let manifest = engine::manifest()?;
    engine::verify_lock(&plan.tool_lock, &manifest)?;
    let reference = tool_reference(&manifest, system)?;
    ensure!(!out.exists(), "bootstrap evidence directory must be new");
    let (_, architecture, _) = runner(system)?;
    ensure!(
        run("uname", &args(&["-m"]), None, None)?.trim() == architecture
            && run("nix", &args(&["config", "show", "system"]), None, None)?.trim() == system,
        "Kvrocks bootstrap requires the requested native worker architecture"
    );
    fs::create_dir(out)?;
    let package: Value = serde_json::from_str(&run(
        "nix",
        &nix_args(&[
            "eval",
            "--json",
            "--no-update-lock-file",
            &reference,
            "--apply",
            "p: { inherit (p) pname version system drvPath outPath outputs; }",
        ]),
        None,
        Some(&out.join("discovery.log")),
    )?)?;
    let source = freeze_source(&manifest, system, &package)?;
    write_json(&out.join("source.json"), &source)?;
    let selected = format!("{}^out", source.derivation);
    let built: Value = serde_json::from_str(&run(
        "nix",
        &nix_args(&["build", "--no-link", "--json", "-L", &selected]),
        None,
        Some(&out.join("build.log")),
    )?)?;
    ensure!(
        built.as_array().is_some_and(|items| items.len() == 1)
            && built[0]["drvPath"] == source.derivation
            && built[0]["outputs"] == serde_json::json!({"out": source.output}),
        "realized Kvrocks tool differs from frozen native derivation"
    );
    let infos: Value = serde_json::from_str(&run(
        "nix",
        &nix_args(&["path-info", "--json", &source.output]),
        None,
        Some(&out.join("path-info.log")),
    )?)?;
    let info = if let Some(info) = infos.get(&source.output) {
        info
    } else {
        ensure!(
            infos.as_array().is_some_and(|items| items.len() == 1)
                && infos[0]["path"] == source.output,
            "tool NAR report differs from frozen output"
        );
        &infos[0]
    };
    let nar_hash = info["narHash"].as_str().context("missing tool NAR hash")?;
    let nar_size = info["narSize"].as_u64().context("missing tool NAR size")?;
    ensure!(
        (nar_hash.starts_with("sha256:") || nar_hash.starts_with("sha256-"))
            && nar_hash.len() <= 80
            && nar_size > 0,
        "invalid tool NAR identity"
    );
    let output = Path::new(&source.output).canonicalize()?;
    let binary = output.join("bin/kvrocks").canonicalize()?;
    ensure!(
        binary.starts_with(&output),
        "tool binary escapes its output"
    );
    let metadata = fs::metadata(&binary)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 256 * 1024 * 1024,
        "invalid native tool binary"
    );
    let receipt = BootstrapReceipt {
        source,
        nar_hash: nar_hash.into(),
        nar_size,
        binary_sha256: digest(&fs::read(binary)?),
    };
    write_json(&out.join("bootstrap.json"), &receipt)?;
    Ok(receipt)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn runtime_inode_open_failure_rolls_back_only_the_new_empty_directory() {
        const CHILD_PATH: &str = "SIMIT_TEST_EXHAUSTED_FD_RUNTIME";
        if let Some(path) = std::env::var_os(CHILD_PATH) {
            let path = std::path::PathBuf::from(path);
            let metadata = path.parent().unwrap().metadata().unwrap();
            let identity = WorkerIdentity {
                user: "fixture".into(),
                uid: metadata.uid(),
                group: "fixture".into(),
                gid: metadata.gid(),
            };
            let existed = path.exists();
            let mut descriptors = Vec::new();
            let exhaustion = loop {
                match fs::File::open("/dev/null") {
                    Ok(file) => descriptors.push(file),
                    Err(error) => break error,
                }
            };
            let result = OwnedRuntime::create(&path, &identity);
            drop(descriptors);
            assert_eq!(exhaustion.raw_os_error(), Some(24)); // EMFILE on Unix.
            let error = result.err().expect("allocation must report the failure");
            if existed {
                assert!(error.to_string().contains("exclusively allocating"));
                assert_eq!(fs::read(path.join("foreign")).unwrap(), b"preserve foreign");
            } else {
                assert!(error.to_string().contains("retaining owned runtime inode"));
                assert!(fs::symlink_metadata(&path).is_err());
            }
            return;
        }
        // The descriptor limit belongs to a separate process, never to the
        // parallel test runner. Exercise the real File::open EMFILE path.
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        let child = || {
            std::process::Command::new("/bin/sh")
                .args([
                    "-c",
                    "ulimit -n 64 || exit; exec \"$1\" --exact review::kvrocks::tests::runtime_inode_open_failure_rolls_back_only_the_new_empty_directory --test-threads=1 --nocapture",
                    "simit-fd-fixture",
                ])
                .arg(std::env::current_exe().unwrap())
                .env(CHILD_PATH, &runtime)
                .output()
                .unwrap()
        };
        let output = child();
        assert!(output.status.success(), "{output:?}");
        assert!(fs::symlink_metadata(&runtime).is_err());
        fs::create_dir(&runtime).unwrap();
        fs::write(runtime.join("foreign"), b"preserve foreign").unwrap();
        let output = child();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            fs::read(runtime.join("foreign")).unwrap(),
            b"preserve foreign"
        );
    }

    #[test]
    fn inode_open_rollback_preserves_nonempty_and_replaced_runtime_paths() {
        let root = tempfile::tempdir().unwrap();
        let metadata = root.path().metadata().unwrap();
        let identity = WorkerIdentity {
            user: "fixture".into(),
            uid: metadata.uid(),
            group: "fixture".into(),
            gid: metadata.gid(),
        };
        let runtime = root.path().join("runtime");
        let result = OwnedRuntime::create_with_setup_and_open(
            &runtime,
            &identity,
            |path| {
                fs::write(path.join("unclaimed"), b"preserve unexpected state")?;
                Err(std::io::Error::from_raw_os_error(24))
            },
            |_| panic!("setup must not run after an inode-open failure"),
        );
        assert!(result.is_err());
        assert_eq!(
            fs::read(runtime.join("unclaimed")).unwrap(),
            b"preserve unexpected state"
        );
        fs::remove_file(runtime.join("unclaimed")).unwrap();
        fs::remove_dir(&runtime).unwrap();

        for open_replacement in [false, true] {
            let retained = root.path().join(format!("retained-{open_replacement}"));
            let result = OwnedRuntime::create_with_setup_and_open(
                &runtime,
                &identity,
                |path| {
                    fs::rename(path, &retained)?;
                    fs::create_dir(path)?;
                    if open_replacement {
                        fs::File::open(path)
                    } else {
                        Err(std::io::Error::from_raw_os_error(24))
                    }
                },
                |_| panic!("setup must not adopt a replacement inode"),
            );
            assert!(result.is_err());
            assert!(runtime.is_dir(), "preserve the empty replacement inode");
            assert!(retained.is_dir(), "preserve the displaced allocated inode");
            fs::remove_dir(&runtime).unwrap();
        }
        let owned = OwnedRuntime::create(&runtime, &identity).unwrap();
        owned.cleanup().unwrap();
        assert!(fs::symlink_metadata(&runtime).is_err());
    }

    #[test]
    fn failed_runtime_initialization_releases_only_its_pinned_empty_inode() {
        let root = tempfile::tempdir().unwrap();
        let metadata = root.path().metadata().unwrap();
        // Deliberately different from the newly allocated inode. Rollback must
        // not require an ownership change which the failing setup never made.
        let identity = WorkerIdentity {
            user: "fixture".into(),
            uid: metadata.uid().checked_add(1).unwrap(),
            group: "fixture".into(),
            gid: metadata.gid(),
        };
        let runtime = root.path().join("runtime");
        let failed = OwnedRuntime::create_with_setup(&runtime, &identity, |_| {
            anyhow::bail!("fixture ownership setup failure")
        });
        assert!(failed.is_err());
        assert!(fs::symlink_metadata(&runtime).is_err());

        let failed = OwnedRuntime::create_with_setup(&runtime, &identity, |_| {
            fs::write(
                runtime.join("unclaimed"),
                b"preserve unexpected initialization state",
            )?;
            anyhow::bail!("fixture mode setup failure")
        });
        assert!(failed.is_err());
        assert_eq!(
            fs::read(runtime.join("unclaimed")).unwrap(),
            b"preserve unexpected initialization state"
        );
        fs::remove_file(runtime.join("unclaimed")).unwrap();
        fs::remove_dir(&runtime).unwrap();

        let retained = root.path().join("retained-allocated-inode");
        let failed = OwnedRuntime::create_with_setup(&runtime, &identity, |_| {
            fs::rename(&runtime, &retained)?;
            fs::create_dir(&runtime)?;
            fs::write(runtime.join("foreign"), b"preserve replacement inode")?;
            anyhow::bail!("fixture path replacement during setup failure")
        });
        assert!(failed.is_err());
        assert!(retained.is_dir());
        assert_eq!(
            fs::read(runtime.join("foreign")).unwrap(),
            b"preserve replacement inode"
        );
        fs::remove_file(runtime.join("foreign")).unwrap();
        fs::remove_dir(&runtime).unwrap();

        let own_identity = WorkerIdentity {
            uid: metadata.uid(),
            ..identity
        };
        let positive = OwnedRuntime::create(&runtime, &own_identity).unwrap();
        positive.cleanup().unwrap();
        assert!(fs::symlink_metadata(&runtime).is_err());
    }
}

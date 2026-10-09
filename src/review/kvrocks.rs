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
        ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
                && !name.starts_with('-'),
            "invalid worker account name"
        );
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
    cleaned: bool,
}

#[cfg(unix)]
impl OwnedRuntime {
    pub fn create(path: &Path, identity: &WorkerIdentity) -> Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

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
        let directory = fs::File::open(path).context("retaining owned runtime inode")?;
        let owned = Self {
            path: path.to_owned(),
            directory,
            identity: identity.clone(),
            cleaned: false,
        };
        std::os::unix::fs::chown(path, Some(identity.uid), Some(identity.gid))
            .context("assigning owned runtime identity")?;
        owned
            .directory
            .set_permissions(fs::Permissions::from_mode(0o750))?;
        owned.verify_owned_path()?;
        Ok(owned)
    }

    fn verify_owned_path(&self) -> Result<()> {
        use std::os::unix::fs::MetadataExt;

        let pinned = self.directory.metadata()?;
        let current = fs::symlink_metadata(&self.path).context("inspecting owned runtime path")?;
        ensure!(
            current.is_dir()
                && current.dev() == pinned.dev()
                && current.ino() == pinned.ino()
                && current.uid() == self.identity.uid
                && current.gid() == self.identity.gid
                && current.mode() & 0o7777 == 0o750,
            "runtime path or identity was replaced; preserve foreign state"
        );
        Ok(())
    }

    fn remove_empty(&mut self) -> Result<()> {
        self.verify_owned_path()?;
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

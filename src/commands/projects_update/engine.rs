use super::schema::*;
use crate::{
    project::CiUpdatePlan,
    review::{canonical_digest, digest, process},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub fn command_output(command: &Command, extra: &[String], cwd: Option<&Path>) -> Result<Vec<u8>> {
    Ok(run_tool(tool_command(command, extra, cwd)?, command.timeout_seconds)?.into_bytes())
}

fn tool_command(
    command: &Command,
    extra: &[String],
    cwd: Option<&Path>,
) -> Result<std::process::Command> {
    command.validate()?;
    let mut argv = command.arguments.clone();
    argv.extend_from_slice(extra);
    let mut child = std::process::Command::new(&command.program);
    child
        .env_clear()
        .args(argv)
        .stdin(std::process::Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(path) = cwd {
        child.current_dir(path);
    }
    // Runtime plumbing is shared; forge and registry authority is not. The
    // adapter receives its sole enrolled API credential through request JSON.
    for key in [
        "PATH",
        "HOME",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "TMPDIR",
        "TMP",
        "TEMP",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NIX_SSL_CERT_FILE",
        "NIX_STATE_DIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            child.env(key, value);
        }
    }
    child.env("PYTHONNOUSERSITE", "1");
    Ok(child)
}

pub(super) fn run_tool(command: std::process::Command, timeout_seconds: u64) -> Result<String> {
    let log = tempfile::NamedTempFile::new()?;
    let result = process::run_command_with_limits(
        command,
        Some(log.path()),
        process::CommandLimits {
            timeout: Duration::from_secs(timeout_seconds),
            stdout_bytes: 16 * 1024 * 1024,
            stderr_bytes: 16 * 1024 * 1024,
        },
    );
    match result {
        Ok(output) => Ok(output),
        Err(error) => {
            // Unit-test tools use synthetic inputs; retain their diagnostics in
            // hosted logs even when a failed Nix build removes its workdir.
            #[cfg(test)]
            eprintln!(
                "fixture tool stderr: {}",
                fs::read_to_string(log.path()).unwrap_or_default()
            );
            let (_, path) = log.keep()?;
            Err(error).with_context(|| format!("tool stderr retained at {}", path.display()))
        }
    }
}

fn git(root: &Path, argv: &[&str]) -> Result<String> {
    git_raw(root, argv).map(|v| v.trim_end().to_owned())
}

fn git_raw(root: &Path, argv: &[&str]) -> Result<String> {
    let mut command = std::process::Command::new("git");
    // Disposable test clones receive their fixture identity before generation.
    // Production Git identity, signing and hooks are never overridden.
    #[cfg(test)]
    command.args([
        "-c",
        "user.name=Updater fixture",
        "-c",
        "user.email=updater@example.invalid",
        "-c",
        "commit.gpgsign=false",
    ]);
    command
        .current_dir(root)
        .args(argv)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    // Repository routing belongs to this disposable checkout. Keep operator
    // identity, signing, config and transport variables intact.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_SHALLOW_FILE",
        "GIT_GRAFT_FILE",
        "GIT_REPLACE_REF_BASE",
        "GIT_PREFIX",
        "GIT_INTERNAL_SUPER_PREFIX",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_QUARANTINE_PATH",
    ] {
        command.env_remove(key);
    }
    // These calls target disposable repositories in unit tests. Registry tests
    // mutate process-global Git configuration concurrently; pin only fixture Git.
    #[cfg(test)]
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    run_tool(command, 300)
}

pub fn lock(root: &Path) -> Result<File> {
    private_directory(root)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("lock"))?;
    fs2::FileExt::lock_exclusive(&file).context("wait for the workspace updater lock")?;
    verify_lock(root, &file)?;
    Ok(file)
}

pub fn verify_lock(root: &Path, held: &File) -> Result<()> {
    let original = same_file::Handle::from_file(held.try_clone()?)?;
    let current = same_file::Handle::from_path(root.join("lock"))
        .context("workspace lock disappeared or became inaccessible")?;
    ensure!(
        original == current,
        "workspace lock identity changed; reconciliation is required"
    );
    Ok(())
}

fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "state directory must not be a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().context("missing state parent")?)?;
    serde_json::to_writer_pretty(&mut temp, value)?;
    temp.write_all(b"\n")?;
    ensure!(
        temp.as_file().metadata()?.len() <= crate::review::MAX_JSON_BYTES,
        "serialized updater state exceeds the JSON read limit"
    );
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub struct FrozenSelection {
    pub projects: Vec<Project>,
    pub inventory_digest: String,
    pub completed: bool,
}

pub fn selection_path(root: &Path, key: &str) -> PathBuf {
    root.join(format!("rollout-{key}.json"))
}

pub fn profile_directory(workspace: &Workspace, name: &str) -> PathBuf {
    workspace
        .state_directory
        .join(format!("profile-{}", digest(name.as_bytes())))
}

pub fn policy_digest(
    config: &Config,
    profile: &Profile,
    workspace: &Workspace,
    consumer_forge: Option<&str>,
    coordinate: Option<&str>,
) -> Result<String> {
    let generator = config
        .forges
        .get(&profile.generator.forge)
        .context("unknown generator forge")?;
    let consumer = consumer_forge
        .map(|name| {
            config
                .forges
                .get_key_value(name)
                .context("unknown consumer forge")
        })
        .transpose()?;
    let Some(coordinate) = coordinate else {
        return canonical_digest(&(profile, workspace, generator, consumer));
    };
    // Other repositories' enrollment and overrides are not this transaction's
    // policy, so editing them keeps its pending candidate or pull request.
    let mut scoped = serde_json::to_value(profile)?;
    let enrolled = serde_json::Value::from(coordinate);
    for list in ["/select/coordinates", "/rollout/canaries"] {
        if let Some(values) = scoped
            .pointer_mut(list)
            .and_then(serde_json::Value::as_array_mut)
        {
            values.retain(|value| value == &enrolled);
        }
    }
    if let Some(overrides) = scoped["projectOverrides"].as_object_mut() {
        overrides.retain(|key, _| key == coordinate);
    }
    canonical_digest(&(scoped, workspace, generator, consumer))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    /// Absent (0) in v1 receipts, which always landed directly. Older binaries
    /// reject v2 fields as unknown rather than misreading a pull request.
    #[serde(default)]
    pub schema_version: u32,
    pub coordinate: String,
    pub config_digest: String,
    pub repository_id: String,
    pub source: String,
    pub candidate: String,
    pub branch: String,
    pub default_branch: String,
    pub policy_digest: String,
    pub phase: String,
    pub managed: BTreeMap<String, Option<String>>,
    pub diff: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<String>,
    /// Forge identifier of the candidate's pull request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    /// Target commit that merged the candidate, when the forge landed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed: Option<String>,
    /// Policy scoped to this coordinate; v2 receipts resume against it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_policy_digest: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Baseline {
    repository_id: String,
    managed: BTreeMap<String, Option<String>>,
}

pub fn status(root: &Path) -> Result<Vec<Receipt>> {
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut receipts = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("repo-")
            && entry.file_type()?.is_dir()
            && entry.path().join("receipt.json").is_file()
        {
            receipts.push(crate::review::read_json(
                &entry.path().join("receipt.json"),
            )?);
        }
    }
    receipts.sort_by(|a: &Receipt, b| a.coordinate.cmp(&b.coordinate));
    Ok(receipts)
}

fn request(
    forge: &Forge,
    coordinate: &str,
    operation: &str,
    revision: Option<&str>,
    branch: Option<&str>,
    workflows: Vec<String>,
    required_checks: Vec<Requirement>,
) -> AdapterRequest {
    AdapterRequest {
        schema_version: 1,
        operation: operation.into(),
        coordinate: coordinate.into(),
        api_url: forge.api_url.clone(),
        credential_file: forge.credential_file.clone(),
        authority: forge.check_authority.clone(),
        revision: revision.map(str::to_owned),
        branch: branch.map(str::to_owned),
        workflows,
        required_checks,
        plan: None,
        landing: None,
        target: None,
        change: None,
    }
}

/// Marks a request as pull-request landing into `target`; this is protocol v2.
fn pull_request(
    mut request: AdapterRequest,
    target: Option<&str>,
    change: Option<&str>,
) -> AdapterRequest {
    request.landing = Some("pull-request".into());
    request.target = target.map(str::to_owned);
    request.change = change.map(str::to_owned);
    request.schema_version = request.derived_version();
    request
}

/// Retires a published maintenance ref under a lease on its candidate. This
/// is cleanup: a ref that is gone or now belongs to another writer stays.
fn delete_maintenance_ref(checkout: &Path, branch: &str, candidate: &str) {
    let reference = format!("refs/heads/{branch}");
    let result = git(checkout, &["ls-remote", "origin", &reference]).and_then(|existing| {
        if existing.is_empty() {
            eprintln!("maintenance ref {branch} is already gone");
            return Ok(());
        }
        git(
            checkout,
            &[
                "push",
                "--quiet",
                &format!("--force-with-lease={reference}:{candidate}"),
                "origin",
                &format!(":{reference}"),
            ],
        )
        .map(drop)
    });
    if let Err(error) = result {
        eprintln!("retained maintenance ref {branch}: {error:#}");
    }
}

fn adapter(forge: &Forge, request: &AdapterRequest, root: &Path) -> Result<AdapterResponse> {
    request.validate()?;
    let snapshots = tempfile::Builder::new()
        .prefix("adapter-")
        .tempdir_in(root)?;
    let mut command = forge.adapter.clone();
    let executable = verified_executable(
        &command.program,
        forge.adapter_sha256.as_deref(),
        snapshots.path(),
        "adapter executable",
    )?;
    let mut inputs = BTreeMap::new();
    if command.program.starts_with("/nix/store") {
        command.program = executable;
    } else {
        inputs.insert(command.program.clone(), executable);
    }
    // Helpers need enrollment even when imported by the entrypoint rather than
    // named on argv. Bind verified bytes at their declared paths for imports.
    for (path, hash) in &forge.adapter_inputs {
        let pinned = verified_input(path, Some(hash), snapshots.path(), "adapter input")?;
        ensure!(
            !inputs.contains_key(path),
            "adapter input overlaps its executable"
        );
        if !path.starts_with("/nix/store") {
            inputs.insert(path.clone(), pinned);
        }
    }
    for argument in &command.arguments {
        let value = if argument.starts_with('-') {
            argument
                .split_once('=')
                .map_or(argument.as_str(), |(_, value)| value)
        } else {
            argument.as_str()
        };
        let path = Path::new(value);
        if path.is_absolute() {
            if path.starts_with("/nix/store") {
                store_file(path, Path::new("/nix/store"))?;
            } else {
                ensure!(inputs.contains_key(path), "adapter input is not enrolled");
            }
        }
    }
    if let Some(credential) = &request.credential_file {
        ensure!(
            !inputs.contains_key(credential),
            "adapter credential overlaps code inputs"
        );
        let pinned = snapshots.path().join("credential");
        fs::write(&pinned, credential_bytes(credential)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&pinned, fs::Permissions::from_mode(0o400))?;
        }
        inputs.insert(credential.clone(), pinned);
    }
    let path = root.join("request.json");
    save(&path, request)?;
    let response: AdapterResponse = serde_json::from_str(&run_tool(
        super::sandbox::adapter(&command, &inputs, &path)?,
        command.timeout_seconds,
    )?)?;
    response.validate(request)?;
    remote_identity(&response.remote, &request.coordinate)?;
    save(
        &root.join(format!(
            "evidence-{}.json",
            canonical_digest(&(request, &response))?
        )),
        &(request, &response),
    )?;
    ensure!(
        !matches!(response.status.as_str(), "unsupported" | "failed"),
        "adapter rejected {} for {}",
        request.operation,
        request.coordinate
    );
    Ok(response)
}

fn wait(
    forge: &Forge,
    request: &AdapterRequest,
    root: &Path,
    deadline: Instant,
) -> Result<AdapterResponse> {
    loop {
        let response = adapter(forge, request, root)?;
        if response.status != "pending" || Instant::now() >= deadline {
            return Ok(response);
        }
        std::thread::sleep(
            Duration::from_secs(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

fn remote_identity(remote: &str, expected: &str) -> Result<()> {
    #[cfg(test)]
    if remote.starts_with("file://") {
        return Ok(());
    }
    let url = url::Url::parse(remote)?;
    ensure!(
        matches!(url.scheme(), "https" | "ssh")
            && (url.port().is_none() || (url.scheme() == "ssh" && url.port() == Some(22)))
            && (url.scheme() != "https" || url.username().is_empty())
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "invalid repository transport"
    );
    let (host, repository) = expected
        .split_once('/')
        .context("missing enrolled repository")?;
    ensure!(
        url.host_str()
            .context("missing repository host")?
            .eq_ignore_ascii_case(host)
            && url
                .path()
                .trim_start_matches('/')
                .strip_suffix(".git")
                .unwrap_or_else(|| url.path().trim_start_matches('/'))
                == repository,
        "adapter remote differs from enrolled coordinate"
    );
    Ok(())
}

fn store_file(path: &Path, store: &Path) -> Result<PathBuf> {
    let resolved = path.canonicalize()?;
    ensure!(
        path.starts_with(store) && resolved.starts_with(store) && resolved.is_file(),
        "store input must resolve to an immutable file inside the Nix store"
    );
    // argv[0] selects multicall applets and can affect wrappers. Resolve only to
    // verify containment; execute/read using the original enrolled store path.
    Ok(path.to_owned())
}

fn verified_input(
    path: &Path,
    expected: Option<&str>,
    snapshots: &Path,
    label: &str,
) -> Result<PathBuf> {
    let Some(expected) = expected else {
        return store_file(path, Path::new("/nix/store"));
    };
    let bytes = fs::read(path)?;
    ensure!(
        digest(&bytes) == expected,
        "{label} digest differs from enrollment"
    );
    if path.starts_with("/nix/store") {
        return store_file(path, Path::new("/nix/store"));
    }
    fs::create_dir_all(snapshots)?;
    // Every verification owns fresh bytes; never rewrite a read-only snapshot
    // retained by another executable or argument from the same transaction.
    let mut directory = tempfile::Builder::new();
    directory.prefix("input-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        directory.permissions(fs::Permissions::from_mode(0o700));
    }
    let directory = directory.tempdir_in(snapshots)?.keep();
    let pinned = directory.join(path.file_name().context("input has no file name")?);
    fs::write(&pinned, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&pinned, fs::Permissions::from_mode(0o500))?;
    }
    Ok(pinned)
}

fn verified_executable(
    path: &Path,
    expected: Option<&str>,
    snapshots: &Path,
    label: &str,
) -> Result<PathBuf> {
    let executable = verified_input(path, expected, snapshots, label)?;
    let mut current = executable.clone();
    let mut seen = BTreeSet::new();
    loop {
        ensure!(
            seen.insert(current.clone()),
            "cyclic executable interpreter chain"
        );
        let bytes = fs::read(&current)?;
        let Some(script) = bytes.strip_prefix(b"#!") else {
            if current == executable {
                // An executable digest does not bind PT_INTERP, RPATH or shared
                // libraries. Native programs must retain their store closure
                // and original location rather than run a relocated snapshot.
                return store_file(path, Path::new("/nix/store"))
                    .context("native executable runtimes must be Nix-store backed");
            }
            return Ok(executable);
        };
        let line = script
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default();
        let mut fields = std::str::from_utf8(line)?.split_whitespace();
        let interpreter = fields.next().context("missing script interpreter")?;
        current = store_file(Path::new(interpreter), Path::new("/nix/store"))
            .context("script interpreter must be an immutable Nix-store file; env-selected and mutable runtimes are not enrolled")?;
        let arguments = fields.collect::<Vec<_>>();
        // The kernel invokes the shebang path, including a multicall program's
        // symlink name (for example BusyBox's sh), not its canonical basename.
        let name = Path::new(interpreter)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let python = name.strip_prefix("python").is_some_and(|version| {
            version
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.')
        });
        ensure!(
            arguments.is_empty()
                || (python && arguments == ["-I"])
                || (matches!(name, "sh" | "bash" | "dash") && arguments == ["-e"]),
            "script shebang arguments can select unenrolled code"
        );
    }
}

fn inspect(
    forge: &Forge,
    project: &Project,
    target: Option<&str>,
    landing: &str,
    root: &Path,
) -> Result<AdapterResponse> {
    let mut request = request(
        forge,
        &project.coordinate,
        "inspect",
        None,
        target,
        vec![],
        vec![],
    );
    if landing == "pull-request" {
        request = pull_request(request, None, None);
    }
    let response = adapter(forge, &request, root)?;
    ensure!(
        response.status == "success",
        "repository inspection incomplete"
    );
    Ok(response)
}

fn workflows(plan: &CiUpdatePlan) -> Vec<String> {
    plan.files
        .iter()
        .map(|f| f.relative_path.to_string_lossy().into_owned())
        .collect()
}

fn git_metadata(checkout: &Path) -> Result<String> {
    let root = checkout.join(".git");
    ensure!(
        fs::symlink_metadata(&root)?.is_dir(),
        "redirected candidate Git metadata"
    );
    let mut pending = vec![root.clone()];
    let mut visited = BTreeSet::new();
    let mut entries = BTreeMap::new();
    // Read-only Git may refresh the index/stat caches. Object contents are
    // content-addressed, but objects/info (including alternates) remains policy.
    while let Some(path) = pending.pop() {
        let resolved = path.canonicalize()?;
        let metadata = fs::metadata(&resolved)?;
        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::MetadataExt;
            metadata.mode()
        };
        #[cfg(not(unix))]
        let permissions = u32::from(metadata.permissions().readonly());
        let content = if metadata.is_dir() {
            if visited.insert(resolved.clone()) {
                for child in fs::read_dir(&path)? {
                    let child = child?.path();
                    if path == root
                        && matches!(
                            child.file_name().and_then(|name| name.to_str()),
                            Some("objects" | "logs" | "index")
                        )
                    {
                        if child.file_name() == Some(std::ffi::OsStr::new("objects"))
                            && child.join("info").exists()
                        {
                            pending.push(child.join("info"));
                        }
                        continue;
                    }
                    pending.push(child);
                }
            }
            "directory".into()
        } else {
            ensure!(
                metadata.is_file(),
                "unsupported candidate Git metadata entry"
            );
            digest(&fs::read(&resolved)?)
        };
        entries.insert(path, (resolved, permissions, content));
    }
    canonical_digest(&entries)
}

fn generate(executable: &Path, checkout: &Path, state_directory: &Path) -> Result<CiUpdatePlan> {
    let metadata = git_metadata(checkout)?;
    let configuration = git(checkout, &["config", "--null", "--list", "--show-origin"])?;
    let nix_state = tempfile::tempdir()?;
    let output = super::checkpoint::protect(state_directory, checkout, || {
        run_tool(
            super::sandbox::generation(executable, checkout, nix_state.path())?,
            900,
        )
        .context("credential-free generation sandbox failed")
    })?;
    let plan: CiUpdatePlan = serde_json::from_str(&output)?;
    // Inspect raw metadata before invoking Git with any generator-written policy.
    ensure!(
        git_metadata(checkout)? == metadata,
        "read-only generator modified Git metadata"
    );
    ensure!(
        git(checkout, &["config", "--null", "--list", "--show-origin"])? == configuration,
        "read-only generator modified effective Git configuration"
    );
    ensure!(
        git(checkout, &["status", "--porcelain"])?.is_empty(),
        "read-only generator plan modified candidate sources"
    );
    ensure!(
        plan.schema == "simit.ci-update-plan.v1",
        "generator does not support the enrolled generation protocol"
    );
    let mut seen = BTreeSet::new();
    for path in plan
        .files
        .iter()
        .map(|f| &f.relative_path)
        .chain(&plan.obsolete)
    {
        relative(path)?;
        ensure!(
            seen.insert(path),
            "duplicate/overlapping generation ownership"
        );
    }
    ensure!(!plan.files.is_empty(), "empty generation plan");
    Ok(plan)
}

fn hashes(
    root: &Path,
    paths: impl Iterator<Item = String>,
    tree: Option<&str>,
) -> Result<BTreeMap<String, Option<String>>> {
    #[cfg(unix)]
    let filemode = tree.is_none()
        && git(
            root,
            &["config", "--bool", "--default", "true", "core.filemode"],
        )? == "true";
    paths
        .map(|p| {
            relative(Path::new(&p))?;
            let path = root.join(&p);
            // Reject symlink parents as well as symlink outputs before reads/writes.
            let mut parent = path.as_path();
            while parent != root {
                match fs::symlink_metadata(parent) {
                    Ok(meta) => ensure!(
                        !meta.file_type().is_symlink(),
                        "generated path traverses a symlink"
                    ),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e.into()),
                }
                parent = parent.parent().context("invalid managed path")?;
            }
            // Canonical mode/blob identities preserve conversion while detecting chmod.
            let hash = if let Some(tree) = tree {
                let object = git(
                    root,
                    &["--literal-pathspecs", "ls-tree", "-z", tree, "--", &p],
                )?;
                if object.is_empty() {
                    None
                } else {
                    let metadata = object
                        .split_once('\t')
                        .context("missing managed tree entry")?
                        .0;
                    ensure!(
                        metadata.starts_with("100644 blob ")
                            || metadata.starts_with("100755 blob "),
                        "managed output is not a regular Git file"
                    );
                    Some(metadata.to_owned())
                }
            } else if path.exists() {
                ensure!(path.is_file(), "managed output is not a file");
                let index = git(
                    root,
                    &["--literal-pathspecs", "ls-files", "--stage", "-z", "--", &p],
                )?;
                let mode = index.split_whitespace().next().unwrap_or("100644");
                #[cfg(unix)]
                let mode = if filemode {
                    use std::os::unix::fs::PermissionsExt;
                    if path.metadata()?.permissions().mode() & 0o100 != 0 {
                        "100755"
                    } else {
                        "100644"
                    }
                } else {
                    mode
                };
                ensure!(
                    matches!(mode, "100644" | "100755"),
                    "managed output is not a regular Git file"
                );
                let object = git(root, &["hash-object", &format!("--path={p}"), "--", &p])?;
                Some(format!("{mode} blob {object}"))
            } else {
                None
            };
            Ok((p, hash))
        })
        .collect()
}

pub fn update(
    config: &Config,
    profile_name: &str,
    profile: &Profile,
    workspace: &Workspace,
    project: &Project,
    push: bool,
    wait_seconds: u64,
) -> Result<Receipt> {
    config.validate()?;
    coordinate(&project.coordinate)?;
    let forge = config
        .forges
        .get(&project.forge)
        .context("unknown consumer forge")?;
    ensure!(
        project.coordinate.starts_with(&format!("{}/", forge.host)),
        "frozen project coordinate differs from the current consumer forge host"
    );
    ensure!(
        profile.select.matches(project)?,
        "project is outside operator-owned coordinate enrollment or selection"
    );
    let deadline = Instant::now() + Duration::from_secs(wait_seconds);
    let state_directory = config.state_directory(workspace)?;
    super::checkpoint::ensure_idle(&state_directory)?;
    let profile_directory = resolved_path(&profile_directory(workspace, profile_name))?;
    ensure!(
        profile_directory.starts_with(&state_directory),
        "profile state escapes its workspace state directory"
    );
    let root = resolved_path(
        &profile_directory.join(format!("repo-{}", digest(project.coordinate.as_bytes()))),
    )?;
    ensure!(
        root.starts_with(&profile_directory),
        "repository state escapes its profile state directory"
    );
    private_directory(&root)?;
    let receipt_path = root.join("receipt.json");
    let config_digest = policy_digest(config, profile, workspace, Some(&project.forge), None)?;
    let project_digest = policy_digest(
        config,
        profile,
        workspace,
        Some(&project.forge),
        Some(&project.coordinate),
    )?;
    let over = profile.project_overrides.get(&project.coordinate);
    let target = over.and_then(|o| o.target_branch.as_deref());
    let landing = over
        .and_then(|o| o.landing.as_deref())
        .unwrap_or(&profile.landing);
    let observed = inspect(forge, project, target, landing, &root)?;
    let mut requirements = observed.requirements.clone();
    for r in profile
        .required_checks
        .iter()
        .chain(over.into_iter().flat_map(|o| &o.required_checks))
    {
        if !requirements.contains(r) {
            requirements.push(r.clone());
        }
    }
    ensure!(
        !requirements.is_empty(),
        "repository requires explicit qualification checks"
    );
    let mut previous: Option<Receipt> = if receipt_path.exists() {
        Some(crate::review::read_json(&receipt_path)?)
    } else {
        None
    };
    if let Some(previous) = &previous {
        ensure!(
            previous.coordinate == project.coordinate
                && previous.repository_id == observed.repository_id,
            "repository identity differs from the enrolled receipt"
        );
        ensure!(
            !matches!(previous.phase.as_str(), "qualified" | "unchanged")
                || root.join("baseline.json").is_file(),
            "adopted receipt has no baseline; explicit reconciliation is required"
        );
    }
    let checkout = root.join("checkout");
    let archive = |retained: &Receipt| -> Result<()> {
        // Archive before replacing the checkout, so interruption cannot leave an
        // unfinished receipt referring to a new clone. Adopted baselines survive.
        fs::rename(
            &receipt_path,
            root.join(format!("invalidated-{}.json", canonical_digest(retained)?)),
        )?;
        File::open(&root)?.sync_all()?;
        if push {
            delete_maintenance_ref(&checkout, &retained.branch, &retained.candidate);
        }
        Ok(())
    };
    // A preview never writes to the forge, so it reports a parked pull request
    // as it stands rather than closing, proposing or retiring anything.
    if !push && previous.as_ref().is_some_and(|r| r.phase == "proposed") {
        return previous.context("missing parked receipt");
    }
    // Re-read a parked pull request first. A stale one is closed rather than
    // merged, since its qualification no longer describes the target.
    if let Some(retained) = previous.as_mut().filter(|r| r.phase == "proposed") {
        let stale = observed.revision != retained.source
            || observed.policy_digest != retained.policy_digest
            || observed.default_branch != retained.default_branch
            || retained.project_policy_digest.as_ref() != Some(&project_digest);
        let reread = pull_request(
            request(
                forge,
                &project.coordinate,
                if stale { "close" } else { "propose" },
                Some(&retained.candidate),
                Some(&retained.branch),
                vec![],
                vec![],
            ),
            Some(target.unwrap_or(&retained.default_branch)),
            retained.change.as_deref(),
        );
        match adapter(forge, &reread, &root)?.change {
            // Merged, possibly by a human: adoption verifies what landed.
            Some(change) if change.state == "merged" => {
                retained.change = Some(change.id);
                retained.landed = change.merge_commit;
                retained.phase = "adopting".into();
                save(&receipt_path, &*retained)?;
            }
            Some(change) if change.state == "open" => retained.change = Some(change.id),
            _ => {
                archive(retained)?;
                previous = None;
            }
        }
    }
    if let Some(retained) = previous.as_ref().filter(|receipt| {
        !matches!(receipt.phase.as_str(), "qualified" | "unchanged")
            && observed.revision != receipt.source
            && observed.revision != receipt.candidate
            && receipt.landed.as_ref() != Some(&observed.revision)
    }) {
        ensure!(
            matches!(
                retained.phase.as_str(),
                "prepared" | "candidate" | "landing"
            ) && retained.default_branch == observed.default_branch
                && retained.policy_digest == observed.policy_digest,
            "landed source or repository policy changed; explicit reconciliation is required"
        );
        git(
            &checkout,
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                "origin",
                &observed.revision,
            ],
        )?;
        ensure!(
            !git(
                &checkout,
                &[
                    "rev-list",
                    "--max-count=1",
                    &retained.candidate,
                    "--not",
                    &observed.revision
                ]
            )?
            .is_empty(),
            "remote advanced beyond the landed candidate; adoption reconciliation is required"
        );
        archive(retained)?;
        previous = None;
    }
    if let Some(previous) = previous
        .as_ref()
        .filter(|r| !matches!(r.phase.as_str(), "qualified" | "unchanged"))
    {
        ensure!(
            if previous.schema_version >= 2 {
                previous.project_policy_digest.as_ref() == Some(&project_digest)
            } else {
                previous.config_digest == config_digest
            },
            "an unfinished rollout uses another policy; resume it with its original configuration"
        );
        ensure!(
            previous.repository_id == observed.repository_id
                && previous.policy_digest == observed.policy_digest
                && previous.default_branch == observed.default_branch,
            "repository identity/default branch/protection changed during rollout"
        );
        ensure!(
            observed.revision == previous.source
                || observed.revision == previous.candidate
                || previous.landed.as_ref() == Some(&observed.revision),
            "remote moved; candidate qualification invalidated"
        );
        ensure!(
            git(&checkout, &["rev-parse", "HEAD"])? == previous.candidate,
            "retained candidate checkout changed"
        );
    } else {
        if checkout.exists() {
            ensure!(
                fs::symlink_metadata(&checkout)?.is_dir(),
                "refusing to retire a redirected state checkout"
            );
            if root.join("clone-owner.json").exists() {
                let owner: String = crate::review::read_json(&root.join("clone-owner.json"))?;
                ensure!(
                    owner == project.coordinate,
                    "refusing to retire another repository's checkout"
                );
            }
            // This reserved path is disposable private state, including legacy
            // partial clones that were interrupted before recording ownership.
            fs::remove_dir_all(&checkout)?;
        }
        let staging = tempfile::Builder::new()
            .prefix("clone-")
            .tempdir_in(&root)?;
        git(
            staging.path(),
            &[
                "clone",
                "--quiet",
                "--no-checkout",
                "--no-hardlinks",
                "--",
                &observed.remote,
                "checkout",
            ],
        )?;
        // Record ownership before publishing the complete clone. An interruption
        // can leave an unused marker, but never an unmarked published checkout.
        save(&root.join("clone-owner.json"), &project.coordinate)?;
        fs::rename(staging.path().join("checkout"), &checkout)?;
        git(&checkout, &["checkout", "--detach", &observed.revision])?;
    }
    let generator_snapshot = tempfile::Builder::new()
        .prefix("generator-")
        .tempdir_in(&root)?;
    let generator = verified_executable(
        &profile.generator.executable,
        profile.generator.executable_sha256.as_deref(),
        generator_snapshot.path(),
        "generator executable",
    )?;
    // Qualify the immutable generator source independently of consumer candidates.
    let generator_forge = &config.forges[&profile.generator.forge];
    let generator_evidence = wait(
        generator_forge,
        &request(
            generator_forge,
            &profile.generator.source_coordinate,
            "qualify",
            Some(&profile.generator.source_revision),
            None,
            vec![],
            profile.generator.required_checks.clone(),
        ),
        &root,
        deadline,
    )?;
    ensure!(
        generator_evidence.status == "success",
        "generator qualification is incomplete"
    );
    let mut receipt = if let Some(previous) =
        previous.filter(|r| !matches!(r.phase.as_str(), "qualified" | "unchanged"))
    {
        previous
    } else {
        let plan = generate(&generator, &checkout, &state_directory)?;
        let paths = plan
            .files
            .iter()
            .map(|f| f.relative_path.to_string_lossy().into_owned())
            .chain(
                plan.obsolete
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned()),
            )
            .collect::<Vec<_>>();
        let before = hashes(&checkout, paths.clone().into_iter(), None)?;
        let baseline_path = root.join("baseline.json");
        if baseline_path.exists() {
            let baseline: Baseline = crate::review::read_json(&baseline_path)?;
            ensure!(
                baseline.repository_id == observed.repository_id,
                "repository identity differs from adopted baseline"
            );
            ensure!(
                hashes(&checkout, baseline.managed.keys().cloned(), Some("HEAD"))?
                    == baseline.managed
                    && hashes(&checkout, baseline.managed.keys().cloned(), None)?
                        == baseline.managed,
                "managed outputs changed since adoption; explicit reconciliation is required"
            );
            ensure!(
                baseline
                    .managed
                    .iter()
                    .all(|(path, hash)| hash.is_none() || before.contains_key(path)),
                "generation plan abandons a managed path; retain it or explicitly mark it obsolete"
            );
            ensure!(
                before
                    .keys()
                    .all(|p| baseline.managed.contains_key(p) || before[p].is_none()),
                "newly managed path already exists outside the baseline"
            );
        } else {
            ensure!(
                profile.enroll_current,
                "initial managed baseline is not enrolled"
            );
        }
        let workflow_paths = workflows(&plan);
        ensure!(
            !workflow_paths.is_empty(),
            "candidate has no owned qualification paths"
        );
        save(&root.join("workflows.json"), &workflow_paths)?;
        crate::project::reconcile_generated_files(
            &checkout,
            &plan.files,
            &plan.obsolete,
            "CI update",
            false,
            false,
        )?;
        let mut stage = vec!["--literal-pathspecs", "add", "--all", "--force", "--"];
        stage.extend(
            paths
                .iter()
                .filter(|p| checkout.join(p).exists() || before[*p].is_some())
                .map(String::as_str),
        );
        git(&checkout, &stage)?;
        let tree = git(&checkout, &["write-tree"])?;
        let managed = hashes(&checkout, paths.into_iter(), Some(&tree))?;
        let diff = git(
            &checkout,
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "HEAD",
                "--",
            ],
        )?;
        let receipt = Receipt {
            schema_version: 2,
            coordinate: project.coordinate.clone(),
            config_digest,
            repository_id: observed.repository_id.clone(),
            source: observed.revision.clone(),
            candidate: String::new(),
            branch: String::new(),
            default_branch: observed.default_branch.clone(),
            policy_digest: observed.policy_digest.clone(),
            phase: "preview".into(),
            managed,
            diff,
            landing: Some(landing.into()),
            change: None,
            landed: None,
            project_policy_digest: Some(project_digest.clone()),
        };
        if receipt.diff.is_empty() {
            if push {
                let qualified = wait(
                    forge,
                    &request(
                        forge,
                        &project.coordinate,
                        "qualify",
                        Some(&observed.revision),
                        Some(target.unwrap_or(&observed.default_branch)),
                        workflow_paths,
                        requirements.clone(),
                    ),
                    &root,
                    deadline,
                )?;
                ensure!(
                    qualified.status == "success"
                        && qualified.repository_id == observed.repository_id,
                    "unchanged source qualification is incomplete"
                );
                let fresh = inspect(forge, project, target, landing, &root)?;
                ensure!(
                    fresh.revision == observed.revision
                        && fresh.repository_id == observed.repository_id
                        && fresh.default_branch == observed.default_branch
                        && fresh.policy_digest == observed.policy_digest,
                    "unchanged source or policy moved before enrollment"
                );
                save(
                    &baseline_path,
                    &Baseline {
                        repository_id: observed.repository_id.clone(),
                        managed: receipt.managed.clone(),
                    },
                )?;
            }
            let receipt = Receipt {
                candidate: observed.revision.clone(),
                phase: "unchanged".into(),
                ..receipt
            };
            if push {
                save(&receipt_path, &receipt)?;
            }
            return Ok(receipt);
        }
        if !push {
            return Ok(receipt);
        }
        let author = git(&checkout, &["var", "GIT_AUTHOR_IDENT"])?;
        let committer = git(&checkout, &["var", "GIT_COMMITTER_IDENT"])?;
        git(
            &checkout,
            &[
                "commit",
                "-m",
                &format!(
                    "ci: adopt qualified simit {}",
                    profile.generator.source_revision
                ),
            ],
        )?;
        let verification = git(
            &checkout,
            &["show", "--format=fuller", "--no-patch", "HEAD"],
        )?;
        fs::write(root.join("commit.txt"), verification)?;
        let identity = |value: &str| value.rsplit_once("> ").map(|(name, _)| format!("{name}>"));
        ensure!(
            identity(&author).as_deref()
                == Some(git(&checkout, &["show", "-s", "--format=%an <%ae>", "HEAD"])?.as_str())
                && identity(&committer).as_deref()
                    == Some(
                        git(&checkout, &["show", "-s", "--format=%cn <%ce>", "HEAD"])?.as_str()
                    ),
            "commit identity differs from operator Git configuration"
        );
        let candidate = git(&checkout, &["rev-parse", "HEAD"])?;
        ensure!(
            git(&checkout, &["show", "-s", "--format=%P", "HEAD"])? == observed.revision,
            "candidate must have exactly the observed source as parent"
        );
        let changed = git(
            &checkout,
            &[
                "diff",
                "--name-only",
                "-z",
                &observed.revision,
                "HEAD",
                "--",
            ],
        )?;
        ensure!(
            changed
                .split('\0')
                .filter(|p| !p.is_empty())
                .all(|p| receipt.managed.contains_key(p)),
            "commit modified paths outside generator ownership"
        );
        let branch = format!(
            "{}{}",
            profile.rollout.maintenance_ref_prefix,
            &candidate[..16]
        );
        git(
            &checkout,
            &["check-ref-format", &format!("refs/heads/{branch}")],
        )?;
        Receipt {
            candidate,
            branch,
            phase: "prepared".into(),
            ..receipt
        }
    };
    ensure!(
        hashes(&checkout, receipt.managed.keys().cloned(), Some("HEAD"))? == receipt.managed
            && hashes(&checkout, receipt.managed.keys().cloned(), None)? == receipt.managed
            && git(&checkout, &["status", "--porcelain"])?.is_empty(),
        "retained candidate files changed"
    );
    let mut tracked_paths = vec!["--literal-pathspecs", "ls-files", "-z", "--"];
    tracked_paths.extend(receipt.managed.keys().map(String::as_str));
    let tracked = git(&checkout, &tracked_paths)?;
    let tracked = tracked.split('\0').collect::<BTreeSet<_>>();
    ensure!(
        receipt
            .managed
            .iter()
            .all(|(path, hash)| tracked.contains(path.as_str()) == hash.is_some()),
        "candidate index does not contain the managed ownership plan"
    );
    if !push {
        receipt.phase = "preview".into();
        return Ok(receipt);
    }
    // Rebuild admission from verified committed blobs on both first publication
    // and resume. Clean filters and prior admission never substitute for the
    // exact bytes this candidate will execute under the current forge policy.
    let admission_plan = CiUpdatePlan {
        schema: "simit.ci-update-plan.v1".into(),
        files: receipt
            .managed
            .iter()
            .filter(|(_, hash)| hash.is_some())
            .map(|(path, _)| {
                Ok(crate::project::GeneratedFile {
                    relative_path: path.into(),
                    content: git_raw(
                        &checkout,
                        &["cat-file", "blob", &format!("{}:{path}", receipt.candidate)],
                    )?,
                })
            })
            .collect::<Result<_>>()?,
        obsolete: receipt
            .managed
            .iter()
            .filter(|(_, hash)| hash.is_none())
            .map(|(path, _)| path.into())
            .collect(),
    };
    let workflow_paths: Vec<String> = crate::review::read_json(&root.join("workflows.json"))?;
    ensure!(
        workflow_paths.iter().collect::<BTreeSet<_>>()
            == receipt
                .managed
                .iter()
                .filter(|(_, hash)| hash.is_some())
                .map(|(path, _)| path)
                .collect(),
        "retained workflow ownership differs from the candidate"
    );
    let target_branch = target.unwrap_or(&observed.default_branch);
    let admit = |branch: &str| -> Result<()> {
        let mut admission = request(
            forge,
            &project.coordinate,
            "admit",
            Some(&receipt.candidate),
            Some(branch),
            workflow_paths.clone(),
            requirements.clone(),
        );
        admission.plan = Some(admission_plan.clone());
        if landing == "pull-request" && branch == target_branch {
            admission = pull_request(admission, Some(branch), None);
        }
        let evidence = adapter(forge, &admission, &root)?;
        ensure!(
            evidence.status == "success" && evidence.repository_id == receipt.repository_id,
            "candidate workflow admission is incomplete"
        );
        Ok(())
    };
    for branch in [receipt.branch.as_str(), target_branch] {
        admit(branch)?;
    }
    save(&receipt_path, &receipt)?;
    if observed.revision == receipt.source && receipt.landed.is_none() {
        let remote_ref = format!("refs/heads/{}", receipt.branch);
        let existing = git(&checkout, &["ls-remote", "origin", &remote_ref])?;
        if existing.is_empty() {
            git(
                &checkout,
                &[
                    "push",
                    &format!("--force-with-lease={remote_ref}:"),
                    "origin",
                    &format!("{}:{remote_ref}", receipt.candidate),
                ],
            )?;
        } else {
            ensure!(
                existing.split_whitespace().next() == Some(&receipt.candidate),
                "maintenance ref belongs to another writer"
            );
        }
        receipt.phase = "candidate".into();
        save(&receipt_path, &receipt)?;
        let candidate = wait(
            forge,
            &request(
                forge,
                &project.coordinate,
                "qualify",
                Some(&receipt.candidate),
                Some(&receipt.branch),
                workflow_paths.clone(),
                requirements.clone(),
            ),
            &root,
            deadline,
        )?;
        ensure!(
            candidate.repository_id == receipt.repository_id,
            "candidate qualification repository changed"
        );
        if candidate.status != "success" {
            return Ok(receipt);
        }
        let fresh = inspect(forge, project, target, landing, &root)?;
        ensure!(
            fresh.repository_id == receipt.repository_id
                && fresh.revision == receipt.source
                && fresh.default_branch == receipt.default_branch
                && fresh.policy_digest == receipt.policy_digest,
            "remote identity, source or policy moved after qualification"
        );
        admit(target_branch)?;
        if landing == "pull-request" {
            receipt.phase = "proposed".into();
            save(&receipt_path, &receipt)?;
            let change = |operation: &str, id: Option<&str>| {
                pull_request(
                    request(
                        forge,
                        &project.coordinate,
                        operation,
                        Some(&receipt.candidate),
                        Some(&receipt.branch),
                        vec![],
                        vec![],
                    ),
                    Some(target_branch),
                    id,
                )
            };
            let proposed = adapter(forge, &change("propose", None), &root)?
                .change
                .context("missing proposed pull request")?;
            let merged = if proposed.state == "open" {
                wait(forge, &change("merge", Some(&proposed.id)), &root, deadline)?
                    .change
                    .context("missing merged pull request")?
            } else {
                proposed
            };
            receipt.change = Some(merged.id.clone());
            match merged.state.as_str() {
                // Parked until approvals or forge mergeability arrive.
                "open" => {
                    save(&receipt_path, &receipt)?;
                    return Ok(receipt);
                }
                "closed" => {
                    save(&receipt_path, &receipt)?;
                    anyhow::bail!("pull request {} closed without merging", merged.id)
                }
                _ => receipt.landed = merged.merge_commit,
            }
        } else {
            receipt.phase = "landing".into();
            save(&receipt_path, &receipt)?;
            // The candidate is a direct child of source; the explicit lease is a compare-and-swap.
            let target_ref = format!("refs/heads/{target_branch}");
            git(
                &checkout,
                &[
                    "push",
                    &format!("--force-with-lease={target_ref}:{}", receipt.source),
                    "origin",
                    &format!("{}:{target_ref}", receipt.candidate),
                ],
            )?;
        }
    }
    receipt.phase = "adopting".into();
    save(&receipt_path, &receipt)?;
    // A pull request can land a merge, squash or rebase commit instead of the
    // candidate. It must carry exactly the candidate's tree onto its source.
    let landed = receipt.landed.clone().unwrap_or(receipt.candidate.clone());
    let fresh = inspect(forge, project, target, landing, &root)?;
    ensure!(
        fresh.repository_id == receipt.repository_id
            && fresh.revision == landed
            && fresh.default_branch == receipt.default_branch
            && fresh.policy_digest == receipt.policy_digest,
        "landed source or policy changed before adoption"
    );
    if landed != receipt.candidate {
        git(
            &checkout,
            &["fetch", "--quiet", "--no-tags", "origin", &landed],
        )?;
        let tree = |commit: &str| git(&checkout, &["rev-parse", &format!("{commit}^{{tree}}")]);
        ensure!(
            tree(&landed)? == tree(&receipt.candidate)?
                && git(&checkout, &["rev-parse", &format!("{landed}^1")])? == receipt.source,
            "landed commit differs from the qualified candidate tree or source"
        );
    }
    let adopted = wait(
        forge,
        &request(
            forge,
            &project.coordinate,
            "qualify",
            Some(&landed),
            Some(target_branch),
            workflow_paths,
            requirements,
        ),
        &root,
        deadline,
    )?;
    ensure!(
        adopted.repository_id == receipt.repository_id,
        "adoption qualification repository changed"
    );
    if adopted.status == "success" {
        let final_state = inspect(forge, project, target, landing, &root)?;
        ensure!(
            final_state.revision == landed
                && final_state.repository_id == receipt.repository_id
                && final_state.policy_digest == receipt.policy_digest
                && final_state.default_branch == receipt.default_branch,
            "remote moved before adoption receipt"
        );
        save(
            &root.join("baseline.json"),
            &Baseline {
                repository_id: receipt.repository_id.clone(),
                managed: receipt.managed.clone(),
            },
        )?;
        receipt.phase = "qualified".into();
        save(&receipt_path, &receipt)?;
        delete_maintenance_ref(&checkout, &receipt.branch, &receipt.candidate);
    }
    Ok(receipt)
}

// The updater's execution boundary is Linux namespaces; portable policy and
// adapter tests remain in their respective modules on the other platforms.
// Only namespace_* tests execute Bubblewrap; package checks run the rest here.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn oversized_serialized_state_preserves_the_readable_previous_receipt() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("receipt.json");
        save(&path, &"previous readable receipt").unwrap();
        // JSON escaping doubles these bytes although raw command output fits.
        let diff = "\n".repeat(crate::review::MAX_JSON_BYTES as usize / 2);
        assert!(save(&path, &json!({"diff": diff})).is_err());
        let previous: String = crate::review::read_json(&path).unwrap();
        assert_eq!(previous, "previous readable receipt");
    }

    #[test]
    fn namespace_adopted_receipts_cannot_reenroll_a_missing_baseline() {
        for phase in ["qualified", "unchanged"] {
            let f = Fixture::new();
            let adopted = f.update(true).unwrap();
            assert_eq!(adopted.phase, "qualified");
            if phase == "unchanged" {
                assert_eq!(f.update(true).unwrap().phase, phase);
            }
            fs::remove_file(f.root().join("baseline.json")).unwrap();
            let checkout = f.root().join("checkout");
            fs::write(checkout.join("new-managed.txt"), "project-owned content\n").unwrap();
            let source = f.publish(&checkout, "occupy a previously adopted path");
            let receipt = fs::read(f.root().join("receipt.json")).unwrap();
            assert!(
                f.update(true)
                    .unwrap_err()
                    .to_string()
                    .contains("adopted receipt has no baseline")
            );
            assert_eq!(f.remote_head(), source);
            assert_eq!(fs::read(f.root().join("receipt.json")).unwrap(), receipt);
        }
    }

    #[test]
    fn namespace_generation_cannot_read_credentials_or_reach_host_network_and_daemon() {
        let mut f = Fixture::new();
        let secret = f.source.with_file_name("forge-credential");
        fs::write(&secret, "private-forge-token").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        fs::write(
            f.source.join("flake.nix"),
            "{ outputs = _: { simitConfig.ci.extra_setup = [\"safe\"]; }; }\n",
        )
        .unwrap();
        f.publish(&f.source, "enroll a no-input flake");
        f.inject_generator(&format!(r#"
import os, socket, subprocess
from pathlib import Path
assert not Path({secret:?}).exists()
assert not Path('/nix/var/nix/daemon-socket/socket').exists()
assert Path('/nix/var/nix/db/schema').read_text().strip().isdigit()
assert not any(key in os.environ for key in ('SSH_AUTH_SOCK', 'GH_TOKEN', 'GITHUB_TOKEN'))
try:
    socket.create_connection(('127.0.0.1', {port}), timeout=0.1)
except OSError:
    pass
else:
    raise AssertionError('host network is reachable')
argv = ['nix', 'eval', '--json', '--offline', '--no-update-lock-file', '--no-write-lock-file']
safe = subprocess.run([*argv, '.#simitConfig'], capture_output=True, text=True)
assert safe.returncode == 0, safe.stderr
assert 'safe' in safe.stdout
unsafe = subprocess.run([*argv, '--expr', 'builtins.readFile {nix_path}'], capture_output=True, text=True)
assert unsafe.returncode != 0
assert 'private-forge-token' not in unsafe.stdout
"#, secret = secret.to_str().unwrap(), nix_path = serde_json::to_string(secret.to_str().unwrap()).unwrap()));
        assert_eq!(f.update(true).unwrap().phase, "qualified");
        assert_eq!(fs::read_to_string(secret).unwrap(), "private-forge-token");
    }

    #[test]
    fn namespace_generation_resolves_locked_remote_inputs_from_offline_store_registrations() {
        let mut f = Fixture::new();
        let input = f.source.with_file_name("seed").join("source");
        fs::create_dir_all(&input).unwrap();
        fs::write(
            input.join("flake.nix"),
            "{ outputs = _: { marker = \"offline-input\"; }; }\n",
        )
        .unwrap();
        let mut add = std::process::Command::new("nix-store");
        add.arg("--add").arg(&input);
        let stored = run_tool(add, 120).unwrap();
        assert!(Path::new(stored.trim()).is_dir());
        let mut hash = std::process::Command::new("nix");
        hash.args([
            "--extra-experimental-features",
            "nix-command",
            "hash",
            "path",
            "--sri",
        ])
        .arg(&input);
        let nar_hash = run_tool(hash, 120).unwrap().trim().to_owned();
        fs::write(
            f.source.join("flake.nix"),
            r#"{
  inputs.cached.url = "github:fixture/offline";
  outputs = { cached, ... }: { simitConfig.ci.extra_setup = [ cached.marker ]; };
}
"#,
        )
        .unwrap();
        save(&f.source.join("flake.lock"), &json!({
            "version": 7, "root": "root", "nodes": {
                "root": {"inputs": {"cached": "cached"}},
                "cached": {
                    "locked": {"type": "github", "owner": "fixture", "repo": "offline", "rev": "a".repeat(40), "narHash": nar_hash},
                    "original": {"type": "github", "owner": "fixture", "repo": "offline"}
                }
            }
        })).unwrap();
        f.publish(&f.source, "enroll a locked cached remote input");
        f.inject_generator(r#"
import subprocess
value = subprocess.run(['nix', 'eval', '--json', '--offline', '--no-update-lock-file', '--no-write-lock-file', '.#simitConfig'], capture_output=True, text=True)
assert value.returncode == 0, value.stderr
assert 'offline-input' in value.stdout
"#);
        assert_eq!(f.update(true).unwrap().phase, "qualified");
    }

    #[test]
    fn namespace_detached_generator_descendants_cannot_mutate_state_after_generation() {
        let mut f = Fixture::new();
        let target = f.source.with_file_name("descendant-poison");
        let state = f.config.workspaces["projects"].state_directory.clone();
        let held = lock(&state).unwrap();
        f.inject_generator(&format!(
            r#"
import os, time
from pathlib import Path
assert os.getpid() == 1
if os.fork() == 0:
    os.setsid()
    time.sleep(0.2)
    Path({:?}).write_text('poisoned')
    os._exit(0)
"#,
            target.to_str().unwrap()
        ));
        assert_eq!(f.update(true).unwrap().phase, "qualified");
        std::thread::sleep(Duration::from_millis(300));
        assert!(!target.exists());
        verify_lock(&state, &held).unwrap();
        assert!(!state.join("generation-checkpoint.json").exists());
        assert_eq!(f.update(true).unwrap().phase, "unchanged");
    }

    #[test]
    fn namespace_managed_paths_cannot_disappear_without_explicit_retirement() {
        let f = Fixture::new();
        assert_eq!(f.update(true).unwrap().phase, "qualified");
        let checkout = f.root().join("checkout");
        let mut plan = generate(
            &f.config.profiles["fleet"].generator.executable,
            &checkout,
            &f.config.workspaces["projects"].state_directory,
        )
        .unwrap();
        plan.files
            .retain(|file| file.relative_path != Path::new("new-managed.txt"));
        save(&checkout.join("fixture-plan.json"), &plan).unwrap();
        let source = f.publish(&checkout, "omit an adopted managed file");
        assert!(
            f.update(true)
                .unwrap_err()
                .to_string()
                .contains("abandons a managed path")
        );
        assert_eq!(f.remote_head(), source);
        let baseline: Baseline = crate::review::read_json(&f.root().join("baseline.json")).unwrap();
        assert!(baseline.managed["new-managed.txt"].is_some());

        plan.obsolete.push("new-managed.txt".into());
        save(&checkout.join("fixture-plan.json"), &plan).unwrap();
        f.publish(&checkout, "explicitly retire the adopted file");
        let adopted = f.update(true).unwrap();
        assert_eq!(adopted.phase, "qualified");
        assert_eq!(adopted.managed["new-managed.txt"], None);
        assert!(
            git(
                &f.remote,
                &["ls-tree", &adopted.candidate, "--", "new-managed.txt"]
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn managed_identities_follow_git_filemode_without_mutating_the_index() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let paths = || std::iter::once("application.txt".to_owned());
        let committed = hashes(&f.source, paths(), Some("HEAD")).unwrap();
        fs::set_permissions(
            f.source.join("application.txt"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert_ne!(hashes(&f.source, paths(), None).unwrap(), committed);
        assert_eq!(hashes(&f.source, paths(), Some("HEAD")).unwrap(), committed);
        assert!(git(&f.source, &["diff", "--cached"]).unwrap().is_empty());
        git(&f.source, &["config", "core.filemode", "false"]).unwrap();
        assert_eq!(hashes(&f.source, paths(), None).unwrap(), committed);
        git(
            &f.source,
            &["update-index", "--chmod=+x", "--", "application.txt"],
        )
        .unwrap();
        assert_ne!(hashes(&f.source, paths(), None).unwrap(), committed);
    }

    #[test]
    fn namespace_managed_mode_only_changes_require_baseline_reconciliation() {
        use std::os::unix::fs::PermissionsExt;
        for executable in [false, true] {
            let f = Fixture::new();
            let path = ".github/workflows/ci.yaml";
            if executable {
                fs::set_permissions(f.source.join(path), fs::Permissions::from_mode(0o755))
                    .unwrap();
                f.publish(&f.source, "enroll executable managed source");
            }
            assert_eq!(f.update(true).unwrap().phase, "qualified");
            let checkout = f.root().join("checkout");
            fs::set_permissions(
                checkout.join(path),
                fs::Permissions::from_mode(if executable { 0o644 } else { 0o755 }),
            )
            .unwrap();
            let source = f.publish(&checkout, "change only the managed Git mode");
            let baseline = fs::read(f.root().join("baseline.json")).unwrap();
            let receipt = fs::read(f.root().join("receipt.json")).unwrap();
            assert!(
                f.update(true)
                    .unwrap_err()
                    .to_string()
                    .contains("managed outputs changed")
            );
            assert_eq!(f.remote_head(), source);
            assert_eq!(fs::read(f.root().join("baseline.json")).unwrap(), baseline);
            assert_eq!(fs::read(f.root().join("receipt.json")).unwrap(), receipt);
        }
    }

    #[test]
    fn namespace_legacy_blob_only_baselines_require_explicit_reconciliation() {
        let f = Fixture::new();
        assert_eq!(f.update(true).unwrap().phase, "qualified");
        let path = f.root().join("baseline.json");
        let mut baseline: Baseline = crate::review::read_json(&path).unwrap();
        for object in baseline.managed.values_mut().flatten() {
            *object = object.split_whitespace().next_back().unwrap().to_owned();
        }
        save(&path, &baseline).unwrap();
        let legacy = fs::read(&path).unwrap();
        let source = f.remote_head();
        assert!(
            f.update(true)
                .unwrap_err()
                .to_string()
                .contains("explicit reconciliation")
        );
        assert_eq!(fs::read(path).unwrap(), legacy);
        assert_eq!(f.remote_head(), source);
    }

    #[test]
    fn namespace_managed_baselines_use_git_blobs_across_checkout_conversion() {
        let f = Fixture::new();
        fs::write(f.source.join(".gitattributes"), "* text eol=crlf\n").unwrap();
        f.publish(&f.source, "enroll checkout conversion");
        let adopted = f.update(true).unwrap();
        assert_eq!(adopted.phase, "qualified");
        // A separate fresh clone proves checkout conversion without relying on
        // checkout-index to rewrite an already up-to-date working file.
        let converted = f.source.with_file_name("converted");
        git(
            &f.source,
            &[
                "clone",
                "--quiet",
                f.remote.to_str().unwrap(),
                converted.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert!(
            fs::read(converted.join("new-managed.txt"))
                .unwrap()
                .ends_with(b"\r\n")
        );
        assert_eq!(f.update(true).unwrap().phase, "unchanged");
    }

    #[test]
    fn namespace_unrelated_non_utf8_filenames_do_not_block_managed_updates() {
        use std::os::unix::ffi::OsStringExt;
        let f = Fixture::new();
        let name = std::ffi::OsString::from_vec(vec![b'a', 0xff]);
        fs::write(f.source.join(&name), "project-owned bytes\n").unwrap();
        f.publish(&f.source, "retain a non-UTF-8 project filename");
        assert_eq!(f.update(true).unwrap().phase, "qualified");
        assert_eq!(f.update(true).unwrap().phase, "unchanged");
        assert_eq!(
            fs::read(f.root().join("checkout").join(name)).unwrap(),
            b"project-owned bytes\n"
        );
    }

    #[test]
    fn workspace_lock_waits_for_another_profile_before_proceeding() {
        use std::sync::mpsc;
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let held = lock(&state).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let contender = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = lock(&state).and_then(|file| verify_lock(&state, &file));
            done_tx.send(result).unwrap();
        });
        started_rx.recv().unwrap();
        let before_release = done_rx.recv_timeout(Duration::from_millis(100));
        drop(held);
        let after_release = done_rx.recv_timeout(Duration::from_secs(10));
        contender.join().unwrap();
        assert!(matches!(
            before_release,
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        after_release.unwrap().unwrap();
    }

    #[test]
    fn replacing_the_workspace_lock_retains_recovery_and_stops_the_transaction() {
        let f = Fixture::new();
        let state = f.config.workspaces["projects"].state_directory.clone();
        let held = lock(&state).unwrap();
        let path = state.join("lock");
        let checkout = state.join("checkout");
        fs::create_dir(&checkout).unwrap();
        let source = f.remote_head();
        let result = super::super::checkpoint::protect(&state, &checkout, || {
            fs::remove_file(&path)?;
            fs::write(&path, "")?;
            Ok(())
        });
        assert!(result.unwrap_err().to_string().contains("workspace lock"));
        assert!(verify_lock(&state, &held).is_err());
        assert_eq!(f.remote_head(), source);
        assert!(!f.root().join("receipt.json").exists());
        let marker: serde_json::Value =
            crate::review::read_json(&state.join("generation-checkpoint.json")).unwrap();
        let snapshot = Path::new(marker["snapshot"].as_str().unwrap());
        assert!(snapshot.starts_with(&state));
        assert!(snapshot.join("lock").is_file());
    }

    #[test]
    fn clone_identity_removes_only_one_transport_suffix() {
        for scheme in ["https://", "ssh://git@"] {
            assert!(
                remote_identity(
                    &format!("{scheme}fixture.example/team/demo.git.git"),
                    "fixture.example/team/demo.git"
                )
                .is_ok()
            );
            assert!(
                remote_identity(
                    &format!("{scheme}fixture.example/team/demo.git.git"),
                    "fixture.example/team/demo"
                )
                .is_err()
            );
        }
    }

    #[test]
    fn inventory_cannot_enroll_an_unlisted_repository_or_resume_it_directly() {
        let mut f = Fixture::new();
        f.project.coordinate = "fixture.example/team/not-enrolled".into();
        f.write_inventory();
        assert!(f.cli("fleet", true).is_err());
        assert!(
            f.update(true)
                .unwrap_err()
                .to_string()
                .contains("operator-owned coordinate enrollment")
        );
        assert!(!f.root().exists());
        let control: serde_json::Value = crate::review::read_json(&f.control).unwrap();
        assert_eq!(control, json!({}));
    }

    #[test]
    fn interrupted_generation_blocks_before_adapter_dispatch() {
        let f = Fixture::new();
        let state = &f.config.workspaces["projects"].state_directory;
        private_directory(state).unwrap();
        save(
            &state.join("generation-checkpoint.json"),
            &json!({"snapshot":"retained-recovery"}),
        )
        .unwrap();
        assert!(
            f.update(true)
                .unwrap_err()
                .to_string()
                .contains("interrupted generation")
        );
        let control: serde_json::Value = crate::review::read_json(&f.control).unwrap();
        assert_eq!(control, json!({}));
        assert!(!f.root().exists());
    }

    #[test]
    fn namespace_generators_cannot_poison_state_even_when_they_fail() {
        for fail in [false, true] {
            for name in [
                "baseline.json",
                "receipt.json",
                "workflows.json",
                "../../rollout-poison.json",
            ] {
                let mut f = Fixture::new();
                let adopted = f.update(true).unwrap();
                // Other retained repositories can contain source symlinks. Their
                // targets must never be followed while checkpointing/restoring.
                let link = f.config.workspaces["projects"]
                    .state_directory
                    .join("retained-link");
                std::os::unix::fs::symlink(f.source.join("application.txt"), &link).unwrap();
                let target = f.root().join(name);
                let original = fs::read(&target).ok();
                let generator =
                    fs::read_to_string(&f.config.profiles["fleet"].generator.executable).unwrap();
                f.inject_generator(&format!(
                    "from pathlib import Path\nPath({:?}).write_text('poisoned')\n{}",
                    target.to_str().unwrap(),
                    if fail { "raise SystemExit(71)\n" } else { "" }
                ));
                assert!(
                    f.update(true)
                        .unwrap_err()
                        .to_string()
                        .contains("generation sandbox failed")
                );
                assert_eq!(fs::read(&target).ok(), original);
                assert_eq!(
                    fs::read_link(&link).unwrap(),
                    f.source.join("application.txt")
                );
                assert_eq!(
                    fs::read_to_string(f.source.join("application.txt")).unwrap(),
                    "application source\n"
                );
                assert_eq!(f.remote_head(), adopted.candidate);
                assert!(
                    !f.config.workspaces["projects"]
                        .state_directory
                        .join("generation-checkpoint.json")
                        .exists()
                );
                let enrolled = &mut f.config.profiles.get_mut("fleet").unwrap().generator;
                fs::write(&enrolled.executable, &generator).unwrap();
                enrolled.executable_sha256 = Some(digest(generator.as_bytes()));
                assert_eq!(f.update(true).unwrap().phase, "unchanged");
            }
        }
    }

    #[test]
    fn repeated_input_verification_preserves_each_private_snapshot() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let snapshots = temp.path().join("snapshots");
        fs::write(&input, b"first enrolled bytes").unwrap();
        let first_digest = digest(&fs::read(&input).unwrap());
        let first = verified_input(&input, Some(&first_digest), &snapshots, "fixture").unwrap();
        let repeated = verified_input(&input, Some(&first_digest), &snapshots, "fixture").unwrap();
        assert_ne!(first, repeated);
        fs::write(&input, b"newly enrolled bytes").unwrap();
        let before = fs::read_dir(&snapshots).unwrap().count();
        assert!(
            verified_input(&input, Some(&first_digest), &snapshots, "fixture")
                .unwrap_err()
                .to_string()
                .contains("fixture digest differs from enrollment")
        );
        assert_eq!(fs::read_dir(&snapshots).unwrap().count(), before);
        let changed = verified_input(
            &input,
            Some(&digest(&fs::read(&input).unwrap())),
            &snapshots,
            "fixture",
        )
        .unwrap();
        for snapshot in [&first, &repeated] {
            assert_eq!(fs::read(snapshot).unwrap(), b"first enrolled bytes");
        }
        assert_eq!(fs::read(&changed).unwrap(), b"newly enrolled bytes");
        for snapshot in [first, repeated, changed] {
            assert!(snapshot.starts_with(&snapshots));
            assert_eq!(
                fs::metadata(&snapshot).unwrap().permissions().mode() & 0o777,
                0o500
            );
            assert_eq!(
                fs::metadata(snapshot.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn script_execution_requires_an_immutable_interpreter_chain() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("script");
        let snapshots = temp.path().join("snapshots");
        for shebang in ["#!/usr/bin/env python3", "#!/bin/sh", "#!/mutable/python3"] {
            fs::write(&script, format!("{shebang}\nprint('unqualified')\n")).unwrap();
            assert!(
                verified_executable(
                    &script,
                    Some(&digest(&fs::read(&script).unwrap())),
                    &snapshots,
                    "fixture"
                )
                .unwrap_err()
                .to_string()
                .contains("script interpreter must be an immutable Nix-store file"),
                "{shebang}"
            );
        }
        let f = Fixture::new();
        let generator = &f.config.profiles["fleet"].generator;
        let python = fs::read_to_string(&generator.executable).unwrap();
        let python = python
            .lines()
            .next()
            .unwrap()
            .strip_prefix("#!")
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap();
        let shell = super::super::sandbox::tool("sh").unwrap();
        for shebang in [
            format!("#!{} /mutable/loader", shell.display()),
            format!("#!{} -c", shell.display()),
            format!("#!{python} /mutable/loader"),
            format!("#!{python} -I -c"),
        ] {
            fs::write(&script, format!("{shebang}\nprint('unqualified')\n")).unwrap();
            assert!(
                verified_executable(
                    &script,
                    Some(&digest(&fs::read(&script).unwrap())),
                    &snapshots,
                    "fixture"
                )
                .unwrap_err()
                .to_string()
                .contains("script shebang arguments can select unenrolled code"),
                "{shebang}"
            );
        }
        fs::write(&script, format!("#!{} -e\nexit 0\n", shell.display())).unwrap();
        verified_executable(
            &script,
            Some(&digest(&fs::read(&script).unwrap())),
            &snapshots,
            "fixture",
        )
        .expect("an immutable shell invocation must admit the single -e argument");
        assert!(
            verified_executable(
                &generator.executable,
                generator.executable_sha256.as_deref(),
                &snapshots,
                "fixture"
            )
            .is_ok()
        );
    }

    #[test]
    fn native_executable_digests_cannot_enroll_mutable_runtime_closures() {
        let temp = tempfile::tempdir().unwrap();
        let native = temp.path().join("adapter");
        let python = std::process::Command::new("python3")
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .unwrap();
        assert!(python.status.success());
        let source = fs::canonicalize(String::from_utf8(python.stdout).unwrap().trim()).unwrap();
        assert!(source.starts_with("/nix/store"));
        fs::copy(&source, &native).unwrap();
        let hash = digest(&fs::read(&native).unwrap());
        assert_eq!(
            verified_executable(&source, Some(&hash), temp.path(), "fixture").unwrap(),
            source
        );
        let result = verified_executable(&native, Some(&hash), temp.path(), "fixture");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("native executable runtimes must be Nix-store backed")
        );
    }

    #[test]
    fn inventory_cannot_select_a_weaker_policy_for_the_same_forge_host() {
        let mut f = Fixture::new();
        let mut weaker: Forge =
            serde_json::from_value(serde_json::to_value(&f.config.forges["fixture"]).unwrap())
                .unwrap();
        weaker.check_authority = "999".into();
        f.config.forges.insert("weaker".into(), weaker);
        f.project.forge = "weaker".into();
        f.write_inventory();
        let source = f.remote_head();
        assert!(
            f.cli("fleet", true)
                .unwrap_err()
                .to_string()
                .contains("forge hosts must be unique")
        );
        assert!(!f.config.workspaces["projects"].state_directory.exists());
        assert!(!f.root().exists());
        assert_eq!(f.remote_head(), source);
        assert_eq!(fs::read_to_string(&f.control).unwrap().trim(), "{}");
    }

    #[test]
    fn namespace_frozen_unattempted_projects_recheck_the_current_consumer_forge_host() {
        let mut f = Fixture::new();
        let mut generator_forge: Forge =
            serde_json::from_value(serde_json::to_value(&f.config.forges["fixture"]).unwrap())
                .unwrap();
        generator_forge.host = "generator.example".into();
        f.config.forges.insert("generator".into(), generator_forge);
        let generator = &mut f.config.profiles.get_mut("fleet").unwrap().generator;
        generator.forge = "generator".into();
        generator.source_coordinate = "generator.example/team/generator".into();
        let mut second = f.project.clone();
        second.coordinate = "fixture.example/team/next".into();
        second.relative_path = "other/next".into();
        f.write_inventory();
        let inventory_path = f.config.workspaces["projects"]
            .inventory
            .file
            .as_ref()
            .unwrap();
        let mut inventory: Inventory = crate::review::read_json(inventory_path).unwrap();
        inventory.projects.push(second.clone());
        save(inventory_path, &inventory).unwrap();
        f.cli("fleet", true).unwrap();
        let source = f.remote_head();
        let calls = fs::read(&f.control).unwrap();
        let second_root = profile_directory(&f.config.workspaces["projects"], "fleet")
            .join(format!("repo-{}", digest(second.coordinate.as_bytes())));
        assert!(!second_root.exists());
        fs::remove_file(inventory_path).unwrap();
        f.config.forges.get_mut("fixture").unwrap().host = "replacement.example".into();
        f.config.validate().unwrap();
        assert!(f.cli("fleet", true).is_err());
        assert_eq!(
            fs::read(&f.control).unwrap(),
            calls,
            "stale frozen coordinates must not reach the adapter"
        );
        assert!(!second_root.exists());
        assert_eq!(f.remote_head(), source);
    }

    #[test]
    fn namespace_generators_cannot_change_git_identity_signing_remotes_or_hooks() {
        for mutation in [
            "git config --local user.name 'Generator override'",
            "git config --local commit.gpgsign true",
            "git config --local remote.origin.url https://unselected.example/team/demo.git",
            "git config --local core.hooksPath /tmp/generator-hooks",
            "printf '#!/bin/sh\\nexit 71\\n' > .git/hooks/pre-commit; chmod +x .git/hooks/pre-commit",
        ] {
            let mut f = Fixture::new();
            f.inject_generator(&format!(
                "import subprocess\nresult = subprocess.run({mutation:?}, shell=True, capture_output=True, text=True)\nassert result.returncode != 0, 'Git mutation succeeded'\nassert 'Read-only file system' in result.stderr, result.stderr\n"
            ));
            let source = f.remote_head();
            assert_eq!(f.update(false).unwrap().phase, "preview", "{mutation}");
            assert_eq!(f.remote_head(), source);
        }
    }

    #[test]
    fn namespace_unlanded_candidates_rebuild_after_source_movement_and_retain_the_old_receipt() {
        for phase in ["prepared", "candidate", "landing"] {
            let f = Fixture::new();
            f.control(json!({"candidate":"pending"}));
            let mut pending = f.update(true).unwrap();
            pending.phase = phase.into();
            save(&f.root().join("receipt.json"), &pending).unwrap();
            fs::write(
                f.source.join("application.txt"),
                "concurrent application change\n",
            )
            .unwrap();
            let source = f.publish(&f.source, "advance while the candidate waits");
            f.control(json!({}));
            let adopted = f.update(true).unwrap();
            assert_eq!(adopted.phase, "qualified");
            assert_eq!(adopted.source, source);
            assert_ne!(adopted.candidate, pending.candidate);
            let archived: Receipt = crate::review::read_json(&f.root().join(format!(
                "invalidated-{}.json",
                canonical_digest(&pending).unwrap()
            )))
            .unwrap();
            assert_eq!(
                serde_json::to_value(archived).unwrap(),
                serde_json::to_value(pending).unwrap()
            );
            assert_eq!(
                git(
                    &f.remote,
                    &["show", &format!("{}:application.txt", adopted.candidate)]
                )
                .unwrap(),
                "concurrent application change"
            );
        }
    }

    #[test]
    fn namespace_unrelated_target_history_invalidates_unlanded_candidates() {
        for phase in ["prepared", "candidate", "landing"] {
            let f = Fixture::new();
            f.control(json!({"candidate":"pending"}));
            let mut pending = f.update(true).unwrap();
            pending.phase = phase.into();
            save(&f.root().join("receipt.json"), &pending).unwrap();
            git(&f.source, &["checkout", "--orphan", "replacement"]).unwrap();
            fs::write(f.source.join("application.txt"), "unrelated replacement\n").unwrap();
            git(&f.source, &["add", "--all"]).unwrap();
            git(
                &f.source,
                &["commit", "--quiet", "-m", "unrelated replacement"],
            )
            .unwrap();
            git(
                &f.source,
                &[
                    "push",
                    "--force",
                    f.remote.to_str().unwrap(),
                    "HEAD:refs/heads/trunk",
                ],
            )
            .unwrap();
            let source = f.remote_head();
            f.control(json!({}));
            let adopted = f.update(true).unwrap();
            assert_eq!(adopted.phase, "qualified");
            assert_eq!(adopted.source, source);
            assert_ne!(adopted.candidate, pending.candidate);
            let archived: Receipt = crate::review::read_json(&f.root().join(format!(
                "invalidated-{}.json",
                canonical_digest(&pending).unwrap()
            )))
            .unwrap();
            assert_eq!(
                serde_json::to_value(archived).unwrap(),
                serde_json::to_value(pending).unwrap()
            );
            assert_eq!(
                git(
                    &f.remote,
                    &["show", &format!("{}:application.txt", adopted.candidate)]
                )
                .unwrap(),
                "unrelated replacement"
            );
        }
    }

    #[test]
    fn namespace_source_movement_after_landing_preserves_adoption_reconciliation() {
        for phase in ["landing", "adopting"] {
            let f = Fixture::new();
            f.control(json!({"adoption":"pending"}));
            let mut landed = f.update(true).unwrap();
            assert_eq!(landed.phase, "adopting");
            landed.phase = phase.into();
            save(&f.root().join("receipt.json"), &landed).unwrap();
            let checkout = f.root().join("checkout");
            fs::write(checkout.join("application.txt"), "after landing\n").unwrap();
            let source = f.publish(&checkout, "advance beyond the landed candidate");
            f.control(json!({}));
            assert!(
                f.update(true)
                    .unwrap_err()
                    .to_string()
                    .contains("reconciliation is required")
            );
            assert_eq!(f.remote_head(), source);
            let retained: Receipt =
                crate::review::read_json(&f.root().join("receipt.json")).unwrap();
            assert_eq!(retained.candidate, landed.candidate);
            assert_eq!(retained.phase, phase);
        }
    }

    #[test]
    fn namespace_credential_bearing_clone_urls_are_rejected_before_evidence_is_saved() {
        for remote in [
            "https://TOKEN@fixture.example/team/demo.git",
            "https://git:TOKEN@fixture.example/team/demo.git",
            "https://fixture.example:8443/team/demo.git",
            "ssh://git@fixture.example:2222/team/demo.git",
        ] {
            let f = Fixture::new();
            f.control(json!({"remote":remote}));
            assert!(f.update(false).is_err());
            let root = profile_directory(&f.config.workspaces["projects"], "fleet")
                .join(format!("repo-{}", digest(f.project.coordinate.as_bytes())));
            assert!(!fs::read_dir(root).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("evidence-")
            }));
        }
        assert!(
            remote_identity(
                "https://fixture.example/team/demo.git",
                "fixture.example/team/demo"
            )
            .is_ok()
        );
        assert!(
            remote_identity(
                "ssh://git@fixture.example/team/demo.git",
                "fixture.example/team/demo"
            )
            .is_ok()
        );
        for remote in [
            "https://fixture.example:443/team/demo.git",
            "ssh://git@fixture.example:22/team/demo.git",
        ] {
            assert!(
                remote_identity(remote, "fixture.example/team/demo").is_ok(),
                "{remote}"
            );
        }
    }

    #[test]
    fn namespace_adapters_see_only_enrolled_helpers_and_their_own_credential() {
        for interpreter in [false, true] {
            let mut f = Fixture::new();
            let entrypoint = f.config.forges["fixture"].adapter.program.clone();
            let helper = f.control.with_file_name("enrolled-helper.py");
            let undeclared = f.control.with_file_name("undeclared-helper.py");
            let credential = f.control.with_file_name("enrolled-credential");
            let unrelated = f.control.with_file_name("other-forge-credential");
            fs::write(&helper, "marker = 'verified helper'\n").unwrap();
            fs::write(
                &undeclared,
                "raise AssertionError('unverified helper loaded')\n",
            )
            .unwrap();
            fs::write(&credential, "synthetic-enrolled-token").unwrap();
            fs::write(&unrelated, "synthetic-unrelated-token").unwrap();
            let original = fs::read_to_string(&entrypoint).unwrap();
            let (shebang, body) = original.split_once('\n').unwrap();
            let probe = format!(
                r#"
import json, os, pathlib, sys
assert os.getpid() == 1
assert os.environ['HOME'] == '/tmp/home'
assert not any(key in os.environ for key in ('GITHUB_TOKEN', 'UNLISTED_FORGE_TOKEN', 'PYTHONPATH'))
for path in {:?}:
    assert not pathlib.Path(path).exists(), path
assert not pathlib.Path('/run/current-system').exists()
assert not pathlib.Path('/nix/var/nix/daemon-socket/socket').exists()
request = json.loads(pathlib.Path(sys.argv[-1]).read_text())
token = pathlib.Path(request['credentialFile'])
assert token.read_text() == 'synthetic-enrolled-token'
try:
    token.write_text('changed')
except OSError:
    pass
else:
    raise AssertionError('credential mount is writable')
"#,
                [&undeclared, &unrelated, &f.control, &f.remote, &f.source]
            );
            // The oracle replaces the host helper after verification, before
            // this import. Only the pinned bytes may be visible in the child.
            let after_response = format!(
                "\nscope = dict()\nexec(pathlib.Path({helper:?}).read_text(), scope)\nassert scope['marker'] == 'verified helper'\n"
            );
            fs::write(
                &entrypoint,
                format!("{shebang}\n{probe}\n{body}\n{after_response}"),
            )
            .unwrap();
            let forge = f.config.forges.get_mut("fixture").unwrap();
            forge.credential_file = Some(credential.clone());
            forge.adapter_sha256 = Some(digest(&fs::read(&entrypoint).unwrap()));
            forge
                .adapter_inputs
                .insert(helper.clone(), digest(&fs::read(&helper).unwrap()));
            if interpreter {
                forge.adapter.program = super::super::sandbox::tool("python3").unwrap();
                forge.adapter.arguments =
                    vec!["-I".into(), entrypoint.to_string_lossy().into_owned()];
                forge.adapter_sha256 = None;
                forge
                    .adapter_inputs
                    .insert(entrypoint.clone(), digest(&fs::read(&entrypoint).unwrap()));
            }
            f.config.validate().unwrap();
            let source = f.remote_head();
            f.control(json!({"replaceHelper":helper}));
            let root = f.root();
            private_directory(&root).unwrap();
            let forge = &f.config.forges["fixture"];
            let request = request(
                forge,
                &f.project.coordinate,
                "inspect",
                None,
                None,
                vec![],
                vec![],
            );
            assert_eq!(adapter(forge, &request, &root).unwrap().status, "success");
            assert!(
                fs::read_to_string(&helper)
                    .unwrap()
                    .contains("unverified replacement")
            );
            let calls = fs::read(&f.control).unwrap();
            fs::write(
                &helper,
                "raise AssertionError('replaced helper executed')\n",
            )
            .unwrap();
            assert!(
                adapter(forge, &request, &root)
                    .unwrap_err()
                    .to_string()
                    .contains("adapter input digest differs from enrollment")
            );
            assert_eq!(fs::read(&f.control).unwrap(), calls);
            assert_eq!(
                fs::read_to_string(credential).unwrap(),
                "synthetic-enrolled-token"
            );
            assert_eq!(f.remote_head(), source);
            assert!(!f.root().join("receipt.json").exists());
        }
    }

    #[test]
    fn remote_host_case_is_insensitive_but_repository_case_is_exact() {
        for remote in [
            "https://FiXtUrE.example/Team/Demo.git",
            "ssh://git@FiXtUrE.example/Team/Demo.git",
        ] {
            remote_identity(remote, "fixture.example/Team/Demo").unwrap();
            assert!(remote_identity(remote, "fixture.example/team/demo").is_err());
            assert!(remote_identity(remote, "other.example/Team/Demo").is_err());
        }
    }

    #[test]
    fn namespace_replaced_adapter_programs_and_script_arguments_cannot_qualify_a_candidate() {
        for interpreter in [false, true] {
            let mut f = Fixture::new();
            let forge = f.config.forges.get_mut("fixture").unwrap();
            let mut script = forge.adapter.program.clone();
            let mut original = fs::read_to_string(&script).unwrap();
            if interpreter {
                let python = original
                    .lines()
                    .next()
                    .unwrap()
                    .strip_prefix("#!")
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .unwrap();
                let shell = super::super::sandbox::tool("sh").unwrap();
                original = format!(
                    "#!/bin/sh\nexec '{python}' - \"$@\" <<'SIMIT_ADAPTER_ORACLE'\n{original}\nSIMIT_ADAPTER_ORACLE\n"
                );
                script = f.control.with_file_name("adapter-input.sh");
                fs::write(&script, &original).unwrap();
                forge.adapter.program = shell.clone();
                forge.adapter.arguments = vec![script.to_string_lossy().into_owned()];
                forge.adapter_sha256 = Some(digest(&fs::read(shell).unwrap()));
                forge
                    .adapter_inputs
                    .insert(script.clone(), digest(original.as_bytes()));
            }
            f.config.validate().unwrap();
            f.control(json!({"candidate":"pending"}));
            let pending = f.update(true).unwrap();
            assert_eq!(pending.phase, "candidate");
            let marker = f.control.with_file_name("unverified-adapter-ran");
            let injected = if interpreter {
                format!("\ntouch '{}'\n", marker.display())
            } else {
                format!(
                    "\nimport pathlib; pathlib.Path({:?}).write_text('unverified')\n",
                    marker.to_str().unwrap()
                )
            };
            fs::write(&script, original.replacen('\n', &injected, 1)).unwrap();
            f.control(json!({}));
            assert!(f.update(true).is_err());
            assert!(!marker.exists());
            assert_eq!(f.remote_head(), pending.source);
        }
    }

    #[test]
    fn immutable_store_inputs_cannot_resolve_to_mutable_targets() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("store");
        fs::create_dir(&store).unwrap();
        let store = store.canonicalize().unwrap();
        let mutable = temp.path().join("mutable");
        fs::write(&mutable, "unverified bytes").unwrap();
        let escaping = store.join("escaping");
        std::os::unix::fs::symlink(&mutable, &escaping).unwrap();
        assert!(store_file(&escaping, &store).is_err());
        let immutable = store.join("immutable");
        fs::write(&immutable, "store bytes").unwrap();
        let valid = store.join("valid");
        std::os::unix::fs::symlink(&immutable, &valid).unwrap();
        assert_eq!(store_file(&valid, &store).unwrap(), valid);
    }

    #[test]
    fn store_executables_keep_their_enrolled_invocation_path() {
        let directory = tempfile::tempdir().unwrap();
        let shell = super::super::sandbox::tool("sh").unwrap();
        let hash = digest(&fs::read(&shell).unwrap());
        for expected in [None, Some(hash.as_str())] {
            let verified =
                verified_executable(&shell, expected, directory.path(), "fixture").unwrap();
            assert_eq!(verified, shell);
            let output = std::process::Command::new(&verified)
                .args(["-c", "printf '%s' \"$0\""])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                shell.to_str().unwrap()
            );
        }
    }

    #[test]
    fn credential_resolution_rejects_store_aliases_and_snapshots_runtime_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().join("runtime-token");
        fs::write(&runtime, "synthetic-runtime-token").unwrap();
        let alias = directory.path().join("credential-link");
        std::os::unix::fs::symlink(&runtime, &alias).unwrap();
        assert_eq!(
            credential_bytes(&alias).unwrap(),
            b"synthetic-runtime-token"
        );
        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(super::super::sandbox::tool("sh").unwrap(), &alias).unwrap();
        assert!(
            credential_bytes(&alias)
                .unwrap_err()
                .to_string()
                .contains("outside the Nix store")
        );
        assert_eq!(fs::read(&runtime).unwrap(), b"synthetic-runtime-token");
        assert!(credential_bytes(directory.path()).is_err());
    }

    #[test]
    fn inherited_git_routing_cannot_redirect_checkout_staging_or_commits() {
        const CHILD: &str = "SIMIT_UPDATER_GIT_ROUTING_FIXTURE";
        if let Some(root) = std::env::var_os(CHILD) {
            let root = PathBuf::from(root);
            assert_eq!(
                git(&root, &["rev-parse", "--show-toplevel"]).unwrap(),
                root.to_str().unwrap()
            );
            fs::write(root.join("owned.txt"), "new fixture bytes\n").unwrap();
            git(&root, &["add", "--", "owned.txt"]).unwrap();
            git(
                &root,
                &["commit", "--quiet", "-m", "disposable routing fixture"],
            )
            .unwrap();
            assert_eq!(
                git(&root, &["show", "HEAD:owned.txt"]).unwrap(),
                "new fixture bytes"
            );
            return;
        }
        let f = Fixture::new();
        let target = f.source.with_file_name("routing-target");
        fs::create_dir(&target).unwrap();
        git(&target, &["init", "--quiet", "--initial-branch=trunk"]).unwrap();
        let before = git(&f.source, &["rev-parse", "HEAD"]).unwrap();
        let index = fs::read(f.source.join(".git/index")).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "commands::projects_update::engine::tests::inherited_git_routing_cannot_redirect_checkout_staging_or_commits", "--nocapture"])
            .env(CHILD, &target)
            .env("GIT_DIR", f.source.join(".git"))
            .env("GIT_WORK_TREE", &f.source)
            .env("GIT_COMMON_DIR", f.source.join(".git"))
            .env("GIT_INDEX_FILE", f.source.join(".git/index"))
            .env("GIT_OBJECT_DIRECTORY", f.source.join(".git/objects"))
            .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", f.source.join(".git/objects"))
            .env("GIT_NAMESPACE", "unrelated")
            .env("GIT_SHALLOW_FILE", f.source.join(".git/nonexistent-shallow"))
            .output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(git(&f.source, &["rev-parse", "HEAD"]).unwrap(), before);
        assert_eq!(fs::read(f.source.join(".git/index")).unwrap(), index);
        assert!(!f.source.join("owned.txt").exists());
        assert_eq!(
            git(&target, &["show", "HEAD:owned.txt"]).unwrap(),
            "new fixture bytes"
        );
    }

    #[test]
    fn resolved_workspace_overlap_is_rejected_before_state_creation() {
        let mut f = Fixture::new();
        let workspace = f.config.workspaces.get_mut("projects").unwrap();
        fs::create_dir_all(&workspace.root).unwrap();
        let alias = workspace.state_directory.with_file_name("state-alias");
        std::os::unix::fs::symlink(&workspace.root, &alias).unwrap();
        workspace.state_directory = alias.join("nested/updater");
        let source = workspace.root.clone();
        let permissions = fs::metadata(&source).unwrap().permissions();
        f.write_inventory();
        assert!(f.cli("fleet", true).is_err());
        assert!(!source.join("nested").exists());
        assert_eq!(fs::metadata(source).unwrap().permissions(), permissions);
    }

    #[test]
    fn namespace_profiles_keep_independent_candidates_receipts_and_baselines() {
        let mut f = Fixture::new();
        let mut other = serde_json::to_value(&f.config.profiles["fleet"]).unwrap();
        other["generator"]["sourceRevision"] = json!("b".repeat(40));
        f.config
            .profiles
            .insert("other".into(), serde_json::from_value(other).unwrap());
        f.write_inventory();
        f.control(json!({"candidate":"pending"}));
        assert!(f.cli("fleet", true).is_err());
        // A different profile must prepare its own preview rather than resume
        // the first profile's candidate with incompatible policy.
        f.cli("other", false).unwrap();
        assert!(f.cli("other", true).is_err());
        let workspace = &f.config.workspaces["projects"];
        let fleet_root = profile_directory(workspace, "fleet");
        let other_root = profile_directory(workspace, "other");
        let first = status(&fleet_root).unwrap();
        let second = status(&other_root).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_ne!(first[0].candidate, second[0].candidate);
        let first_receipt = serde_json::to_value(&first[0]).unwrap();
        f.control(json!({}));
        f.cli("other", true).unwrap();
        assert_eq!(
            serde_json::to_value(&status(&fleet_root).unwrap()[0]).unwrap(),
            first_receipt
        );
        assert_eq!(status(&other_root).unwrap()[0].phase, "qualified");
        let repo = format!("repo-{}", digest(f.project.coordinate.as_bytes()));
        assert!(!fleet_root.join(&repo).join("baseline.json").exists());
        assert!(other_root.join(&repo).join("baseline.json").is_file());
    }

    #[test]
    fn namespace_unrelated_configuration_edits_resume_the_same_frozen_candidate() {
        let mut f = Fixture::new();
        f.write_inventory();
        f.control(json!({"candidate":"pending"}));
        assert!(f.cli("fleet", true).is_err());
        let pending = status(&profile_directory(
            &f.config.workspaces["projects"],
            "fleet",
        ))
        .unwrap()
        .remove(0);
        let mut cfg = serde_json::to_value(&f.config).unwrap();
        cfg["profiles"]["unused"] = cfg["profiles"]["fleet"].clone();
        cfg["forges"]["unused"] = cfg["forges"]["fixture"].clone();
        cfg["forges"]["unused"]["host"] = json!("unused.example");
        cfg["forges"]["unused"]["apiUrl"] = json!("https://unused.example/api");
        cfg["workspaces"]["unused"] = cfg["workspaces"]["projects"].clone();
        cfg["workspaces"]["unused"]["root"] = json!(f.source.with_file_name("unused-projects"));
        cfg["workspaces"]["unused"]["stateDirectory"] =
            json!(f.source.with_file_name("unused-state"));
        f.config = serde_json::from_value(cfg).unwrap();
        f.config.validate().unwrap();
        // Resumption must use the frozen inventory, even when rediscovery is unavailable.
        fs::remove_file(
            f.config.workspaces["projects"]
                .inventory
                .file
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        f.control(json!({}));
        f.cli("fleet", true).unwrap();
        let adopted = status(&profile_directory(
            &f.config.workspaces["projects"],
            "fleet",
        ))
        .unwrap()
        .remove(0);
        assert_eq!(adopted.phase, "qualified");
        assert_eq!(adopted.candidate, pending.candidate);
        assert_eq!(adopted.config_digest, pending.config_digest);
    }

    #[test]
    fn namespace_referenced_policy_changes_still_block_pending_candidates() {
        for field in ["profile", "workspace", "forge"] {
            let mut f = Fixture::new();
            f.control(json!({"candidate":"pending"}));
            let pending = f.update(true).unwrap();
            match field {
                "profile" => {
                    f.config
                        .profiles
                        .get_mut("fleet")
                        .unwrap()
                        .rollout
                        .batch_size = 2
                }
                "workspace" => {
                    f.config
                        .workspaces
                        .get_mut("projects")
                        .unwrap()
                        .inventory
                        .max_age_seconds = 600
                }
                _ => f.config.forges.get_mut("fixture").unwrap().check_authority = "999".into(),
            }
            assert!(
                f.update(true)
                    .unwrap_err()
                    .to_string()
                    .contains("another policy"),
                "{field}"
            );
            assert_eq!(f.remote_head(), pending.source);
        }
    }

    #[test]
    fn namespace_executes_enrolled_snapshot_when_generator_path_changes_during_qualification() {
        let f = Fixture::new();
        let generator = &f.config.profiles["fleet"].generator.executable;
        f.control(json!({"replaceGenerator":generator}));
        let receipt = f.update(true).unwrap();
        assert_eq!(receipt.phase, "qualified");
        assert_eq!(
            fs::read_to_string(generator).unwrap(),
            "#!/bin/sh\nexit 71\n"
        );
        assert!(
            f.update(true)
                .unwrap_err()
                .to_string()
                .contains("digest differs")
        );
    }

    #[test]
    fn namespace_interrupted_initial_clones_resume_without_manual_state_removal() {
        for complete in [false, true] {
            let f = Fixture::new();
            let root = profile_directory(&f.config.workspaces["projects"], "fleet")
                .join(format!("repo-{}", digest(f.project.coordinate.as_bytes())));
            fs::create_dir_all(&root).unwrap();
            let checkout = root.join("checkout");
            if complete {
                git(
                    &root,
                    &["clone", "--quiet", f.remote.to_str().unwrap(), "checkout"],
                )
                .unwrap();
            } else {
                fs::create_dir(&checkout).unwrap();
                fs::write(checkout.join("partial"), "interrupted clone\n").unwrap();
            }
            let receipt = f.update(true).unwrap();
            assert_eq!(receipt.phase, "qualified");
            assert_eq!(
                fs::read_to_string(f.source.join("application.txt")).unwrap(),
                "application source\n"
            );
        }
    }

    #[test]
    fn namespace_continuation_reaches_later_batches_and_preserves_canary_barriers() {
        for qualification_status in ["failed", "pending"] {
            for (stop_on_failure, canary) in [(false, false), (false, true), (true, false)] {
                let mut f = Fixture::new();
                let profile = f.config.profiles.get_mut("fleet").unwrap();
                profile.rollout.stop_on_failure = stop_on_failure;
                if canary {
                    profile.rollout.canaries.push(f.project.coordinate.clone());
                }
                let mut second = f.project.clone();
                second.coordinate = "fixture.example/team/next".into();
                second.relative_path = "other/next".into();
                let workspace = &f.config.workspaces["projects"];
                save(
                    workspace.inventory.file.as_ref().unwrap(),
                    &Inventory {
                        schema_version: 1,
                        root: workspace.root.clone(),
                        observed_at: chrono::Utc::now(),
                        producer: "fixture".into(),
                        policy_digest: "b".repeat(64),
                        projects: vec![f.project.clone(), second.clone()],
                    },
                )
                .unwrap();
                let config = f.control.with_file_name("updates.json");
                save(&config, &f.config).unwrap();
                f.control(json!({"coordinateStatus":{&f.project.coordinate:{"candidate":qualification_status}}}));
                let run = || {
                    super::super::run(super::super::Args {
                        config: config.clone(),
                        profile: "fleet".into(),
                        repo: vec![],
                        dry_run: false,
                        plan: false,
                        push: true,
                        status: false,
                        wait_seconds: 0,
                        limit: None,
                        json: true,
                    })
                };
                let source = f.remote_head();
                assert!(run().is_err());
                if stop_on_failure || canary {
                    assert!(run().is_err());
                    assert_eq!(f.remote_head(), source);
                    assert!(
                        !status(&profile_directory(workspace, "fleet"))
                            .unwrap()
                            .iter()
                            .any(|r| r.coordinate == second.coordinate)
                    );
                } else {
                    run().unwrap();
                    assert!(
                        status(&profile_directory(workspace, "fleet"))
                            .unwrap()
                            .iter()
                            .any(|r| r.coordinate == second.coordinate && r.phase == "qualified")
                    );
                }
            }
        }
    }

    #[test]
    fn adapters_inherit_only_explicit_runtime_environment() {
        let secrets = [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "CODEBERG_TOKEN",
            "FORGEJO_TOKEN",
            "GITEA_TOKEN",
            "GITLAB_TOKEN",
            "ATTIC_TOKEN",
            "CARGO_REGISTRY_TOKEN",
            "CARGO_REGISTRIES_PRIVATE_TOKEN",
            "NPM_TOKEN",
            "NODE_AUTH_TOKEN",
            "TWINE_PASSWORD",
            "UV_PUBLISH_PASSWORD",
            "UNLISTED_FORGE_TOKEN",
            "NIX_CONFIG",
            "PYTHONPATH",
        ];
        if std::env::var_os("SIMIT_UPDATER_ENV_PROBE").is_none() {
            // Seed a private parent process instead of racing process-global
            // environment mutation with the rest of the test suite.
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child.args([
                "--exact",
                "commands::projects_update::engine::tests::adapters_inherit_only_explicit_runtime_environment",
                "--nocapture",
            ]).env("SIMIT_UPDATER_ENV_PROBE", "1");
            for key in secrets {
                child.env(key, "synthetic-unrelated-credential");
            }
            let output = child.output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success(),
                "{stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                stdout.contains("1 passed"),
                "environment probe did not execute: {stdout}"
            );
            return;
        }
        let command = Command {
            program: super::super::sandbox::tool("python3").unwrap(),
            arguments: vec![
                "-I".into(),
                "-c".into(),
                "import json, os; print(json.dumps(dict(os.environ)))".into(),
            ],
            timeout_seconds: 10,
        };
        let environment: BTreeMap<String, String> =
            serde_json::from_slice(&command_output(&command, &[], None).unwrap()).unwrap();
        for key in secrets.into_iter().chain([
            "SIMIT_UPDATER_ENV_PROBE",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_NOSYSTEM",
        ]) {
            assert!(!environment.contains_key(key), "inherited {key}");
        }
        assert_eq!(environment["PATH"], std::env::var("PATH").unwrap());
        assert_eq!(environment["HOME"], std::env::var("HOME").unwrap());
        assert_eq!(environment["GIT_TERMINAL_PROMPT"], "0");
        // Nix Python wrappers may normalize "1" to "true"; Python treats
        // every nonempty value as disabling user-site packages.
        assert!(!environment["PYTHONNOUSERSITE"].is_empty());
    }

    #[test]
    fn workflow_inventory_does_not_assume_actions_trigger_syntax() {
        for path in [
            ".crow/build.yaml",
            ".gitlab-ci.yml",
            ".github/workflows/ci.yaml",
        ] {
            let plan = CiUpdatePlan {
                schema: "simit.ci-update-plan.v1".into(),
                files: vec![crate::project::GeneratedFile {
                    relative_path: path.into(),
                    content: "backend-specific configuration\n".into(),
                }],
                obsolete: vec![],
            };
            assert_eq!(workflows(&plan), vec![path]);
        }
    }

    #[test]
    fn namespace_inventory_commands_cannot_rewrite_adopted_state_even_when_they_fail() {
        for exit_code in [0, 17] {
            let mut f = Fixture::new();
            f.write_inventory();
            fs::create_dir_all(f.root()).unwrap();
            let baseline = f.root().join("baseline.json");
            fs::write(&baseline, "retained adopted baseline").unwrap();
            let python =
                fs::read_to_string(&f.config.profiles["fleet"].generator.executable).unwrap();
            let python = python
                .lines()
                .next()
                .unwrap()
                .strip_prefix("#!")
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap();
            let inventory = &mut f.config.workspaces.get_mut("projects").unwrap().inventory;
            let mut output: Inventory =
                crate::review::read_json(&inventory.file.take().unwrap()).unwrap();
            output.projects.clear();
            let output = serde_json::to_string(&output).unwrap();
            inventory.command = Some(Command {
                program: python.into(),
                arguments: vec![
                    "-I".into(),
                    "-c".into(),
                    format!(
                        "import errno, os\nfrom pathlib import Path\nassert os.getpid() == 1\ntry:\n Path({:?}).write_text('poisoned baseline')\nexcept OSError as error:\n assert error.errno == errno.EROFS, error\nelse:\n raise AssertionError('inventory rewrote updater state')\nprint({output:?})\nraise SystemExit({exit_code})",
                        baseline.to_str().unwrap()
                    ),
                ],
                timeout_seconds: 30,
            });
            let error = f.cli("fleet", true).unwrap_err();
            assert!(
                format!("{error:#}").contains(if exit_code == 0 {
                    "profile selects no eligible repositories"
                } else {
                    "failed (Some(17))"
                }),
                "{error:#}"
            );
            assert_eq!(
                fs::read_to_string(baseline).unwrap(),
                "retained adopted baseline"
            );
            assert!(!f.root().join("request.json").exists());
            assert_eq!(fs::read_to_string(&f.control).unwrap().trim(), "{}");
        }
    }

    #[test]
    fn operator_status_accepts_a_policy_symlink_without_mutating_state() {
        let f = Fixture::new();
        let config = f.control.with_file_name("updates.json");
        save(&config, &f.config).unwrap();
        let alias = f.control.with_file_name("updates-link.json");
        std::os::unix::fs::symlink("updates.json", &alias).unwrap();
        super::super::run(super::super::Args {
            config: alias,
            profile: "fleet".into(),
            repo: vec![],
            dry_run: false,
            plan: false,
            push: false,
            status: true,
            wait_seconds: 0,
            limit: None,
            json: true,
        })
        .unwrap();
        assert!(!f.config.workspaces["projects"].state_directory.exists());
    }

    #[test]
    fn namespace_custom_adapter_qualifies_non_actions_plan_through_adoption() {
        let f = Fixture::new();
        fs::write(f.source.join("fixture-backend"), ".crow/build.yaml\n").unwrap();
        git(&f.source, &["add", "fixture-backend"]).unwrap();
        git(
            &f.source,
            &["commit", "--quiet", "-m", "enroll fixture backend"],
        )
        .unwrap();
        git(
            &f.source,
            &["push", f.remote.to_str().unwrap(), "HEAD:refs/heads/trunk"],
        )
        .unwrap();
        f.control(json!({"expectedWorkflows":[".crow/build.yaml","new-managed.txt"]}));
        let receipt = f.update(true).unwrap();
        assert_eq!(receipt.phase, "qualified");
        assert_eq!(f.remote_head(), receipt.candidate);
        assert!(
            git(
                &f.remote,
                &["show", &format!("{}:.crow/build.yaml", receipt.candidate)]
            )
            .unwrap()
            .contains("name: CI")
        );
    }

    struct Oracle(std::process::Child);
    impl Drop for Oracle {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    struct Fixture {
        _oracle: Oracle,
        _temporary: tempfile::TempDir,
        config: Config,
        project: Project,
        control: PathBuf,
        remote: PathBuf,
        source: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let source = root.join("source");
            fs::create_dir(&source).unwrap();
            git(&source, &["init", "--quiet", "--initial-branch=trunk"]).unwrap();
            for (key, value) in [
                ("user.name", "Updater fixture"),
                ("user.email", "updater@example.invalid"),
                ("commit.gpgsign", "false"),
            ] {
                git(&source, &["config", "--local", key, value]).unwrap();
            }
            fs::create_dir_all(source.join(".github/workflows")).unwrap();
            fs::write(
                source.join(".github/workflows/ci.yaml"),
                "old managed workflow\n",
            )
            .unwrap();
            fs::write(source.join("application.txt"), "application source\n").unwrap();
            git(&source, &["add", "--all"]).unwrap();
            git(&source, &["commit", "--quiet", "-m", "fixture baseline"]).unwrap();
            let remote = root.join("remote.git");
            git(
                root,
                &[
                    "clone",
                    "--bare",
                    "--quiet",
                    source.to_str().unwrap(),
                    remote.to_str().unwrap(),
                ],
            )
            .unwrap();
            let control = root.join("control.json");
            save(&control, &json!({})).unwrap();
            let python = std::process::Command::new("python3")
                .args(["-c", "import sys; print(sys.executable)"])
                .output()
                .unwrap();
            assert!(python.status.success());
            let python =
                fs::canonicalize(String::from_utf8(python.stdout).unwrap().trim()).unwrap();
            assert!(
                python.starts_with("/nix/store"),
                "updater engine fixtures require the Nix test shell's immutable Python runtime"
            );
            let python = python.to_str().unwrap();
            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
            // Enroll the renderer bytes and an immutable, isolated interpreter.
            let generator = root.join("generator");
            fs::write(
                &generator,
                format!(
                    "#!{python} -I\n{}",
                    fs::read_to_string(fixtures.join("updater-generator.py")).unwrap()
                ),
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&generator, fs::Permissions::from_mode(0o700)).unwrap();
            let adapter = root.join("adapter");
            let ready = root.join("oracle-port");
            let mut oracle = Oracle(
                std::process::Command::new(python)
                    .env_clear()
                    .env("PATH", std::env::var_os("PATH").unwrap())
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .arg("-I")
                    .arg(fixtures.join("updater-adapter.py"))
                    .arg(&control)
                    .arg(&remote)
                    .arg(&ready)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(10);
            let port = loop {
                if let Ok(port) = fs::read_to_string(&ready)
                    && let Ok(port) = port.parse::<u16>()
                {
                    break port;
                }
                assert!(
                    oracle.0.try_wait().unwrap().is_none(),
                    "oracle exited before readiness"
                );
                assert!(Instant::now() < deadline, "oracle did not become ready");
                std::thread::sleep(Duration::from_millis(10));
            };
            fs::write(
                &adapter,
                format!(
                    "#!{python} -I\noracle_url = 'http://127.0.0.1:{port}'\n{}",
                    fs::read_to_string(fixtures.join("updater-adapter-client.py")).unwrap()
                ),
            )
            .unwrap();
            fs::set_permissions(&adapter, fs::Permissions::from_mode(0o700)).unwrap();
            let config: Config = serde_json::from_value(json!({
                "schemaVersion":1,
                "workspaces":{"projects":{"root":root.join("projects"),"stateDirectory":root.join("state"),"inventory":{"file":root.join("inventory.json")}}},
                "forges":{"fixture":{"host":"fixture.example","apiUrl":"https://fixture.example/api","adapter":{"program":adapter},"adapterSha256":digest(&fs::read(&adapter).unwrap()),"checkAuthority":"777"}},
                "profiles":{"fleet":{"workspace":"projects","scopes":["ci"],"select":{"coordinates":["fixture.example/team/demo","fixture.example/team/next"]},"enrollCurrent":true,"rollout":{"maintenanceRefPrefix":"maintenance/"},"generator":{"executable":generator,"executableSha256":digest(&fs::read(&generator).unwrap()),"sourceRevision":"a".repeat(40),"sourceCoordinate":"fixture.example/team/generator","forge":"fixture","requiredChecks":[{"name":"generator","authority":"777"}]}}}
            })).unwrap();
            config.validate().unwrap();
            let project = Project {
                coordinate: "fixture.example/team/demo".into(),
                forge: "fixture".into(),
                relative_path: "arbitrary/layout/demo".into(),
                kind: "owned-repo".into(),
                lifecycle: "active".into(),
                access: "normal".into(),
                checkout_role: "canonical".into(),
                simit_ci: "auto".into(),
                mirrors: vec![],
            };
            Self {
                _oracle: oracle,
                _temporary: temp,
                config,
                project,
                control,
                remote,
                source,
            }
        }
        fn root(&self) -> PathBuf {
            profile_directory(&self.config.workspaces["projects"], "fleet").join(format!(
                "repo-{}",
                digest(self.project.coordinate.as_bytes())
            ))
        }
        fn publish(&self, checkout: &Path, message: &str) -> String {
            git(checkout, &["add", "--all"]).unwrap();
            git(checkout, &["commit", "--quiet", "-m", message]).unwrap();
            git(
                checkout,
                &[
                    "push",
                    self.remote.to_str().unwrap(),
                    "HEAD:refs/heads/trunk",
                ],
            )
            .unwrap();
            self.remote_head()
        }
        fn write_inventory(&self) {
            let workspace = &self.config.workspaces["projects"];
            save(
                workspace.inventory.file.as_ref().unwrap(),
                &Inventory {
                    schema_version: 1,
                    root: workspace.root.clone(),
                    observed_at: chrono::Utc::now(),
                    producer: "fixture".into(),
                    policy_digest: "b".repeat(64),
                    projects: vec![self.project.clone()],
                },
            )
            .unwrap();
        }
        fn cli(&self, profile: &str, push: bool) -> Result<()> {
            let config = self.control.with_file_name("updates.json");
            save(&config, &self.config)?;
            super::super::run(super::super::Args {
                config,
                profile: profile.into(),
                repo: vec![],
                dry_run: false,
                plan: false,
                push,
                status: false,
                wait_seconds: 0,
                limit: None,
                json: true,
            })
        }
        fn update(&self, push: bool) -> Result<Receipt> {
            update(
                &self.config,
                "fleet",
                &self.config.profiles["fleet"],
                &self.config.workspaces["projects"],
                &self.project,
                push,
                0,
            )
        }
        fn remote_head(&self) -> String {
            git(&self.remote, &["rev-parse", "refs/heads/trunk"]).unwrap()
        }
        fn control(&self, value: serde_json::Value) {
            save(&self.control, &value).unwrap();
        }
        fn land_pull_requests(mut self) -> Self {
            self.config.profiles.get_mut("fleet").unwrap().landing = "pull-request".into();
            self
        }
        fn pulls(&self) -> serde_json::Value {
            crate::review::read_json(&self.control.with_file_name("pulls.json")).unwrap()
        }
        fn maintenance_refs(&self) -> String {
            git(&self.remote, &["for-each-ref", "refs/heads/maintenance/"]).unwrap()
        }
        fn inject_generator(&mut self, code: &str) {
            let generator = &mut self.config.profiles.get_mut("fleet").unwrap().generator;
            let original = fs::read_to_string(&generator.executable).unwrap();
            fs::write(
                &generator.executable,
                original.replacen('\n', &format!("\n{code}\n"), 1),
            )
            .unwrap();
            generator.executable_sha256 = Some(digest(&fs::read(&generator.executable).unwrap()));
        }
    }

    #[test]
    fn namespace_preview_pending_candidate_and_interrupted_adoption_resume_same_commit() {
        let f = Fixture::new();
        let source = f.remote_head();
        let preview = f.update(false).unwrap();
        assert_eq!(preview.phase, "preview");
        assert!(preview.diff.contains("new-managed.txt"));
        assert_eq!(f.remote_head(), source);
        f.control(json!({"candidate":"pending"}));
        let pending = f.update(true).unwrap();
        assert_eq!(pending.phase, "candidate");
        assert_eq!(f.remote_head(), source);
        let status_before = status(&profile_directory(
            &f.config.workspaces["projects"],
            "fleet",
        ))
        .unwrap();
        assert_eq!(status_before[0].candidate, pending.candidate);
        f.control(json!({"adoption":"pending"}));
        let adopting = f.update(true).unwrap();
        assert_eq!(adopting.phase, "adopting");
        assert_eq!(adopting.candidate, pending.candidate);
        assert_eq!(f.remote_head(), pending.candidate);
        f.control(json!({}));
        let adopted = f.update(true).unwrap();
        assert_eq!(adopted.phase, "qualified");
        assert_eq!(adopted.candidate, pending.candidate);
        assert_eq!(
            git(
                &f.remote,
                &["show", &format!("{}:application.txt", adopted.candidate)]
            )
            .unwrap(),
            "application source"
        );
        assert_eq!(f.update(true).unwrap().phase, "unchanged");
        assert_eq!(
            fs::read_to_string(f.source.join(".github/workflows/ci.yaml")).unwrap(),
            "old managed workflow\n"
        );
    }

    #[test]
    fn namespace_concurrent_default_branch_writer_is_not_overwritten_by_landing() {
        let f = Fixture::new();
        let source = f.remote_head();
        fs::write(
            f.source.join("application.txt"),
            "concurrent application change\n",
        )
        .unwrap();
        git(&f.source, &["add", "--all"]).unwrap();
        git(&f.source, &["commit", "--quiet", "-m", "concurrent writer"]).unwrap();
        let race = git(&f.source, &["rev-parse", "HEAD"]).unwrap();
        git(
            &f.source,
            &[
                "push",
                f.remote.to_str().unwrap(),
                "HEAD:refs/heads/race-object",
            ],
        )
        .unwrap();
        assert_eq!(f.remote_head(), source);
        f.control(json!({"raceAtInspection":2,"raceRevision":race}));
        assert!(f.update(true).is_err());
        assert_eq!(f.remote_head(), race);
        assert_eq!(
            status(&profile_directory(
                &f.config.workspaces["projects"],
                "fleet"
            ))
            .unwrap()[0]
                .phase,
            "landing"
        );
    }

    #[test]
    fn namespace_changed_policy_or_source_invalidates_retained_candidate() {
        let f = Fixture::new();
        f.control(json!({"candidate":"pending"}));
        let pending = f.update(true).unwrap();
        f.control(json!({"policy":"c".repeat(64)}));
        assert!(
            f.update(true)
                .unwrap_err()
                .to_string()
                .contains("protection changed")
        );
        f.control(json!({"defaultBranch":"main"}));
        assert!(f.update(true).is_err());
        assert_eq!(f.remote_head(), pending.source);
    }

    #[test]
    fn namespace_ignored_managed_outputs_are_committed_and_later_edits_block_updates() {
        let f = Fixture::new();
        fs::write(f.source.join(".gitignore"), "new-managed.txt\n").unwrap();
        git(&f.source, &["add", ".gitignore"]).unwrap();
        git(
            &f.source,
            &["commit", "--quiet", "-m", "ignore generated output"],
        )
        .unwrap();
        git(
            &f.source,
            &["push", f.remote.to_str().unwrap(), "HEAD:refs/heads/trunk"],
        )
        .unwrap();
        let adopted = f.update(true).unwrap();
        assert_eq!(adopted.phase, "qualified");
        assert_eq!(
            git(
                &f.remote,
                &["show", &format!("{}:new-managed.txt", adopted.candidate)]
            )
            .unwrap(),
            "new output"
        );
        let checkout = profile_directory(&f.config.workspaces["projects"], "fleet")
            .join(format!("repo-{}", digest(f.project.coordinate.as_bytes())))
            .join("checkout");
        fs::write(
            checkout.join(".github/workflows/ci.yaml"),
            "project-owned workflow edit\n",
        )
        .unwrap();
        git(&checkout, &["add", "--all"]).unwrap();
        git(
            &checkout,
            &["commit", "--quiet", "-m", "local workflow edit"],
        )
        .unwrap();
        git(&checkout, &["push", "origin", "HEAD:refs/heads/trunk"]).unwrap();
        let moved = f.remote_head();
        assert!(
            f.update(true)
                .unwrap_err()
                .to_string()
                .contains("managed outputs changed")
        );
        assert_eq!(f.remote_head(), moved);
    }

    #[test]
    fn namespace_failed_or_unsupported_qualification_cannot_land() {
        let f = Fixture::new();
        let source = f.remote_head();
        f.control(json!({"candidate":"pending"}));
        let pending = f.update(true).unwrap();
        assert_eq!(pending.phase, "candidate");
        for status in ["failed", "unsupported", "skipped", "cancelled"] {
            f.control(json!({"candidate":status}));
            let error = f.update(true).unwrap_err().to_string();
            assert!(
                error.contains(if matches!(status, "failed" | "unsupported") {
                    "adapter rejected qualify"
                } else {
                    "invalid adapter status"
                }),
                "{status}: {error}"
            );
            let request: AdapterRequest =
                crate::review::read_json(&f.root().join("request.json")).unwrap();
            assert_eq!(request.operation, "qualify");
            assert_eq!(
                request.revision.as_deref(),
                Some(pending.candidate.as_str())
            );
            assert_eq!(request.branch.as_deref(), Some(pending.branch.as_str()));
            assert_eq!(f.remote_head(), source);
        }
        f.control(json!({}));
        assert_eq!(f.update(true).unwrap().phase, "qualified");
    }

    #[test]
    fn namespace_admission_binds_merged_required_checks_to_both_branches() {
        let mut f = Fixture::new();
        let profile = f.config.profiles.get_mut("fleet").unwrap();
        profile.required_checks.push(Requirement {
            name: "profile-check".into(),
            authority: "777".into(),
        });
        profile.project_overrides.insert(
            f.project.coordinate.clone(),
            Override {
                enable: true,
                reason: None,
                target_branch: None,
                required_checks: vec![Requirement {
                    name: "override-check".into(),
                    authority: "777".into(),
                }],
                landing: None,
            },
        );
        f.control(json!({"expectedAdmissionChecks":[
            {"name":"test","authority":"777"},
            {"name":"profile-check","authority":"777"},
            {"name":"override-check","authority":"777"}
        ]}));
        assert_eq!(f.update(true).unwrap().phase, "qualified");
    }

    #[test]
    fn namespace_failed_admission_cannot_publish_a_receipt_or_maintenance_ref() {
        let f = Fixture::new();
        let source = f.remote_head();
        assert_eq!(f.update(false).unwrap().phase, "preview");
        let expected = [".github/workflows/ci.yaml", "new-managed.txt"]
            .into_iter()
            .map(|path| {
                (
                    path,
                    fs::read_to_string(f.root().join("checkout").join(path)).unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for status in ["unsupported", "failed", "pending"] {
            f.control(json!({"admission": status, "expectedAdmissionBlobs": expected}));
            let error = f.update(true).unwrap_err().to_string();
            assert!(
                error.contains(if status == "pending" {
                    "admission is incomplete"
                } else {
                    "adapter rejected admit"
                }),
                "{status}: {error}"
            );
            assert!(!f.root().join("receipt.json").exists());
            assert!(
                git(&f.remote, &["for-each-ref", "refs/heads/maintenance/"])
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(f.remote_head(), source);
        }
        f.control(json!({"expectedAdmissionBlobs": expected}));
        assert_eq!(f.update(true).unwrap().phase, "qualified");
    }

    #[test]
    fn namespace_resumed_candidates_repeat_admission_before_any_push() {
        for phase in ["prepared", "candidate", "landing"] {
            let f = Fixture::new();
            f.control(json!({"candidate":"pending"}));
            let mut retained = f.update(true).unwrap();
            retained.phase = phase.into();
            save(&f.root().join("receipt.json"), &retained).unwrap();
            let reference = format!("refs/heads/{}", retained.branch);
            git(&f.remote, &["update-ref", "-d", &reference]).unwrap();
            let before = fs::read(f.root().join("receipt.json")).unwrap();
            f.control(json!({"admission":"failed"}));
            assert!(
                f.update(true)
                    .unwrap_err()
                    .to_string()
                    .contains("adapter rejected admit")
            );
            assert_eq!(f.remote_head(), retained.source);
            assert!(
                git(&f.remote, &["for-each-ref", &reference])
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(fs::read(f.root().join("receipt.json")).unwrap(), before);
            f.control(json!({}));
            let adopted = f.update(true).unwrap();
            assert_eq!(adopted.phase, "qualified");
            assert_eq!(adopted.candidate, retained.candidate);
        }
    }

    #[test]
    fn namespace_pull_requests_land_every_merge_style_through_adoption() {
        for style in ["merge", "squash", "rebase", "ff"] {
            let f = Fixture::new().land_pull_requests();
            let source = f.remote_head();
            f.control(json!({"mergeStyle": style}));
            let receipt = f.update(true).unwrap();
            assert_eq!(receipt.phase, "qualified", "{style}");
            assert_eq!(receipt.schema_version, 2);
            assert_eq!(receipt.change.as_deref(), Some("1"));
            let landed = receipt.landed.clone().unwrap();
            assert_eq!(f.remote_head(), landed);
            assert_eq!(landed == receipt.candidate, style == "ff", "{style}");
            let checkout = f.root().join("checkout");
            let tree = |commit: &str| git(&checkout, &["rev-parse", &format!("{commit}^{{tree}}")]);
            assert_eq!(tree(&landed).unwrap(), tree(&receipt.candidate).unwrap());
            assert_eq!(
                git(&checkout, &["rev-parse", &format!("{landed}^1")]).unwrap(),
                source
            );
            assert_eq!(f.pulls()[0]["state"], "merged");
            assert!(f.maintenance_refs().is_empty(), "{style}");
            assert_eq!(f.update(true).unwrap().phase, "unchanged");
        }
    }

    #[test]
    fn namespace_pull_requests_park_until_mergeable_and_resume_the_same_change() {
        let f = Fixture::new().land_pull_requests();
        let source = f.remote_head();
        f.control(json!({"merge": "pending"}));
        let parked = f.update(true).unwrap();
        assert_eq!(parked.phase, "proposed");
        assert_eq!(parked.change.as_deref(), Some("1"));
        assert_eq!(f.remote_head(), source);
        assert_eq!(f.update(true).unwrap().phase, "proposed");
        assert_eq!(f.pulls().as_array().unwrap().len(), 1);
        f.control(json!({}));
        let adopted = f.update(true).unwrap();
        assert_eq!(adopted.phase, "qualified");
        assert_eq!(adopted.candidate, parked.candidate);
        assert_eq!(adopted.change, parked.change);
        assert_eq!(Some(f.remote_head()), adopted.landed);
    }

    #[test]
    fn namespace_stale_or_closed_pull_requests_retire_their_ref_and_regenerate() {
        for change in ["target", "policy", "closed"] {
            let mut f = Fixture::new().land_pull_requests();
            f.control(json!({"merge": "pending"}));
            let parked = f.update(true).unwrap();
            assert_eq!(parked.phase, "proposed");
            f.control(json!({}));
            match change {
                "target" => {
                    fs::write(f.source.join("application.txt"), "moved target\n").unwrap();
                    f.publish(&f.source, "advance the pull request target");
                }
                "policy" => {
                    f.config
                        .profiles
                        .get_mut("fleet")
                        .unwrap()
                        .rollout
                        .batch_size = 2
                }
                _ => f.control(json!({"humanClose": true})),
            }
            let adopted = f.update(true).unwrap();
            assert_eq!(adopted.phase, "qualified", "{change}");
            assert_eq!(adopted.change.as_deref(), Some("2"));
            let pulls = f.pulls();
            assert_eq!(pulls[0]["state"], "closed", "{change}");
            assert_eq!(pulls[1]["state"], "merged", "{change}");
            assert!(f.maintenance_refs().is_empty(), "{change}");
            assert!(
                f.root()
                    .join(format!(
                        "invalidated-{}.json",
                        canonical_digest(&parked).unwrap()
                    ))
                    .is_file()
            );
        }
    }

    #[test]
    fn namespace_human_merged_pull_requests_are_verified_then_adopted() {
        let f = Fixture::new().land_pull_requests();
        f.control(json!({"merge": "pending"}));
        let parked = f.update(true).unwrap();
        f.control(json!({"merge": "pending", "humanMerge": "squash"}));
        let adopted = f.update(true).unwrap();
        assert_eq!(adopted.phase, "qualified");
        assert_eq!(adopted.candidate, parked.candidate);
        let landed = adopted.landed.unwrap();
        assert_ne!(landed, parked.candidate);
        assert_eq!(f.remote_head(), landed);
        assert!(f.maintenance_refs().is_empty());
    }

    #[test]
    fn namespace_landed_tree_or_parent_mismatch_fails_closed() {
        for style in ["wrong-tree", "wrong-parent"] {
            let f = Fixture::new().land_pull_requests();
            f.control(json!({"mergeStyle": style}));
            for _ in 0..2 {
                assert!(
                    f.update(true)
                        .unwrap_err()
                        .to_string()
                        .contains("differs from the qualified candidate tree"),
                    "{style}"
                );
            }
            let retained: Receipt =
                crate::review::read_json(&f.root().join("receipt.json")).unwrap();
            assert_eq!(retained.phase, "adopting");
            assert_eq!(retained.landed, Some(f.remote_head()));
            assert!(!f.root().join("baseline.json").exists());
        }
    }

    #[test]
    fn namespace_parked_pull_requests_do_not_block_later_repositories() {
        for canary in [false, true] {
            let mut f = Fixture::new().land_pull_requests();
            if canary {
                let coordinate = f.project.coordinate.clone();
                let profile = f.config.profiles.get_mut("fleet").unwrap();
                profile.rollout.canaries.push(coordinate);
            }
            let mut second = f.project.clone();
            second.coordinate = "fixture.example/team/next".into();
            second.relative_path = "other/next".into();
            f.write_inventory();
            let inventory_path = f.config.workspaces["projects"]
                .inventory
                .file
                .clone()
                .unwrap();
            let mut inventory: Inventory = crate::review::read_json(&inventory_path).unwrap();
            inventory.projects.push(second.clone());
            save(&inventory_path, &inventory).unwrap();
            f.control(json!({"coordinateStatus":{&f.project.coordinate:{"merge":"pending"}}}));
            let profile_root = profile_directory(&f.config.workspaces["projects"], "fleet");
            let phase = |coordinate: &str| {
                status(&profile_root)
                    .unwrap()
                    .into_iter()
                    .find(|receipt| receipt.coordinate == coordinate)
                    .map(|receipt| receipt.phase)
            };
            for _ in 0..2 {
                assert_eq!(f.cli("fleet", true).is_err(), canary);
            }
            assert_eq!(phase(&f.project.coordinate).as_deref(), Some("proposed"));
            assert_eq!(
                phase(&second.coordinate).as_deref(),
                (!canary).then_some("qualified")
            );
        }
    }

    #[test]
    fn namespace_v1_receipts_resume_as_direct_landing_and_retire_their_ref() {
        let f = Fixture::new();
        f.control(json!({"candidate":"pending"}));
        let pending = f.update(true).unwrap();
        assert_eq!(pending.phase, "candidate");
        let path = f.root().join("receipt.json");
        let mut legacy: serde_json::Value = crate::review::read_json(&path).unwrap();
        for key in ["schemaVersion", "landing", "projectPolicyDigest"] {
            legacy.as_object_mut().unwrap().remove(key).unwrap();
        }
        save(&path, &legacy).unwrap();
        assert!(!f.maintenance_refs().is_empty());
        f.control(json!({}));
        let adopted = f.update(true).unwrap();
        assert_eq!(adopted.phase, "qualified");
        assert_eq!(adopted.schema_version, 0);
        assert_eq!(adopted.candidate, pending.candidate);
        assert_eq!(adopted.landed, None);
        assert_eq!(f.remote_head(), pending.candidate);
        assert!(f.maintenance_refs().is_empty());
    }

    #[test]
    fn namespace_other_repository_policy_edits_keep_pending_candidates() {
        for own in [false, true] {
            let mut f = Fixture::new();
            f.control(json!({"candidate":"pending"}));
            let pending = f.update(true).unwrap();
            let coordinate = if own {
                f.project.coordinate.clone()
            } else {
                "fixture.example/team/other".to_owned()
            };
            let profile = f.config.profiles.get_mut("fleet").unwrap();
            profile.select.coordinates.push(coordinate.clone());
            profile.project_overrides.insert(
                coordinate,
                serde_json::from_value(json!({"targetBranch":"trunk"})).unwrap(),
            );
            f.control(json!({}));
            let resumed = f.update(true);
            if own {
                assert!(resumed.unwrap_err().to_string().contains("another policy"));
            } else {
                let adopted = resumed.unwrap();
                assert_eq!(adopted.phase, "qualified");
                assert_eq!(adopted.candidate, pending.candidate);
            }
        }
    }

    #[test]
    fn namespace_previews_never_retire_parked_pull_requests_or_refs() {
        let f = Fixture::new().land_pull_requests();
        f.control(json!({"merge": "pending"}));
        assert_eq!(f.update(true).unwrap().phase, "proposed");
        fs::write(f.source.join("application.txt"), "moved target\n").unwrap();
        f.publish(&f.source, "advance the parked target");
        let receipt = fs::read(f.root().join("receipt.json")).unwrap();
        let refs = f.maintenance_refs();
        assert!(!refs.is_empty());
        assert_eq!(f.update(false).unwrap().phase, "proposed");
        assert_eq!(fs::read(f.root().join("receipt.json")).unwrap(), receipt);
        assert_eq!(f.maintenance_refs(), refs);
        assert_eq!(f.pulls()[0]["state"], "open");
        // A preview that invalidates a direct candidate keeps its published ref.
        let f = Fixture::new();
        f.control(json!({"candidate": "pending"}));
        f.update(true).unwrap();
        fs::write(f.source.join("application.txt"), "moved target\n").unwrap();
        f.publish(&f.source, "advance the pending target");
        let refs = f.maintenance_refs();
        assert!(!refs.is_empty());
        assert_eq!(f.update(false).unwrap().phase, "preview");
        assert_eq!(f.maintenance_refs(), refs);
    }
}

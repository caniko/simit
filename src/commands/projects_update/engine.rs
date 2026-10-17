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
        .args(argv)
        .stdin(std::process::Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(path) = cwd {
        child.current_dir(path);
    }
    for key in process::UNTRUSTED_TOOL_ENV {
        child.env_remove(key);
    }
    for key in [
        "PYTHONPATH",
        "PYTHONHOME",
        "BASH_ENV",
        "ENV",
        "LD_PRELOAD",
        "LD_AUDIT",
        "LD_LIBRARY_PATH",
        "DYLD_INSERT_LIBRARIES",
        "DYLD_LIBRARY_PATH",
        "NODE_OPTIONS",
        "NODE_PATH",
        "RUBYOPT",
        "RUBYLIB",
        "PERL5OPT",
        "PERL5LIB",
    ] {
        child.env_remove(key);
    }
    child.env("PYTHONNOUSERSITE", "1");
    Ok(child)
}

fn run_tool(command: std::process::Command, timeout_seconds: u64) -> Result<String> {
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
            let (_, path) = log.keep()?;
            Err(error).with_context(|| format!("tool stderr retained at {}", path.display()))
        }
    }
}

fn git(root: &Path, argv: &[&str]) -> Result<String> {
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
    // These calls target disposable repositories in unit tests. Registry tests
    // mutate process-global Git configuration concurrently; pin only fixture Git.
    #[cfg(test)]
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    run_tool(command, 300).map(|v| v.trim_end().to_owned())
}

pub fn lock(root: &Path) -> Result<File> {
    private_directory(root)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("lock"))?;
    fs2::FileExt::try_lock_exclusive(&file).context("another updater owns this workspace")?;
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
    canonical_digest(&(profile, workspace, generator, consumer))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
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
    }
}

fn adapter(forge: &Forge, request: &AdapterRequest, root: &Path) -> Result<AdapterResponse> {
    request.validate()?;
    let snapshots = tempfile::Builder::new()
        .prefix("adapter-")
        .tempdir_in(root)?;
    let mut command = forge.adapter.clone();
    command.program = verified_executable(
        &command.program,
        forge.adapter_sha256.as_deref(),
        snapshots.path(),
        "adapter executable",
    )?;
    for argument in &mut command.arguments {
        let (prefix, value) = if argument.starts_with('-') {
            argument
                .split_once('=')
                .map_or((None, argument.as_str()), |(flag, value)| {
                    (Some(flag), value)
                })
        } else {
            (None, argument.as_str())
        };
        let path = Path::new(value);
        if path.is_absolute() {
            let pinned = verified_input(
                path,
                forge.adapter_inputs.get(path).map(String::as_str),
                snapshots.path(),
                "adapter input",
            )?;
            *argument = prefix.map_or_else(
                || pinned.to_string_lossy().into_owned(),
                |flag| format!("{flag}={}", pinned.display()),
            );
        }
    }
    let path = root.join("request.json");
    save(&path, request)?;
    let response: AdapterResponse = serde_json::from_slice(&command_output(
        &command,
        &["--request".into(), path.to_string_lossy().into_owned()],
        Some(snapshots.path()),
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
            && (url.scheme() != "https" || url.username().is_empty())
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "invalid repository transport"
    );
    ensure!(
        format!(
            "{}/{}",
            url.host_str().context("missing repository host")?,
            url.path()
                .trim_start_matches('/')
                .strip_suffix(".git")
                .unwrap_or_else(|| url.path().trim_start_matches('/'))
        ) == expected,
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
    Ok(resolved)
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
    let directory = snapshots.join(digest(path.as_os_str().as_encoded_bytes()));
    fs::create_dir_all(&directory)?;
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
            return Ok(executable);
        };
        let line = script
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default();
        let interpreter = std::str::from_utf8(line)?
            .split_whitespace()
            .next()
            .context("missing script interpreter")?;
        current = store_file(Path::new(interpreter), Path::new("/nix/store"))
            .context("script interpreter must be an immutable Nix-store file; env-selected and mutable runtimes are not enrolled")?;
    }
}

fn inspect(
    forge: &Forge,
    project: &Project,
    target: Option<&str>,
    root: &Path,
) -> Result<AdapterResponse> {
    let response = adapter(
        forge,
        &request(
            forge,
            &project.coordinate,
            "inspect",
            None,
            target,
            vec![],
            vec![],
        ),
        root,
    )?;
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
    let command = Command {
        program: executable.to_owned(),
        arguments: vec![
            "init".into(),
            "ci".into(),
            "--primary-only".into(),
            "--plan-json".into(),
        ],
        timeout_seconds: 900,
    };
    let output = super::checkpoint::protect(state_directory, checkout, || {
        command_output(&command, &[], Some(checkout))
    })?;
    let plan: CiUpdatePlan = serde_json::from_slice(&output)?;
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
            // Git blob identities survive clean/smudge and newline conversion.
            let hash = if let Some(tree) = tree {
                let object = git(
                    root,
                    &[
                        "--literal-pathspecs",
                        "ls-tree",
                        "--object-only",
                        tree,
                        "--",
                        &p,
                    ],
                )?;
                if object.is_empty() {
                    None
                } else {
                    Some(object)
                }
            } else if path.exists() {
                ensure!(path.is_file(), "managed output is not a file");
                Some(git(
                    root,
                    &["hash-object", &format!("--path={p}"), "--", &p],
                )?)
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
    let config_digest = policy_digest(config, profile, workspace, Some(&project.forge))?;
    let over = profile.project_overrides.get(&project.coordinate);
    let target = over.and_then(|o| o.target_branch.as_deref());
    let observed = inspect(forge, project, target, &root)?;
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
    }
    let checkout = root.join("checkout");
    if let Some(retained) = previous.as_ref().filter(|receipt| {
        !matches!(receipt.phase.as_str(), "qualified" | "unchanged")
            && observed.revision != receipt.source
            && observed.revision != receipt.candidate
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
            git(
                &checkout,
                &["merge-base", &retained.candidate, &observed.revision]
            )? != retained.candidate,
            "remote advanced beyond the landed candidate; adoption reconciliation is required"
        );
        // Archive before replacing the checkout, so interruption cannot leave an
        // unfinished receipt referring to a new clone. Adopted baselines survive.
        fs::rename(
            &receipt_path,
            root.join(format!("invalidated-{}.json", canonical_digest(retained)?)),
        )?;
        File::open(&root)?.sync_all()?;
        previous = None;
    }
    if let Some(previous) = previous
        .as_ref()
        .filter(|r| !matches!(r.phase.as_str(), "qualified" | "unchanged"))
    {
        ensure!(
            previous.config_digest == config_digest,
            "an unfinished rollout uses another policy; resume it with its original configuration"
        );
        ensure!(
            previous.repository_id == observed.repository_id
                && previous.policy_digest == observed.policy_digest
                && previous.default_branch == observed.default_branch,
            "repository identity/default branch/protection changed during rollout"
        );
        ensure!(
            observed.revision == previous.source || observed.revision == previous.candidate,
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
        if diff.is_empty() {
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
                let fresh = inspect(forge, project, target, &root)?;
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
                        managed: managed.clone(),
                    },
                )?;
            }
            let receipt = Receipt {
                coordinate: project.coordinate.clone(),
                config_digest,
                repository_id: observed.repository_id,
                source: observed.revision.clone(),
                candidate: observed.revision,
                branch: String::new(),
                default_branch: observed.default_branch,
                policy_digest: observed.policy_digest,
                phase: "unchanged".into(),
                managed,
                diff,
            };
            if push {
                save(&receipt_path, &receipt)?;
            }
            return Ok(receipt);
        }
        if !push {
            return Ok(Receipt {
                coordinate: project.coordinate.clone(),
                config_digest,
                repository_id: observed.repository_id,
                source: observed.revision,
                candidate: String::new(),
                branch: String::new(),
                default_branch: observed.default_branch,
                policy_digest: observed.policy_digest,
                phase: "preview".into(),
                managed,
                diff,
            });
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
                .all(|p| managed.contains_key(p)),
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
            coordinate: project.coordinate.clone(),
            config_digest,
            repository_id: observed.repository_id.clone(),
            source: observed.revision.clone(),
            candidate,
            branch,
            default_branch: observed.default_branch.clone(),
            policy_digest: observed.policy_digest.clone(),
            phase: "prepared".into(),
            managed,
            diff,
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
    save(&receipt_path, &receipt)?;
    if observed.revision == receipt.source {
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
        let workflows: Vec<String> = crate::review::read_json(&root.join("workflows.json"))?;
        let candidate = wait(
            forge,
            &request(
                forge,
                &project.coordinate,
                "qualify",
                Some(&receipt.candidate),
                Some(&receipt.branch),
                workflows,
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
        let fresh = inspect(forge, project, target, &root)?;
        ensure!(
            fresh.repository_id == receipt.repository_id
                && fresh.revision == receipt.source
                && fresh.default_branch == receipt.default_branch
                && fresh.policy_digest == receipt.policy_digest,
            "remote identity, source or policy moved after qualification"
        );
        receipt.phase = "landing".into();
        save(&receipt_path, &receipt)?;
        // The candidate is a direct child of source; the explicit lease is a compare-and-swap.
        let target_ref = format!("refs/heads/{}", target.unwrap_or(&receipt.default_branch));
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
    receipt.phase = "adopting".into();
    save(&receipt_path, &receipt)?;
    let fresh = inspect(forge, project, target, &root)?;
    ensure!(
        fresh.repository_id == receipt.repository_id
            && fresh.revision == receipt.candidate
            && fresh.default_branch == receipt.default_branch
            && fresh.policy_digest == receipt.policy_digest,
        "landed source or policy changed before adoption"
    );
    let workflows = crate::review::read_json(&root.join("workflows.json"))?;
    let adopted = wait(
        forge,
        &request(
            forge,
            &project.coordinate,
            "qualify",
            Some(&receipt.candidate),
            Some(target.unwrap_or(&receipt.default_branch)),
            workflows,
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
        let final_state = inspect(forge, project, target, &root)?;
        ensure!(
            final_state.revision == receipt.candidate
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
    }
    Ok(receipt)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn managed_paths_cannot_disappear_without_explicit_retirement() {
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
    fn managed_baselines_use_git_blobs_across_checkout_conversion() {
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
    fn unrelated_non_utf8_filenames_do_not_block_managed_updates() {
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
    fn replacing_the_workspace_lock_retains_recovery_and_stops_the_transaction() {
        let mut f = Fixture::new();
        let state = f.config.workspaces["projects"].state_directory.clone();
        let held = lock(&state).unwrap();
        let path = state.join("lock");
        f.inject_generator(&format!("from pathlib import Path\nimport os\nlock = Path({:?})\nmode = lock.stat().st_mode & 0o777\nlock.unlink()\nlock.touch()\nos.chmod(lock, mode)\n", path.to_str().unwrap()));
        let source = f.remote_head();
        assert!(
            f.update(true)
                .unwrap_err()
                .to_string()
                .contains("workspace lock")
        );
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
    fn generators_cannot_poison_state_even_when_they_fail() {
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
                        .contains("persistent updater state")
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
                .is_err(),
                "{shebang}"
            );
        }
        let f = Fixture::new();
        let generator = &f.config.profiles["fleet"].generator;
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
    fn frozen_unattempted_projects_recheck_the_current_consumer_forge_host() {
        let mut f = Fixture::new();
        let generator_forge =
            serde_json::from_value(serde_json::to_value(&f.config.forges["fixture"]).unwrap())
                .unwrap();
        f.config.forges.insert("generator".into(), generator_forge);
        f.config.profiles.get_mut("fleet").unwrap().generator.forge = "generator".into();
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
    fn generators_cannot_change_git_identity_signing_remotes_or_hooks() {
        for mutation in [
            "git config --local user.name 'Generator override'",
            "git config --local commit.gpgsign true",
            "git config --local remote.origin.url https://unselected.example/team/demo.git",
            "git config --local core.hooksPath /tmp/generator-hooks",
            "printf '#!/bin/sh\\nexit 71\\n' > .git/hooks/pre-commit; chmod +x .git/hooks/pre-commit",
        ] {
            let mut f = Fixture::new();
            f.inject_generator(&format!(
                "import subprocess\nsubprocess.run({mutation:?}, shell=True, check=True)\n"
            ));
            let source = f.remote_head();
            assert!(
                f.update(true)
                    .unwrap_err()
                    .to_string()
                    .contains("modified Git metadata"),
                "{mutation}"
            );
            assert_eq!(f.remote_head(), source);
            assert!(!f.root().join("receipt.json").exists());
        }
    }

    #[test]
    fn unlanded_candidates_rebuild_after_source_movement_and_retain_the_old_receipt() {
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
    fn source_movement_after_landing_preserves_adoption_reconciliation() {
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
    fn credential_bearing_clone_urls_are_rejected_before_evidence_is_saved() {
        for remote in [
            "https://TOKEN@fixture.example/team/demo.git",
            "https://git:TOKEN@fixture.example/team/demo.git",
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
    }

    #[test]
    fn replaced_adapter_programs_and_script_arguments_cannot_qualify_a_candidate() {
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
                let shell =
                    fs::canonicalize(std::env::var_os("BASH").unwrap_or_else(|| "/bin/sh".into()))
                        .unwrap();
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
        assert_eq!(
            store_file(&valid, &store).unwrap(),
            immutable.canonicalize().unwrap()
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
    fn profiles_keep_independent_candidates_receipts_and_baselines() {
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
    fn unrelated_configuration_edits_resume_the_same_frozen_candidate() {
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
    fn referenced_policy_changes_still_block_pending_candidates() {
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
    fn executes_enrolled_snapshot_when_generator_path_changes_during_qualification() {
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
    fn interrupted_initial_clones_resume_without_manual_state_removal() {
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
    fn continuation_reaches_later_batches_and_preserves_canary_barriers() {
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
    fn uncredentialed_tools_strip_secrets_and_keep_operator_git_configuration() {
        let command = Command {
            program: "/bin/tool".into(),
            arguments: vec![],
            timeout_seconds: 1,
        };
        let child = tool_command(&command, &[], None).unwrap();
        let environment = child.get_envs().collect::<BTreeMap<_, _>>();
        for key in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GITHUB_OAUTH_TOKEN",
            "GITHUB_TOKEN_CMD",
            "CODEBERG_TOKEN",
            "CODEFLOE_TOKEN",
            "ATTIC_TOKEN",
            "CACHIX_AUTH_TOKEN",
            "CACHIX_SIGNING_KEY",
            "NIX_CONFIG",
            "NIX_PATH",
            "NIX_USER_CONF_FILES",
        ] {
            assert_eq!(
                environment.get(std::ffi::OsStr::new(key)),
                Some(&None),
                "{key}"
            );
        }
        assert!(!environment.contains_key(std::ffi::OsStr::new("GIT_CONFIG_GLOBAL")));
        assert!(!environment.contains_key(std::ffi::OsStr::new("GIT_CONFIG_NOSYSTEM")));
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
    fn custom_adapter_qualifies_non_actions_plan_through_adoption() {
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

    struct Fixture {
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
            fs::write(&adapter, format!("#!{python} -I\nimport sys\nsys.argv = [sys.argv[0], {:?}, {:?}, *sys.argv[1:]]\n{}", control.to_str().unwrap(), remote.to_str().unwrap(), fs::read_to_string(fixtures.join("updater-adapter.py")).unwrap())).unwrap();
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
    fn preview_pending_candidate_and_interrupted_adoption_resume_same_commit() {
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
    fn concurrent_default_branch_writer_is_not_overwritten_by_landing() {
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
    fn changed_policy_or_source_invalidates_retained_candidate() {
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
    fn ignored_managed_outputs_are_committed_and_later_edits_block_updates() {
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
    fn failed_or_unsupported_qualification_cannot_land() {
        let f = Fixture::new();
        let source = f.remote_head();
        for status in ["failed", "unsupported", "skipped", "cancelled"] {
            f.control(json!({"candidate":status}));
            assert!(f.update(true).is_err(), "{status}");
            assert_eq!(f.remote_head(), source);
        }
    }
}

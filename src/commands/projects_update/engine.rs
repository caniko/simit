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
    let path = root.join("request.json");
    save(&path, request)?;
    let response: AdapterResponse = serde_json::from_slice(&command_output(
        &forge.adapter,
        &["--request".into(), path.to_string_lossy().into_owned()],
        None,
    )?)?;
    response.validate(request)?;
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
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "invalid repository transport"
    );
    ensure!(
        format!(
            "{}/{}",
            url.host_str().context("missing repository host")?,
            url.path().trim_start_matches('/').trim_end_matches(".git")
        ) == expected,
        "adapter remote differs from enrolled coordinate"
    );
    Ok(())
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
    remote_identity(&response.remote, &project.coordinate)?;
    Ok(response)
}

fn workflows(plan: &CiUpdatePlan) -> Vec<String> {
    plan.files
        .iter()
        .map(|f| f.relative_path.to_string_lossy().into_owned())
        .collect()
}

fn generate(profile: &Profile, checkout: &Path) -> Result<CiUpdatePlan> {
    let command = Command {
        program: profile.generator.executable.clone(),
        arguments: vec![
            "init".into(),
            "ci".into(),
            "--primary-only".into(),
            "--plan-json".into(),
        ],
        timeout_seconds: 900,
    };
    let plan: CiUpdatePlan =
        serde_json::from_slice(&command_output(&command, &[], Some(checkout))?)?;
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
            let hash = if path.exists() {
                Some(digest(&fs::read(path)?))
            } else {
                None
            };
            Ok((p, hash))
        })
        .collect()
}

pub fn update(
    config: &Config,
    profile: &Profile,
    workspace: &Workspace,
    project: &Project,
    push: bool,
    wait_seconds: u64,
) -> Result<Receipt> {
    let deadline = Instant::now() + Duration::from_secs(wait_seconds);
    let root = workspace
        .state_directory
        .join(format!("repo-{}", digest(project.coordinate.as_bytes())));
    private_directory(&root)?;
    let receipt_path = root.join("receipt.json");
    let config_digest = canonical_digest(&(config, profile))?;
    let forge = &config.forges[&project.forge];
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
    let previous: Option<Receipt> = if receipt_path.exists() {
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
            let owner: String = crate::review::read_json(&root.join("clone-owner.json"))?;
            ensure!(
                owner == project.coordinate
                    && fs::symlink_metadata(&checkout)?.is_dir()
                    && git(&checkout, &["rev-parse", "--show-toplevel"])?
                        == checkout.canonicalize()?.to_string_lossy(),
                "refusing to retire an unowned or redirected checkout"
            );
            // Only updater-owned disposable clones are retired.
            fs::remove_dir_all(&checkout)?;
        }
        git(
            &root,
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
        save(&root.join("clone-owner.json"), &project.coordinate)?;
        git(&checkout, &["checkout", "--detach", &observed.revision])?;
    }
    if let Some(d) = &profile.generator.executable_sha256 {
        ensure!(
            &digest(&fs::read(&profile.generator.executable)?) == d,
            "generator executable digest differs from enrollment"
        );
    }
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
        let plan = generate(profile, &checkout)?;
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
        let before = hashes(&checkout, paths.clone().into_iter())?;
        let baseline_path = root.join("baseline.json");
        if baseline_path.exists() {
            let baseline: Baseline = crate::review::read_json(&baseline_path)?;
            ensure!(
                baseline.repository_id == observed.repository_id,
                "repository identity differs from adopted baseline"
            );
            ensure!(
                hashes(&checkout, baseline.managed.keys().cloned())? == baseline.managed,
                "managed outputs changed since adoption; explicit reconciliation is required"
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
        let managed = hashes(&checkout, paths.into_iter())?;
        let mut stage = vec!["--literal-pathspecs", "add", "--all", "--force", "--"];
        stage.extend(
            managed
                .keys()
                .filter(|p| managed[*p].is_some() || before[*p].is_some())
                .map(String::as_str),
        );
        git(&checkout, &stage)?;
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
        hashes(&checkout, receipt.managed.keys().cloned())? == receipt.managed
            && git(&checkout, &["status", "--porcelain"])?.is_empty(),
        "retained candidate files changed"
    );
    let tracked = git(&checkout, &["ls-files", "-z"])?;
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
            let python = String::from_utf8(python.stdout).unwrap().trim().to_owned();
            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
            // Generator enrollment is an actual executable digest, with a tiny
            // launcher pointing to the test-only renderer.
            let generator = root.join("generator");
            fs::write(
                &generator,
                format!(
                    "#!/bin/sh\nexec '{}' '{}' \"$@\"\n",
                    python,
                    fixtures.join("updater-generator.py").display()
                ),
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&generator, fs::Permissions::from_mode(0o700)).unwrap();
            let config: Config = serde_json::from_value(json!({
                "schemaVersion":1,
                "workspaces":{"projects":{"root":root.join("projects"),"stateDirectory":root.join("state"),"inventory":{"file":root.join("inventory.json")}}},
                "forges":{"fixture":{"host":"fixture.example","apiUrl":"https://fixture.example/api","adapter":{"program":python,"arguments":[fixtures.join("updater-adapter.py"),control,remote]},"checkAuthority":"777"}},
                "profiles":{"fleet":{"workspace":"projects","scopes":["ci"],"enrollCurrent":true,"rollout":{"maintenanceRefPrefix":"maintenance/"},"generator":{"executable":generator,"executableSha256":digest(&fs::read(&generator).unwrap()),"sourceRevision":"a".repeat(40),"sourceCoordinate":"fixture.example/team/generator","forge":"fixture","requiredChecks":[{"name":"generator","authority":"777"}]}}}
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
        fn update(&self, push: bool) -> Result<Receipt> {
            update(
                &self.config,
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
        let status_before = status(&f.config.workspaces["projects"].state_directory).unwrap();
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
            status(&f.config.workspaces["projects"].state_directory).unwrap()[0].phase,
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
        let checkout = f.config.workspaces["projects"]
            .state_directory
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

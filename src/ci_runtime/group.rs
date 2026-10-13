//! Shared-store grouped Nix execution with source and discovered-check admission.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use super::{
    cancellation::{Cancellation, Cancelled},
    process::{CommandRunner, Runner},
};

// Only this small wrapper crosses execve. Bash builtins read/eval the private
// file without stripping trailing newlines or changing bash -c's $0, $@, stdin,
// errexit or pipefail semantics. The script itself never occupies argv or env.
pub(crate) const HOOK_FILE_COMMAND: &str = "IFS= read -r -d '' SIMIT_NIX_HOOK_SCRIPT < \"$SIMIT_NIX_HOOK_FILE\" || :\neval -- \"$SIMIT_NIX_HOOK_SCRIPT\"";

/// Maximum compact JSON spec size, shared by generation, library admission and CLI reads.
pub const MAX_SPEC_BYTES: u64 = 16 * 1024 * 1024;

struct SpecBudget(u64);

impl Write for SpecBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.0 {
            return Err(std::io::Error::other(
                "serialized group spec exceeds the 16 MiB byte budget",
            ));
        }
        self.0 -= bytes.len() as u64;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Versioned execution input. This describes execution, not qualification authority.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupSpec {
    /// Input schema, currently `simit.nix-group-spec.v1`.
    pub schema: String,
    /// Ordered installables assigned to this worker.
    pub members: Vec<String>,
    /// Complete enrolled execution plan across workers.
    pub plan: Vec<String>,
    /// Systems whose discovered checks must all have execution owners.
    pub check_systems: Vec<String>,
    /// Only `--max-jobs` and `--cores`; zero cores means use available CPU cores.
    pub limits: Vec<String>,
    /// Scripts run after successful builds with valid output maps.
    pub post_build: Vec<String>,
    /// Scripts run after every attempted member.
    pub post_build_always: Vec<String>,
    /// Members requiring materialization followed by current-worker rebuild.
    #[serde(default)]
    pub fresh_installables: Vec<String>,
}

impl GroupSpec {
    /// Read generated workflow environment without interpreting shell code or argv.
    pub fn from_environment() -> Result<Self> {
        fn list(key: &str) -> Result<Vec<String>> {
            serde_json::from_str(&std::env::var(key).with_context(|| format!("missing {key}"))?)
                .with_context(|| format!("invalid {key}"))
        }
        Ok(Self {
            schema: "simit.nix-group-spec.v1".into(),
            members: list("INSTALLABLES")?,
            plan: list("SIMIT_NIX_PLAN")?,
            check_systems: list("SIMIT_NIX_CHECK_SYSTEMS")?,
            limits: list("SIMIT_NIX_LIMITS")?,
            post_build: list("SIMIT_NIX_POST_BUILD")?,
            post_build_always: list("SIMIT_NIX_POST_BUILD_ALWAYS")?,
            fresh_installables: match std::env::var("SIMIT_NIX_FRESH_INSTALLABLES") {
                Ok(value) => serde_json::from_str(&value)?,
                Err(std::env::VarError::NotPresent) => Vec::new(),
                Err(error) => return Err(error.into()),
            },
        })
    }

    /// Validate all input fences before any Git or Nix process is dispatched.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == "simit.nix-group-spec.v1",
            "unknown group spec schema"
        );
        for (name, list) in [("members", &self.members), ("plan", &self.plan)] {
            ensure!(
                !list.is_empty() && list.len() <= 4096,
                "group {name} is empty or oversized"
            );
            ensure!(
                list.iter().collect::<BTreeSet<_>>().len() == list.len(),
                "duplicate group {name}"
            );
            ensure!(
                list.iter().all(|s| !s.is_empty()
                    && !s.starts_with('-')
                    && s.len() <= 4096
                    && !s.contains(['\n', '\r', '\0'])),
                "invalid group {name}"
            );
        }
        ensure!(
            self.members.iter().all(|s| self.plan.contains(s)),
            "group members outside the enrolled plan"
        );
        ensure!(
            self.fresh_installables
                .iter()
                .all(|s| self.plan.contains(s))
                && self
                    .fresh_installables
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    == self.fresh_installables.len(),
            "invalid fresh installables"
        );
        ensure!(
            self.check_systems.len() <= 64
                && self.check_systems.iter().collect::<BTreeSet<_>>().len()
                    == self.check_systems.len(),
            "duplicate or excessive check systems"
        );
        ensure!(
            self.check_systems.iter().all(|s| !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))),
            "invalid check system"
        );
        ensure!(
            self.limits.len() <= 4 && self.limits.len() % 2 == 0,
            "invalid Nix limits"
        );
        let mut flags = BTreeSet::new();
        for pair in self.limits.chunks_exact(2) {
            ensure!(
                matches!(pair[0].as_str(), "--max-jobs" | "--cores")
                    && flags.insert(&pair[0])
                    && pair[1]
                        .parse::<u32>()
                        .is_ok_and(|n| n <= 1024 && (pair[0] == "--cores" || n > 0)),
                "invalid Nix limits"
            );
        }
        for hooks in [&self.post_build, &self.post_build_always] {
            ensure!(
                hooks.len() <= 256
                    && hooks
                        .iter()
                        .all(|s| s.len() <= 1024 * 1024 && !s.contains('\0')),
                "invalid group hooks"
            );
        }
        // Count encoded bytes (including JSON escapes) without allocating an
        // oversized serialized copy of otherwise individually valid lists.
        serde_json::to_writer(SpecBudget(MAX_SPEC_BYTES), self)?;
        Ok(())
    }
}

/// Workflow identity, supplied by the provider environment rather than a receipt.
#[derive(Debug)]
pub struct Source {
    /// Enrolled push head or PR event head (not the synthetic merge).
    pub head: String,
    /// Provider event name.
    pub event: String,
    /// Provider run id retained as a decimal string for v1 compatibility.
    pub run: String,
    /// Provider attempt number; acceptance policy is deliberately outside this executor.
    pub attempt: u64,
}

impl Source {
    /// Resolve the exact event head from the provider's local event file.
    pub fn from_environment() -> Result<Self> {
        let event = std::env::var("GITHUB_EVENT_NAME")?;
        let head = if event == "pull_request" {
            let payload: Value = read_json(
                Path::new(&std::env::var("GITHUB_EVENT_PATH")?),
                16 * 1024 * 1024,
            )?;
            payload["pull_request"]["head"]["sha"]
                .as_str()
                .context("missing PR head")?
                .to_owned()
        } else {
            std::env::var("GITHUB_SHA")?
        };
        Ok(Self {
            head,
            event,
            run: std::env::var("GITHUB_RUN_ID")?,
            attempt: std::env::var("GITHUB_RUN_ATTEMPT")?.parse()?,
        })
    }

    fn validate(&self) -> Result<()> {
        ensure!(oid(&self.head), "invalid event head");
        ensure!(
            matches!(
                self.event.as_str(),
                "push" | "pull_request" | "workflow_dispatch"
            ),
            "unsupported group event"
        );
        ensure!(
            self.run.len() <= 20
                && self.run.bytes().all(|b| b.is_ascii_digit())
                && self.run.parse::<u64>().is_ok_and(|n| n > 0)
                && self.attempt > 0,
            "invalid provider run identity"
        );
        Ok(())
    }
}

/// One member's retained result, preserving `simit.nix-group.v1` fields.
#[derive(Debug, Serialize, Deserialize)]
pub struct MemberResult {
    /// Enrolled installable.
    pub installable: String,
    /// Build status (or 125 for dispatch/limit failure).
    pub exit_status: i32,
    /// Whether successful output bytes form a valid Nix output map.
    pub output_map_valid: bool,
    /// Relative numbered member evidence directory.
    pub evidence: String,
    /// Success hook statuses, in configured order.
    pub post_build_statuses: Vec<i32>,
    /// Diagnostic hook statuses, in configured order.
    pub post_build_always_statuses: Vec<i32>,
    /// For fresh members, only successful `--rebuild` establishes fresh execution.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fresh_execution: Option<bool>,
}

/// Local execution evidence. It does not replace fresh provider-backed qualification.
#[derive(Debug, Serialize, Deserialize)]
pub struct GroupReceipt {
    /// Retained receipt schema.
    pub schema: String,
    /// Observed source commit.
    pub head: String,
    /// Observed source tree.
    pub tree: String,
    /// Provider run id.
    pub run: String,
    /// Provider attempt.
    pub attempt: u64,
    /// Provider event.
    pub event: String,
    /// True only after every assigned member is attempted.
    pub complete: bool,
    /// One result per attempted member, in enrollment order.
    pub results: Vec<MemberResult>,
    /// Absent for incomplete receipts; no incomplete receipt can establish success.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub success: Option<bool>,
}

/// Execute an enrolled group, retaining a failed receipt rather than discarding failed members.
pub fn execute(spec: &GroupSpec, source: &Source, cwd: &Path, root: &Path) -> Result<GroupReceipt> {
    execute_with(
        spec,
        source,
        cwd,
        root,
        &mut CommandRunner,
        &Cancellation::default(),
    )
}

/// Execute with a caller-owned cancellation token and injectable process boundary.
/// The library installs no signal handlers. CLI callers own handlers for their
/// process lifetime; embedding applications retain their own signal policy.
pub fn execute_with(
    spec: &GroupSpec,
    source: &Source,
    cwd: &Path,
    root: &Path,
    runner: &mut impl Runner,
    cancellation: &Cancellation,
) -> Result<GroupReceipt> {
    cancellation.check()?;
    spec.validate()?;
    source.validate()?;
    ensure!(
        fs::symlink_metadata(root)?.file_type().is_dir(),
        "group results must be an existing directory, not a symlink"
    );
    for name in ["receipt.json", "installables.json", "coverage.json"] {
        ensure!(
            fs::symlink_metadata(root.join(name))
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "group evidence already exists: {name}"
        );
    }
    // Interrupted executions and shell-only diagnostics can leave member evidence
    // without a top-level receipt. Reserve every decimal entry, including indices
    // outside this spec and dangling symlinks, before observing Git/Nix or writing.
    for entry in fs::read_dir(root)? {
        cancellation.check()?;
        let name = entry?.file_name();
        ensure!(
            !name.to_str().is_some_and(|name| {
                !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit())
            }),
            "member evidence already exists: {}",
            name.to_string_lossy()
        );
    }
    let root = root.canonicalize()?;
    let cwd = cwd.canonicalize()?;
    let head = observe(&cwd, "git", &["rev-parse", "HEAD"], runner, cancellation)?;
    let tree = observe(
        &cwd,
        "git",
        &["rev-parse", "HEAD^{tree}"],
        runner,
        cancellation,
    )?;
    ensure!(
        head.trim() == source.head && oid(tree.trim()),
        "Group source is not the enrolled event head"
    );
    observe(
        &cwd,
        "git",
        &["diff", "--exit-code", "HEAD", "--"],
        runner,
        cancellation,
    )?;
    save(&root.join("installables.json"), &spec.members)?;
    let mut receipt = GroupReceipt {
        schema: "simit.nix-group.v1".into(),
        head: source.head.clone(),
        tree: tree.trim().into(),
        run: source.run.clone(),
        attempt: source.attempt,
        event: source.event.clone(),
        complete: false,
        results: Vec::new(),
        success: None,
    };
    save(&root.join("receipt.json"), &receipt)?;
    let mut coverage = BTreeMap::new();
    for system in &spec.check_systems {
        let attr = format!(".#checks.{system}");
        let checks: BTreeMap<String, String> = serde_json::from_str(&observe(
            &cwd,
            "nix",
            &[
                "eval",
                "--json",
                "--no-update-lock-file",
                &attr,
                "--apply",
                "checks: builtins.mapAttrs (_: value: value.drvPath) checks",
            ],
            runner,
            cancellation,
        )?)?;
        ensure!(!checks.is_empty(), "Check inventory is empty or invalid");
        for (name, derivation) in checks {
            ensure!(
                store_path(&derivation) && derivation.ends_with(".drv"),
                "invalid check derivation"
            );
            let attr = if identifier(&name) {
                name
            } else {
                serde_json::to_string(&name)?
            };
            coverage.insert(format!(".#checks.{system}.{attr}"), derivation);
        }
    }
    save(
        &root.join("coverage.json"),
        &json!({"head": receipt.head, "tree": receipt.tree, "plan": spec.plan, "checks": coverage}),
    )?;
    let uncovered: Vec<_> = coverage
        .keys()
        .filter(|attr| !spec.plan.contains(attr))
        .collect();
    ensure!(
        uncovered.is_empty(),
        "Discovered flake checks have no execution owner: {uncovered:?}"
    );
    let mut failed = false;
    for (index, installable) in spec.members.iter().enumerate() {
        cancellation.check()?;
        let folder = root.join(index.to_string());
        // Fallback diagnostics may have created folders after an earlier setup failure.
        ensure!(!folder.exists(), "member evidence directory already exists");
        fs::create_dir(&folder)?;
        fs::write(folder.join("installable"), format!("{installable}\n"))?;
        let fresh = spec.fresh_installables.contains(installable);
        let mut status = build(
            &cwd,
            &folder,
            installable,
            &spec.limits,
            if fresh {
                BuildPhase::Materialize
            } else {
                BuildPhase::Build
            },
            runner,
            cancellation,
        )?;
        let materialize_valid = !fresh
            || (status == 0
                && output_map_valid(&fs::read(folder.join("materialization-result.json"))?));
        if fresh && materialize_valid {
            status = build(
                &cwd,
                &folder,
                installable,
                &spec.limits,
                BuildPhase::Rebuild,
                runner,
                cancellation,
            )?;
        } else if fresh {
            // Retain the failed materialization as the member result too.
            fs::copy(
                folder.join("materialization-result.json"),
                folder.join("result.json"),
            )?;
        }
        let valid = materialize_valid
            && status == 0
            && output_map_valid(&fs::read(folder.join("result.json"))?);
        let success_hooks = if valid {
            hooks(
                &spec.post_build,
                &cwd,
                &folder,
                installable,
                "post-build",
                runner,
                cancellation,
            )?
        } else {
            Vec::new()
        };
        let diagnostic_hooks = hooks(
            &spec.post_build_always,
            &cwd,
            &folder,
            installable,
            "post-build-always",
            runner,
            cancellation,
        )?;
        cancellation.check()?;
        save(&folder.join("diagnostics-complete"), &diagnostic_hooks)?;
        failed |= status != 0
            || !valid
            || success_hooks
                .iter()
                .chain(&diagnostic_hooks)
                .any(|&code| code != 0);
        let row = MemberResult {
            installable: installable.clone(),
            exit_status: status,
            output_map_valid: valid,
            evidence: index.to_string(),
            post_build_statuses: success_hooks,
            post_build_always_statuses: diagnostic_hooks,
            fresh_execution: fresh.then_some(status == 0 && valid),
        };
        save(&folder.join("status.json"), &row)?;
        println!("{}", serde_json::to_string(&row)?);
        receipt.results.push(row);
        save(&root.join("receipt.json"), &receipt)?;
    }
    cancellation.check()?;
    receipt.complete = receipt.results.len() == spec.members.len();
    receipt.success = Some(receipt.complete && !failed);
    save(&root.join("receipt.json"), &receipt)?;
    Ok(receipt)
}

fn observe(
    cwd: &Path,
    program: &str,
    args: &[&str],
    runner: &mut impl Runner,
    cancellation: &Cancellation,
) -> Result<String> {
    let temp = tempfile::tempdir()?;
    let stdout = temp.path().join("stdout");
    let stderr = temp.path().join("stderr");
    let status = dispatch(
        std::process::Command::new(program)
            .args(args)
            .current_dir(cwd),
        &stdout,
        &stderr,
        runner,
        cancellation,
    )?;
    ensure!(
        status == 0,
        "{program} observation failed ({status}): {}",
        String::from_utf8_lossy(&fs::read(&stderr)?)
    );
    Ok(fs::read_to_string(stdout)?)
}

#[derive(Clone, Copy)]
enum BuildPhase {
    Build,
    Materialize,
    Rebuild,
}

fn build(
    cwd: &Path,
    folder: &Path,
    installable: &str,
    limits: &[String],
    phase: BuildPhase,
    runner: &mut impl Runner,
    cancellation: &Cancellation,
) -> Result<i32> {
    let materialize = matches!(phase, BuildPhase::Materialize);
    let name = if materialize {
        "materialization-result"
    } else {
        "result"
    };
    let log = if matches!(phase, BuildPhase::Rebuild) {
        "fresh-build.log"
    } else {
        "build.log"
    };
    let mut command = std::process::Command::new("nix");
    command.current_dir(cwd).args([
        "build",
        "--no-link",
        "--no-update-lock-file",
        "--print-build-logs",
        "--json",
    ]);
    if matches!(phase, BuildPhase::Rebuild) {
        command.args(["--rebuild", "--option", "builders", ""]);
    }
    command.args(limits).arg(installable);
    // Dispatch/limit failures are member failures, not reasons to lose later evidence.
    member_status(dispatch(
        &mut command,
        &folder.join(format!("{name}.json")),
        &folder.join(log),
        runner,
        cancellation,
    ))
}

fn member_status(result: Result<i32>) -> Result<i32> {
    match result {
        Err(error) if error.is::<Cancelled>() => Err(error),
        Err(_) => Ok(125),
        Ok(status) => Ok(status),
    }
}

fn hooks(
    scripts: &[String],
    cwd: &Path,
    folder: &Path,
    installable: &str,
    category: &str,
    runner: &mut impl Runner,
    cancellation: &Cancellation,
) -> Result<Vec<i32>> {
    scripts
        .iter()
        .enumerate()
        .map(|(index, script)| {
            cancellation.check()?;
            let script_path = folder.join(format!("{category}-{index}.script"));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut script_file = options.open(&script_path)?;
            script_file.write_all(script.as_bytes())?;
            drop(script_file);
            let mut command = std::process::Command::new("bash");
            command
                .current_dir(cwd)
                .args(["-e", "-o", "pipefail", "-c", HOOK_FILE_COMMAND])
                .env("SIMIT_NIX_HOOK_FILE", &script_path)
                .env("INSTALLABLE", installable)
                .env("SIMIT_NIX_BUILD_RESULTS", folder);
            member_status(dispatch(
                &mut command,
                &folder.join(format!("{category}-{index}.stdout")),
                &folder.join(format!("{category}-{index}.log")),
                runner,
                cancellation,
            ))
        })
        .collect()
}

fn dispatch(
    command: &mut std::process::Command,
    stdout: &Path,
    stderr: &Path,
    runner: &mut impl Runner,
    cancellation: &Cancellation,
) -> Result<i32> {
    cancellation.check()?;
    let result = runner.run(command, stdout, stderr, cancellation);
    cancellation.check()?;
    result
}

fn output_map_valid(raw: &[u8]) -> bool {
    serde_json::from_slice::<Value>(raw)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .is_some_and(|rows| {
            !rows.is_empty()
                && rows.iter().all(|row| {
                    row["drvPath"]
                        .as_str()
                        .is_some_and(|s| store_path(s) && s.ends_with(".drv"))
                        && row["outputs"].as_object().is_some_and(|outputs| {
                            !outputs.is_empty()
                                && outputs.values().all(|p| p.as_str().is_some_and(store_path))
                        })
                })
        })
}

fn store_path(path: &str) -> bool {
    path.strip_prefix("/nix/store/").is_some_and(|name| {
        !name.is_empty()
            && !name.contains('/')
            && !name.contains(['\0', '\n', '\r'])
            && name != "."
            && name != ".."
    })
}
fn identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn oid(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn read_json<T: serde::de::DeserializeOwned>(path: &Path, maximum: u64) -> Result<T> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "input must be a regular file"
    );
    let mut raw = Vec::new();
    fs::File::open(path)?
        .take(maximum + 1)
        .read_to_end(&mut raw)?;
    ensure!(raw.len() as u64 <= maximum, "JSON input exceeds budget");
    Ok(serde_json::from_slice(&raw)?)
}

fn save(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent: PathBuf = path.parent().context("evidence path has no parent")?.into();
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, value)?;
    temp.write_all(b"\n")?;
    temp.persist(path)?;
    Ok(())
}

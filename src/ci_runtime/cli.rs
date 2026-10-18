//! Thin CLI adapters for versioned CI runtime contracts.
use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use std::path::PathBuf;
#[path = "cli_git.rs"]
mod git;
#[path = "cli_objects.rs"]
mod objects;

/// CI execution commands. Receipts are evidence, never standalone acceptance.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Execute every enrolled Nix member on one worker and retain per-member evidence.
    NixGroup(GroupArgs),
    /// Reconcile retained native ZIPs under an explicit consumer contract (no provider writes).
    LogReconcile(super::log_reconcile::Args),
    /// Capture enrolled original GitHub producers with bounded read-only observation.
    ObserveOriginal(ObserveArgs),
    /// Revalidate exact uploaded custody, provider digest, members and retention.
    VerifyOriginalRetention(RetentionArgs),
}

/// Source-enrolled custody capture inputs.
#[derive(Debug, Args)]
pub struct ObserveArgs {
    /// Versioned source-owned JSON enrollment with exact required job names.
    #[arg(long)]
    pub spec: PathBuf,
    /// Fresh evidence directory; original packets are never reused or overwritten.
    #[arg(long)]
    pub results: PathBuf,
}

/// Independently supplied inputs for native uploaded-custody verification.
#[derive(Debug, Args)]
pub struct RetentionArgs {
    /// Versioned source-owned JSON enrollment.
    #[arg(long)]
    pub spec: PathBuf,
    /// Original local captured packet.
    #[arg(long)]
    pub results: PathBuf,
    /// Native upload action artifact id.
    #[arg(long)]
    pub artifact_id: u64,
    /// Native upload action SHA-256 digest.
    #[arg(long)]
    pub artifact_digest: String,
}

#[cfg(windows)]
#[path = "cli_windows.rs"]
mod windows;

struct CliCancellation {
    token: super::cancellation::Cancellation,
}
#[cfg(windows)]
impl Drop for CliCancellation {
    fn drop(&mut self) {
        windows::finished();
    }
}

fn cancellation() -> Result<CliCancellation> {
    let cancellation = super::cancellation::Cancellation::default();
    #[cfg(unix)]
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, cancellation.0.clone())?;
    }
    #[cfg(windows)]
    windows::register(cancellation.clone())?;
    Ok(CliCancellation {
        token: cancellation,
    })
}

fn observer_inputs(
    path: &std::path::Path,
    results: &std::path::Path,
    cancellation: &super::cancellation::Cancellation,
    retention: bool,
) -> Result<(
    super::observer::Spec,
    super::observer::Identity,
    super::observer::GitHubTransport,
)> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut name = results
        .file_name()
        .context("observer results must have a directory name")?
        .to_os_string();
    name.push(if retention {
        ".retention.preflight-"
    } else {
        ".preflight-"
    });
    let preflight = tempfile::Builder::new()
        .prefix(&name)
        .tempdir_in(
            results
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new(".")),
        )?
        .keep();
    super::observer::preflight_receipt(&preflight, false, None)?;
    let outcome = (|| {
        let raw = super::observer::read_preflight(
            path,
            super::observer::MAX_SPEC_BYTES,
            deadline,
            cancellation,
        )?;
        let spec: super::observer::Spec = serde_json::from_slice(&raw)?;
        let identity =
            super::observer::Identity::from_environment_before(&spec, deadline, cancellation)?;
        let merge = if identity.event == "pull_request" {
            let event: serde_json::Value = serde_json::from_str(&identity.event_json)?;
            let base = event["pull_request"]["base"]["sha"]
                .as_str()
                .context("missing PR base revision")?
                .to_owned();
            Some((
                std::env::var("GITHUB_SHA").context("missing protected PR merge revision")?,
                base,
            ))
        } else {
            None
        };
        let committed = verify_source_spec(
            path,
            &raw,
            &identity.head,
            merge
                .as_ref()
                .map(|(sha, base)| (sha.as_str(), base.as_str())),
            deadline,
            cancellation,
        )?;
        // The committed blob is policy, even when configured checkout/clean filters
        // change its representation. Never deserialize the transformed policy.
        let spec: super::observer::Spec = serde_json::from_slice(&committed)?;
        identity.validate(&spec)?;
        super::observer::preflight_check(deadline, cancellation)?;
        let token = observer_token(
            std::env::var("GH_TOKEN").ok(),
            std::env::var("GITHUB_TOKEN").ok(),
        )?;
        Ok((
            spec,
            identity,
            super::observer::GitHubTransport::new(token)?,
        ))
    })();
    let outcome = outcome.and_then(|inputs| {
        super::observer::preflight_check(deadline, cancellation)?;
        Ok(inputs)
    });
    super::observer::preflight_receipt(
        &preflight,
        outcome.is_ok(),
        outcome.as_ref().err().map(|error| format!("{error:#}")),
    )?;
    let outcome = outcome.and_then(|inputs| {
        super::observer::preflight_check(deadline, cancellation)?;
        Ok(inputs)
    });
    if let Err(error) = &outcome {
        super::observer::preflight_receipt(&preflight, false, Some(format!("{error:#}")))?;
    }
    outcome
}

fn observer_token(primary: Option<String>, fallback: Option<String>) -> Result<String> {
    primary
        .filter(|token| !token.is_empty())
        .or_else(|| fallback.filter(|token| !token.is_empty()))
        .context("missing GitHub token environment variable")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(unix, windows))]
    #[test]
    fn source_probe_cancellation_interrupts_a_running_child_and_retains_its_output() {
        use std::time::{Duration, Instant};
        let root = tempfile::tempdir().unwrap();
        let cancellation = super::super::cancellation::Cancellation::default();
        let token = cancellation.clone();
        let output = root.path().join("slow-git");
        let cancel = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut started = false;
            while Instant::now() < deadline {
                if std::fs::read(&output)
                    .is_ok_and(|bytes| bytes == b"probe started\n" || bytes == b"probe started\r\n")
                {
                    started = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            token.cancel();
            (started, Instant::now())
        });
        let start = Instant::now();
        #[cfg(unix)]
        let mut command = {
            let mut command = std::process::Command::new("/bin/sh");
            command.args(["-c", "printf 'probe started\\n'; while :; do :; done"]);
            command
        };
        #[cfg(windows)]
        let mut command = {
            // Native built-ins avoid managed PowerShell startup in the probe's
            // intentionally cleared environment. /d disables cmd AutoRun hooks.
            let mut command = std::process::Command::new(r"C:\Windows\System32\cmd.exe");
            command.args([
                "/d",
                "/c",
                "echo probe started& for /l %i in (0,0,1) do @rem",
            ]);
            command
        };
        let result = source_probe_bytes(
            root.path(),
            "slow-git",
            &mut command,
            &SourceControl {
                deadline: start + Duration::from_secs(15),
                cancellation: &cancellation,
            },
            1024,
        );
        let (started, cancelled_at) = cancel.join().unwrap();
        assert!(
            started,
            "probe did not emit its start marker; stderr: {}",
            String::from_utf8_lossy(&std::fs::read(root.path().join("slow-git.errors")).unwrap())
        );
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(cancelled_at.elapsed() < Duration::from_secs(3));
        assert_eq!(
            String::from_utf8(std::fs::read(root.path().join("slow-git")).unwrap())
                .unwrap()
                .replace("\r\n", "\n"),
            "probe started\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn source_probe_clears_loader_shell_and_credential_environment_before_launch() {
        let root = tempfile::tempdir().unwrap();
        let hook = root.path().join("startup");
        let marker = root.path().join("injected");
        std::fs::write(&hook, format!("printf injected > '{}'", marker.display())).unwrap();
        let cancellation = super::super::cancellation::Cancellation::default();
        let raw = source_probe_bytes(
            root.path(),
            "environment",
            std::process::Command::new("/bin/sh")
                .args(["-c", "test -z \"$BASH_ENV$ENV$LD_PRELOAD$LD_LIBRARY_PATH$DYLD_INSERT_LIBRARIES$GH_TOKEN$GITHUB_TOKEN\" && printf clean"])
                .env("BASH_ENV", &hook)
                .env("ENV", &hook)
                .env("LD_PRELOAD", "inert-fixture-library")
                .env("LD_LIBRARY_PATH", root.path())
                .env("DYLD_INSERT_LIBRARIES", "inert-fixture-library")
                .env("GH_TOKEN", "fixture-token")
                .env("GITHUB_TOKEN", "fixture-token"),
            &SourceControl {
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
                cancellation: &cancellation,
            },
            1024,
        )
        .unwrap();
        assert_eq!(raw, b"clean");
        assert!(!marker.exists());
    }

    #[test]
    fn cancellation_before_identity_retains_an_incomplete_preflight_receipt() {
        for retention in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let spec = root.path().join("spec.json");
            std::fs::write(&spec, b"{}").unwrap();
            let cancellation = super::super::cancellation::Cancellation::default();
            cancellation.cancel();
            let results = root.path().join("packet");
            let error = observer_inputs(&spec, &results, &cancellation, retention)
                .err()
                .unwrap();
            assert!(error.to_string().contains("cancelled"));
            let diagnostic = std::fs::read_dir(root.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.is_dir())
                .unwrap();
            let receipt: serde_json::Value =
                serde_json::from_slice(&std::fs::read(diagnostic.join("receipt.json")).unwrap())
                    .unwrap();
            assert_eq!(receipt["capture_complete"], false);
            assert_eq!(receipt["qualified"], false);
            assert_eq!(receipt["preflight_complete"], false);
            assert!(receipt["error"].as_str().unwrap().contains("cancelled"));
            assert!(!results.exists());
        }
    }

    #[test]
    fn empty_primary_token_uses_nonempty_fallback_without_exposing_credentials() {
        assert_eq!(
            observer_token(Some(String::new()), Some("inert-fallback".into())).unwrap(),
            "inert-fallback"
        );
        assert_eq!(
            observer_token(Some("inert-primary".into()), Some("inert-fallback".into())).unwrap(),
            "inert-primary"
        );
        assert!(observer_token(Some(String::new()), Some(String::new())).is_err());
        assert!(observer_token(None, None).is_err());
    }
}

struct SourceControl<'a> {
    deadline: std::time::Instant,
    cancellation: &'a super::cancellation::Cancellation,
}

fn source_probe(
    root: &std::path::Path,
    name: &str,
    command: &mut std::process::Command,
    control: &SourceControl<'_>,
) -> Result<String> {
    Ok(String::from_utf8(source_probe_bytes(
        root,
        name,
        command,
        control,
        1024 * 1024,
    )?)?
    .trim_end_matches(['\r', '\n'])
    .to_owned())
}

fn source_probe_bytes(
    root: &std::path::Path,
    name: &str,
    command: &mut std::process::Command,
    control: &SourceControl<'_>,
    maximum: u64,
) -> Result<Vec<u8>> {
    let output = root.join(name);
    let errors = root.join(format!("{name}.errors"));
    super::observer::preflight_check(control.deadline, control.cancellation)?;
    let remaining = control
        .deadline
        .checked_duration_since(std::time::Instant::now())
        .context("source-owned observer spec probe deadline expired")?;
    // An absolute protected Git executable still needs a clean child launch:
    // inherited loader/shell variables run before any Git argument is checked.
    command.env_clear();
    #[cfg(windows)]
    command
        .env("SystemRoot", r"C:\Windows")
        .env("WINDIR", r"C:\Windows");
    command
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
    let status = super::process::run_probe(
        command,
        &output,
        &errors,
        control.cancellation,
        remaining,
        maximum,
    )?;
    ensure!(
        status == 0,
        "source-owned observer spec Git probe failed ({name})"
    );
    super::observer::read_preflight(&output, maximum, control.deadline, control.cancellation)
}

fn verify_source_spec(
    path: &std::path::Path,
    raw: &[u8],
    head: &str,
    merge: Option<(&str, &str)>,
    deadline: std::time::Instant,
    cancellation: &super::cancellation::Cancellation,
) -> Result<Vec<u8>> {
    let control = SourceControl {
        deadline,
        cancellation,
    };
    super::observer::preflight_check(deadline, cancellation)?;
    let probe = tempfile::tempdir()?;
    let git = git::executable()?;
    let actual_head = source_probe(
        probe.path(),
        "head",
        std::process::Command::new(&git)
            .arg("--no-replace-objects")
            .args(["rev-parse", "HEAD"]),
        &control,
    )?;
    ensure!(
        actual_head == head,
        "observer checkout differs from exact source head"
    );
    let commit = objects::object(
        probe.path(),
        &git,
        "commit",
        head,
        "source-commit",
        &control,
        1024 * 1024,
    )?;
    if let Some((merge_sha, base)) = merge {
        ensure!(
            merge_sha.len() == 40
                && merge_sha
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "invalid protected PR merge revision"
        );
        let merge = objects::object(
            probe.path(),
            &git,
            "commit",
            merge_sha,
            "merge",
            &control,
            65536,
        )?;
        let merge = std::str::from_utf8(&merge)?;
        let parents = merge
            .split("\n\n")
            .next()
            .unwrap_or("")
            .lines()
            .filter_map(|line| line.strip_prefix("parent "))
            .collect::<Vec<_>>();
        ensure!(
            parents.len() == 2 && parents[1] == head && parents[0] == base,
            "PR source head/base differs from protected merge revision"
        );
    }
    let repository = source_probe(
        probe.path(),
        "root",
        std::process::Command::new(&git)
            .arg("--no-replace-objects")
            .args(["rev-parse", "--show-toplevel"]),
        &control,
    )?;
    let repository = std::fs::canonicalize(repository)?;
    let path = std::fs::canonicalize(path)?;
    let relative = path
        .strip_prefix(&repository)
        .context("source-owned observer spec must be inside the checkout")?;
    let relative = relative
        .components()
        .map(|part| {
            part.as_os_str()
                .to_str()
                .context("source-owned observer spec path must be UTF-8")
        })
        .collect::<Result<Vec<_>>>()?
        .join("/");
    let committed = objects::blob_id(probe.path(), &git, &commit, &relative, &control)?;
    let blob = objects::object(
        probe.path(),
        &git,
        "blob",
        &committed,
        "blob",
        &control,
        super::observer::MAX_SPEC_BYTES,
    )?;
    // Compare the immutable loaded bytes without executing repository-defined
    // clean filters. Only built-in CRLF/LF equivalence is admitted; policy is
    // always parsed from the bounded committed blob, never converted input.
    ensure!(
        raw == blob || json_newlines(raw) == json_newlines(&blob),
        "source-owned observer spec differs from committed blob"
    );
    Ok(blob)
}

fn json_newlines(raw: &[u8]) -> Vec<u8> {
    raw.iter()
        .enumerate()
        .filter_map(|(index, &byte)| {
            (byte != b'\r' || raw.get(index + 1) != Some(&b'\n')).then_some(byte)
        })
        .collect()
}

/// Group execution input locations.
#[derive(Debug, Args)]
pub struct GroupArgs {
    /// Versioned JSON spec; omitted to use generated workflow environment lists.
    #[arg(long)]
    pub spec: Option<PathBuf>,
    /// Evidence root (must exist and have no previous receipt).
    #[arg(long)]
    pub results: Option<PathBuf>,
}

/// Dispatch through the same public library used by native consumers.
pub fn run(command: Command) -> Result<()> {
    match command {
        Command::LogReconcile(args) => super::log_reconcile::run(args),
        Command::ObserveOriginal(args) => {
            let cancellation = cancellation()?;
            let (spec, identity, mut transport) =
                observer_inputs(&args.spec, &args.results, &cancellation.token, false)?;
            super::observer::observe(
                &spec,
                &identity,
                &args.results,
                &mut transport,
                &cancellation.token,
            )?;
            Ok(())
        }
        Command::VerifyOriginalRetention(args) => {
            let cancellation = cancellation()?;
            let (spec, identity, mut transport) =
                observer_inputs(&args.spec, &args.results, &cancellation.token, true)?;
            let receipt = super::observer::verify_retention(
                &spec,
                &identity,
                &args.results,
                args.artifact_id,
                &args.artifact_digest,
                &mut transport,
                &cancellation.token,
            )?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
            Ok(())
        }
        Command::NixGroup(args) => {
            let cancellation = super::cancellation::Cancellation::default();
            // This CLI exits after its one command. Retain handlers for that entire
            // lifetime; unregistering signal-hook actions does not restore defaults.
            // The library never installs these process-global handlers.
            #[cfg(unix)]
            for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
                signal_hook::flag::register(signal, cancellation.0.clone())?;
            }
            let spec = match args.spec {
                Some(path) => super::group::read_json(&path, super::group::MAX_SPEC_BYTES)?,
                None => super::group::GroupSpec::from_environment()?,
            };
            let root = args
                .results
                .or_else(|| std::env::var_os("SIMIT_NIX_BUILD_RESULTS").map(PathBuf::from));
            let root = root.filter(|path| !path.as_os_str().is_empty()).ok_or_else(|| {
                anyhow::anyhow!(
                    "a nonempty results directory is required via --results or SIMIT_NIX_BUILD_RESULTS"
                )
            })?;
            let source = super::group::Source::from_environment()?;
            let receipt = super::group::execute_with(
                &spec,
                &source,
                &std::env::current_dir()?,
                &root,
                &mut super::process::CommandRunner,
                &cancellation,
            )?;
            ensure!(
                receipt.success == Some(true),
                "one or more grouped Nix members failed; see retained evidence"
            );
            Ok(())
        }
    }
}

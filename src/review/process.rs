//! All target/tool arguments are passed as argv, never shell programs.
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{Read, Seek},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) const UNTRUSTED_TOOL_ENV: &[&str] = &[
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
];

pub fn command(program: &str, args: &[String], cwd: Option<&Path>) -> Command {
    let mut c = Command::new(program);
    c.args(args).stdin(Stdio::null());
    if let Some(p) = cwd {
        c.current_dir(p);
    }
    if program != "gh" {
        for key in UNTRUSTED_TOOL_ENV {
            c.env_remove(key);
        }
    }
    c.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    c
}
pub fn run(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    log: Option<&Path>,
) -> Result<String> {
    run_command(command(program, args, cwd), log)
}

// The realization bound must fit inside the 360 minute hosted job.
const BUILD_TIMEOUT_SECONDS: u64 = 19800;
const _: () = assert!(BUILD_TIMEOUT_SECONDS < 360 * 60);

/// Realizing a target (for example headless VTK/ParaView) outlasts the 35 minute
/// default; bound it to fit inside the 360 minute hosted job, with a larger log.
pub fn run_build(args: &[String], log: Option<&Path>) -> Result<String> {
    run_command_with_limits(
        command("nix", args, None),
        log,
        CommandLimits {
            timeout: Duration::from_secs(BUILD_TIMEOUT_SECONDS),
            stdout_bytes: 16 * 1024 * 1024,
            stderr_bytes: 256 * 1024 * 1024,
        },
    )
}

#[derive(Clone, Copy)]
pub struct CommandLimits {
    pub timeout: Duration,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
}

pub fn run_command(c: Command, log: Option<&Path>) -> Result<String> {
    let program = c.get_program().to_string_lossy().into_owned();
    let timeout = if program == "gh" {
        60
    } else if program == "git" {
        300
    } else {
        2100
    };
    run_command_with_limits(
        c,
        log,
        CommandLimits {
            timeout: Duration::from_secs(timeout),
            stdout_bytes: 16 * 1024 * 1024,
            stderr_bytes: 64 * 1024 * 1024,
        },
    )
}

pub fn run_command_with_limits(
    mut c: Command,
    log: Option<&Path>,
    limits: CommandLimits,
) -> Result<String> {
    ensure!(
        !limits.timeout.is_zero() && limits.stdout_bytes > 0 && limits.stderr_bytes > 0,
        "process limits must be positive"
    );
    let deadline = Instant::now()
        .checked_add(limits.timeout)
        .context("process deadline exceeds the monotonic clock")?;
    let program = c.get_program().to_string_lossy().into_owned();
    let mut stdout = tempfile::tempfile()?;
    let stderr = if let Some(p) = log {
        File::create(p)?
    } else {
        tempfile::tempfile()?
    };
    c.stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?));
    let mut child = c
        .spawn()
        .with_context(|| format!("cannot start {program}"))?;
    let mut poll_interval = Duration::from_millis(1);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline
            || stdout.metadata()?.len() > limits.stdout_bytes
            || stderr.metadata()?.len() > limits.stderr_bytes
        {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("{program} exceeded time/output limits");
        }
        std::thread::sleep(poll_interval.min(deadline.saturating_duration_since(Instant::now())));
        poll_interval = (poll_interval * 2).min(Duration::from_millis(100));
    };
    // Tool stderr can contain target-controlled strings or credentials: never echo it in a privileged job.
    ensure!(
        status.success(),
        "{program} failed ({:?}); see retained tool log",
        status.code()
    );
    ensure!(
        stdout.metadata()?.len() <= limits.stdout_bytes
            && stderr.metadata()?.len() <= limits.stderr_bytes,
        "tool output too large"
    );
    stdout.rewind()?;
    let mut text = String::new();
    stdout.read_to_string(&mut text)?;
    Ok(text)
}
pub fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}
pub fn nix_args(values: &[&str]) -> Vec<String> {
    let mut a = args(values);
    a.extend(args(&[
        "--option",
        "accept-flake-config",
        "false",
        "--option",
        "allow-import-from-derivation",
        "false",
        "--option",
        "builders",
        "",
        "--option",
        "max-jobs",
        "2",
        "--option",
        "cores",
        "2",
        "--option",
        "max-silent-time",
        "600",
        "--option",
        "timeout",
        "18000",
        "--option",
        "eval-cache",
        "false",
    ]));
    a
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn build_bounds_fit_inside_the_hosted_review_job() {
        let job = include_str!("assets/review-repository.yml");
        assert!(job.contains("    timeout-minutes: 360\n"));
        assert!(nix_args(&[]).windows(2).any(|w| w == ["timeout", "18000"]));
    }

    fn limits() -> CommandLimits {
        CommandLimits {
            timeout: Duration::from_secs(2),
            stdout_bytes: 64,
            stderr_bytes: 64,
        }
    }

    #[test]
    fn account_process_bounds_reject_completed_stdout_and_stderr_overflow() {
        for script in ["printf '%080d' 0", "printf '%080d' 0 >&2"] {
            let result = run_command_with_limits(
                command("/bin/sh", &args(&["-c", script]), None),
                None,
                limits(),
            );
            assert!(result.is_err(), "accepted oversized tool output: {script}");
        }
        assert_eq!(
            run_command_with_limits(
                command("/bin/sh", &args(&["-c", "printf '1001\\n'"]), None),
                None,
                limits(),
            )
            .unwrap(),
            "1001\n"
        );
    }

    #[test]
    fn account_process_deadline_terminates_a_stalled_lookup() {
        let start = Instant::now();
        let result = run_command_with_limits(
            command("/bin/sh", &args(&["-c", "exec sleep 30"]), None),
            None,
            CommandLimits {
                timeout: Duration::from_millis(50),
                ..limits()
            },
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("time/output limits")
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

//! Byte-preserving member process execution; nonzero statuses remain evidence.
use super::cancellation::Cancellation;
use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// Injectable process boundary for native contracts and library consumers.
pub trait Runner {
    /// Run one command with retained byte streams, checking cancellation before dispatch.
    fn run(
        &mut self,
        command: &mut Command,
        stdout: &Path,
        stderr: &Path,
        cancellation: &Cancellation,
    ) -> Result<i32>;
}

/// Production process implementation with owned process-group cleanup.
#[derive(Default)]
pub struct CommandRunner;
impl Runner for CommandRunner {
    fn run(
        &mut self,
        command: &mut Command,
        stdout: &Path,
        stderr: &Path,
        cancellation: &Cancellation,
    ) -> Result<i32> {
        run_bounded(
            command,
            stdout,
            stderr,
            cancellation,
            Limits::default(),
            None,
        )
    }
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = rustix::process::Pid::from_raw(self.0.id() as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Limits {
    timeout: Duration,
    stdout: u64,
    stderr: u64,
}

pub(super) fn run_with_input(
    command: &mut Command,
    input: File,
    stdout: &Path,
    stderr: &Path,
    cancellation: &Cancellation,
) -> Result<i32> {
    run_bounded(
        command,
        stdout,
        stderr,
        cancellation,
        Limits {
            stdout: 32 * 1024 * 1024,
            ..Limits::default()
        },
        Some(input),
    )
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(6 * 60 * 60),
            stdout: 16 * 1024 * 1024,
            stderr: 64 * 1024 * 1024,
        }
    }
}

fn run_bounded(
    command: &mut Command,
    stdout_path: &Path,
    stderr_path: &Path,
    cancellation: &Cancellation,
    limits: Limits,
    input: Option<File>,
) -> Result<i32> {
    cancellation.check()?;
    let stdout = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stdout_path)?;
    let mut stderr = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stderr_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .stdin(input.map_or_else(Stdio::null, Stdio::from))
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    let result = (|| {
        let mut child = ChildGuard(command.spawn().context("cannot dispatch group process")?);
        let deadline = Instant::now()
            .checked_add(limits.timeout)
            .context("invalid process deadline")?;
        loop {
            cancellation.check()?;
            ensure!(
                stdout.metadata()?.len() <= limits.stdout
                    && stderr.metadata()?.len() <= limits.stderr,
                "group process exceeded output limits"
            );
            if let Some(status) = child.0.try_wait()? {
                ensure!(
                    stdout.metadata()?.len() <= limits.stdout
                        && stderr.metadata()?.len() <= limits.stderr,
                    "completed group process exceeded output limits"
                );
                return Ok(status.code().unwrap_or(128));
            }
            ensure!(
                Instant::now() < deadline,
                "group process exceeded time limit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    if let Err(error) = &result {
        writeln!(stderr, "simit: {error}")?;
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn nonzero_output_bytes_are_retained_and_overflow_is_denied() {
        for (script, expected) in [
            ("printf raw; printf diagnostic >&2; exit 7", Some(7)),
            ("printf '%0100d' 0", None),
        ] {
            let root = tempfile::tempdir().unwrap();
            let result = run_bounded(
                Command::new("sh").args(["-c", script]),
                &root.path().join("stdout"),
                &root.path().join("stderr"),
                &Cancellation::default(),
                Limits {
                    timeout: Duration::from_secs(2),
                    stdout: 32,
                    stderr: 64,
                },
                None,
            );
            assert_eq!(result.ok(), expected);
            assert!(root.path().join("stdout").is_file());
        }
    }
    #[test]
    fn deadline_kills_descendants_not_just_the_shell() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("late-effect");
        let start = Instant::now();
        let result = run_bounded(
            Command::new("sh")
                .args(["-c", "(sleep 0.2; touch \"$MARKER\") & wait"])
                .env("MARKER", &marker),
            &root.path().join("stdout"),
            &root.path().join("stderr"),
            &Cancellation::default(),
            Limits {
                timeout: Duration::from_millis(50),
                stdout: 32,
                stderr: 64,
            },
            None,
        );
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        std::thread::sleep(Duration::from_millis(250));
        assert!(!marker.exists());
    }

    #[test]
    fn descriptor_input_is_not_replaced_with_null_and_output_is_retained() {
        use std::io::Seek;
        let root = tempfile::tempdir().unwrap();
        let mut input = tempfile::tempfile().unwrap();
        input.write_all(b"\0binary\xff").unwrap();
        input.rewind().unwrap();
        let status = run_with_input(
            Command::new("sh").args(["-c", "cat; printf diagnostic >&2"]),
            input,
            &root.path().join("stdout"),
            &root.path().join("stderr"),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(status, 0);
        assert_eq!(
            std::fs::read(root.path().join("stdout")).unwrap(),
            b"\0binary\xff"
        );
        assert_eq!(
            std::fs::read(root.path().join("stderr")).unwrap(),
            b"diagnostic"
        );
    }
}

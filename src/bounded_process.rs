//! Shared byte-preserving execution with caller-selected bounds.
//!
//! Owns a process group on Unix, not a containment boundary for descendants
//! that escape that group. Runtime managers must additionally clean their cgroup.
use crate::cancellation::Cancellation;
use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
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

struct ChildGuard<'a>(Child, &'a AtomicBool);
impl Drop for ChildGuard<'_> {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = rustix::process::Pid::from_raw(self.0.id() as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
        self.1.store(true, Ordering::SeqCst);
    }
}

/// Hard bounds for one process, including retained diagnostics.
pub struct Limits {
    /// Monotonic execution deadline relative to dispatch.
    pub timeout: Duration,
    /// Maximum retained stdout bytes.
    pub stdout: u64,
    /// Maximum retained stderr bytes, including runner diagnostics.
    pub stderr: u64,
}

/// Terminal process failures; callers never classify diagnostic text.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProcessError {
    /// Bounds must describe a representable, nonzero execution interval.
    #[error("invalid process limits: {0}")]
    InvalidLimits(&'static str),
    /// Nonblocking owned-process cleanup is supported on Unix platforms.
    #[error("bounded runtime execution requires Unix")]
    UnsupportedPlatform,
    /// Caller requested cancellation.
    #[error("group execution cancelled")]
    Cancelled,
    /// The caller's monotonic deadline expired.
    #[error("group process exceeded time limit")]
    Timeout,
    /// A byte stream exceeded its admitted budget.
    #[error("group process exceeded output limits")]
    OutputLimit,
    /// Dispatch, capture or evidence I/O failed. This is never retryable denial.
    #[error("process execution failed: {0}")]
    Execution(#[source] anyhow::Error),
}

/// Execute one probe with explicit bounds; nonzero exits remain typed statuses.
pub fn execute(
    command: &mut Command,
    stdout: &Path,
    stderr: &Path,
    cancellation: &Cancellation,
    limits: Limits,
) -> std::result::Result<i32, ProcessError> {
    if !cfg!(unix) {
        return Err(ProcessError::UnsupportedPlatform);
    }
    if limits.timeout.is_zero() || Instant::now().checked_add(limits.timeout).is_none() {
        return Err(ProcessError::InvalidLimits(
            "timeout must be nonzero and representable",
        ));
    }
    run_bounded(command, stdout, stderr, cancellation, limits, None).map_err(|error| {
        if error.is::<crate::cancellation::Cancelled>() {
            ProcessError::Cancelled
        } else {
            match error.downcast::<ProcessError>() {
                Ok(error) => error,
                Err(error) => ProcessError::Execution(error),
            }
        }
    })
}

pub(crate) fn run_with_input(
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

// Source authorization keeps the existing portable CI runner, not the public
// Unix-only runtime-admission contract. Its caller owns the shared preflight clock.
pub(crate) fn run_probe(
    command: &mut Command,
    stdout: &Path,
    stderr: &Path,
    cancellation: &Cancellation,
    timeout: Duration,
    maximum: u64,
) -> Result<i32> {
    ensure!(
        timeout <= Duration::from_secs(30),
        "probe timeout exceeds bound"
    );
    run_bounded(
        command,
        stdout,
        stderr,
        cancellation,
        Limits {
            timeout,
            stdout: maximum,
            stderr: 65536,
        },
        None,
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

/// Reuse bounded process ownership for read-only provider/signature queries.
pub(crate) fn query(command: &mut Command, timeout: Duration, maximum: u64) -> Result<Vec<u8>> {
    let root = tempfile::tempdir()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    }
    let stdout = root.path().join("stdout");
    let stderr = root.path().join("stderr");
    let status = run_probe(
        command,
        &stdout,
        &stderr,
        &Cancellation::default(),
        timeout.min(Duration::from_secs(30)),
        maximum,
    )?;
    // Child diagnostics can contain URLs or credentials. Do not copy them into reports.
    ensure!(
        status == 0,
        "read-only evidence query failed (exit {status})"
    );
    Ok(std::fs::read(stdout)?)
}

/// Credential stdout stays in zeroized memory. Stderr is bounded and discarded.
#[cfg(unix)]
pub(crate) fn query_secret(
    command: &mut Command,
    timeout: Duration,
) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
    use std::os::unix::process::CommandExt;
    let stopped = AtomicBool::new(false);
    let deadline = Instant::now()
        .checked_add(timeout)
        .context("invalid credential deadline")?;
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut raw = zeroize::Zeroizing::new(Vec::new());
    let failed = AtomicBool::new(false);
    std::thread::scope(|scope| -> Result<()> {
        let mut child = ChildGuard(
            command
                .spawn()
                .context("cannot query provider credential")?,
            &stopped,
        );
        let output = child.0.stdout.take().context("missing credential stdout")?;
        let errors = child.0.stderr.take().context("missing credential stderr")?;
        fcntl_setfl(&output, fcntl_getfl(&output)? | OFlags::NONBLOCK)?;
        fcntl_setfl(&errors, fcntl_getfl(&errors)? | OFlags::NONBLOCK)?;
        let stdout_reader = std::thread::Builder::new().spawn_scoped(scope, || {
            let result = capture(output, &mut *raw, 4096, &stopped);
            if result.is_err() {
                failed.store(true, Ordering::SeqCst);
            }
            result
        })?;
        let stderr_reader = std::thread::Builder::new().spawn_scoped(scope, || {
            let result = capture(errors, std::io::sink(), 64 * 1024, &stopped);
            if result.is_err() {
                failed.store(true, Ordering::SeqCst);
            }
            result
        })?;
        let outcome = (|| {
            loop {
                ensure!(
                    !failed.load(Ordering::SeqCst),
                    "credential query output limit exceeded"
                );
                ensure!(
                    Instant::now() < deadline,
                    "credential query deadline exhausted"
                );
                if let Some(status) = child.0.try_wait()? {
                    ensure!(status.success(), "provider credential query failed");
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })();
        drop(child);
        let stdout_result = stdout_reader
            .join()
            .map_err(|_| anyhow::anyhow!("credential reader failed"))?;
        let stderr_result = stderr_reader
            .join()
            .map_err(|_| anyhow::anyhow!("credential reader failed"))?;
        outcome?;
        stdout_result?;
        stderr_result?;
        ensure!(
            Instant::now() < deadline,
            "credential query deadline exhausted"
        );
        Ok(())
    })?;
    Ok(raw)
}

#[cfg(not(unix))]
pub(crate) fn query_secret(_: &mut Command, _: Duration) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    anyhow::bail!("bounded in-memory credential queries require Unix process ownership")
}

fn run_bounded(
    command: &mut Command,
    stdout_path: &Path,
    stderr_path: &Path,
    cancellation: &Cancellation,
    limits: Limits,
    input: Option<File>,
) -> Result<i32> {
    run_bounded_with_readers(
        command,
        stdout_path,
        stderr_path,
        cancellation,
        limits,
        input,
        &mut Readers::default(),
    )
}

#[derive(Default)]
struct Readers {
    #[cfg(test)]
    fail_after: Option<usize>,
    #[cfg(test)]
    started: usize,
}

impl Readers {
    fn start<'scope, 'env: 'scope>(
        &mut self,
        scope: &'scope std::thread::Scope<'scope, 'env>,
        input: impl Read + Send + 'scope,
        output: File,
        maximum: u64,
        signals: (&'scope AtomicBool, &'scope AtomicBool),
    ) -> Result<std::thread::ScopedJoinHandle<'scope, Result<()>>> {
        #[cfg(test)]
        {
            ensure!(
                self.fail_after != Some(self.started),
                "cannot start output capture reader"
            );
            self.started += 1;
        }
        let (stopped, failed) = signals;
        std::thread::Builder::new()
            .spawn_scoped(scope, move || {
                let result = capture(input, output, maximum, stopped);
                if result.is_err() {
                    failed.store(true, Ordering::SeqCst);
                }
                result
            })
            .context("cannot start output capture reader")
    }
}

fn run_bounded_with_readers(
    command: &mut Command,
    stdout_path: &Path,
    stderr_path: &Path,
    cancellation: &Cancellation,
    limits: Limits,
    input: Option<File>,
    readers: &mut Readers,
) -> Result<i32> {
    cancellation.check()?;
    let deadline = Instant::now()
        .checked_add(limits.timeout)
        .context("invalid process deadline")?;
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let result = {
        let stopped = AtomicBool::new(false);
        let capture_failed = AtomicBool::new(false);
        std::thread::scope(|scope| {
            cancellation.check()?;
            let mut child = ChildGuard(
                command.spawn().context("cannot dispatch group process")?,
                &stopped,
            );
            let output = child.0.stdout.take().context("missing stdout pipe")?;
            let errors = child.0.stderr.take().context("missing stderr pipe")?;
            #[cfg(unix)]
            {
                // Nonblocking reads allow cleanup even when an escaped descendant
                // retains a pipe. Readers drain available bytes before stopping.
                use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
                fcntl_setfl(&output, fcntl_getfl(&output)? | OFlags::NONBLOCK)?;
                fcntl_setfl(&errors, fcntl_getfl(&errors)? | OFlags::NONBLOCK)?;
            }
            let stdout_file = stdout.try_clone()?;
            let stderr_file = stderr.try_clone()?;
            let stdout_reader = readers.start(
                scope,
                output,
                stdout_file,
                limits.stdout,
                (&stopped, &capture_failed),
            )?;
            let stderr_reader = readers.start(
                scope,
                errors,
                stderr_file,
                limits.stderr,
                (&stopped, &capture_failed),
            )?;
            let outcome = (|| {
                loop {
                    cancellation.check()?;
                    ensure!(
                        !capture_failed.load(Ordering::SeqCst),
                        "group process output capture failed"
                    );
                    // Completion is only admitted when observed within the
                    // deadline, not merely because the next poll sees an exit.
                    if Instant::now() >= deadline {
                        return Err(ProcessError::Timeout.into());
                    }
                    if let Some(status) = child.0.try_wait()? {
                        #[cfg(unix)]
                        let signal_status = {
                            use std::os::unix::process::ExitStatusExt;
                            status.signal().map(|signal| -signal)
                        };
                        #[cfg(not(unix))]
                        let signal_status = None;
                        return Ok(status.code().or(signal_status).unwrap_or(128));
                    }
                    std::thread::sleep(
                        deadline
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(20)),
                    );
                }
            })();
            // Kill owned descendants before joining readers; inherited pipes
            // must never prevent cancellation or deadline cleanup.
            drop(child);
            let stdout_result = stdout_reader
                .join()
                .map_err(|_| anyhow::anyhow!("stdout capture thread panicked"))?;
            let stderr_result = stderr_reader
                .join()
                .map_err(|_| anyhow::anyhow!("stderr capture thread panicked"))?;
            if outcome
                .as_ref()
                .is_err_and(|error: &anyhow::Error| error.is::<crate::cancellation::Cancelled>())
            {
                return outcome;
            }
            stdout_result?;
            stderr_result?;
            if outcome.is_ok() && Instant::now() >= deadline {
                return Err(ProcessError::Timeout.into());
            }
            outcome
        })
    };
    if let Err(error) = &result {
        // Diagnostics share the same hard stderr budget as child bytes.
        let diagnostic = format!("simit: {error}\n");
        let remaining = limits.stderr.saturating_sub(stderr.metadata()?.len());
        let length = diagnostic
            .len()
            .min(remaining.try_into().unwrap_or(usize::MAX));
        stderr.write_all(&diagnostic.as_bytes()[..length])?;
    }
    result
}

fn capture(
    mut input: impl Read,
    mut output: impl Write,
    maximum: u64,
    stopped: &AtomicBool,
) -> Result<()> {
    let mut remaining = maximum;
    let mut buffer = zeroize::Zeroizing::new([0; 64 * 1024]);
    loop {
        // Observe cleanup before the read. If cleanup races a WouldBlock read,
        // retry once after observing it so bytes queued before child exit cannot
        // be discarded merely because EOF is held open by a descendant.
        let finished = stopped.load(Ordering::SeqCst);
        let count = match input.read(&mut *buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if finished {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let admitted = count.min(remaining.try_into().unwrap_or(usize::MAX));
        output.write_all(&buffer[..admitted])?;
        remaining -= admitted as u64;
        if admitted != count {
            return Err(ProcessError::OutputLimit.into());
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{os::fd::OwnedFd, os::unix::net::UnixStream};

    #[test]
    fn credential_queries_bound_both_streams_and_deadlines_without_exposing_bytes() {
        for (script, timeout, expected) in [
            ("printf inert-secret", Duration::from_secs(2), true),
            ("printf inert-secret; exit 7", Duration::from_secs(2), false),
            ("head -c 8192 /dev/zero", Duration::from_secs(2), false),
            (
                "head -c 131072 /dev/zero >&2",
                Duration::from_secs(2),
                false,
            ),
            ("sleep 10", Duration::from_millis(20), false),
        ] {
            let start = Instant::now();
            let result = query_secret(Command::new("sh").args(["-c", script]), timeout);
            assert_eq!(result.is_ok(), expected, "{script}");
            match result {
                Ok(raw) => assert_eq!(&*raw, b"inert-secret"),
                Err(error) => assert!(!format!("{error:#}").contains("inert-secret")),
            }
            assert!(start.elapsed() < Duration::from_secs(2));
        }
    }

    #[test]
    fn cleanup_racing_a_would_block_read_does_not_drop_queued_bytes() {
        struct ExitRace<'a> {
            stopped: &'a AtomicBool,
            reads: usize,
        }
        impl Read for ExitRace<'_> {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                self.reads += 1;
                if self.reads == 1 {
                    self.stopped.store(true, Ordering::SeqCst);
                    return Err(std::io::ErrorKind::WouldBlock.into());
                }
                if self.reads > 2 {
                    return Ok(0);
                }
                bytes[..3].copy_from_slice(b"\xff\0x");
                Ok(3)
            }
        }
        let stopped = AtomicBool::new(false);
        let output = tempfile::tempfile().unwrap();
        let mut retained = output.try_clone().unwrap();
        capture(
            ExitRace {
                stopped: &stopped,
                reads: 0,
            },
            output,
            3,
            &stopped,
        )
        .unwrap();
        use std::io::{Seek, SeekFrom};
        retained.seek(SeekFrom::Start(0)).unwrap();
        let mut raw = Vec::new();
        retained.read_to_end(&mut raw).unwrap();
        assert_eq!(raw, b"\xff\0x");
    }

    #[test]
    fn sustained_concurrent_streams_cancel_and_retain_diagnostics() {
        let root = tempfile::tempdir().unwrap();
        let token = Cancellation::default();
        let marker = root.path().join("started");
        let late = root.path().join("late-effect");
        let (mut peer, input) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        std::thread::scope(|scope| {
            let run = scope.spawn(|| run_bounded(
                Command::new("sh").args(["-c", "exec 3<&0; printf '\\377\\000diagnostic' >&2; touch \"$MARKER\"; (cat <&3; touch \"$LATE\") & (while :; do printf 'abcdefgh'; sleep 0.01; done) & while :; do printf 'errors' >&2; sleep 0.01; done"])
                    .env("MARKER", &marker).env("LATE", &late),
                &root.path().join("stdout"), &root.path().join("stderr"), &token,
                Limits { timeout: Duration::from_secs(2), stdout: 65536, stderr: 65536 }, Some(File::from(OwnedFd::from(input))),
            ));
            let deadline = Instant::now() + Duration::from_secs(1);
            while !marker.exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            token.cancel();
            assert!(
                run.join()
                    .unwrap()
                    .unwrap_err()
                    .is::<crate::cancellation::Cancelled>()
            );
        });
        let stderr = std::fs::read(root.path().join("stderr")).unwrap();
        assert!(stderr.starts_with(b"\xff\0diagnostic"));
        assert!(
            stderr
                .windows(b"cancelled".len())
                .any(|bytes| bytes == b"cancelled")
        );
        assert!(stderr.len() <= 65536);
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        assert!(!late.exists());
    }

    #[test]
    fn escaped_pipe_fixture() {
        if let Some(marker) = std::env::var_os("SIMIT_ESCAPED_PIPE_MARKER") {
            rustix::process::setsid().unwrap();
            std::fs::write(marker, std::process::id().to_string()).unwrap();
            std::thread::sleep(Duration::from_secs(10));
        }
    }

    #[test]
    fn inherited_pipe_descriptors_do_not_require_eof_after_child_exit() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("escaped-pid");
        let start = Instant::now();
        let result = run_bounded(
            Command::new("sh").args(["-c", "\"$TEST_EXE\" --exact bounded_process::tests::escaped_pipe_fixture --nocapture & while test ! -s \"$SIMIT_ESCAPED_PIPE_MARKER\"; do sleep 0.01; done; printf '\\377\\000retained'"])
                .env("TEST_EXE", std::env::current_exe().unwrap()).env("SIMIT_ESCAPED_PIPE_MARKER", &marker),
            &root.path().join("stdout"), &root.path().join("stderr"), &Cancellation::default(),
            Limits { timeout: Duration::from_secs(2), stdout: 65536, stderr: 65536 }, None,
        );
        // The escaped fixture is deliberately outside our process group. Clean
        // it up explicitly after proving the runner did not wait for its EOF.
        let pid = std::fs::read_to_string(marker).unwrap().parse().unwrap();
        let _ = rustix::process::kill_process(
            rustix::process::Pid::from_raw(pid).unwrap(),
            rustix::process::Signal::KILL,
        );
        assert_eq!(result.unwrap(), 0);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(
            std::fs::read(root.path().join("stdout"))
                .unwrap()
                .ends_with(b"\xff\0retained")
        );
    }
    #[test]
    fn partial_reader_start_failure_returns_error_and_cleans_up() {
        for fail_after in [0, 1] {
            let root = tempfile::tempdir().unwrap();
            let marker = root.path().join("late-effect");
            let (mut peer, input) = UnixStream::pair().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let start = Instant::now();
            let result = run_bounded_with_readers(
                Command::new("sh")
                    .args(["-c", "cat; touch \"$MARKER\""])
                    .env("MARKER", &marker),
                &root.path().join("stdout"),
                &root.path().join("stderr"),
                &Cancellation::default(),
                Limits {
                    timeout: Duration::from_secs(2),
                    stdout: 32,
                    stderr: 256,
                },
                Some(File::from(OwnedFd::from(input))),
                &mut Readers {
                    fail_after: Some(fail_after),
                    started: 0,
                },
            );
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("cannot start output capture reader")
            );
            assert!(start.elapsed() < Duration::from_secs(1));
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
            assert!(!marker.exists());
        }
    }

    #[test]
    fn child_cleanup_stops_a_reader_even_without_pipe_eof() {
        struct Pending;
        impl Read for Pending {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::WouldBlock.into())
            }
        }
        let stopped = AtomicBool::new(false);
        let output = tempfile::tempfile().unwrap();
        std::thread::scope(|scope| {
            use std::os::unix::process::CommandExt;
            let reader = scope.spawn(|| capture(Pending, output, 32, &stopped));
            let child = ChildGuard(
                Command::new("sh")
                    .args(["-c", "sleep 10"])
                    .process_group(0)
                    .spawn()
                    .unwrap(),
                &stopped,
            );
            drop(child);
            assert!(stopped.load(Ordering::SeqCst));
            reader.join().unwrap().unwrap();
        });
    }
    #[test]
    fn rapid_output_is_capped_before_evidence_files_grow() {
        for script in [
            "head -c 262144 /dev/zero",
            "head -c 262144 /dev/zero >&2",
            "head -c 262144 /dev/zero & head -c 262144 /dev/zero >&2; wait",
        ] {
            let root = tempfile::tempdir().unwrap();
            let stdout = root.path().join("stdout");
            let stderr = root.path().join("stderr");
            let result = run_bounded(
                Command::new("sh").args(["-c", script]),
                &stdout,
                &stderr,
                &Cancellation::default(),
                Limits {
                    timeout: Duration::from_secs(2),
                    stdout: 32,
                    stderr: 64,
                },
                None,
            );
            assert!(result.is_err());
            assert!(std::fs::metadata(&stdout).unwrap().len() <= 32);
            assert!(std::fs::metadata(&stderr).unwrap().len() <= 64);
        }
    }
    #[test]
    fn both_pipes_drain_exact_limit_binary_output_after_child_exit() {
        let root = tempfile::tempdir().unwrap();
        let stdout = root.path().join("stdout");
        let stderr = root.path().join("stderr");
        let status = run_bounded(
            Command::new("sh").args([
                "-c",
                "head -c 131072 /dev/zero & head -c 131072 /dev/zero >&2; wait; exit 7",
            ]),
            &stdout,
            &stderr,
            &Cancellation::default(),
            Limits {
                timeout: Duration::from_secs(2),
                stdout: 131072,
                stderr: 131072,
            },
            None,
        )
        .unwrap();
        assert_eq!(status, 7);
        assert_eq!(std::fs::read(stdout).unwrap(), vec![0; 131072]);
        assert_eq!(std::fs::read(stderr).unwrap(), vec![0; 131072]);
    }
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
        // Keep the descendant blocked until cleanup. A timed side effect races
        // the supervisor's scheduling and can precede an otherwise correct kill.
        let (mut peer, input) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let start = Instant::now();
        let result = run_bounded(
            Command::new("sh")
                .args([
                    "-c",
                    "exec 3<&0; (printf ready; cat <&3; touch \"$MARKER\") & wait",
                ])
                .env("MARKER", &marker),
            &root.path().join("stdout"),
            &root.path().join("stderr"),
            &Cancellation::default(),
            Limits {
                timeout: Duration::from_secs(1),
                stdout: 32,
                stderr: 64,
            },
            Some(File::from(OwnedFd::from(input))),
        );
        assert!(matches!(
            result.unwrap_err().downcast_ref::<ProcessError>(),
            Some(ProcessError::Timeout)
        ));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(std::fs::read(root.path().join("stdout")).unwrap(), b"ready");
        // EOF proves all inherited socket holders exited, including the
        // background child; killing just the waiting shell cannot produce it.
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
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

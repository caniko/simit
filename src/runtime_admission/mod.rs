//! Bounded readiness observations, independent of policy and manager integration.
//!
//! Only explicitly enrolled nonzero exit statuses retry. Success is one fresh
//! observation, never a cached grant or an atomic continuity guarantee. Libraries
//! install no signal handlers; the caller owns cancellation and cgroup containment.

#[cfg(feature = "runtime-admission-cli")]
pub mod cli;
use crate::bounded_process::{Limits, ProcessError, execute};
pub use crate::cancellation::Cancellation;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Maximum serialized spec and receipt size. Admission validates before effects.
pub const MAX_EVIDENCE_BYTES: usize = 64 * 1024;

/// Caller-selected probe; no shell parsing or inherited environment is performed.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    /// Absolute executable path.
    pub executable: PathBuf,
    /// Exact argument vector, excluding the executable.
    pub args: Vec<String>,
    /// Complete environment; inherited loader and shell variables are scrubbed.
    pub environment: BTreeMap<String, String>,
    /// Absolute working directory.
    pub working_directory: PathBuf,
}

/// Versioned bounded input; use [`AdmissionSpec::validate`] before execution.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionSpec {
    /// Contract version, currently 1.
    pub schema_version: u32,
    /// Readiness probe, not a payload command.
    pub probe: Probe,
    /// Nonzero exit codes that indicate retryable policy denial.
    pub retryable_statuses: Vec<i32>,
    /// Maximum total attempts, including the first, at most 256.
    pub max_attempts: u32,
    /// Cancellable delay between denied attempts in milliseconds.
    pub backoff_ms: u64,
    /// Maximum execution time of each probe in milliseconds.
    pub attempt_timeout_ms: u64,
    /// Deadline for the entire invocation, including retries and backoff.
    pub overall_timeout_ms: u64,
    /// Aggregate stdout budget across every attempt.
    pub stdout_bytes: u64,
    /// Aggregate stderr and runner diagnostic budget across every attempt.
    pub stderr_bytes: u64,
}

/// Fully validated immutable input. Cannot be forged or modified by consumers.
pub struct ValidatedAdmissionSpec {
    spec: AdmissionSpec,
    sha256: String,
}

/// Input and evidence failures, distinct from the receipt's admission outcome.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AdmissionError {
    /// Input violates a versioned constraint.
    #[error("invalid admission spec: {0}")]
    InvalidSpec(&'static str),
    /// Input or receipt JSON is malformed.
    #[error("admission JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    /// Evidence or input I/O failed; admission cannot succeed.
    #[error("admission I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

impl AdmissionSpec {
    /// Validate the complete aggregate input before creating evidence or processes.
    pub fn validate(self) -> Result<ValidatedAdmissionSpec, AdmissionError> {
        let invalid = AdmissionError::InvalidSpec;
        if self.schema_version != 1 {
            return Err(invalid("unsupported schema_version"));
        }
        if !self.probe.executable.is_absolute() || !self.probe.working_directory.is_absolute() {
            return Err(invalid("executable and working_directory must be absolute"));
        }
        let paths = [&self.probe.executable, &self.probe.working_directory];
        if paths
            .iter()
            .any(|path| path.to_str().is_none_or(|value| value.contains('\0')))
        {
            return Err(invalid("paths must be UTF-8 without NUL"));
        }
        if self.probe.args.len() > 256 || self.probe.args.iter().any(|arg| arg.contains('\0')) {
            return Err(invalid("invalid argument vector"));
        }
        if self.probe.environment.len() > 256
            || self.probe.environment.iter().any(|(key, value)| {
                key.is_empty() || key.contains(['=', '\0']) || value.contains('\0')
            })
        {
            return Err(invalid("invalid explicit environment"));
        }
        if self.retryable_statuses.len() > 255
            || self
                .retryable_statuses
                .iter()
                .any(|status| !(1..=255).contains(status))
        {
            return Err(invalid("retryable statuses must be exit codes 1..255"));
        }
        if !(1..=256).contains(&self.max_attempts)
            || !(1..=3_600_000).contains(&self.overall_timeout_ms)
            || !(1..=self.overall_timeout_ms).contains(&self.attempt_timeout_ms)
            || self.backoff_ms > self.overall_timeout_ms
        {
            return Err(invalid("invalid attempt, deadline or backoff budget"));
        }
        if !(1..=8 * 1024 * 1024).contains(&self.stdout_bytes)
            || !(1..=8 * 1024 * 1024).contains(&self.stderr_bytes)
        {
            return Err(invalid("stream budgets must be 1..8 MiB"));
        }
        // Bound allocation before serialization, including callers using the Rust
        // API instead of the bounded JSON reader. Escaped JSON is checked below.
        let mut strings = self
            .probe
            .args
            .iter()
            .map(String::len)
            .chain(
                self.probe
                    .environment
                    .iter()
                    .flat_map(|(key, value)| [key.len(), value.len()]),
            )
            .chain(paths.iter().map(|path| path.as_os_str().len()));
        if strings
            .try_fold(0usize, |total, length| total.checked_add(length))
            .is_none_or(|length| length > MAX_EVIDENCE_BYTES)
        {
            return Err(invalid("aggregate probe input exceeds 64 KiB"));
        }
        let bytes = serde_json::to_vec(&self)?;
        if bytes.len() > MAX_EVIDENCE_BYTES {
            return Err(invalid("serialized input exceeds 64 KiB"));
        }
        Ok(ValidatedAdmissionSpec {
            spec: self,
            sha256: hex::encode(Sha256::digest(bytes)),
        })
    }

    /// Read at most the admitted input budget, rejecting unknown fields and tails.
    pub fn read(path: &Path) -> Result<ValidatedAdmissionSpec, AdmissionError> {
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(MAX_EVIDENCE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_EVIDENCE_BYTES {
            return Err(AdmissionError::InvalidSpec("input exceeds 64 KiB"));
        }
        serde_json::from_slice::<Self>(&bytes)?.validate()
    }
}

/// Terminal invocation classification. Only `Admitted` allows a pending start.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum AdmissionOutcome {
    /// A fresh successful probe completed within all bounds.
    Admitted,
    /// Policy denial was not enrolled for retry.
    Denied {
        /// Exact exit or negative signal status.
        status: i32,
    },
    /// Every admitted attempt was denied.
    AttemptsExhausted,
    /// The invocation's overall deadline expired.
    DeadlineExhausted,
    /// Aggregate retained bytes were exhausted.
    OutputExhausted,
    /// A per-probe deadline expired before the overall deadline.
    ProbeTimedOut,
    /// Caller cancelled; no subsequent probe may dispatch.
    Cancelled,
    /// Spawn, capture or process I/O failed, never a retryable policy denial.
    ProcessFailed,
}

/// Bounded evidence for one actual probe dispatch.
#[derive(Debug, Deserialize, Serialize)]
pub struct Attempt {
    /// One-based index; bytes are retained as `attempt-N.stdout` / `.stderr`.
    pub number: u32,
    /// Actual observation start, Unix milliseconds.
    pub started_unix_ms: u64,
    /// Actual observation completion, Unix milliseconds.
    pub finished_unix_ms: u64,
    /// Exact exit code or negative signal; absent for runner failure.
    pub status: Option<i32>,
    /// Retained stdout length.
    pub stdout_bytes: u64,
    /// Retained stderr length including runner diagnostics.
    pub stderr_bytes: u64,
}

/// Invocation evidence; it is an observation, not authorization or a durable grant.
#[derive(Debug, Deserialize, Serialize)]
pub struct AdmissionReceipt {
    /// Receipt contract version.
    pub schema_version: u32,
    /// SHA-256 of validated canonical JSON; avoids retaining environment values.
    pub spec_sha256: String,
    /// Invocation start, Unix milliseconds.
    pub started_unix_ms: u64,
    /// Invocation finish, Unix milliseconds.
    pub finished_unix_ms: u64,
    /// Monotonic elapsed milliseconds.
    pub elapsed_ms: u64,
    /// All dispatched probes, at most 256.
    pub attempts: Vec<Attempt>,
    /// Terminal outcome.
    pub outcome: AdmissionOutcome,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Observe readiness using a new, private evidence directory for each invocation.
///
/// An exhausted invocation never resumes on later restoration. The caller must
/// request a new invocation with a new directory. The caller also owns cleanup
/// of escaped descendants via its runtime-manager cgroup.
pub fn admit(
    validated: &ValidatedAdmissionSpec,
    evidence: &Path,
    cancellation: &Cancellation,
) -> Result<AdmissionReceipt, AdmissionError> {
    let spec = &validated.spec;
    let start = Instant::now();
    let deadline = start + Duration::from_millis(spec.overall_timeout_ms);
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(evidence)?;
    let mut receipt = AdmissionReceipt {
        schema_version: 1,
        spec_sha256: validated.sha256.clone(),
        started_unix_ms: now_ms(),
        finished_unix_ms: 0,
        elapsed_ms: 0,
        attempts: Vec::new(),
        outcome: AdmissionOutcome::AttemptsExhausted,
    };
    let mut stdout_left = spec.stdout_bytes;
    let mut stderr_left = spec.stderr_bytes;
    for number in 1..=spec.max_attempts {
        if cancellation.check().is_err() {
            receipt.outcome = AdmissionOutcome::Cancelled;
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            receipt.outcome = AdmissionOutcome::DeadlineExhausted;
            break;
        }
        if stdout_left == 0 || stderr_left == 0 {
            receipt.outcome = AdmissionOutcome::OutputExhausted;
            break;
        }
        let stdout = evidence.join(format!("attempt-{number}.stdout"));
        let stderr = evidence.join(format!("attempt-{number}.stderr"));
        let began = now_ms();
        let result = execute(
            Command::new(&spec.probe.executable)
                .args(&spec.probe.args)
                .env_clear()
                .envs(&spec.probe.environment)
                .current_dir(&spec.probe.working_directory),
            &stdout,
            &stderr,
            cancellation,
            Limits {
                timeout: remaining.min(Duration::from_millis(spec.attempt_timeout_ms)),
                stdout: stdout_left,
                stderr: stderr_left,
            },
        );
        // Cancellation can race the runner's pre-dispatch check, which creates no files.
        if matches!(&result, Err(ProcessError::Cancelled)) && !stdout.exists() {
            receipt.outcome = AdmissionOutcome::Cancelled;
            break;
        }
        let out_bytes = fs::metadata(&stdout)?.len();
        let err_bytes = fs::metadata(&stderr)?.len();
        stdout_left = stdout_left.saturating_sub(out_bytes);
        stderr_left = stderr_left.saturating_sub(err_bytes);
        receipt.attempts.push(Attempt {
            number,
            started_unix_ms: began,
            finished_unix_ms: now_ms(),
            status: result.as_ref().ok().copied(),
            stdout_bytes: out_bytes,
            stderr_bytes: err_bytes,
        });
        receipt.outcome = match result {
            Err(ProcessError::Cancelled) => AdmissionOutcome::Cancelled,
            Err(ProcessError::OutputLimit) => AdmissionOutcome::OutputExhausted,
            Err(ProcessError::Timeout) if Instant::now() >= deadline => {
                AdmissionOutcome::DeadlineExhausted
            }
            Err(ProcessError::Timeout) => AdmissionOutcome::ProbeTimedOut,
            Err(_) => AdmissionOutcome::ProcessFailed,
            Ok(_) if cancellation.check().is_err() => AdmissionOutcome::Cancelled,
            Ok(_) if Instant::now() >= deadline => AdmissionOutcome::DeadlineExhausted,
            Ok(0) => AdmissionOutcome::Admitted,
            Ok(status) if !spec.retryable_statuses.contains(&status) => {
                AdmissionOutcome::Denied { status }
            }
            Ok(_) => AdmissionOutcome::AttemptsExhausted,
        };
        if receipt.outcome != AdmissionOutcome::AttemptsExhausted || number == spec.max_attempts {
            break;
        }
        let wake = (Instant::now() + Duration::from_millis(spec.backoff_ms)).min(deadline);
        while Instant::now() < wake && cancellation.check().is_ok() {
            std::thread::sleep(
                wake.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(10)),
            );
        }
    }
    receipt.finished_unix_ms = now_ms();
    receipt.elapsed_ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let bytes = serde_json::to_vec(&receipt)?;
    if bytes.len() > MAX_EVIDENCE_BYTES {
        return Err(AdmissionError::InvalidSpec("receipt exceeds 64 KiB"));
    }
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(evidence.join("receipt.json"))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(receipt)
}

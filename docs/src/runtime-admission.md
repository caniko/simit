# Runtime admission

Simit's `runtime_admission` library runs bounded readiness probes. Use the
published library directly through Cargo or run its standalone
`simit runtime admit` adapter. Consumers such as canix-toolbelt own networking
policy and systemd integration.

## Contract

Each invocation has its own attempt count, monotonic overall deadline,
per-probe deadline, cancellable backoff, and aggregate stdout/stderr budgets.
Only explicitly enrolled nonzero exit codes are retryable; diagnostic text never
affects retry eligibility. Cancellation, probe hangs, byte overflow,
dispatch/capture failure and unenrolled denial are terminal.

Success records one fresh observation, not a durable grant, cached result, or
atomic guarantee of readiness at workload start. Every invocation dispatches
a fresh probe. Recovery resumes a pending invocation only within that
invocation's remaining budget; once that budget is exhausted, a new start
request requires a new invocation.

## Version-1 input

```json
{
  "schema_version": 1,
  "probe": {
    "executable": "/absolute/path/to/readiness-probe",
    "args": ["ready"],
    "environment": {},
    "working_directory": "/"
  },
  "retryable_statuses": [75],
  "max_attempts": 30,
  "backoff_ms": 1000,
  "attempt_timeout_ms": 2000,
  "overall_timeout_ms": 60000,
  "stdout_bytes": 65536,
  "stderr_bytes": 65536
}
```

Use absolute executable and working directory paths and explicit argument and
environment vectors. The runner clears inherited environment variables and
never parses shell commands. Callers that explicitly select a shell own its
command policy. Receipts omit probe environment values. The runner rejects
unknown fields, unsupported versions, NULs and oversized input before execution.

Limits: 64 KiB input, 1 byte to 8 MiB aggregate per stream, 256 attempts, and
one hour total execution. The per-probe deadline cannot exceed the overall
deadline. The runner clamps each probe to the invocation's remaining time and
per-stream byte budgets.

```sh
simit runtime admit --spec /path/to/spec.json --evidence /run/my-gate/invocation-1
```

Use a new evidence directory under an existing parent. On Unix the runner
creates it with mode 0700. It saves probe streams as `attempt-N.stdout` and
`attempt-N.stderr`. The `receipt.json` file (max 64 KiB) records input SHA-256,
actual observation timestamps, monotonic elapsed time, all attempt statuses
and retained lengths, and a typed terminal outcome. The CLI prints the receipt
as JSON and succeeds only for `admitted`. An existing evidence directory is
an error, not a cached success.

Call `AdmissionSpec::validate` and `admit`. Disabling the default
`runtime-admission-cli` feature leaves the library API available. The library
installs no process-global signal handlers; the CLI owns SIGINT/SIGTERM
handlers for one invocation.

## Runtime-manager adapter obligations

For immutable service roots, generate one host-root gate per enrolled service
in the workload's actual manager. Append additive `Requires` and `After`
dependencies to the workload. Use `Type=oneshot`, `RemainAfterExit=no`, and
bounded internal retries. The gate must neither inherit nor receive the
workload's root/image, credentials or resource namespace, and the adapter must
not create admission bind targets in the image. Preserve workload-authored
conditions, pre/post command resets and ordering, root, UID/groups, namespaces
and resource policy.

DNS denial belongs to gate accounting, not workload `NRestarts`/StartLimit.
After successful admission, native workload Restart/RestartSec/StartLimit
semantics apply. Avoid outer gate restarts that reset the admitted budget.

The runner cleans its owned Unix process group for the probe and drains
nonblocking streams with bounded byte retention. For descendants that escape the
group, the adapter must supply and qualify runtime-manager **cgroup** cleanup.
This boundary does not support non-Unix runtime execution.

Systemd acceptance also requires actual read-only image
and root-directory VM tests without admission mount targets, fresh admission
on automatic restart, within-budget recovery and terminal exhaustion,
concurrent/coalesced requests, canceled starts, explicit stops, shutdown,
orphan-gate cleanup and no surviving cgroup workers. Library tests don't
qualify the adapter or installed host.

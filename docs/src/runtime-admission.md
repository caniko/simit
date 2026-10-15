# Runtime admission

Simit's `runtime_admission` library runs bounded readiness probes. The
independent `simit runtime admit` command is a thin adapter over that library.
Networking policy and systemd service integration belong to consumers such as
canix-toolbelt. Consume the published library directly through Cargo.

## Contract

Each invocation has its own attempt count, monotonic overall deadline,
per-probe deadline, cancellable backoff, and aggregate stdout/stderr budgets.
Only explicitly enrolled nonzero exit codes retry. Cancellation, a hanging
probe, byte overflow, dispatch/capture failure and unenrolled denial are
terminal. Diagnostic text never decides retry eligibility.

Success records one fresh observation. It is not a durable grant, cached
success, or an atomic guarantee that readiness persists until the workload
starts. Every new invocation dispatches a new probe. Recovery resumes a
pending invocation only within its budget; after exhaustion a new start
request must create a new invocation.

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

The executable and working directory must be absolute. Argument and environment
vectors are explicit; the runner clears inherited environment variables and
performs no shell parsing. If the caller explicitly selects a shell, that
caller owns the shell command's policy. Probe environment values are not
copied into receipts. Unknown fields, unsupported versions, NULs and oversized
input reject before execution.

Input is capped at 64 KiB. Each stream's aggregate budget is 1 byte to 8 MiB,
attempts are capped at 256, and the overall execution budget is at most one
hour. The per-probe deadline cannot exceed the overall deadline. A probe is
clamped to the invocation's remaining time and byte budgets.

```sh
simit runtime admit --spec /path/to/spec.json --evidence /run/my-gate/invocation-1
```

The evidence directory must be new and its parent must exist. On Unix it is
created with mode 0700. Probe streams are retained as `attempt-N.stdout` and
`attempt-N.stderr`; `receipt.json` is capped at 64 KiB and contains input
SHA-256, actual observation timestamps, monotonic elapsed time, all attempt
statuses and retained lengths, and a typed terminal outcome. The CLI also
prints the receipt as JSON and exits successfully only for `admitted`.
Reusing an existing evidence directory is an error, not success reuse.

Library consumers call `AdmissionSpec::validate` and `admit`. Consumers can
disable the default `runtime-admission-cli` feature; the library API remains
available. Libraries do not install process-global signal handlers. The CLI
owns SIGINT/SIGTERM handlers for its one invocation.

## Runtime-manager adapter obligations

For immutable service roots, generate one host-root gate per enrolled service
in the workload's actual manager. The workload gets additive `Requires` and
`After` dependencies. Use `Type=oneshot`, `RemainAfterExit=no`, and bounded
internal retries. Do not inherit the workload's root/image, credentials or
resource namespace into the gate, and do not create admission bind targets in
the image. Preserve the workload's authored conditions, pre/post command
resets and ordering, root, UID/groups, namespaces and resource policy.

DNS denial belongs to gate accounting, not workload `NRestarts`/StartLimit.
After successful admission, native workload Restart/RestartSec/StartLimit
semantics apply. Avoid outer gate restarts that reset the admitted budget.

The shared runner cleans its owned Unix process group and drains nonblocking
streams with bounded byte retention. Escaped descendants are outside that
process group: the adapter must supply and qualify runtime-manager **cgroup**
cleanup. Non-Unix runtime execution is not supported by this boundary.

Systemd integration acceptance additionally requires actual read-only image
and root-directory VM tests without admission mount targets, fresh admission
on automatic restart, within-budget recovery and terminal exhaustion,
concurrent/coalesced requests, canceled starts, explicit stops, shutdown,
orphan-gate cleanup and no surviving cgroup workers. Library tests do not
establish those adapter or installed-host acceptance claims.

# Native original-producer custody

`simit ci observe-original` replaces long-lived `gh run watch` collectors with a
native read-only observer. Its public library lives in `ci_runtime::observer`.
It polls all enrolled originals under one monotonic discovery, retry, capture and
revalidation deadline. An early watch exit cannot establish terminal state.

Repository-owned JSON enrollment contains the complete required job-name list:

```json
{
  "schema": "simit.original-observer-spec.v1",
  "repository": "owner/repository",
  "workflows": { ".github/workflows/ci.yaml": ["test"] },
  "timeout_seconds": 19680,
  "poll_seconds": 60
}
```

```sh
simit ci observe-original --spec .github/original-producers.json --results "$RUNNER_TEMP/producer-evidence"
simit ci verify-original-retention --spec .github/original-producers.json \
  --results "$RUNNER_TEMP/producer-evidence" \
  --artifact-id "$ARTIFACT_ID" --artifact-digest "$ARTIFACT_DIGEST"
```

The CLI requires a GitHub-hosted branch-push/PR original attempt, the native
event/environment identity and a nonempty `GH_TOKEN` (or nonempty `GITHUB_TOKEN`
fallback). Library callers
supply identity, a transport and cancellation without global signal handlers.
The CLI compares the immutable checkout snapshot to the tracked blob at the
observed source commit before credential or provider access. Only built-in
CRLF/LF equivalence is admitted; repository-defined clean filters are never
executed. Git probes receive neither observer token and cannot lazily fetch
missing objects. The bounded committed blob is the policy deserialized by the
observer. Modified, staged, untracked
and outside-checkout substitutions fail; local Git replacement objects cannot
substitute the source blob. Unrelated worktree edits do not affect enrollment.
These Git probes share one thirty-second deadline. The active
observer workflow cannot enroll itself as a producer.
Source probes resolve Git only from fixed protected platform installation
directories (including administrator-installed Nix profiles), never the mutable
job `PATH`. Unsupported installations fail before provider access. On Unix the
installed path, symlinks and resolved binary must remain root-owned and protected
against replacement; Windows uses the hosted image's fixed Program Files Git.
Nix packages bind Git and the CA bundle from their declared derivation inputs at
compile time, retaining these immutable store references in the runtime closure.
The corresponding job environment variables cannot override those bindings.
The original event/identity snapshot that authorized the blob is also the one
validated against its policy and returned to capture; the event file is not
reloaded after the probes.
The initial event target must also match protected `GITHUB_REF`: the exact branch
ref for push and `refs/pull/<number>/merge` for supported PR observations. Editing
the event file cannot redirect discovery to another same-SHA branch or PR.
PR enrollment also requires the protected `GITHUB_SHA` merge commit to be
available locally; its parents must equal the event's base and enrolled source
head. When
checking out the PR head directly, retain/fetch the protected merge commit as
well. A missing merge object fails source admission, and an old checkout plus
rewritten event cannot certify historical runs from the same PR.
The native transport authenticates only fixed-origin GitHub GETs; approved HTTPS
log/artifact redirects are bounded and receive no GitHub credential.
Authenticated requests disable environment proxies. On OpenSSL platforms,
default roots are disabled and an explicit packaged or root-protected platform
CA bundle supplies trust independently of `SSL_CERT_FILE`/`SSL_CERT_DIR`.
Windows and macOS use their native OS trust stores. Job-selected proxy and CA
settings cannot change this observer transport's trust.
Each synchronous transfer owns and joins a scoped worker runtime, so library
callers can use the transport inside Tokio tasks without nesting runtimes or
leaving detached transfer work.
Repository-name periods, including consecutive periods within a name, remain
valid; actual traversal path segments and escaped separators are rejected.

Use a 360-minute hosted workflow timeout with the example's 328-minute observer
budget. Bound setup as well as observation: for example, checkout (2 minutes),
Nix setup (5), engine provisioning (10), capture plus finalization (331), upload
(3), retention verification (6), and diagnostic upload (1) total 358 minutes.
The capture step includes its bounded CLI source probe and two-minute native
finalization reserve. The producer
and its queue must fit inside the finite observation budget. Deadline exhaustion
and cooperative cancellation retain an explicit incomplete receipt. Upload it
with `if: always()`; a failed, cancelled, skipped or incomplete capture never
establishes acceptance. Pre-existing evidence directories are rejected.
Windows CLI commands register a native console handler for Ctrl+C/Break and
console close/logoff/shutdown events. Close-type callbacks wait for command cleanup
for up to four seconds inside the normal Windows termination budget; forced OS
termination can still interrupt finalization. Library callers retain ownership
of cancellation and install no process-global handlers.

Only bounded transient responses (408, 429, 500, 502, 503, 504, rate-limited 403
identified by provider headers or a secondary-limit message, and temporary
transport failures) retry, at most five attempts per GET. Provider retry windows
are honored inside the aggregate deadline;
exhausted primary quota uses `x-ratelimit-reset` when Retry-After is absent.
Secondary limits without that header wait at least one minute; headerless 429
responses use bounded exponential fallback. Derived waits remain in the request
diagnostics and never extend the aggregate deadline. Expiry during that wait
remains failure. Other authorization failures,
malformed successful responses, duplicate producer selections and identity drift
fail closed. Received response bytes and sanitized metadata remain in numbered
request files. Neither credentials nor signed download URLs are recorded.
Capping an error body preserves its transient HTTP status for bounded retry;
oversized successful responses and cancellation remain permanent failures.
Cancellation is rechecked after atomic final receipt publication. If it arrived
during serialization/persistence, the observer republishes an explicit incomplete
receipt and returns a cancellation error while preserving captured files.
Request-layer/protocol failures before headers also retry within this budget;
locally malformed request construction remains permanent.

Admission checks the complete timeout/poll/workflow combination against the
65,536-GET ceiling, including bounded pagination, capture and all transient retry
attempts. Unsupported polling configurations fail before creating a packet.
Provider workflow paths may carry a validated ref suffix; enrollment still binds
the exact source-owned YAML path. The suffix is validated as a Git ref, separately
from the branch/PR target bound by the run's native identity fields. Ref validation
checks Git's forbidden ASCII bytes, preserving valid Unicode whitespace and
non-ASCII control characters. SHA discovery skips identifiable other targets,
preserving exact-target duplicate rejection and
continuous identity validation after selection. Log and artifact downloads use
the remaining aggregate deadline; metadata GETs retain their ninety-second cap.
Header and body transfers are interruptible: the native async transport checks
cancellation at most every 100 milliseconds while waiting, retaining received
partial bytes and dropping the in-flight transfer on cancellation or expiry.
`@` inside a valid
matching branch ref remains supported. CLI JSON inputs admit the same 16 MiB
envelope that covers every accepted job-name inventory. Serialized aggregate job metadata is bounded by
the same 16 MiB ceiling at capture and retention, including pretty-print overhead.

Every completed producer binds repository, run id, head, workflow, event and
attempt one before and after capture. Jobs are fetched from the attempt-one API
endpoint; their optional `run_attempt` field must equal one when present.
Exact required jobs and their native steps
must all be terminal and successful. Provider log ZIPs retain their exact bytes;
member admission is CRC-checked, byte-bounded and rejects duplicates, symlinks,
unsafe names, multidisk and unsupported ZIP directories. No archive is extracted
and no producer-controlled content is executed.
Evidence member names use `/`-joined path components on every host, including
Windows, so local inventories match the uploaded artifact's ZIP names exactly.
Finalization reserves bounded space for event/spec JSON, per-workflow snapshots
and request metadata in addition to the provider body/copy allowance. Request
metadata is capped at 4 KiB per GET; the same generated-evidence reserve is carried
through expanded ZIP admission and raw artifact download.

Receipt producer rows retain compact validated identity while full provider
snapshots remain in the packet. Atomic receipt publication owns and cleans its
temporary file on failed serialization; an interrupted write cannot block the
final incomplete receipt.
Producer append is transactional against the cumulative receipt envelope. A map
that cannot fit is retained in its original capture files while its duplicated
receipt row is rolled back. If final inventory growth exceeds that envelope,
publication writes an explicit incomplete failure record, omitting duplicated map
data as necessary. Original archives, map files and response bytes stay intact;
partial receipt inventories cannot establish retention or merge acceptance.

Uploaded packet downloads have a separate aggregate
byte budget: the doubled captured-byte allowance, bounded generated evidence,
the final receipt, outer ZIP framing and metadata reserves. Expanded ZIP admission includes the receipt's
byte and file-count allowances as well. Bounded single-disk ZIP64 directories support the full admitted
packet file inventory, retaining central-directory duplicate and CRC checks.

Source targeting also binds case-sensitive branch refs. A PR must have exactly
one native association matching the event's PR number, head ref/repository/SHA
and base ref/repository/SHA. Missing or ambiguous associations and base-snapshot
drift reject custody, including during retention revalidation. An identical head
on another branch or PR cannot supply the enrolled original execution.

After the ordinary immutable artifact upload, native retention verification writes
to a sibling diagnostics directory, appending `.retention` to the complete packet
name so `packet.v1` and `packet.v2` keep independent diagnostics. It checks
the independently supplied enrollment and event, all local evidence hashes, the
actual provider ZIP digest and members, the upload's original observer identity,
at least thirty days of retention, fresh producer success and fresh artifact
metadata. Verification retains its GET evidence in a fresh sibling
`producer-evidence.retention` directory. Preserve that diagnostic directory too.
Retention repeats the complete head/event/target selection and requires exactly
the receipt's workflow-to-original-run mapping. New exact-target duplicates,
missing originals and replacements fail; identifiable other targets stay excluded.
Its independent metadata allowance includes all selection pages and bounded
transient retries without reducing the admitted raw upload download allowance.

The receipt's `qualified` field stays false. These contracts establish capture,
content and retention evidence; repository-specific coverage, source signatures,
authorization, required CI, reviews, merge readiness and release acceptance remain
separate binders. Existing completion-driven custody policies and historical
packets retain their original contracts.

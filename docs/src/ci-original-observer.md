# Native original-producer custody

`simit ci observe-original` replaces long-lived `gh run watch` collectors with a
native read-only observer (`ci_runtime::observer` is its public library). It
polls all enrolled originals under one monotonic deadline for discovery, retry,
capture and revalidation. An early watch exit cannot establish terminal state.

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

The CLI requires a GitHub-hosted branch-push/PR original attempt, native
event/environment identity and nonempty `GH_TOKEN` (or nonempty `GITHUB_TOKEN`
fallback). Library callers supply identity, transport and cancellation without
global signal handlers. Before credential or provider access, the CLI compares
the immutable checkout snapshot to the tracked blob at the observed source
commit. It accepts only built-in CRLF/LF equivalence and never runs
repository-defined clean filters. Git probes get neither observer token and
cannot lazily fetch missing objects. The observer deserializes the bounded
committed blob as policy. Modified, staged, untracked and outside-checkout
substitutions fail; Git replacement objects cannot substitute the source blob.
Unrelated worktree edits do not affect enrollment. Before use, the CLI
independently hashes the bounded commit, each tree on the spec path, merge
commit and blob bytes with Git's object framing. Loose/packed objects with
changed contents under the same ID fail admission. Prior repository checks
cannot authorize later unchecked reads.
Event/spec preflight and every Git probe share one thirty-second deadline and the
CLI cancellation token, including commit/tree/blob authorization and probe
output reads. Probe children get a minimal clean environment without credentials,
shell startup hooks or dynamic-loader settings. This cannot sanitize an observer
process already injected before startup; the clean observer launch contract
below is required. The CLI opens input files without following the final
symlink, uses nonblocking opens on Unix and checks the open handle for a regular
file. Byte limits, deadline and cancellation apply between chunks and before
event parsing. The CLI installs cancellation handlers before preflight and
atomically retains an explicit non-accepting `simit.original-preflight.v1`
diagnostic in a fresh `<results>.preflight-*` sibling (or
`<results>.retention.preflight-*` for retention). Retain these fresh sibling
directories with diagnostic uploads even when failure precedes identity or
source enrollment. They never certify a packet. The active observer workflow
cannot enroll itself. Source probes resolve Git only from fixed protected
platform installation directories, including administrator-installed Nix
profiles, never job `PATH`. Unsupported installations fail before provider
access. On Unix, the installed path, every symlink, and the resolved binary
must all remain root-owned and protected against replacement; Windows uses the
hosted image's fixed Program Files Git. Nix packages bind Git and the CA bundle from
declared derivation inputs at compile time and keep immutable store references
in the runtime closure. Job environment variables cannot override these bindings.
The observer validates the same event/identity snapshot that authorized the blob
against policy and returns it to capture without reloading the event file.
The initial event target must also match protected `GITHUB_REF`: the exact branch
ref for push and `refs/pull/<number>/merge` for supported PR observations. Editing
the event file cannot redirect discovery to another same-SHA branch or PR.
PR enrollment requires the local protected `GITHUB_SHA` merge commit, with
parents equal to the event's base and enrolled source head. If checking out the
PR head directly, also retain/fetch that merge commit. Missing merge objects
fail source admission; an old checkout and rewritten event cannot certify
historical runs from the same PR.
The native transport authenticates only fixed-origin GitHub GETs; approved HTTPS
log/artifact redirects are bounded and receive no GitHub credential.
Authenticated requests ignore environment proxies. OpenSSL platforms disable
default roots and use an explicit packaged or root-protected platform CA bundle,
independent of `SSL_CERT_FILE`/`SSL_CERT_DIR`. Windows and macOS use native OS
trust stores. Job proxy and CA settings cannot change transport trust.
Each synchronous transfer owns and joins a scoped worker runtime, so Tokio
callers neither nest runtimes nor leave detached transfers.
Each DNS lookup future creates Hickory's asynchronous resolver on its transfer
runtime. No blocking `getaddrinfo` worker delays runtime drop, and reused HTTP
clients keep no resolver tied to a stopped runtime. Stalled DNS shares the
cancellation/deadline allowance for headers and body reads. Fixed public
GitHub/provider origins use DNS directly, not hosts file overrides: synchronous
parsing of an arbitrarily large hosts file would prevent timely cancellation
before the first DNS packet. The resolver uses reqwest's documented
[`dns_resolver` boundary](https://docs.rs/reqwest/0.12.28/reqwest/struct.ClientBuilder.html#method.dns_resolver).
Repository names may contain periods, including consecutive periods; traversal
segments and escaped separators are rejected.

Use a 360-minute hosted workflow timeout for the example's 328-minute observer
budget. Bound setup too: for example, checkout (2 minutes), Nix setup (5),
engine provisioning (10), capture plus finalization (331), upload (3), retention
verification (6), and diagnostic upload (1) total 358 minutes. Capture includes
the bounded CLI source probe and two-minute native finalization reserve. The
producer and its queue must fit the observation budget. Deadline exhaustion and
cooperative cancellation retain an explicit incomplete receipt. Upload with
`if: always()`; failed, cancelled, skipped or incomplete capture never
establishes acceptance. Existing evidence directories are rejected. Windows CLI
commands register a native console handler for Ctrl+C/Break and console
close/logoff/shutdown events. Close-type callbacks wait up to four seconds for
cleanup within the normal Windows termination budget; forced OS termination
can still interrupt finalization. Library callers own cancellation and install
no process-global handlers.

Only bounded transient responses (408, 429, 500, 502, 503, 504, rate-limited 403
identified by provider headers or a secondary-limit message, and temporary
transport failures) retry, at most five attempts per GET. Honor provider retry
windows within the aggregate deadline. Without Retry-After, exhausted primary
quota uses `x-ratelimit-reset`, secondary limits wait at least one minute, and
headerless 429 responses use bounded exponential fallback. Request diagnostics
record derived waits; waits never extend the deadline, and expiry during the
wait exhausts the aggregate deadline and fails the operation. Other
authorization failures, malformed successful responses, duplicate producer
selections and identity drift fail closed. Numbered request files retain
received bytes and sanitized metadata, never credentials or signed download URLs.
Capping an error body preserves its transient HTTP status for bounded retry;
oversized successful responses and cancellation remain permanent failures.
The observer rechecks cancellation after atomically publishing the final receipt.
If cancelled during serialization/persistence, it republishes an explicit
incomplete receipt, returns a cancellation error and preserves captured files.
Request-layer/protocol failures before headers also retry within this budget;
locally malformed request construction remains permanent.

Before creating a packet, admission checks the full timeout/poll/workflow
configuration against the 65,536-GET ceiling, including bounded pagination,
complete job-log downloads, capture and every transient retry; unsupported
polling configurations fail without creating a packet.
Enrollment binds the exact source-owned YAML path. Any provider workflow suffix
must be a valid Git ref, separate from the native run identity's branch/PR target.
Ref validation checks forbidden ASCII bytes while preserving valid Unicode
whitespace and non-ASCII control characters. SHA discovery skips identifiable
other targets, rejects exact-target duplicates and keeps validating identity
after selection. Retention's five-minute budget starts at function entry and
covers local inventory, bounded rereads, hashing, producer ZIP expansion,
artifact retrieval and provider revalidation. The thirty-second CLI preflight
fits within the six-minute retention step. Log and artifact downloads use the
remaining deadline; metadata GETs keep their ninety-second cap.
While waiting for headers or body, the native async transport checks cancellation
at most every 100 milliseconds. On cancellation or expiry, it retains partial
bytes and drops the transfer. Valid workflow filenames and refs may contain `@`.
The observer's full native `@refs/...` suffix is separate from its filename.
Producer paths match full enrolled filenames; ambiguous path/ref interpretations
fail closed, never selecting a truncated name. CLI JSON inputs allow 16 MiB,
enough for every accepted job-name inventory. Capture and retention cap
serialized aggregate job metadata at 16 MiB, including pretty-print overhead.

Before and after capture, bind every completed producer's repository, run ID,
head, workflow, event and attempt one. Fetch jobs from the attempt-one API;
`run_attempt`, if present, must equal one. Every exact required job and its
native steps must finish successfully. Keep exact provider log ZIP bytes;
member admission is CRC-checked and byte-bounded, rejecting duplicate members,
symlinks, unsafe names, multidisk and unsupported ZIP directories. Never extract
archives or execute producer-controlled content.
Fetch each admitted original job ID's complete plain-text log through GitHub's
documented
[`actions/jobs/{job_id}/logs` endpoint](https://docs.github.com/en/rest/actions/workflow-jobs#download-job-logs-for-a-workflow-run).
Keep the nonempty bytes as `<run_id>/job-logs/<job_id>.txt`. Match the run ZIP's
top-level aggregate logs to the complete job-log inventory by content hash,
including counts of identical logs from different jobs. A nonempty ZIP with
only some jobs or individual steps is insufficient. Never infer job IDs from
sanitized ZIP filenames. Join evidence paths with `/` on every host, including
Windows, so local inventory names match the uploaded ZIP exactly.
Beyond the provider body/copy allowance, finalization reserves bounded space
for event/spec JSON, per-workflow snapshots and request metadata (capped at
4 KiB per GET). Expanded ZIP admission and raw artifact download keep this same
reserve.

Receipt rows keep compact validated producer identities; the packet keeps full
provider snapshots. Atomic receipt publication owns its temporary file and
cleans it on serialization failure, so interrupted writes cannot block the
final incomplete receipt. Producer appends are transactional within the
cumulative receipt limit. If a map does not fit, roll back its duplicate
receipt row and keep the original capture files. If the final inventory
outgrows the limit, write an explicit incomplete failure record, omitting
duplicate map data as needed. Keep original archives, maps and response bytes;
partial inventories cannot establish retention or merge acceptance.

Uploaded packet downloads have a separate aggregate byte budget: twice the
capture byte allowance, bounded generated evidence, the final receipt, outer
ZIP framing and metadata reserves. Expanded ZIP admission also includes receipt
byte and file-count allowances. Bounded single-disk ZIP64 directories support
the full admitted inventory with central-directory duplicate and CRC checks.

Source targeting binds case-sensitive branch refs. For a PR, require exactly
one native PR association matching the event's PR number and head and base
ref/repository/SHA. Missing or ambiguous associations and base-snapshot drift
reject custody, including at retention revalidation. An identical head on
another branch or PR cannot supply the enrolled original.

After ordinary immutable artifact upload, native retention verification appends
`.retention` to the full packet name for sibling diagnostics, keeping `packet.v1`
and `packet.v2` separate. It checks independently supplied enrollment and event,
all local evidence hashes, the actual provider ZIP digest and members, the
upload's original observer identity, at least thirty days of retention, fresh
producer success and fresh artifact metadata. It re-fetches complete attempt-one
jobs and logs from GitHub. Jobs and native steps must match captured provider
JSON regardless of job order. Complete logs are independently fetched again by
validated original IDs and hashed to verify equality with each job's uploaded
byte hash. Both run ZIPs must cover exactly that complete aggregate job-log
inventory.

Across downloads of the same completed run, GitHub may regenerate ZIP container
metadata or omit or restore redundant nested `<positive-integer>_<name>.txt`
step copies. These are diagnostics, not coverage proof. Only those members may
appear or disappear; for every member name present in both ZIPs, its content
hash must match. All other names and hashes, including aggregate logs,
`system.txt` and unknown files, must match exactly. Changed complete logs,
missing aggregate/system logs, changed shared step contents and unsafe ZIPs
fail. Retain both raw archives and all per-job responses without replacing the
capture. Comparison bytes bind to uploaded member hashes so a local swap after
inventory cannot substitute different jobs or logs in an accepted verification.
A recomputed unsigned receipt and valid artifact digest cannot authorize forged
logs.
Preserve the fresh sibling `producer-evidence.retention` directory with
verification GET evidence. Retention repeats complete head/event/target
selection and requires the exact receipt workflow-to-original-run mapping.
New exact-target duplicates, missing originals and replacements fail; identifiable
other targets stay excluded. An independent metadata allowance covers all
selection pages and bounded transient retries without reducing the raw upload
download allowance. Per-job GETs, artifact/log retries and fresh jobs pagination
share one aggregate byte allowance without resetting or enlarging it. Per-response
ceilings and the entry deadline still apply. Retain received bytes in diagnostics
and never refund them on retry.

Packets without complete per-job logs cannot establish stronger coverage.
Preserve historical packets and their original engine qualification; never
backfill, rewrite or reclassify failed originals.

The receipt's `qualified` field stays false. These contracts establish capture,
content and retention evidence; repository-specific coverage, source signatures,
authorization, required CI, reviews, merge readiness and release acceptance remain
separate binders. Existing completion-driven custody policies and historical
packets retain their original contracts.

## Clean observer launch

Run capture, upload and retention in a **fresh, dedicated GitHub-hosted observer
job**, not on a worker after producer-controlled steps. A `needs` dependency
transfers completion state, not producer environment, processes or writable
filesystem. Do not execute producer scripts, setup hooks or downloaded
executables in the observer job. Provision only the reviewed immutable observer
engine and pinned trusted actions. Treat the checkout as data. Give only
capture and retention the least-privilege read-only credential.

This boundary must exist **before the observer's shell and dynamic loader start**.
Clearing `LD_PRELOAD`, `LD_AUDIT`, `LD_LIBRARY_PATH`, `DYLD_*`, `BASH_ENV` or `ENV`
inside `simit` is too late: injected code has run and could have read the
credential. A clean Git child environment protects that child, not an
already-compromised observer. Step-local overrides cannot remove earlier
same-user background processes. Neither fresh output directories nor unsigned
receipts make worker sharing safe.

`.github/workflows/qualify-observer-isolation.yaml` qualifies this launch
contract on original hosted workers with **inert credentials only**. The
producer proves a persisted loader library runs before process code. The
separate observer proves producer environment and filesystem do not cross that
job boundary, runs candidate CLI/source-admission tests and launches the
candidate binary with the inert credential and loader marker. `observe-original`
uses real native event/source binding and tracked fixture policy. It requires
an explicitly nonaccepting capture receipt because the inert credential cannot
authenticate to GitHub. This qualifies the launch contract on those workers and
tests the candidate engine. Production consumers must enroll a reviewed
immutable engine and never execute the observed producer's engine build or
scripts.

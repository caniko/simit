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
event/environment identity and `GH_TOKEN` (or `GITHUB_TOKEN`). Library callers
supply identity, a transport and cancellation without global signal handlers.
The CLI binds the exact loaded spec bytes to the tracked blob at the observed
source commit before credential or provider access. Modified, staged, untracked
and outside-checkout substitutions fail; local Git replacement objects cannot
substitute the source blob. Unrelated worktree edits do not affect enrollment.
These Git probes share one thirty-second deadline. The active
observer workflow cannot enroll itself as a producer.
The native transport authenticates only fixed-origin GitHub GETs; approved HTTPS
log/artifact redirects are bounded and receive no GitHub credential.

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

Only bounded transient responses (408, 429, 500, 502, 503, 504, rate-limited 403
with `Retry-After`, and temporary transport failures) retry, at most five attempts
per GET. Provider retry windows are honored inside the aggregate deadline;
expiry during that wait remains failure. Other authorization failures,
malformed successful responses, duplicate producer selections and identity drift
fail closed. Received response bytes and sanitized metadata remain in numbered
request files. Neither credentials nor signed download URLs are recorded.
Capping an error body preserves its transient HTTP status for bounded retry;
oversized successful responses and cancellation remain permanent failures.

Admission checks the complete timeout/poll/workflow combination against the
65,536-GET ceiling, including bounded pagination, capture and all transient retry
attempts. Unsupported polling configurations fail before creating a packet.
Provider workflow paths may carry a validated ref suffix; enrollment still binds
the exact source-owned YAML path. The suffix is validated as a Git ref, separately
from the branch/PR target bound by the run's native identity fields. SHA discovery
skips identifiable other targets, preserving exact-target duplicate rejection and
continuous identity validation after selection. Log and artifact downloads use
the remaining aggregate deadline; metadata GETs retain their ninety-second cap.
`@` inside a valid
matching branch ref remains supported. CLI JSON inputs admit the same 16 MiB
envelope that covers every accepted job-name inventory. Serialized aggregate job metadata is bounded by
the same 16 MiB ceiling at capture and retention, including pretty-print overhead.

Every completed producer binds repository, run id, head, workflow, event and
attempt one before and after capture. Exact required jobs and their native steps
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
final incomplete receipt. Uploaded packet downloads have a separate aggregate
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

The receipt's `qualified` field stays false. These contracts establish capture,
content and retention evidence; repository-specific coverage, source signatures,
authorization, required CI, reviews, merge readiness and release acceptance remain
separate binders. Existing completion-driven custody policies and historical
packets retain their original contracts.

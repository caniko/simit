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

Only bounded transient responses (408, 429, 500, 502, 503, 504 and temporary
transport failures) retry, at most five attempts per GET, with a bounded
retry-after/backoff inside the aggregate deadline. Authorization failures,
malformed successful responses, duplicate producer selections and identity drift
fail closed. Received response bytes and sanitized metadata remain in numbered
request files. Neither credentials nor signed download URLs are recorded.

Every completed producer binds repository, run id, head, workflow, event and
attempt one before and after capture. Exact required jobs and their native steps
must all be terminal and successful. Provider log ZIPs retain their exact bytes;
member admission is CRC-checked, byte-bounded and rejects duplicates, symlinks,
unsafe names, multidisk and unsupported ZIP directories. No archive is extracted
and no producer-controlled content is executed.

Source targeting also binds case-sensitive branch refs. A PR must have exactly
one native association matching the event's PR number, head ref/repository/SHA
and base ref/repository/SHA. Missing or ambiguous associations and base-snapshot
drift reject custody, including during retention revalidation. An identical head
on another branch or PR cannot supply the enrolled original execution.

After the ordinary immutable artifact upload, native retention verification checks
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

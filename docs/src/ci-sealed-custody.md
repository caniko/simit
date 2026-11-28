# Sealed original custody (`sealed-v2`)

`sealed-v2` is a prospective acceptance contract: it captures completed
originals before waiting for siblings, verifies complete evidence, and retains
it in immutable artifacts for later custody. Changing protocols never repairs
or accepts a failed historical `strict-v1` ZIP comparison. Native test/build
gates and consumer-owned execution acceptance remain authoritative.

## Select the contract

```toml
[ci.custody]
protocol = "sealed-v2"
repository = "owner/repository"
producer_attempt = 1
status_context = "simit/producer-custody"

[ci.custody.producers.CI]
workflow = ".github/workflows/ci.yaml"
jobs = ["test"]
```

Omit `protocol` or set `strict-v1` to keep the existing generated observer and v1
receipts. Unknown values fail before writes. Legacy receipts and fresh downloads
of ephemeral producer logs cannot fill a v2 capsule. Installation, live drills,
and collector retirement each still require owner acceptance.

The generated workflow has three hosted jobs:

1. **identity** establishes the stable target, with contents-read permissions
   and no token environment. Positively unenrolled paths exit before side effects.
2. **seal** captures only the completed original that triggered it. Concurrency
   keys on target and producer run ID, so aggregate queues cannot delay another
   original's capture. Contents/actions/checks are read-only. The pinned artifact
   action uploads the admitted capsule or nonaccepting failure diagnostics. The
   job has a 15-minute timeout and a five-minute upload.
3. **aggregate** independently downloads and verifies exact sealed capsules,
   uploads a receipt naming their immutable IDs/digests, then freshly revalidates
   before posting status on the producer source head. It has contents/actions/
   checks read and statuses write, target-scoped concurrency, and a 25-minute
   timeout. Upload and report run after failures.

All four rendered programs must match the legacy `custody.py` definitions
without the entry point, followed by `custody_sealed.py`. Before any write,
the full UTF-8 workflow, including three policy copies and YAML escaping, must
fit the unchanged 500,000-byte limit.

In actionlint 1.7.12, the
[external-command runner](https://github.com/rhysd/actionlint/blob/914e7df21a07ef503a81201c76d2b11c789d3fca/process.go)
writes the entire script to a pipe before starting Pyflakes. Scripts larger
than the pipe buffer can hang the combined check. Retain that failed gate;
run `actionlint -pyflakes '' WORKFLOW` for workflow/expression/shell checks and
`pyflakes EXACT_RENDERED_PROGRAM.py` separately for every distinct inline program.
The generated v2 copies are identical and must first pass source-byte parity.

## Capsule completeness

Each capsule binds the full policy, source head/tree, numeric repository ID,
exact branch/event/PR/base snapshot, original run ID/attempt, and observer
installation SHA. The name hashes these inputs. The receipt uses
`simit.producer-capsule.v2`; `manifest.json` hashes every other retained file.
The provider's artifact ID and SHA-256 digest seal the archive.

Capture retains the terminal native run, complete job and artifact inventories,
direct required-job logs, raw provider log ZIP and member hashes, required artifact
ZIPs, and native historical executed-workflow proof/commit/blob/bytes. It keeps
the source-head commit separate from the workflow revision. Custody requires the
immutable native `WorkflowRun.file` definition, not checkout logs, today's merge
ref, or the producer head.

A nonempty successful provider ZIP is not enough. Every non-skipped native
terminal job needs a nonempty, unambiguous step inventory. Every non-skipped
completed step, including setup and post-job cleanup, needs a nonempty exact
member `<job-name>/<step-number>_<step-name>.txt`. Job names and step numbers
must be unique. Whole-job rollups cannot substitute. Missing steps, empty
members, unsupported filename transformations, and ambiguous job names fail
closed. Slashes/backslashes in job or step names are unsupported by this version of
completeness admission. Skipped required jobs cannot qualify.
Seal fences native job/artifact inventories again before sealing.

Existing wire, retention, expansion, inventory, request and six-minute budgets
still apply. Capsules admit at most 100 native jobs; sealed JSON records cap at
1 MiB. Verification counts nested ZIPs toward the same expansion/member budget.
Archives are hashed and read in memory under those bounds, never extracted or
executed.

## Independent aggregation and revocation

The aggregate schema is `simit.producer-custody.v2`. Discovery admits exactly one
original per enrolled path/name/target. Bare and ref-qualified paths retain exact
head/ref/PR alias checks. Duplicate runs, wrong names/qualifiers, reruns, ambiguous
PR associations and snapshot drift reject. Missing or running originals and
not-yet-uploaded sibling capsules remain waiting on the first observation.

Capsule reuse requires exactly one artifact with the exact name. Verify its
native owner run, repository, default branch, observer workflow path/name,
original attempt, installation head, and native executed-file revision. The
native **seal job must have completed successfully**, and for reuse from a
prior observer, that prior observer must be terminal. Actions can replace
pending aggregates even with `cancel-in-progress: false`; a failed/cancelled
aggregate still permits seal reuse without making the observation successful. A
collector rerun invalidates reuse.

Verification recomputes the outer digest and every manifest/member checksum,
rechecks nested archive inventories and log completeness, verifies the native
executed workflow and immutable source bytes, and freshly fences producer jobs
and artifacts. Different policies/installations/snapshots never share capsules.
The current upload's action ID/digest must match native metadata. No producer-owned
artifact, legacy receipt, name-only lookup, or overwrite replacement is proof.

Reporting independently checks uploaded aggregate bytes against the local receipt
and re-verifies exact capsule IDs/digests. It freshly rediscovers the originals
and checks native execution definitions, job/artifact inventories and collector
seal provenance before publishing with the existing bounded status semantics. Native checks and status
lookup share one six-minute budget, with no second lookup cycle. The independent
failure POST keeps its own 90-second reserve after exhaustion.
Success requires every original and required job to succeed and both capsule and
aggregate uploads to be independently verified. Missing uploaded current evidence,
deletion, expiry, overwrite, duplicates, changed digests, native drift, lookup
failure or upload failure publishes failure at the safely admitted source context.
A waiting receipt cannot downgrade an earlier successful context to harmless
pending when its accepted capsule set is now missing.

v2 does not require fresh **producer** ZIP equality: later log pruning cannot
change verified sealed bytes. Fresh native revocation and exact artifact
availability remain mandatory. Overwrite creates a new artifact ID;
deletion/expiry invalidates future verification. The configured 32-day retention
does not guarantee custody beyond provider expiry. Report logs and
`report-failure.json` record post-upload failures. The diagnostic comes after
upload and is not part of the uploaded receipt.

## Installed acceptance

Local generated-program tests do not establish installed runtime or token access.
Retain the installation/source/generator/policy/protection identities from the
[installation procedure](ci-custody-acceptance.md), plus these v2 rows:

| Drill                     | Required evidence                                                                                                                         |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Both completion orders    | First seal upload completes while its sibling runs; independent aggregate uploads waiting/pending, then all exact capsules yield success. |
| Native completeness       | Full original ZIP matches native executed steps; nonempty pruned ZIP remains failed and has no admissible capsule manifest.               |
| Durable reuse             | Independently verify sealed IDs/digests after original ZIP pruning; retain the old strict ZIP failure separately.                         |
| Upload failures           | Capsule or aggregate upload failure revokes the established target; no name-only or local-byte substitute.                                |
| Deletion/expiry/overwrite | Exact capsule ID cannot be replaced by a new same-named artifact, and future verification fails.                                          |
| Concurrent completions    | Each seal executes independently; replaced/cancelled aggregate outcomes remain retained while successful seal evidence can be reused.     |
| Native drift              | Producer/collector rerun, execution-file change, duplicate/ref alias, PR snapshot drift or job/artifact change revokes success.           |
| Early exclusion           | Unenrolled same-name paths cause no receipt, provider lookup, stale-receipt read, upload or status effect.                                |

Retain all original and observer failures, capsule IDs/digests, seal-job IDs,
aggregate artifact IDs/digests, post-upload native records and status transitions.
Accept source correctness, durable custody and installed runtime as separate
claims. Collector retirement is a subsequent owner-controlled change.

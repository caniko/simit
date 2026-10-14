# Sealed original custody (`sealed-v2`)

`sealed-v2` is a prospective acceptance contract. It captures each completed
original before waiting for siblings, verifies complete evidence, and uses
immutable artifacts for later custody. A new protocol never repairs or accepts a
failed historical `strict-v1` ZIP comparison. Native test/build gates and
consumer-owned execution acceptance remain authoritative.

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

Omitting `protocol` retains the existing generated observer and v1 receipts.
`strict-v1` explicitly selects that default. Unknown protocol values reject
before writes. Neither a legacy receipt nor fresh downloads of a producer's
ephemeral logs can fill a v2 capsule. Installation, live drills, and collector
retirement still require their own owner acceptance.

The generated workflow has three hosted jobs:

1. **identity** establishes the stable target, with contents-read permissions
   and no token environment. Positively unenrolled paths exit before side effects.
2. **seal** captures only the completed trigger original. Its concurrency key
   includes the target and producer run ID, so an aggregate queue cannot delay
   another original's capture. Contents/actions/checks are read-only. The job
   uploads its admitted capsule, or nonaccepting failure diagnostics, using the
   pinned artifact action. It has a 15-minute timeout and a five-minute upload.
3. **aggregate** independently downloads and verifies exact sealed capsules,
   uploads a receipt naming their immutable IDs/digests, then freshly revalidates
   before posting status on the producer source head. It has contents/actions/
   checks read and statuses write, target-scoped concurrency, and a 25-minute
   timeout. Upload and report run after failures.

Every inline program is the legacy `custody.py` definitions without its entry
point, followed by `custody_sealed.py`. All four rendered copies must match that
composition. The entire UTF-8 workflow, including three policy copies and YAML
escaping, must fit the unchanged 500,000-byte admission limit before any write.

For static qualification, actionlint 1.7.12's
[external-command runner](https://github.com/rhysd/actionlint/blob/914e7df21a07ef503a81201c76d2b11c789d3fca/process.go)
writes the entire script to a pipe before starting Pyflakes. Scripts exceeding
the pipe's capacity can hang that combined invocation. Retain that failed gate;
run `actionlint -pyflakes '' WORKFLOW` for workflow/expression/shell checks and
`pyflakes EXACT_RENDERED_PROGRAM.py` separately for every distinct inline program.
The generated v2 copies are identical and must first pass source-byte parity.

## Capsule completeness

Each capsule binds the full policy, source head/tree, repository numeric identity,
exact branch/event/PR/base snapshot, original run ID/attempt, and observer
installation SHA. Its name hashes all of those inputs. The receipt schema is
`simit.producer-capsule.v2`; `manifest.json` hashes every retained file except
itself. The provider's artifact ID and SHA-256 digest seal the entire archive.

Capture retains the terminal native run, complete job and artifact inventories,
direct required-job logs, raw provider log ZIP and member hashes, required artifact
ZIPs, and native historical executed-workflow proof/commit/blob/bytes. It also
retains the source-head commit separately from the executed workflow revision.
The immutable native `WorkflowRun.file` definition remains required; checkout
logs, today's merge ref, or the producer head cannot substitute for it.

A successful provider ZIP being nonempty is insufficient. Every non-skipped
native terminal job must have a nonempty, unambiguous step inventory. Every
non-skipped completed step, including setup and post-job cleanup, must have a
nonempty exact member `<job-name>/<step-number>_<step-name>.txt`. Job names and
step numbers must be unique. Whole-job rollups cannot replace step members.
Missing steps, empty members, unsupported filename transformations, or ambiguous
job names fail closed. Slashes/backslashes in job or step names are unsupported
by this version of completeness admission. Skipped required jobs still cannot
qualify. Native job/artifact inventories are fenced again before sealing.

The existing wire, retention, expansion, inventory, request and six-minute
budgets remain in force. Each capsule admits at most 100 native jobs; sealed JSON
records are limited to 1 MiB. Nested ZIPs count toward the same expansion/member
budget during verification. Archives are hashed and read in memory under those
bounds, never extracted or executed.

## Independent aggregation and revocation

The aggregate schema is `simit.producer-custody.v2`. Discovery admits exactly one
original per enrolled path/name/target. Bare and ref-qualified paths retain exact
head/ref/PR alias checks. Duplicate runs, wrong names/qualifiers, reruns, ambiguous
PR associations and snapshot drift reject. Missing or running originals and
not-yet-uploaded sibling capsules remain waiting on the first observation.

Capsule reuse requires exactly one exact-named artifact. The verifier checks its
native owner run, repository, default branch, observer workflow path/name,
original attempt, installation head, and native executed-file revision. The
native **seal job must have completed successfully**. A prior observer must be
terminal, but its aggregate may have failed or been cancelled: Actions can replace
a pending aggregate even with `cancel-in-progress: false`. This preserves the
successful seal without changing that observer's failed/cancelled result into a
successful observation. A collector rerun invalidates reuse.

Verification recomputes the outer digest and every manifest/member checksum,
rechecks nested archive inventories and log completeness, verifies the native
executed workflow and immutable source bytes, and freshly fences producer jobs
and artifacts. Different policies/installations/snapshots never share capsules.
The current upload's action ID/digest must match native metadata. No producer-owned
artifact, legacy receipt, name-only lookup, or overwrite replacement is proof.

Reporting independently verifies the uploaded aggregate bytes against the local
receipt and re-verifies its exact capsule IDs/digests. It freshly rediscovers the
originals, checks native execution definitions, job/artifact inventories and
collector seal provenance, then publishes with the existing bounded status
semantics. Native checks and status lookup share one six-minute report budget;
the independent failure POST retains its own 90-second reserve after budget
exhaustion. Reporting never starts another six-minute lookup cycle.
Success requires every original and required job to succeed and both capsule and
aggregate uploads to be independently verified. Missing uploaded current evidence,
deletion, expiry, overwrite, duplicates, changed digests, native drift, lookup
failure or upload failure publishes failure at the safely admitted source context.
A waiting receipt cannot downgrade an earlier successful context to harmless
pending when its accepted capsule set is now missing.

Fresh **producer** ZIP equality is not a v2 acceptance gate: later provider log
pruning cannot alter already verified sealed bytes. Fresh native revocation and
exact artifact availability are still mandatory. Artifacts are ID-specific;
overwrite creates a new ID, and deletion/expiry invalidates future verification.
The configured 32-day retention does not promise custody beyond provider expiry.
Post-upload failures are also recorded in report logs and `report-failure.json`;
that diagnostic is produced after upload and is not part of the uploaded receipt.

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

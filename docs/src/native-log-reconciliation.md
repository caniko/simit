# Native Actions log reconciliation

`simit ci log-reconcile` is a read-only qualification-consumer operation. It
separates durable native evidence from current provider archive observations.
It performs no status writes, reruns, deletions, merges or observer installation.
GitHub native ZIP membership is **not assumed immutable**.

## Ownership and migration

The historical PR #60 failure was the independent member-equality assertion in
the external `verify-current-custody.py`, called by `verify-successor.py`.
Generated `src/render/ci/custody.py` retained native bytes and a digest but
admitted a nonempty inventory; its `report()` revalidated producer identity.
Changing that reporter would not repair the consumer's failed requirement.

Use the native command instead of that recurring external archive-comparison
operation. Preserve the historical helper bytes and failure receipts. Its JSON
result is the new consumer boundary, not a replacement for artifact-semantic,
native CI, review, branch-protection or installed-observer acceptance gates.
Renderer sources and generated workflows are unaffected by this migration.

The default `strict-fresh-v1` contract requires an independently acquired current
native archive with the same complete member content. Different ZIP packaging
is distinguished from member loss. The explicit `--contract retained-native-v1`
policy permits **only omission-only** later observations when all of these pass:

1. Exact original bytes and every raw member independently validate and survive
   durable-storage read-back.
2. An approved observer's detached OpenPGP signature authenticates the acquisition
   receipt, exact repository ID/run/attempt/source/event/branch/PR/base/workflow
   identity, executed workflow revision/blob, inventory and retained jobs digest.
3. The original covers the attempt-specific executed step inventory plus each
   executed job's unambiguous native aggregate and system records. Skipped
   steps are listed; unsupported name transformations, ambiguous mappings or
   missing steps remain unverified. Non-emitting exceptions require signed,
   exact job/step/name/reason/evidence-digest bindings and independently retained
   evidence bytes; an opaque digest is insufficient.
4. Every surviving current member matches that original's raw name, size and
   content digest. Added/changed members, invalid archives and unavailable
   observations do not use the exception.
5. Fresh producer, job/step inventory, source branch or open PR/base, workflow
   enrollment and workflow-blob queries pass, and pass again before success.

Selecting this contract is a reviewable consumer-policy change. It does not
declare a historical strict comparison successful or change protected policy.
The operator supplies observer approval outside candidate control. A candidate
receipt, digest filename, maximum member count, equal pair of downloads, or
new signature on an old unsigned capture cannot establish original provenance.
The legacy importer deliberately never authenticates its unsigned records.

## Machine-readable boundary

Input schema: `simit.native-log-manifest.v1`. Each `originals` entry supplies an
`identity`, original `archive`, attempt-specific `jobs`, exact local
`observations`, `supporting_records`, optional observer `receipt` and `signature`,
optional unsigned historical `capture_record`, and `non_emitting_evidence`
paths for any signed exceptions. The checksum record compares
checksums/source bindings only. Use the Rust `Manifest`/`Original` types in
`ci_runtime::log_reconcile` for the complete contract.

Output schema: `simit.native-log-reconciliation.v1`. Distinct verdicts include
`custody_integrity`, `custody_provenance`, `custody_completeness`,
`producer_eligibility`, `provider_archive_state`, member differences,
`selected_original_digest`, supporting receipt digests and explicit blockers.
Stored historical observations are diagnostic only; current acceptance requires
`--refresh-producers --refresh-archives`. Overall acceptance also rejects any
retained added/changed-member conflict or invalid archive variant.

```text
simit ci log-reconcile --manifest manifest.json --results new-private-directory
simit ci log-reconcile --manifest manifest.json --results another-new-directory --contract retained-native-v1 --observer-approval operator-approval.json --refresh-producers --refresh-archives
```

Observer receipts use `simit.native-log-observer.v1`, with
`acquisition = "native-https-download"` and the exact attempt-1 native endpoint.
Approval supplies a public `keyring`, exact signing `fingerprint` and approved
`observer_revision`. `gpgv` runs against isolated exact-byte copies; it does not
alter an operator keyring or signing configuration. It requires an exact-byte
SHA-2 signature and retains the receipt, signature, keyring and their independent
digests alongside the original. Receipt timestamps are
authenticated observer **claims**, not independently trusted timestamps.

## Capture, bounds and failure handling

Native bytes are never extracted, executed, unioned or reconstructed. Inventories
stream hashes and retain raw filename bytes as hexadecimal, uncompressed sizes
and content digests. Admission counts actual central-directory records before
the ZIP library allocates its inventory, rejects duplicates/case/Unicode/path
aliases, validates local/central identities and CRCs, and supports single-disk
ZIP64. Unsupported variants remain invalid.

Ceilings mirror the existing collector: 512 MiB compressed and expanded bytes,
16 MiB central directory, 10,000 actual members, 256 requests and 360 seconds.
Shared inspection/query/persistence/read-back budgets cannot be raised or
restarted during a reconciliation. Process queries reuse the
native CI runtime's bounded process-group ownership. Stored JSON is bounded to
16 MiB. Capture directories are bounded to 4,096 entries and 12 levels; a manifest
allows at most 100 originals and 32 historical observations/records per original.
Serialized reports are bounded as well.

Persistence uses private temporary files, fsync, atomic no-clobber placement at
`native-sha256/<digest>.zip`, directory fsync and independent full read-back.
Concurrent captures never replace a prior original. Failed/interrupted captures
and result directories remain nonaccepting. The command exits nonzero unless
its selected contract passes, while retaining diagnostic JSON.

Current native downloads allow at most two explicit HTTPS redirects to declared
Actions/Azure log-storage hosts. Authorization is attached only to the exact
GitHub API request; storage requests carry neither authorization nor cookies.
Credential buffers are zeroized on success and error paths. Reports retain only
selected HTTP headers, status, host and local timestamp
claims. Signed URL queries and credentials are excluded. Full native bytes and
metadata remain private; `upstream-incident-bundle.json` contains inventories,
digests and redacted metadata, never log contents. It is not automatically
published. The report also records the executable digest of the exact local
reconciliation engine; that digest does not establish hosted qualification.

## Prompt capture and installation remain separate

The historical collector waits for all enrolled siblings before downloading
anything. That can delay a completed original while ephemeral native logs change.
The separate prospective `sealed-v2` source already proposes per-original sealing
before aggregation. That collector improvement needs its own source qualification
and trusted default-branch installation; it is not incorporated here.

This consumer stores each supplied/native capture immediately. A partial first
capture never becomes complete merely because later downloads agree. Earlier
captures can support acceptance only through approved exact-producer observer
receipts. There is no cross-run artifact discovery or producer-code execution.
Neither this implementation nor a successful local reconciliation establishes
installed-observer acceptance or repairs GitHub's archive backend.

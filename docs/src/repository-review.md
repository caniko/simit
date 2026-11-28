# Repository review controllers and clients

Simit owns the generic review engine, versioned JSON contracts, pinned Nixpkgs
adapter, and deterministic GitHub Actions templates. Each deployment repository
owns its engine pin, cache and recipe policy, credential names, and activation.
Review is opt-in, separate from project CI and release publication.

## Controller

Add this to `simit.toml`, then run `simit init ci --review-only`:

```toml
[review]
role = "controller"
```

Generation owns `review-repository.yml`, `publish-review.yml`, and the local
`setup-nix` action. `simit init ci --review-only --check --diff` checks all three;
the CI registry also detects action drift. Generation never creates secrets or
changes `policy.json`. Regular CI regeneration keeps configured review files.

Use **direct, exact, reviewed** `simit` and `nixpkgs` flake inputs. Expose
`simit.lib.mkReviewTools { inherit system nixpkgs; }.package` as
`packages.<system>.repo-review` and `.selectorCheck` as a check. Pin Nixpkgs
to supply Nixpkgs-review 3.7.0. The package contains both frontends, the Python
adapter and tools, and an immutable engine manifest. Every generated job verifies
the manifest against the controller lock. Plans retain the full controller tool
lock, binding engine and tool identities without changing v1 schemas.
Unpackaged binaries support schema/validation; execution needs the pinned
package manifest.

A lean deployment can set `inputs.simit.flake = false` and import
`(simit + "/nix/review-tools.nix") { source = simit; inherit system nixpkgs; }`
directly. The same constructor skips Simit's development and website dependencies;
the controller still locks the exact engine revision and content hash.

Keep `policy.json` under controller review: approved recipes require repository,
full commit, directory, and source input, with no implicit fixture exemption.
Cache profiles allow only public endpoints and approved keys. Simit validates
profile identifiers and still accepts legacy names. `ATTIC_SERVER`,
`ATTIC_CACHE`, and `ATTIC_TOKEN` retain their existing deployment meanings.
Simit's ordinary `[prebuild.attic]` settings do not grant review-cache authority.
Publication stays disabled until endpoint/key verification and policy review.

### Compiler-cache admission

Optional `"compiler_cache": "kvrocks-v1"` selects the typed compiler-cache
contract. Omission preserves legacy request serialization and identity.
The field is independent of Attic `cache_profile`; it neither selects a disk
fallback nor accepts a worker command.

Admission requires a Linux flake or external-flake head request with
`checks-rebuild-v1`. PRs bind `expected_head` and `expected_base`; revision
requests need a full lowercase commit SHA matching `expected_head`. Plans bind
the resolved target to that immutable revision. Darwin, Nixpkgs, merge-mode
and mutable-revision requests are ineligible.

The bootstrap derives `legacyPackages.<system>.kvrocks` only from the packaged
engine manifest's direct controller Nixpkgs revision. It independently freezes
the package version, native derivation and output, realizes that exact output
without remote builders or IFD, and keeps its NAR and binary hashes. The request
platform must match the worker architecture and native Nix system. Neither the
consumer flake nor recipe nor their cache-dependent Rust closures supply this tool.

Worker preflight parses actual runner and Nix build-group account records,
rejects root/ambiguous identities, and refuses existing runtime state, even
dangling symlinks. Metadata checks require a non-symlink runtime directory
owned by that runner/group with mode `0750` and a Unix socket with the same
ownership and mode `0660`. These checks preserve foreign state without reserving
the path or proving service ownership, Unix-only listeners, sandbox admission,
or compiler-cache operation.

Operators must opt in to worker allocation before execution:
`repo-review build --compiler-cache-opt-in kvrocks-v1`. The generated build job
reads `vars.REPO_REVIEW_COMPILER_CACHE`. An immutable cache request with a
missing or mismatched worker opt-in fails before execution; the build never runs
silently uncached. Operators enable the variable and approve the actual hosted
allocation.

The packaged helper exclusively allocates per-run state below `/var/lib`, a
root-owned configuration directory below `/run`, and the canonical runtime
directory. It starts one transient Kvrocks service with the observed worker UID
and `nixbld` GID. Limits: 2 GiB memory, 200% CPU, 64 tasks, 8,192 descriptors, and
4 GiB SST database size via `max-db-size`. Explicit client/RocksDB file maxima
plus Kvrocks' 128 reserved descriptors fit the hard descriptor limit. The worker
can only read the root-owned configuration.

Within a 30-second service-start budget, readiness verifies the live systemd
invocation, process credentials/executable, kernel cgroup limits, socket UID,
GID, and mode, process-owned Unix-only listeners, and exact bounded RESP2 PONG.
An independently managed root supervisor retains the allocation inodes for the
build. Worker/control-pipe EOF or termination triggers a 30-second SIGTERM grace,
then SIGKILL if needed in the verified service's own cgroup. State removal
requires the verified service's cgroup to be empty and continued ownership of
the original directory, configuration and socket identities. It preserves
foreign or replaced state. PID-1 supervision lets cleanup finish after runner
cancellation kills the worker tree. A finite supervisor lifetime bounds
abandoned jobs.

The full host identity/native-tool/allocation/startup sequence shares a
240-second deadline. External operations use smaller caps or the remaining
budget. The worker allows the full budget plus launcher and frame handoff.
Startup frames fit the full 16 MiB plan-file contract plus bounded native/
worker receipts, with a 32 MiB ceiling and five-second read deadline. The
service's 61-minute lifetime exceeds the generated 60-minute build window,
including readiness/shutdown allowance. The independent supervisor also retains
host-startup and cleanup budgets.

Normal control-pipe EOF revalidates that the originally admitted invocation is
still active and that the owned socket identity still matches before
acknowledging stop. Exit between polls permits verified cleanup but not a
successful cache-lifetime receipt.

Before consumer realization, the engine freezes and rebuilds an independent
ordinary-sandbox socket probe with the canonical socket mounted via
`extra-sandbox-paths`.
The effective plan and retained probe bytes bind admission to the native tool,
service/supervisor invocations and immutable run. Consumer package/check builds
reuse the mount, preserve frozen targets and rebuild every requested check.
Passing cache-enabled reports need admission and verified cleanup receipts.
The native tool and probe retain GC roots throughout the lifecycle.

Transport receipts don't establish compiler-cache acceptance. Each consumer's
exact pinned wrapper/compiler must show a first cache write and second hit in
an ordinary sandbox, the expected versioned namespace, zero cache I/O errors
and daemon cleanup. Generator qualification, operator allocation/opt-in and
consumer production/runtime qualification are separate gates.

## Client

Clients need neither a Cargo project nor a flake. Set an exact controller pin
and valid v1 request JSON string, then generate the client workflow:

```toml
[review]
role = "client"
controller = "your-owner/your-controller"
revision = "0123456789abcdef0123456789abcdef01234567"
request = '{"schema_version":1,...}'
```

Replace the abbreviated request with complete JSON from `simit review example`.
The generated reusable-workflow call pins a full SHA and forwards no secrets.
GitHub OIDC resolves controller identity independently of the calling repository.

## Dispatch an exact reviewed controller

GitHub workflow dispatch accepts branch or tag names, not raw commits.
Supply both the reviewed full SHA and a named ref that resolves to it:

```sh
simit review dispatch request.json --controller your-owner/your-controller \
  --revision 0123456789abcdef0123456789abcdef0123456789 \
  --dispatch-ref reviewed-release
```

The CLI rejects missing, ambiguous, unsafe, and mismatched refs before submission.
It sends the reviewed SHA as `controller_revision`. Before checkout the workflow
checks it against its GitHub OIDC identity, so a ref that moves after preflight
fails rather than selecting another controller. Keep the workflow registered on
the controller's default branch and publish the ref through its existing review
process.

Reusable callers may explicitly forward the optional report-only `GH_TOKEN`
secret to the reporting job only. Omission keeps reporting manual. Generated
clients forward no secrets.

## Explicit Nixpkgs additions and scoped broken warnings

For `nixpkgs`, optional `packages` and `checks` list dot-separated attribute
paths. For each requested system, selection takes the union of explicit
attributes and upstream changed-package discovery, even if discovery has no
system entry. An explicit attribute without a usable derivation fails selection
instead of yielding a no-change result. `checks` retain test identity through
frozen selection, realization, and collection.

Set `"skip_discovery": true` (nixpkgs backend, at least one of `packages` or
`checks`) to build exactly the listed attributes. This skips upstream whole-tree
changed-package evaluation and leaves `changed_attributes` empty in the report.
The selector accepts attribute paths such as
`nixosTests.postgresql.postgresql.postgresql_18` and rejects Nix expressions.

Optional `nixpkgs_broken_warnings` lists only explicitly requested top-level
package attributes. These fields add a package and packaging test, warning only
for the package:

```json
{
  "backend": "nixpkgs",
  "packages": ["vortex"],
  "checks": ["vortex.tests.packaging"],
  "nixpkgs_broken_warnings": ["vortex"]
}
```

Insert these fields into a complete v1 request. On the tested checkout the
adapter resolves the attribute's `lib.getName` for the named broken handler:
`openssl_3` uses `problems.handlers."openssl".broken = "warn"`. It evaluates
warned subtrees separately, so unrelated selected attributes with the same
package name don't inherit the warning. Upstream base/head discovery still
includes additions absent from the base. Requests cannot supply arbitrary Nix
configuration or evaluation arguments. Global broken-package permission remains
off during final selection. The pinned evaluator's `Attr.broken` means evaluation
failed without a usable derivation identity, not another copy of `meta.broken`.
Warnings don't admit failed evaluations; scoped warnings still require usable
evaluated derivations.

The frozen report binds additions, scoped warnings, test identities, system
and exact head/base to the request. Omitting the array preserves serialization
and digests. Deployment still needs a qualified immutable engine/controller pin
and actual package/test/cache receipts for the consumer's exact candidate.

## Interfaces and trust boundaries

`simit review` exposes `schema`, `example`, `validate`, `plan`, `dispatch`,
`status`, `report`, `build`, `collect`, `publish`, `fetch`, and verification
commands. `repo-review` remains the compatible argument-array frontend;
`publish` promotes by approved digest. `engine-info` and `verify-engine`
expose/check the engine manifest. JSON contracts remain v1.

In PR head mode, commit/tree resolution and checkout use the PR's head
repository, including forks. Merge mode uses the base repository's verified
merge commit. The v1 plan's `target.repository` and `target.id` still name the
base repository, home of the PR and report. Drift audits cover review and
ordinary CI workflows; review doesn't change the inferred CI provider or
platform.

Separate jobs handle resolution, secretless target builds/tests, collection,
approved publication, fresh-store retrieval, and reporting. Missing platforms,
failed checks, incomplete closure export, or failed retrieval prevent a pass.
Consumer retrieval enforces signature verification for exact output paths
without building or editing consumer configuration. Generic reviews never
approve or merge PRs.

The crate requires Rust 1.88. Windows supports contract and client operations;
Nix execution needs a supported Nix host; Attic publication needs Unix.
Compatibility tests run on Linux and Windows. The pinned controller's CI tests
actual Nix builds, adapter behavior, closure transfer, cache misses, and
signatures. Passing CI doesn't establish operational readiness. The descriptor
reports `ready: false` until separately reviewed live acceptance, publication
approval, and activation.

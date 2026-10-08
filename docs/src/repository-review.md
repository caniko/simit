# Repository review controllers and clients

Simit owns the generic review engine, versioned JSON contracts, pinned Nixpkgs
adapter, and deterministic GitHub Actions templates. A deployment repository
owns its pinned engine, cache and recipe policy, credential names, and activation.
This is opt-in and separate from ordinary project CI and release publication.

## Controller

Add this to `simit.toml`, then run `simit init ci --review-only`:

```toml
[review]
role = "controller"
```

Generation owns `review-repository.yml`, `publish-review.yml`, and the local
`setup-nix` action. `simit init ci --review-only --check --diff` checks all three;
the CI registry also detects action drift. Generation never creates secrets or
changes `policy.json`. Regular CI regeneration retains configured review files.

Use **direct, exact, reviewed** `simit` and `nixpkgs` flake inputs. Expose
`simit.lib.mkReviewTools { inherit system nixpkgs; }.package` as the controller's
`packages.<system>.repo-review`, and `.selectorCheck` as a check. The Nixpkgs
tool pin must supply Nixpkgs-review 3.7.0. This package includes both frontends,
the Python adapter and tools, and an immutable engine manifest. Every generated
job verifies that manifest against the controller lock. Plans retain the whole
controller tool lock, binding engine and tool identities without changing v1
schemas. Unpackaged developer binaries support schema/validation, but execution
needs the pinned package manifest.

A lean deployment can set `inputs.simit.flake = false` and import
`(simit + "/nix/review-tools.nix") { source = simit; inherit system nixpkgs; }`
directly. This uses the same tool constructor without importing Simit's
development and website dependency graph; the controller still locks the exact
engine source revision and its content hash.

Keep `policy.json` under controller review: approved recipes require repository,
full commit, directory, and source input; there is no implicit fixture exemption.
Cache profiles contain public endpoints and approved keys only. Profile names
are validated identifiers, with the legacy names still accepted. `ATTIC_SERVER`,
`ATTIC_CACHE`, and `ATTIC_TOKEN` retain their existing deployment meanings.
Simit's ordinary `[prebuild.attic]` settings do not grant review-cache authority.
Publication stays disabled until endpoint/key verification and policy review.

## Client

Clients can have no Cargo project or flake. Configure an exact controller pin and
a valid v1 request JSON string, then generate the client workflow:

```toml
[review]
role = "client"
controller = "your-owner/your-controller"
revision = "0123456789abcdef0123456789abcdef01234567"
request = '{"schema_version":1,...}'
```

Get complete request JSON with `simit review example`; the abbreviated example
above is explanatory and must be replaced. The generated reusable-workflow call
pins a full SHA and forwards no secrets. Controller identity is resolved through
GitHub OIDC, independently of the calling repository.

## Interfaces and trust boundaries

`simit review` exposes `schema`, `example`, `validate`, `plan`, `dispatch`,
`status`, `report`, `build`, `collect`, `publish`, `fetch`, and verification
commands. `repo-review` remains the compatible argument-array frontend;
`publish` is the digest-approved promotion command. `engine-info` and
`verify-engine` expose/check the engine manifest. JSON contracts remain v1.

For PR head mode, commit/tree resolution and source checkout use the PR's head
repository, including forks. Merge mode uses the base repository's verified
merge commit. The v1 plan's `target.repository` and `target.id` continue to name
the base repository, where the PR and its report belong. Review workflows are
audited for drift alongside ordinary CI, but do not change its inferred provider
or platform.

Resolution, secretless target builds/tests, collection, approved publication,
fresh-store retrieval, and reporting remain separate jobs. Missing platforms,
failed checks, incomplete closure export, and failed retrieval cannot pass.
Consumer retrieval enforces signatures for exact output paths without building
or editing consumer configuration. Generic reviews never approve or merge PRs.

The crate retains Rust 1.85. Windows supports contract and client operations;
Nix execution requires a supported Nix host, and Attic publication explicitly
requires Unix. Compatibility is tested on Linux and Windows. Actual Nix builds,
adapter behavior, closure transfer, cache misses, and signatures run in the
pinned controller's CI. CI success does not imply operational readiness: the
service descriptor continues to report `ready: false` until separately reviewed
live acceptance, publication approval, and activation.

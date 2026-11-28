# Chaosbox v1 integration (simit)

Chaosbox is a multi-crate Rust service with a project-owned custom Nix flake
(harbor-rs and harbor-db), declarative required gates, and coordinated,
dependency-ordered workspace publication on GitHub Actions.

Pinning handoff, validated with `cargo check --tests` and
generator/publish-runner fixtures in `tests/workspace_publish.rs`
(offline/local, no crates.io contact or uploads). Full `cargo test` runs in
the Nix devshell/CI with `cc`.

## Supported simit.toml

```toml
[flake]
scope = "hooks-only"
mode = "custom"

[flake.expected_outputs]
checks = ["test-gel"]
apps = ["test-gel"]

[ci]
platform = "github"
provider = "actions"
runtime = "nix"
runner = "ubuntu-latest"
workspace = true
workspace_strategy = "aggregate"
publish_crates = true
publish_strategy = "coordinated"
with_docs = true
# Formatting, lint, unit tests, docs, and packaging remain the generic
# workspace lane; project-specific Rust checks stay in the flake/devshell.

[[ci.required_gates]]
id = "gel-integration"
run = "nix run .#test-gel"
timeout_minutes = 30

[release.signing]
key = "<maintainer OpenPGP fingerprint>"
trust_root = "keys/maintainers.gpg"
required = true

[release.github]
repo = "<owner>/chaosbox"
target_branch = "trunk"
```

- Keep `flake` project-owned. Simit wires hooks (`simit init flake --scope hooks-only`)
  and reads declared `packages`/`apps`/`checks`; it generates no Crane/toolchain setup.
- Optional `check_command` replaces only the generic Cargo lane. `nix flake check`
  runs unless `nix_flake_check = false`. Prefer the flake check or
  `nix run .#test-gel` for Gel; never put test-only setup in global `[ci].extra_setup`.
- Keep gate `id` values stable and unique even after sanitization: `a_b` and
  `a-b` both become `gate-a-b`. IDs must match `[a-zA-Z0-9_-]+`; they form
  `gate-<id>` job names and `needs` references.
- `publish_strategy = "coordinated"` requires `publish_crates = true` and
  `--workspace`. Ordinary CI keeps member or aggregate strategy and member
  policies. In v1, every non-GitHub backend (including Forgejo/Crow/GitLab)
  explicitly rejects coordinated publication.

## Generation and drift-check commands

```sh
simit init flake --scope hooks-only
simit init ci --platform github --runtime nix --workspace --workspace-strategy aggregate --publish-crates --coordinated-publish
simit init ci --platform github --check --diff
```

First generation in write mode saves effective `[ci]` settings in `simit.toml`
(`publish_strategy`, `required_gates`, runners, gates). Regeneration and
`--check` share inputs and render deterministic output: alphabetical tie-breaks,
sorted `needs`, stable job names. `--check` only reports drift: changed
(`differs`), missing (`is missing`), or obsolete managed workflows (`is extra`).
Write mode prunes only stale Simit-owned publish workflows: `publish-crate-*.yaml`
when coordinated, `publish-workspace.yaml` when member-scoped. Handwritten
workflows stay.

Read-only modes (`--check`, `--print`, `simit release plan`, `simit release
verify`) leave `Cargo.lock`, `flake.lock`, and project files untouched.
Flake evaluation uses `--no-write-lock-file`; system-runner checks use
`--no-update-lock-file`. Sandboxed runs set `SIMIT_NO_REGISTRY=1` to suppress
registry changes. No recursive drift checks: `init ci
--check` only compares files, never runs gates.

## Declaring `nix run .#test-gel`

Equivalent gate using `nix flake check`:

```toml
[[ci.required_gates]]
id = "gel-integration"
run = "nix flake check --no-write-lock-file"
```

- CI: `gate-gel-integration` runs once per `ci.yaml` push/PR run with scoped
  `env` and `timeout-minutes: 30`. Failure fails the workflow. Gates run alongside
  `test` without a `needs` edge for faster PR feedback; configure branch
  protection to require both `test` and `gate-gel-integration`.
- Release: `trusted-publish-workspace.yaml` runs credential-free gates at the
  exact signed revision. Upload jobs depend on successful package verification
  and all required gates.

Gel service creation/cleanup belongs to `. #test-gel` in the project/harbor-db
test facility. Simit only runs the declared check. Put Gel credentials (e.g.
local fixture tokens) in gate `env`, never global `extra_setup`/`extra_env`.
Scope every release secret (including `CRATES_IO_API_TOKEN`, minisign, and
cosign) to its own independently protected release job. Only default-branch
`simit-crates-io` environment upload jobs receive the crates.io token.

## Coordinated dependency-ordered publication

Select `--coordinated-publish` or `publish_strategy =
"coordinated"`. From `simit release plan` (unchanged `release-plan/v1` JSON),
Simit generates credential-free builds in `.github/workflows/publish-workspace.yaml`
and an independent `trusted-publish-workspace.yaml` publisher. Explicit `needs`
edges enforce order unlike per-member workflows on the same tag.

Runtime contract (lockstep v1):

- Every publishable member's version must equal `$tag` (`cargo pkgid -p <crate>` after
  `git verify-tag` and checkout of the validated SHA).
  Development-only `publish = false` members get no plan entries or publish jobs.
- Run only on signed semver tag pushes (`[0-9]*`); retry the original run.
  No manual branch dispatch or publish-on-PR.
- Least-privilege permissions (`contents: read`, publisher `actions: read`), pinned
  actions, serialized concurrency (`cancel-in-progress: false`).
- Preserve maintainer trust with `test -s keys/maintainers.gpg` and `git
verify-tag`; fail on missing config rather than disable signing.

Publish each crate in plan order:

1. Package the full release set together with `--no-verify` in the credential-free
   tag build. Retain each immutable original-run artifact.
2. Independently admit the native run, signed source and provider digest.
   Reconstruct contents with isolated Cargo config; compare every member.
3. Compile the packaged crate using credential-free `cargo publish --dry-run`.
4. Re-admit on a fresh hosted runner, check the exact registry checksum, then
   upload archive bytes via the registry API only if indicated, without running
   candidate code.
5. Before publishing dependents, wait for exact registry resolution: up to
   20 attempts at 30-second intervals, including checksum-matched recovery.

Registry responses:

- `404` → unpublished; proceed.
- `200` → verify checksum (`crates.io/.../checksum` vs local
  `target/package/<crate>-<version>.crate` sha256). Match → resume (`exit
0`); mismatch → `conflict: ... refusing to treat as success` (`exit 1`).
- `401`/`403` → authorization/ownership, fail fast.
- Other → fail fast (logs distinguish validation and network failures).

Partial failure doesn't roll back publication. Audit `publish-report` (`if:
always()`) and per-crate job statuses. Rerun the original tag-push run:
preflight skips checksum matches and rejects conflicts, and upload jobs publish
missing versions in plan order.

Dependency semantics (tested against `cargo metadata`):

- Normal, build, optional, and target-specific path deps set publish order.
- Dev-deps don't affect order or fail on `publish = false` helpers.
- Directory mapping resolves workspace-inherited versions and renamed
  (`package = "real"`), optional, and target-specific deps. Cycles and
  non-publishable normal deps fail with actionable errors.

## Signed-tag and credential prerequisites

- This workflow uses signed semver tags (`0.1.0`, no `v` prefix; `git
verify-tag` against `keys/maintainers.gpg`). `init release` needs a
  `CHANGELOG.md` section for the version, and artifact signing also needs
  `keys/minisign.pub`.
- Commit `keys/maintainers.gpg`. At generation, supply `[release.signing].key`, `git config
user.signingkey`, or `--maintainer-key`. Required signing without config fails closed.
- Keep `CRATES_IO_API_TOKEN` only in the `simit-crates-io` environment, restricted
  to the exact protected default branch. Remove tag-accessible repository/organization secrets
  for publishing. Enroll a qualified immutable Simit engine via
  `vars.SIMIT_PUBLISHER_REVISION`; generation doesn't provision it.
- Jev alone provides runtime inference. Public CI uses the local protocol fixture.
  Keep real TypeSafe credentials out of ordinary CI. TypeSafe-gated steps belong
  in tag-only publish jobs or self-hosted release runners, never PR jobs.

## Packaging vs verification vs publication

- Archive construction: `cargo package --allow-dirty --no-verify`.
  `simit release plan --dry-run-package` labels it as such.
- Verification/build from packaged contents: `cargo package --allow-dirty`.
- Registry dry-run: `cargo publish --dry-run`.
- Actual publication: independently verified archive API uploads in plan order.

`--dry-run-package` constructs archives with `--no-verify`; Simit never reports
that as verification. New workspace dependents fail stage-2 verification
until prerequisite versions reach crates.io. Fixtures and `cargo package`
failure/success test this sequence:

```sh
simit release plan --json  # confirm order
cargo package -p <leaf> --allow-dirty --no-verify
cargo package -p <leaf> --allow-dirty          # verifies (no internal deps)
cargo publish -p <leaf> --dry-run
cargo publish -p <leaf>                          # then wait for propagation
cargo package -p <dependent> --allow-dirty     # now succeeds (prereq live)
```

For pre-publish checks without contacting crates.io, publish leaves to a
disposable registry (temp `file://`/sparse index or `cargo vendor` and `source`
replacement via `--config`, never committed). Rerun stage 2 for dependents
against packaged, not path, prerequisites. Document which registry checks must
wait for prerequisites. Never commit local overrides or path patches into
publishable manifests.

## Migration from member-scoped outputs

```sh
simit init ci --platform github --runtime nix --workspace --workspace-strategy aggregate --publish-crates --coordinated-publish
simit init ci --platform github --check --diff
```

Write mode writes `publish-workspace.yaml` and deletes any stale marker-owned
`publish-crate-*.yaml` workflows. Revert by regenerating without
`--coordinated-publish` (or with `publish_strategy = "members"`); this removes
marker-owned `publish-workspace.yaml`. Run `--check` after each switch. Without
explicit selection, single-crate, per-member, Forgejo, and Crow behavior is
unchanged.

## Tested examples and actual results

Fixtures (offline, no network):

- `tests/fixtures/release-plan-diamond`: `a → {b,c} → d` + `publish=false`
  `tool`. `simit release plan --json` yields `["a","b","c","d"]` with
  `d.depends_on == ["b","c"]`.
- `tests/fixtures/release-plan-advanced`: workspace-inherited versions, renamed
  (`liba_alias → liba`), optional (`libb`), target-specific (`cfg(unix)` →
  `liba`), dev-only `helper = false`. Plan: `["liba","libb","app"]` without
  `helper`; `app.depends_on` lists only publish-ordering deps.
- `tests/fixtures/release-plan-nonpublishable`: normal dep on
  `publish=false` → `release plan` fails (`depends on non-publishable`).
- `tests/fixtures/release-plan-cycle`: `a ↔ b` → cycle error.
- `tests/fixtures/release-plan-mismatch`: `a 0.1.0` vs `b 0.2.0` →
  `validate_lockstep_versions` fails; the generated workflow also contains
  per-crate `cargo pkgid` tag-equality checks that reject mismatches at runtime.

Generator/publish-runner tests (`tests/workspace_publish.rs`, offline):

- `coordinated_publish_generates_ordered_gated_workflow`: YAML parses;
  `gate-gel-integration` in CI (no `CRATES_IO_API_TOKEN`) and in publish
  (`needs: [validate]`); `publish-b` needs `publish-a`, `publish-d` needs
  `publish-b`+`publish-c`; checksum conflict branch, bounded propagation
  (`seq 1 20`), stage separation, `publish-report` with `if: always()`.
- `coordinated_publish_replaces_per_member_outputs_only`: removes stale generated
  `publish-crate-a.yaml`, keeps `handwritten.yaml`.
- `coordinated_publish_check_detects_drift_and_is_stable`: byte-stable
  regeneration; altered/missing workflows fail `--check`.
- `coordinated_publish_rejects_non_github_backends`: Forgejo with
  `--coordinated-publish` fails explicitly.
- `required_gates_reject_invalid_config`: duplicate `id` fails validation.
- `check_mode_leaves_lockfiles_untouched`: `Cargo.lock`/`flake.lock`
  byte-identical after generate + `--check`.
- `coordinated_publish_preserves_signing_trust_when_config_missing`,
  `..._bounds_propagation_and_distinguishes_failures`,
  `..._conflict_is_not_success_and_resume_is_auditable`: trust, bounded
  retries/status classes, and checksum resume vs conflict.

Here, `cargo check` and `cargo check --tests` passed. The full `cargo test`
suite (including the tests above) runs in the Nix devshell/CI with a C
toolchain; a cached result does not count as fresh execution.

## Pinning

Once this lands, pin a reviewed Simit tag and commit SHA in Chaosbox. Verify:

```sh
simit init ci --platform github --check --diff
simit release plan --json
```

Record the Simit version (`simit --version`), tag, and commit SHA in the
Chaosbox repo notes. This integration does not authorize publishing Simit,
pushing release tags, rotating signing keys, or uploading secrets; obtain
separate release authorization.

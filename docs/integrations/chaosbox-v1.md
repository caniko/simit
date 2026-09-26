# Chaosbox v1 integration (simit)

Multi-crate Rust service: declarative required gates + coordinated
dependency-ordered workspace publication on GitHub Actions, with a
project-owned custom Nix flake (harbor-rs + harbor-db composition).

This file is the exact pinning handoff. It was validated with
`cargo check --tests` and generator/publish-runner fixture tests in
`tests/workspace_publish.rs` (offline/local, no crates.io contact, no real
publishes). Full `cargo test` runs in the Nix devshell/CI where `cc` is
available.

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

Notes:

- `flake` stays project-owned. simit manages hook wiring only
  (`simit init flake --scope hooks-only`); it never generates a competing
  Crane/toolchain setup and consumes declared `packages`/`apps`/`checks`.
- `check_command` is optional. When set it replaces only the generic Cargo
  lane; `nix flake check` still runs unless explicitly disabled with
  `nix_flake_check = false`. Prefer the flake check or `nix run .#test-gel`
  for Gel; do not put test-only setup in global `[ci].extra_setup`.
- Gate `id` values must match `[a-zA-Z0-9_-]+`, be unique (including after
  sanitization: `a_b` vs `a-b` collide as `gate-a-b`), and stay stable:
  they become `gate-<id>` job names and `needs` references.
- `publish_strategy = "coordinated"` requires `publish_crates = true`,
  `--workspace`, and `--workspace-strategy aggregate`. Other backends fail
  explicitly (Forgejo/Crow/GitLab are not silently degraded in v1).

## Generation and drift-check commands

```sh
simit init flake --scope hooks-only
simit init ci --platform github --runtime nix --workspace --workspace-strategy aggregate --publish-crates --coordinated-publish
simit init ci --platform github --check --diff
```

After the first non-check generation, effective settings persist into
`simit.toml` under `[ci]` (`publish_strategy`, `required_gates`, runners,
gates). Regeneration and `--check` use the same inputs; output is
deterministic (alphabetical tie-breaks, sorted `needs`, stable job names).
`--check` reports altered (`differs`), missing (`is missing`), and obsolete
managed workflows (`is extra`); it also removes nothing. Write mode removes
only obsolete simit-owned publish outputs (generated `publish-crate-*.yaml`
when coordinated, generated `publish-workspace.yaml` when member-scoped).
Handwritten workflows are never deleted.

Read-only modes (`--check`, `--print`, `simit release plan`, `simit release
verify`) leave `Cargo.lock`, `flake.lock`, and project files untouched.
Flake evaluation uses `--no-write-lock-file`; system-runner checks use
`--no-update-lock-file`. Sandboxed runs set `SIMIT_NO_REGISTRY=1` to suppress
registry side effects. Generated drift checks do not recurse: `init ci
--check` compares files only and never executes gate commands.

## Declaring `nix run .#test-gel`

Use the gate above. Equivalent flake-check form:

```toml
[[ci.required_gates]]
id = "gel-integration"
run = "nix flake check --no-write-lock-file"
```

Scope:

- CI scope: `gate-gel-integration` job in `ci.yaml`, same push/PR triggers,
  once per run, with scoped `env` and `timeout-minutes: 30`. Failure fails the
  workflow. CI gate jobs run in parallel with `test`; enforcement is via
  branch protection requiring both `test` and `gate-gel-integration` green
  (no `needs` edge by design, for faster PR feedback).
- Release scope: `gate-gel-integration` job in `publish-workspace.yaml`,
  `needs: [validate]`, re-run at the exact signed tag revision before any
  `cargo publish`. Publish jobs `need` the gate chain, so gate failure blocks
  publication.

Gel service creation/cleanup belongs to the project/harbor-db test facility
invoked by `. #test-gel`; simit only invokes the declared check. Credentials
for Gel (e.g. local fixture tokens) go in gate `env`, never in global
`extra_setup`/`extra_env`. Release secrets (`CRATES_IO_API_TOKEN`, minisign,
cosign) appear only in tag-triggered publish jobs, never in ordinary PR jobs.

## Coordinated dependency-ordered publication

Selection is the `--coordinated-publish` flag / `publish_strategy =
"coordinated"` above. The generator consumes `simit release plan`
(`release-plan/v1` JSON, unchanged) and emits one
`.github/workflows/publish-workspace.yaml` with explicit `needs` edges.
Independent per-member workflows triggered by the same tag do not order;
this single workflow does.

Runtime contract (lockstep v1):

- One lockstep workspace release: every publishable member version must equal
  the tag (`cargo pkgid -p <crate>` compared to `$tag` after `git verify-tag`
  - checkout of the validated SHA). Development-only `publish = false`
    members are excluded from the plan and publish jobs.
- Triggers only on tags (`[0-9]*`) + `workflow_dispatch`. No publish-on-PR.
- Least-privilege permissions (`contents: read`, `id-token: write`), pinned
  actions, serialized concurrency (`cancel-in-progress: false`).
- Maintainer trust root preserved: `test -s keys/maintainers.gpg` + `git
verify-tag`; missing configuration fails instead of disabling signing.

Per-crate publish steps in plan order, each with:

1. `cargo package -p <crate> --allow-dirty` (archive + verification build;
   for dependents this succeeds only after prerequisites are live).
2. `cargo publish -p <crate> --dry-run`.
3. Bounded existence preflight + `cargo publish -p <crate>`.
4. Bounded propagation wait (`seq 1 20`, `sleep 30`) before dependents.

Conflict handling is honest, not blind:

- `404` → not yet published; proceed.
- `200` → verify checksum (`crates.io/.../checksum` vs local
  `target/package/<crate>-<version>.crate` sha256). Match → resume (`exit
0`); mismatch → `conflict: ... refusing to treat as success` (`exit 1`).
- `401`/`403` → authorization/ownership, fail fast.
- Other → fail fast (validation vs network distinguished in logs).

A halfway failure is not atomic rollback. The `publish-report` job (`if:
always()`) plus per-crate job statuses form the auditable result. Resume by
re-dispatching the workflow: matching checksums skip, conflicts fail, missing
versions publish in order. Preflight never blindly republishes or skips.

Dependency semantics (tested against `cargo metadata`):

- Normal, build, optional, and target-specific path deps order publishes.
- Dev-deps never order and never fail on `publish = false` helpers.
- Workspace-inherited versions, renamed (`package = "real"`), optional, and
  target-specific deps resolve via directory mapping; cycles and
  non-publishable normal deps fail with actionable errors.

## Signed-tag and credential prerequisites

- Signed semver tags (`0.1.0`, no `v` prefix in this workflow; `git
verify-tag` against `keys/maintainers.gpg`), `CHANGELOG.md` section for the
  version when using `init release` flows, and `keys/minisign.pub` when
  artifact signing is enabled.
- `keys/maintainers.gpg` committed; `[release.signing].key` set or `git config
user.signingkey` / `--maintainer-key` available at generation time.
  Generation fails closed when signing is required but unconfigured.
- `CRATES_IO_API_TOKEN` repo secret for publish jobs only. No secrets on
  untrusted PRs; no untrusted PR code on privileged release runners.
- Jev is the only runtime inference provider; public CI uses the local
  protocol fixture. No real TypeSafe credential in ordinary CI; TypeSafe-gated
  steps (if any) belong in tag-only publish jobs or self-hosted release
  runners, never PR jobs.

## Packaging vs verification vs publication

- Archive construction: `cargo package --allow-dirty --no-verify`
  (`simit release plan --dry-run-package`, now labeled as such).
- Verification/build from packaged contents: `cargo package --allow-dirty`.
- Registry dry-run: `cargo publish --dry-run`.
- Actual publication: `cargo publish` in plan order.

`--no-verify` is never presented as verification. For a new workspace whose
internal versions are not yet on crates.io, dependents' stage-2 verification
fails until prerequisites exist. Staged procedure (tested via fixtures +
`cargo package` failure/success split):

```sh
simit release plan --json  # confirm order
cargo package -p <leaf> --allow-dirty --no-verify
cargo package -p <leaf> --allow-dirty          # verifies (no internal deps)
cargo publish -p <leaf> --dry-run
cargo publish -p <leaf>                          # then wait for propagation
cargo package -p <dependent> --allow-dirty     # now succeeds (prereq live)
```

For pre-publish checks without touching crates.io, publish leaves to a
disposable registry (temp `file://`/sparse index or `cargo vendor` +
`source` replacement via `--config`, never committed) and re-run stage 2 for
dependents against packaged (not path) prerequisites. Document which
registry-dependent checks become possible only after prerequisites exist;
never commit local overrides or path patches into publishable manifests.

## Migration from member-scoped outputs

```sh
simit init ci --platform github --runtime nix --workspace --workspace-strategy aggregate --publish-crates --coordinated-publish
simit init ci --platform github --check --diff
```

Write mode deletes stale generated `publish-crate-*.yaml` (marker-owned
only) and writes `publish-workspace.yaml`. To revert, regenerate without
`--coordinated-publish` (or `publish_strategy = "members"`); stale
`publish-workspace.yaml` is removed the same way. Verify with `--check`
after each switch. Keep single-crate, per-member, Forgejo, and Crow behavior
unchanged unless the new mode is explicitly selected.

## Tested examples and actual results

Fixtures (offline, no network):

- `tests/fixtures/release-plan-diamond`: `a → {b,c} → d` + `publish=false`
  `tool`. `simit release plan --json` yields `["a","b","c","d"]` with
  `d.depends_on == ["b","c"]`.
- `tests/fixtures/release-plan-advanced`: workspace-inherited versions,
  renamed (`liba_alias → liba`), optional (`libb`), target-specific
  (`cfg(unix)` → `liba`), dev-only `helper = false`. Plan yields
  `["liba","libb","app"]`, `helper` excluded, `app.depends_on` contains only
  publish-ordering deps.
- `tests/fixtures/release-plan-nonpublishable`: normal dep on
  `publish=false` → `release plan` fails (`depends on non-publishable`).
- `tests/fixtures/release-plan-cycle`: `a ↔ b` → cycle error.
- `tests/fixtures/release-plan-mismatch`: `a 0.1.0` vs `b 0.2.0` →
  `validate_lockstep_versions` fails; generated workflow contains per-crate
  `cargo pkgid` tag-equality checks that fail at runtime.

Generator/publish-runner tests (`tests/workspace_publish.rs`, offline):

- `coordinated_publish_generates_ordered_gated_workflow`: YAML parses;
  `gate-gel-integration` in CI (no `CRATES_IO_API_TOKEN`) and in publish
  (`needs: [validate]`); `publish-b` needs `publish-a`, `publish-d` needs
  `publish-b`+`publish-c`; checksum conflict branch, bounded propagation
  (`seq 1 20`), stage separation, `publish-report` with `if: always()`.
- `coordinated_publish_replaces_per_member_outputs_only`: stale generated
  `publish-crate-a.yaml` removed, `handwritten.yaml` preserved.
- `coordinated_publish_check_detects_drift_and_is_stable`: regeneration is
  byte-stable; altered/missing workflows fail `--check`.
- `coordinated_publish_rejects_non_github_backends`: Forgejo +
  `--coordinated-publish` fails explicitly.
- `required_gates_reject_invalid_config`: duplicate `id` fails validation.
- `check_mode_leaves_lockfiles_untouched`: `Cargo.lock`/`flake.lock`
  byte-identical after generate + `--check`.
- `coordinated_publish_preserves_signing_trust_when_config_missing`,
  `..._bounds_propagation_and_distinguishes_failures`,
  `..._conflict_is_not_success_and_resume_is_auditable`: focused assertions
  for trust, bounded retries/status classes, and checksum resume vs conflict.

Validation performed here: `cargo check` and `cargo check --tests` pass.
Full `cargo test` (including the above) runs in the Nix devshell/CI with a
C toolchain; do not treat a cached result as fresh execution.

## Pinning

Chaosbox pins a reviewed simit revision (tag + commit SHA) after this change
lands, e.g.:

```sh
simit init ci --platform github --check --diff
simit release plan --json
```

Record the simit version (`simit --version`), tag, and commit SHA in the
Chaosbox repo notes. Do not publish simit, push release tags, rotate signing
keys, or upload secrets as part of this integration without separate release
authorization.

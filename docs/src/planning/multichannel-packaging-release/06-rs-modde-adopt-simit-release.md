# Phase 06 — rs-modde adopts the released simit

> **Recommended Codex model: GPT 5.5 medium**
>
> Moderate operational work: bump a flake input, regenerate workflows with the
> released simit, reconcile any drift, and commit the session's uncommitted
> rs-modde changes in coherent groups. Judgment is needed on what regenerates vs.
> what's intentional, but it's bounded and local. Orchestrator-lite × moderate →
> `medium`.

## Working tree

`/data/nvme0/can/Projects/rs-modde`. Depends on Phase 03 (a released simit to pin)
and on Phase 05's handoff of the chocolatey credential env/secret name. Blocks
Phase 07.

## Goal

rs-modde's `simit` flake input points at the new release; all workflows
(`ci-*`, `publish-crate-*`, `release.yml`) are regenerated with that simit so
`simit init ci --check` and `simit init release --check` are both CLEAN; the
chocolatey wiring matches what Phase 05 actually exposes; and every uncommitted
rs-modde change from this effort is committed in coherent groups.

## Why this matters now

rs-modde currently pins simit `0.15.3`, which lacks `init release` and the
channel generators — so `simit init ci --check` reports version-skew drift and
the committed `release.yml` is "ahead" of the pinned tool. Pinning the release
closes the skew and makes the repo self-consistent and drift-checkable in CI.

## Out of scope

- Do **not** push tags or trigger a release — that's Phase 07.
- Do **not** re-hand-edit generated workflows; regenerate them with simit.
- Do **not** revert the session's intended changes (flake `simitConfig`, dist
  artifacts, deleted `release-artifacts-*.yaml`, simit-generated `release.yml`).

## Plan

1. Bump the `simit` flake input to the new release. Prefer the `update-canix`-
   style flow if applicable, else `nix flake update simit` (or pin the new tag/
   rev) in `rs-modde/flake.nix` + `flake.lock`. Confirm `nix flake metadata`
   shows the new simit.
2. Reconcile the chocolatey wiring with Phase 05's reality: if the choco key is a
   runner-exposed env var, set `[chocolatey].api_key_from_runner = true` and
   `api_key_env` to that name; if it's a Codeberg Actions secret, set
   `api_key_secret` to that name. Drop the fork `nix_tool` to `nixpkgs#chocolatey`
   only if Phase 04 merged and the input nixpkgs includes it; otherwise keep the
   fork ref.
3. Regenerate all workflows with the released simit:
   `simit init ci --platform forgejo` (plain — no `--with-artifacts`) and
   `simit init release`. Then `simit init ci --platform forgejo --check` and
   `simit init release --check` must both be CLEAN.
4. Regenerate dist artifacts (`simit init aur`, `init copr`, `init apt`) and
   confirm `--check` clean.
5. Commit everything in coherent groups (use `grouped-git-commits`): e.g.
   (a) flake simitConfig + input bump, (b) regenerated `.forgejo/workflows/*`
   incl. deletion of `release-artifacts-*.yaml`, (c) regenerated dist artifacts
   (`dist/`, `modde.spec`, `.copr/Makefile`). Do not push tags.

## Acceptance criteria

- [ ] `rs-modde/flake.lock` resolves `simit` to the Phase 03 release (verify the
      rev/version), and `nix flake check`/eval of `.#simitConfig` succeeds.
- [ ] `simit init ci --platform forgejo --check` is CLEAN (version skew gone) and
      `simit init release --check` is CLEAN.
- [ ] The chocolatey step in `release.yml` references the credential exactly as
      Phase 05 exposes it (env name or secret name match).
- [ ] `git -C /data/nvme0/can/Projects/rs-modde status --porcelain` is empty
      after commits (all session changes committed in coherent groups; nothing
      pushed).

## Files likely touched

- `/data/nvme0/can/Projects/rs-modde/flake.nix`, `flake.lock`
- `/data/nvme0/can/Projects/rs-modde/.forgejo/workflows/*` (regenerated)
- `/data/nvme0/can/Projects/rs-modde/{modde.spec,.copr/Makefile,dist/**}`

## Pitfalls

- **Regen reintroduces `release-artifacts-*.yaml`.** Only if `with_artifacts` is
  enabled. Symptom: deleted files come back. Recovery: run `simit init ci`
  without `--with-artifacts`; confirm no `[ci].with_artifacts` is set in
  `simitConfig`. The new model is `init ci` (CI) + `init release` (release).
- **Residual version skew.** If `init ci --check` still drifts after the bump,
  the input didn't actually move. Symptom: same drift list as before. Recovery:
  inspect `flake.lock` `simit` node; ensure the lock points at the release rev.
- **Chocolatey wiring mismatch with Phase 05.** Symptom: the env name in
  `release.yml` doesn't match what the runner exposes → soft-skip at release.
  Recovery: align `api_key_env`/`api_key_from_runner` with Phase 05's recorded
  name before Phase 07.

## Reference

- Skills: `update-canix` (flake-input propagation pattern), `grouped-git-commits`.
- Prior phases: [03-release-simit.md](./03-release-simit.md),
  [05-canix-expose-runner-secrets.md](./05-canix-expose-runner-secrets.md);
  blocks [07-rs-modde-validate-and-release.md](./07-rs-modde-validate-and-release.md).

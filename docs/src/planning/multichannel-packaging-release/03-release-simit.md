# Phase 03 — Release simit (crates.io + Codeberg)

> **Recommended Codex model: GPT 5.5 high**
>
> Orchestrator role over an **irreversible** external publish: `cargo publish`
> to crates.io cannot be undone, and a botched changelog/tag is awkward to walk
> back. The mechanical steps are simple, but verifying the release bar
> (changelog promoted, tag signed, CI publish green, crate live) and reacting to
> a failed publish run needs error-recovery judgment. Complex × orchestrator →
> `high`. Not `max`: simit's own tooling drives the steps; this is well-trodden.

## Working tree

`/data/nvme0/can/Projects/simit`. Depends on Phase 02 (tests + docs + changelog
ready). After this phase the new simit version is on crates.io and Codeberg,
unblocking Phase 06.

## Goal

A new simit release (minor bump — this adds features) is published: version
bumped, `CHANGELOG.md` `[Unreleased]` promoted to the dated release section, a
signed tag pushed, and the Codeberg publish workflow green so the crate is live
on crates.io and the Codeberg release exists.

## Why this matters now

rs-modde must pin a _released_ simit that contains `init release` and the
channel generators (Phase 06). The pinned `0.15.3` predates all of it. Releasing
is the gate between "works in the local working tree" and "rs-modde can adopt
it."

## Out of scope

- Do **not** hand-roll the release (no manual `cargo publish` + `git tag` + sed).
  Use simit's own `simit release` / `simit changelog` flow — the maintainer's
  standing preference.
- Do **not** change feature code; if `simit release` surfaces a gate failure
  (fmt/clippy/test/deny/audit), fix forward minimally, but a large fix is a
  signal to bounce back to Phase 01/02, not to patch here.
- Do **not** touch rs-modde.

## Plan

1. Pre-flight the release bar locally: `simit release verify` (or `simit release
<bump> --dry-run`) and resolve anything it flags (changelog section present,
   signing trust root, clean tree, package builds).
2. Choose the bump: **minor** (new features, no breaking API). Confirm against
   the project's versioning policy (major-anytime is allowed but this is
   additive).
3. Run the release through simit, e.g. `simit release minor` (promotes
   `[Unreleased]` → dated section, bumps `Cargo.toml`/lock, commits, creates the
   signed tag). Review the planned commit/tag with `--dry-run` first.
4. Push the release commit and tag to Codeberg. The `publish-crate` workflow
   (tag-triggered) runs on the atlas runner: it verifies the signed tag, runs
   the gates, and `cargo publish`es. Watch the run (use the `berg` CLI /
   Codeberg Actions UI).
5. Verify post-publish: crate version live on crates.io
   (`https://crates.io/crates/simit`), Codeberg release/tag present, and
   `simit release verify --version <new>` (or equivalent) is satisfied.

## Acceptance criteria

- [ ] `CHANGELOG.md` has a dated `## [<new-version>]` section (no stray
      `[Unreleased]` content) and `simit changelog check` passes.
- [ ] A signed tag `<new-version>` exists on Codeberg and `git verify-tag`
      succeeds against `keys/maintainers.gpg`.
- [ ] The Codeberg `publish-crate` Actions run for the tag is **green**.
- [ ] `curl -fsS https://crates.io/api/v1/crates/simit/<new-version>` returns 200
      (crate version is live).

## Files likely touched

- `/data/nvme0/can/Projects/simit/{Cargo.toml,Cargo.lock,CHANGELOG.md}` (via
  simit tooling) + the release tag.

## Pitfalls

- **crates.io publish is irreversible.** A wrong version or a bad changelog can't
  be unpublished (only yanked). Symptom: realized-too-late mistake. Recovery:
  `--dry-run` everything first; if a bad version ships, `cargo yank` and release
  a corrected patch — do not try to overwrite.
- **CI publish gate failures.** The publish workflow re-runs fmt/clippy/test/
  deny/audit; a gate that passed locally can fail in the container (toolchain
  skew, advisory DB). Symptom: red publish run, crate not published. Recovery:
  read the run log via `berg`, fix forward, re-tag (move the tag with `simit
release sync-up --push` if appropriate) and re-run.
- **Signing trust root missing on the runner.** `git verify-tag` needs
  `keys/maintainers.gpg`. Symptom: validate-tag step fails. Recovery: ensure the
  committed keyring is present and the tag is signed by a key in it.

## Reference

- simit release flow: `simit release --help`, `simit release verify`,
  `simit changelog --help`.
- Codeberg CI inspection: the `berg-codeberg-ci` skill.
- Prior phase: [02-simit-release-readiness.md](./02-simit-release-readiness.md);
  unblocks [06-rs-modde-adopt-simit-release.md](./06-rs-modde-adopt-simit-release.md).

# Phase 02 — simit release readiness: tests, docs, CHANGELOG

> **Recommended Codex model: GPT 5.5 medium**
>
> Moderate-complexity authoring: integration tests that mirror an existing
> pattern, plus user-facing docs and a changelog. Some judgment on coverage
> breadth and doc phrasing, but no architectural decisions. A `low` tier would
> under-cover the new surface; `high` is unnecessary. Leaf role, `medium`.

## Working tree

`/data/nvme0/can/Projects/simit`. Depends on Phase 01 (the homebrew/scoop config
must exist so docs/tests describe the final surface). Same repo/files as 01 — do
not run concurrently with 01.

## Goal

simit is release-ready: the new `init aur|copr|apt`, `init release`, and `dist
aur|copr|apt` commands have integration-test coverage mirroring
`tests/init_chocolatey.rs`, the README/`docs/` document every new command and
config section, and `CHANGELOG.md` has an `[Unreleased]` entry enumerating the
multichannel packaging feature.

## Why this matters now

The new commands currently have unit tests on the render modules and one
`release_workflow` full-pipeline test, but no `init … --check` round-trip
integration tests like the existing channels have, and no user docs. simit's
release bar (and this maintainer's convention) expects documented, tested public
surface before publishing to crates.io in Phase 03.

## Out of scope

- Do **not** bump the version or tag — that's Phase 03 (`simit release`).
- Do **not** add new features or change generator behavior; this phase only
  tests/documents what Phases 00–01 built.
- Do **not** rewrite existing unrelated docs.

## Plan

1. Integration tests (mirror `tests/init_chocolatey.rs`): add
   `tests/init_aur.rs`, `tests/init_copr.rs`, `tests/init_apt.rs`, and
   `tests/init_release.rs` that scaffold a temp workspace fixture (reuse
   `tests/common/`), run the command, assert key output, then assert
   `--check` is clean (idempotent) and `--print` matches `--check` expectations.
   For `init_release`, assert the generated workflow contains the codeberg
   upload + each enabled channel's marker step.
2. Docs: update `README.md` and the relevant `docs/src/` page(s) with a
   "Distribution channels" section documenting `init aur|copr|apt`, `init
release`, `dist aur|copr|apt`, and the `[aur]`/`[copr]`/`[apt]`/
   `[release.codeberg]`/`[release.artifacts]`/`[release.attic]`/`[flatpak]`/
   `[winget]`/`[release.announce]`/`[release.windows_signing]` config sections,
   plus the chocolatey/homebrew/scoop secret-name knobs. Wire any new doc page
   into `docs/src/SUMMARY.md`.
3. `CHANGELOG.md`: add entries under `[Unreleased]` via
   `simit changelog add added "<text>"` for the new commands/config (keep the
   Keep-a-Changelog shape simit enforces).
4. `cargo test` (all targets) + `cargo clippy --all-targets -- -D warnings` +
   `cargo doc --no-deps` clean.
5. Commit (docs, tests, changelog) in coherent groups; do not push.

## Acceptance criteria

- [ ] `cargo test` runs `init_aur`, `init_copr`, `init_apt`, `init_release`
      integration tests and they pass, each asserting an idempotent `--check`.
- [ ] `README.md` documents all new `init`/`dist` commands and names every new
      config section; `docs/src/SUMMARY.md` references any new page.
- [ ] `CHANGELOG.md` `[Unreleased]` enumerates the multichannel packaging feature
      and `simit changelog check` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo doc --no-deps` are
      warning-free.

## Files likely touched

- `/data/nvme0/can/Projects/simit/tests/{init_aur,init_copr,init_apt,init_release}.rs`
- `/data/nvme0/can/Projects/simit/tests/common/` (fixture helpers, if extended)
- `/data/nvme0/can/Projects/simit/{README.md,CHANGELOG.md}`
- `/data/nvme0/can/Projects/simit/docs/src/**` (+ `SUMMARY.md`)

## Pitfalls

- **Multi-package fixture selection.** The new channels use
  `cargo::representative_package` (first workspace member); single-crate test
  fixtures are simplest. Symptom: `select_packages` ambiguity errors. Recovery:
  use a single-package fixture, or pass `--package`.
- **`init release` needs `[release.codeberg]` + build config to render fully.**
  A bare fixture yields an error step or minimal workflow. Recovery: give the
  fixture a minimal `[release.codeberg]` + `[release.artifacts].build_commands`.
- **`simit changelog check` is strict.** Hand-edited changelog lines may fail the
  Keep-a-Changelog validator. Recovery: use `simit changelog add`.

## Reference

- Test pattern to mirror: `simit/tests/init_chocolatey.rs`,
  `simit/tests/chocolatey.rs`.
- Prior phase: [01-homebrew-scoop-secret-config.md](./01-homebrew-scoop-secret-config.md);
  next: [03-release-simit.md](./03-release-simit.md).

# Phase 4 — Integration tests, README, docs site, CHANGELOG

> **Recommended Codex model: GPT 5.5 low**
>
> Mechanical: integration tests follow the existing `tests/homebrew.rs` and
> `tests/init_homebrew_tap.rs` shapes one-to-one, README and docs are short
> additive blocks, and CHANGELOG is a single `simit changelog add` per
> feature. No design content; a smaller model handles this cleanly.

## Working tree

Starts from phases 1–3 merged into `trunk`. This phase ships the user-visible polish.

## Goal

Cover the end-to-end flow with integration tests, document the new commands and `simit.toml` sections, and record the changes in `CHANGELOG.md`.

## Out of scope

- New features.
- Snapshot test files belonging to phases 2/3 (those landed there).

## Plan

1. **End-to-end tests**:
   - `tests/chocolatey.rs`: drive `simit chocolatey render` and `simit chocolatey bump` against a fixture project (mirror `tests/homebrew.rs` setup).
   - `tests/scoop.rs`: same shape for scoop.
   - `tests/init_chocolatey.rs` and `tests/init_scoop_bucket.rs`: mirror `tests/init_homebrew_tap.rs` (check/diff/print modes, bootstrap idempotency).
   - Extend `tests/init_ci.rs` with one full-stack snapshot per platform combination — already covered in phase 3 acceptance criteria; add any missing edge cases (no-arch flag, custom archive pattern).

2. **README**:
   - Add `## Windows packaging` section under the existing Homebrew block.
   - Document `--with-chocolatey`, `--with-scoop`, `--windows-runner`, and the required secrets (`chocolatey_api_key`, `scoop_bucket_token`).

3. **mdBook docs**:
   - New page `docs/src/getting-started/windows-packaging.md`.
   - Add entry to `docs/src/SUMMARY.md` under "Getting started".
   - Cross-link from `release-maintenance.md`.

4. **CHANGELOG**:
   - `simit changelog add Added "Chocolatey package publishing via simit chocolatey and init-ci --with-chocolatey"`.
   - `simit changelog add Added "Scoop bucket publishing via simit scoop and init-ci --with-scoop"`.
   - `simit changelog add Changed "release-artifacts.yaml now uses a Linux + Windows matrix when Windows packagers are enabled"`.

## Acceptance criteria

- [ ] `cargo test` (full suite) passes.
- [ ] `cargo nextest run` passes if used locally.
- [ ] `simit changelog check` passes.
- [ ] `mdbook build docs` produces no warnings; new page is reachable from SUMMARY.
- [ ] README renders cleanly on Codeberg (verify by eye — anchor links, code fences).
- [ ] `simit init-ci --platform forgejo --check` on simit itself still passes.

## Files likely touched

- `tests/chocolatey.rs` (new)
- `tests/scoop.rs` (new)
- `tests/init_chocolatey.rs` (new)
- `tests/init_scoop_bucket.rs` (new)
- `tests/init_ci.rs` (additions)
- `README.md`
- `docs/src/getting-started/windows-packaging.md` (new)
- `docs/src/SUMMARY.md`
- `docs/src/release-maintenance.md`
- `CHANGELOG.md` (via `simit changelog add`)

## Pitfalls

- **Test isolation**: integration tests must not require `choco` or `scoop` on the host. Stub the push path (assert env-var checks and command-line construction without executing).
- **Doc drift**: every flag added in phases 1–3 must appear in the README flag table; grep `--with-` and ensure parity.
- **CHANGELOG section**: put under `[Unreleased]`; do not promote — that's the user's release decision.

## Reference

- Test shapes: [tests/homebrew.rs](../../../../tests/homebrew.rs), [tests/init_homebrew_tap.rs](../../../../tests/init_homebrew_tap.rs).
- README pattern: existing Homebrew block in [README.md](../../../../README.md).

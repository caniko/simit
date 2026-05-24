# Phase 02 — Package-scope the publish-workflow version extractor

> **Recommended Codex model: GPT 5.5 high**
>
> Complex refactor at the orchestrator role: changes a function
> signature, threads a new parameter through multiple call sites,
> requires a non-trivial extractor design choice (`cargo pkgid -p
> <name>` vs `--manifest-path` vs `jq`), and demands a multi-member
> workspace fixture in the test harness. A `medium` tier would likely
> pick a working but suboptimal extractor and miss one of the call
> sites or test cases; `max` is unjustified because the blast radius is
> a single Rust file plus tests.

## Working tree

`/data/nvme0/can/Projects/simit`.

## Goal

`validate_release_tag_step` extracts the version of *the package being
published*, not of whichever package happens to appear first in
`packages[]`. A new workspace fixture in `tests/init_ci.rs` (≥2
members at different versions) proves the extractor picks the
published package's version regardless of `packages[]` ordering.

## Why this matters now

Phase 01 unblocks workspaces where all members share a version (today's
detritus state). The moment any member diverges (`detritus-protocol
0.2.0` while client/server stay at `0.1.0`), the workflow would silently
compare the tag to the wrong package and either pass when it should
fail or fail when it should pass. Latent silent-wrong-answer bugs are
worse than the loud failure 01 fixes. The dossier flagged this as a
Phase 1b follow-up.

## Out of scope

- Renaming or restructuring the publish workflow beyond the extractor.
- Changing the trust/GPG verification block — keep it textually
  identical so existing maintainer signatures still verify.
- Touching downstream consumers — Phase 04 sweeps them after release.

## Plan

1. Add a `package_name: &str` (or `Option<&str>` for whole-workspace
   safety nets) parameter to
   [`validate_release_tag_step`](../../../../src/render/ci.rs#L1648-L1665)
   and any helper it calls.
2. Update each call site in [`src/render/ci.rs`](../../../../src/render/ci.rs)
   (`publish_workflow`, the nix and plain-cargo branches, and any
   workspace-fanout caller) to pass the publishing package's name.
3. Replace the body of `version_check` with a package-scoped
   extractor. Recommended approach: `cargo pkgid -p <name> | awk -F'[#@]' '{print $NF}'`
   — no new runtime dependencies, robust to workspace layout, and
   stable across cargo versions. Document the rationale inline as one
   short comment. If `cargo pkgid`'s output format is uncertain across
   the project's MSRV, fall back to
   `cargo metadata --no-deps --format-version 1 --manifest-path crates/<pkg>/Cargo.toml`
   piped through the same `grep -o … | head -n1 | cut` chain — single
   match guaranteed because metadata is now per-package.
4. Add a workspace fixture to the test harness in
   [`tests/init_ci.rs`](../../../../tests/init_ci.rs) with two members
   at different versions (e.g., `member-a@0.1.0`, `member-b@0.2.0`).
   Initialise CI with `--workspace`, then assert the generated
   `publish-crate-member-a.yaml` extractor resolves *only* to `0.1.0`
   and `publish-crate-member-b.yaml` resolves *only* to `0.2.0`.
   Practical assertion: pipe the extracted snippet through `bash -c`
   in the test or, more robustly, assert the workflow contains the
   exact `cargo pkgid -p member-a` (or `--manifest-path
   crates/member-a/Cargo.toml`) form.
5. Re-run the validation gate:
   ```sh
   cargo fmt --all -- --check
   cargo clippy --all-targets --all-features -- --deny warnings
   cargo test --all-features
   cargo package --list
   ```
6. Bump to `0.15.2` (or `0.16.0` if signature-changing call surface
   counts as a minor under the project's semver policy — check
   `RELEASING.md`). Write CHANGELOG; follow Phase 01's release flow.

## Acceptance criteria

- [ ] `validate_release_tag_step` takes a package-name parameter and
      every call site supplies it.
- [ ] The generated publish workflow for a workspace member references
      *only* that member's manifest/pkgid, not the workspace-wide
      `cargo metadata` output.
- [ ] New test fixture in `tests/init_ci.rs` covers a 2-member
      workspace with diverging versions; it fails on `0.15.1` and
      passes on this phase's branch.
- [ ] `cargo test --all-features`, fmt, clippy (with `--deny warnings`),
      and `cargo package --list` all clean.
- [ ] Release tag pushed and crates.io publish succeeds.

## Files likely touched

- `src/render/ci.rs` (function signature + every caller + extractor body)
- `tests/init_ci.rs` (new workspace fixture + assertions)
- `CHANGELOG.md`
- `Cargo.toml`, `Cargo.lock`
- Possibly `tests/fixtures/` if a new on-disk fixture skeleton is
  needed for the workspace test.

## Pitfalls

- **Symptom:** test still passes when ordering of `packages[]` swaps.
  **Cause:** assertion is too loose (just checks "contains the right
  version") rather than tying the version to the member's manifest.
  **Recovery:** assert on the full extractor line per workflow, not
  just on a version substring.
- **Symptom:** `cargo pkgid -p X` produces a URL form on older cargo
  versions that breaks `awk -F'[#@]'`. **Cause:** pkgid format
  evolved. **Recovery:** keep the `--manifest-path` fallback and
  switch to it; document the cargo-version compatibility in the
  CHANGELOG.
- **Symptom:** `validate_release_tag_step` is also reachable from a
  non-publish-workflow caller you missed. **Cause:** `grep -n
  validate_release_tag_step src/` found one definition but multiple
  call paths to `publish_workflow`. **Recovery:** grep for every
  reference before signing the commit, not after.
- **Symptom:** A downstream regen in Phase 04 produces a different
  extractor than expected. **Cause:** different `package_name`
  inference in single-crate vs workspace flows. **Recovery:** in
  single-crate flows, infer the package name from the manifest
  rather than passing an empty default.

## Reference

- Research dossier:
  [`../publish-workflow-version-extraction-research.md`](../publish-workflow-version-extraction-research.md)
  — see "Work That Should Survive Into The Long-Term Plan" item 2.
- Generator: [`src/render/ci.rs:1648-1665`](../../../../src/render/ci.rs#L1648-L1665).
- Existing publish-workflow tests:
  [`tests/init_ci.rs:573-625`](../../../../tests/init_ci.rs#L573-L625),
  [`tests/init_ci.rs:680-691`](../../../../tests/init_ci.rs#L680-L691).
- Related phase: [`01-simit-commit-and-release.md`](./01-simit-commit-and-release.md)
  — must land before this; same function is touched.

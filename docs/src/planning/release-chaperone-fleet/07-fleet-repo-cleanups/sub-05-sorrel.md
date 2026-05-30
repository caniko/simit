# Phase 07.05 — `sorrel` cleanup for chaperone

> **Recommended Codex model: GPT 5.5 / medium**
>
> First-time multi-crate publish (8 crates), no `CHANGELOG.md`,
> substantially dirty worktree, flake-drift claim that needs
> re-verification on a clean tree, publish-order to encode. Multiple
> coupled decisions; one step up from the other repo sub-layers but
> not as far as rs-modde. Orchestrator role on moderately complex
> work — 5.5 at `medium`.

## Working tree

[/data/nvme0/can/Projects/sorrel](file:///data/nvme0/can/Projects/sorrel)
— feature branch off `trunk`. Remote:
`ssh://git@codeberg.org/caniko/sorrel.git`.

## Goal

`sorrel` is chaperone-ready:

- local worktree is clean; the substantial set of dirty
  `crates/sorrel-*/` files at snapshot time is either committed in
  scoped commits or stashed with a maintainer-visible note;
- a `CHANGELOG.md` exists with a `0.1.0` entry (or per-crate
  changelogs are documented as the project's equivalent);
- `simit` flake input tracks current `trunk`;
- `simit init ci --workspace --check --diff` is clean;
- `simit init flake --check --diff` is clean. **Re-verify** the
  dossier's claim that the generator wants to rewrite sorrel's
  flake — the original check was done on a dirty worktree and may
  have been confused.
- Publish order is documented (in the repo's `RELEASE.md` or
  `CHANGELOG.md`) or computable via `simit release plan` (if phase
  06 has landed).

Crates (all at `0.1.0`, none on crates.io): `sorrel-io`,
`sorrel-cache`, `sorrel-compute`, `sorrel-gpu`, `sorrel-data`,
`sorrel-render`, `sorrel-ui`, `sorrel`.

## Why this matters now

The dossier names sorrel as the second-hardest sub-layer: dirty
worktree, missing changelog, eight first-publish crates with
cross-crate path dependencies, and a flake-drift claim that is
not fully trustworthy because it was observed on the same dirty
worktree.

## Out of scope

- Publishing any of the 8 crates (chaperone, separately).
- Resolving the technical content of the dirty worktree edits —
  the sub-layer only triages whether they should ship together,
  separately, or wait.
- Reserving crate names on crates.io for the 8 crates (separate
  user action; sub-layer flags whether to do this).

## Plan

1. **Re-verify the snapshot.**

   ```sh
   git fetch origin
   git status --short
   git diff --stat
   git log --oneline --left-right origin/trunk...HEAD
   git log --oneline --left-right origin/trunk...origin/simit-ci-adoption-20260525
   ls -la .forgejo/workflows/ 2>/dev/null
   test -f CHANGELOG.md && echo CHANGELOG present || echo CHANGELOG missing
   ```

2. **Triage the dirty worktree with the user.** The dossier
   snapshot showed modifications across `Cargo.lock`, `Cargo.toml`,
   `crates/sorrel-data/`, `crates/sorrel-io/`, `crates/sorrel-ui/`,
   `crates/sorrel/`, plus docs. Group into themes (e.g. "data
   refactor", "IO provider update", "docs sync", "untracked
   generated workflows") and present each as a candidate
   release-candidate commit or stash. **Do not commit anything
   without explicit user buy-in on the grouping.**

3. **Bump the simit flake input** to current `trunk` (same pattern
   as sub-01 step 2). Do this before re-running any `simit init
ci` so the CLI is current.

4. **Merge the adoption branch.** `git merge --ff-only
origin/simit-ci-adoption-20260525` onto the working branch
   (post-triage).

5. **Re-verify the flake-drift claim on a clean tree.** Run:

   ```sh
   nix develop -c simit init flake --check --diff
   ```

   If the diff is now small (just hooks-file changes), the
   dossier's "wholesale rewrite" concern was confused by the
   dirty tree — proceed normally. If the diff is still
   wholesale-rewrite, fall back to the phase 03 hooks-only path
   (or document as awaiting phase 03 if not landed).

6. **Add `CHANGELOG.md`.** Use Keep a Changelog format with a
   `0.1.0 - YYYY-MM-DD` entry summarizing the first-publish
   scope. If the project prefers per-crate changelogs, instead
   add a top-level pointer file explaining the convention and add
   per-crate `CHANGELOG.md` entries for `0.1.0`.

7. **Regenerate workspace CI:**

   ```sh
   nix develop -c simit init ci \
     --platform forgejo \
     --runtime cargo \
     --runner atlas \
     --workspace \
     --with-audit --with-deny --with-docs
   ```

   (Or bare `simit init ci` if phase 01 has landed and config is in
   place.)

8. **Document publish order.** If phase 06 has landed:

   ```sh
   nix develop -c simit release plan
   ```

   Capture the output in `RELEASE.md` or `CHANGELOG.md`. If
   phase 06 has not landed, compute the order manually from
   `cargo metadata` and document it. The 8 crates have local path
   dependencies; the leaf crates (`sorrel-io`, `sorrel-cache`,
   etc.) must publish before the umbrella (`sorrel`).

9. **First-publish dry-run per crate.** In the computed order:

   ```sh
   nix develop -c cargo package -p sorrel-io --allow-dirty
   nix develop -c cargo package -p sorrel-cache --allow-dirty
   nix develop -c cargo package -p sorrel-compute --allow-dirty
   nix develop -c cargo package -p sorrel-gpu --allow-dirty
   nix develop -c cargo package -p sorrel-data --allow-dirty
   nix develop -c cargo package -p sorrel-render --allow-dirty
   nix develop -c cargo package -p sorrel-ui --allow-dirty
   nix develop -c cargo package -p sorrel --allow-dirty
   ```

   And `cargo publish --dry-run -p <name>` for each. The dry-run
   for downstream crates will fail because path dependencies are
   not yet on crates.io — note this in the PR description; it is
   expected for first publish.

10. **Confirm crate-name availability.** `cargo search <name>
--limit 1` per crate. If any name is taken, flag for the user;
    do not silently rename.

11. **Run chaperone bar checks.** Prefer `simit release verify`
    if phase 05 is landed; otherwise the manual sequence.

12. **PR.** Title: `Prepare sorrel for first multi-crate publish`.
    Body covers: dirty-worktree triage decisions, simit input
    bump, adoption-branch merge, flake-drift re-verification
    result, CHANGELOG addition, regenerated CI, publish-order
    documentation, crate-name availability status.

## Acceptance criteria

- [ ] `git status --short` is empty.
- [ ] `nix develop -c simit --version` matches current `trunk`.
- [ ] Adoption-branch content is on the working branch.
- [ ] `simit init ci --workspace --platform forgejo --check
--diff` is clean.
- [ ] `simit init flake --check --diff` is clean OR the flake
      drift has been re-verified on a clean tree and either
      addressed (phase 03 path) or explicitly documented as
      awaiting phase 03.
- [ ] `CHANGELOG.md` exists with the intended first-publish entry,
      or the per-crate changelog convention is documented.
- [ ] `cargo package -p <crate>` succeeds for all 8 crates.
- [ ] Publish order is documented in the repo.
- [ ] PR description records the crate-name availability check
      results for each of the 8 names.

## Files likely touched

- `flake.nix`, `flake.lock` (simit input bump)
- `CHANGELOG.md` (new)
- `RELEASE.md` (new, optional, for publish order)
- `.forgejo/workflows/ci-sorrel-*.yaml`
- `.forgejo/workflows/publish-crate-sorrel-*.yaml`
- `crates/sorrel-*/Cargo.toml` (if version/metadata edits are part
  of the triaged worktree changes)
- Whatever else lands from the dirty-worktree triage in step 2

## Pitfalls

- **Do not commit the dirty worktree as one giant catch-all.**
  Triage with the user; group by intent; one or more scoped
  commits with maintainer-readable messages.
- **Do not skip the flake-drift re-verification on a clean tree.**
  The dossier flags this claim as "verify before implementation"
  because the original check was done on a dirty worktree.
- **Do not invent a publish order.** Use `cargo metadata` (or
  phase 06's `simit release plan`) as the authoritative source.
- **Do not claim crate-name availability without checking.** All
  8 names must be free on crates.io before any of them can be
  published.
- **Do not expect downstream `cargo publish --dry-run` to succeed
  on first publish.** Path dependencies that are not yet on
  crates.io will fail the dry-run; that is expected and only
  resolves when each leaf crate goes live in order.

## Reference

- Research dossier: per-repo cleanups (sorrel); flake-drift claim
  flagged as "verify before implementation".
- Phase 06 (`simit release plan` — workspace order).
- Phase 07 README.
- Phase 11 sub-05 in
  `/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/sub-05-sorrel.md`
  (predecessor).

# Plan: hooks-enforcement-followups

> **Recommended Codex model for plan-set orchestration: GPT 5.5 medium**
>
> Three independent follow-ups surfaced by the `hooks-enforcement`
> verify pass. None require cross-repo design coordination beyond
> the simit → canix release propagation in phase 03; sequencing is
> straightforward. `medium` is sufficient — there's no novel
> design content, just three discrete cleanups to ship.

## Scope

Close out the three `missed-signal:` / operational surprises
recorded in `../hooks-enforcement/.calibration.json` after the
hooks-enforcement plan verified `shipped`:

1. **`simit hooks install`** is not defensive about a stale local
   `core.hooksPath`. Phase 04 execution wrote
   `core.hooksPath = .git/hooks` into all six affected repos
   (rogue write, not by simit or pre-commit), which bypassed the
   canix dispatcher and silently broke the AI-strip system step.
   The installer should detect this and either warn or auto-fix.
2. **`simit projects scan`** skips registry entries whose paths no
   longer exist on disk (`/data/nvme0/can/Projects/skillctl` was
   the case in point). Behavior is "correct" but invisible — the
   stale entry kept showing `hooks: installed` indefinitely.
   Either prune by default, always report missing paths, or flag
   them in `projects list`.
3. **Installed simit profile is stale.** Canix's home-manager
   profile is pinned to simit 0.15.2; local `Cargo.toml` is
   already at 0.15.3 (unreleased). 0.15.2 cannot parse the new
   `hooks = "conflicted"` value in the registry, so any user who
   runs `simit` from PATH after a registry rewrite gets a parse
   error. Release 0.15.3 and propagate the bump into canix.

## Current state

- `hooks-enforcement` plan: verified `shipped` (5/5 phases passed)
  with two `missed-signal:` surprises and one operational note
  (items 1, 2, 3 above) — see
  [`../hooks-enforcement/.calibration.json`](../hooks-enforcement/.calibration.json).
- Six affected repos (detritus, open-data-license,
  rs-memory-admission, simit, skillnet, sorrel) have their local
  `core.hooksPath` unset — the global canix dispatcher governs
  end-to-end. AI-strip + project hooks both fire correctly.
- `simit projects show /data/nvme0/can/Projects/skillctl` still
  reports `hooks: installed` even though the directory is gone.
- `simit --version` on `$PATH` reports 0.15.2; local checkout
  Cargo.toml says 0.15.3.

## Phase table

| Phase | File                                                                                 | Depends on | Touches                                                               | Blocking?      | Parallel with |
| ----- | ------------------------------------------------------------------------------------ | ---------- | --------------------------------------------------------------------- | -------------- | ------------- |
| 01    | [01-simit-defensive-installer.md](./01-simit-defensive-installer.md)                 | —          | simit repo (`src/commands/hooks.rs`, tests)                           | yes (gates 03) | 02            |
| 02    | [02-simit-scan-surfaces-missing-paths.md](./02-simit-scan-surfaces-missing-paths.md) | —          | simit repo (`src/commands/projects.rs`, `src/registry.rs`, tests)     | yes (gates 03) | 01            |
| 03    | [03-release-and-propagate-to-canix.md](./03-release-and-propagate-to-canix.md)       | 01, 02     | simit repo (CHANGELOG, version bump if needed, tag), canix flake.lock | no (terminal)  | —             |

Per-phase model recommendations (also surfaced in each phase
file's callout block): 01=`5.5 medium`, 02=`5.5 medium`,
03=`5.5 medium`.

## Parallelism layer

**Wave 0** (start from current tree):

- **01** (defensive installer) and **02** (scan surfaces missing
  paths) run in parallel — both touch the simit repo but in
  different files (`src/commands/hooks.rs` vs.
  `src/commands/projects.rs` / `src/registry.rs`). No shared-file
  contention. CHANGELOG.md is touched by both; merge order
  determines which entry lands first.

**Wave 1** (unlocked after 01 and 02 both land):

- **03** (release & propagate) — bumps the simit version if
  needed, tags, watches the publish workflow, then bumps simit
  in canix via `update-canix` flow. Cannot start before 01+02
  because the release should contain both fixes.

**Plan exhausted** after wave 1.

## External repo coordination

Two repos:

- `/data/nvme0/can/Projects/simit` — phases 01, 02, 03 (commit/tag).
- `/data/nvme0/can/Projects/canix` — phase 03 (flake.lock bump,
  activation).

Phase 03 uses the `update-canix` skill flow: push simit, run
`nix flake update simit` in canix, commit the lock bump, activate.
Activation propagates the new `simit` binary to PATH.

## Shared-file lockstep

- 01 and 02 both append to simit's `CHANGELOG.md` `[Unreleased]`.
  Trivial merge; whichever lands first, the other rebases.
- 03 finalizes the `[Unreleased]` section into a versioned entry
  — must run after both 01 and 02 have added their lines.

## Whole-set acceptance criteria

- [ ] `simit hooks install` in a repo with
      `core.hooksPath = .git/hooks` (local) AND a friendly
      dispatcher as the effective system path either warns
      loudly or auto-unsets the rogue local value (per
      phase 01's chosen design).
- [ ] `simit projects scan` (no flags) surfaces every registry
      entry whose path no longer exists, either by pruning them
      or by printing them as `missing` in the output. Stale
      entries no longer silently report misleading feature
      states.
- [ ] `simit projects list` flags missing-path entries visibly
      (e.g. in the attention footer added by hooks-enforcement
      phase 05).
- [ ] `simit --version` on the user's PATH reports `0.15.3` (or
      whatever version contains both fixes) after canix
      activation.
- [ ] `simit projects show /data/nvme0/can/Projects/skillctl`
      (the canonical stale entry) either reports `missing` /
      doesn't exist after the post-release scan-and-prune cycle,
      or is removed entirely.

## Global constraints

- Do not modify the canix dispatcher (`hooks/dispatcher.sh`) in
  this plan set. It's working correctly; any change risks
  reopening the verify gap.
- Do not change the simit feature-status enum shape — phase 02
  is a behavioral fix on scan, not a schema change.
- Phase 03 must follow simit's existing release decorum (CI
  publish on tag, no manual `cargo publish`). Re-read the
  prior `publish-version-extractor-fix` plan if release flow has
  changed since the hooks-enforcement work landed.

## Reference

- Triggering verify report and surprises:
  [`../hooks-enforcement/.calibration.json`](../hooks-enforcement/.calibration.json).
- Prior plan (now retired-ready pending these followups):
  [`../hooks-enforcement/README.md`](../hooks-enforcement/README.md).
- Simit version source: `Cargo.toml` (workspace package version).
- Canix simit input: `flake.nix` (input `simit`) and
  `home/user/can/personal-pc/simit.nix`.
- Update-canix flow: `update-canix` skill.

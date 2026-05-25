# Phase 03 — Release simit and propagate to canix

> **Recommended Codex model: GPT 5.5 medium**
>
> Sub-agent release orchestration: finalize CHANGELOG, confirm
> the version bump, tag, push, watch publish-crate workflow,
> then bump the canix flake input via the `update-canix` skill
> flow and activate. Each step has small judgement calls
> (version-bump semantics, publish workflow retry, lock-bump
> commit message) but no novel design. `low` would skip the
> publish workflow watch and prematurely move to canix; `high`
> is wasted unless the publish workflow fails in a new way.

## Working tree

Multi-repo:

- `/data/nvme0/can/Projects/simit` — release tagging.
- `/data/nvme0/can/Projects/canix` — flake.lock bump and
  activation.

## Goal

The simit version on the user's PATH (`simit --version`) reports
the new release that contains both phase-01 (defensive
installer) and phase-02 (missing-path surfacing) work. The
canix flake input is updated, the lock change is committed, and
the home-manager activation has propagated the new binary.
`simit projects show /data/nvme0/can/Projects/skillctl`
reflects the new missing-path surfacing behavior.

## Why this matters now

After phases 01 and 02 land, the local simit checkout will have
two new behaviors. The installed simit profile (currently
`0.15.2`) cannot parse the new `hooks = "conflicted"` value
introduced by hooks-enforcement phase 01, let alone use the new
`--fix` flag or `missing` surfacing. Every `simit` invocation
from PATH is silently outdated until canix is bumped and
activated. Until then, the user's only working simit is
`./target/release/simit` from the local checkout.

Originating operational note:
[`../hooks-enforcement/.calibration.json`](../hooks-enforcement/.calibration.json)
`note: installed simit profile (0.15.2) is stale...`.

## Out of scope

- Releasing canix itself (no canix version bump needed; this is
  just a flake input update + activation).
- Changing the publish workflow or simit's release infrastructure
  (use whatever exists today — see
  `../publish-version-extractor-fix/`).
- Backporting fixes to older simit versions. Forward-roll only.
- Verifying simit on other canix hosts. Only the user's primary
  PC needs activation as part of this phase; other hosts pick up
  the new simit on their next normal activation cycle.

## Plan

1. **Determine the target version.**
   `/data/nvme0/can/Projects/simit/Cargo.toml` reports `0.15.3`
   (unreleased). Confirm by `grep '^version' Cargo.toml` in the
   simit root. The two unreleased fixes from phases 01 and 02
   are both small behavioral improvements (no breaking API
   changes) — `0.15.3` is the right next semver. Only bump
   further (to `0.16.0`) if either phase landed something that
   removes a public API surface, which neither did.

2. **Finalize CHANGELOG.** Open
   `/data/nvme0/can/Projects/simit/CHANGELOG.md`. Phases 01 and
   02 each added entries under `## [Unreleased]`. Promote them:
   - Replace `## [Unreleased]` with `## [0.15.3] - <YYYY-MM-DD>`
     (use today's date, in absolute form).
   - Add a fresh empty `## [Unreleased]` above it.
   - Add the comparison link at the bottom (mirror existing
     format from `## [0.15.2]` and earlier).

   Verify with `cargo run -- changelog show 0.15.3` (or
   whatever the existing simit changelog inspection command is).

3. **Pre-flight gates** in the simit checkout:

   ```sh
   cd /data/nvme0/can/Projects/simit
   cargo fmt --all -- --check
   cargo test --all-features
   cargo clippy --all-targets --all-features -- --deny warnings
   cargo package --list
   ```

   All must pass. If clippy fires unexpectedly, fix and re-run.
   Do NOT skip with `--allow-dirty` or `--no-verify`.

4. **Commit and tag.** Branch should be `trunk` (verify with
   `git branch --show-current`). Make a release commit:

   ```sh
   git add CHANGELOG.md
   git commit -m "release: simit 0.15.3"
   git tag v0.15.3
   git push origin trunk
   git push origin v0.15.3
   ```

   If a `simit release` subcommand exists and is the project's
   conventional release path, use it instead of manual tag
   push — re-read the project's `release-maintenance.md` mdBook
   page if uncertain.

5. **Watch the publish-crate workflow.** Codeberg hosts simit's
   CI. Use the `berg-codeberg-ci` skill to find the
   tag-triggered publish workflow run:

   ```sh
   berg run list --workflow publish-crate-simit.yaml | head -5
   berg run watch <run-id>
   ```

   Expected: the workflow publishes simit `0.15.3` to crates.io.
   If publish fails on a `cargo publish` step, diagnose via the
   `rust-crate-publish-workflow` skill — typical failures are
   version-already-exists (the tag was pushed twice) or
   transient registry timeouts.

   Block on success — do not move to step 6 until crates.io
   shows `0.15.3` at
   <https://crates.io/crates/simit/versions>.

6. **Bump simit in canix.** Use the `update-canix` skill flow:

   ```sh
   cd /data/nvme0/can/Projects/canix
   nix flake update simit
   nix flake check --no-build  # eval-only sanity
   git diff flake.lock          # confirm simit input bumped to v0.15.3 rev
   git add flake.lock
   git commit -m "flake.lock: bump simit to 0.15.3"
   ```

   Do not push canix unless your usual flow requires remote
   activation — per the `update-canix` skill, local activation
   is sufficient for the user's own host.

7. **Activate canix.** Use whatever the user's normal
   activation command is — `canix activate`,
   `home-manager switch --flake .`, or `nh home switch .`. Per
   the canix-cli skill, `canix activate` is canonical:

   ```sh
   canix activate
   ```

   On success, the user's `$HOME/.nix-profile/bin/simit` (or
   equivalent) now points at the new derivation.

8. **Verify end-to-end.**

   ```sh
   simit --version
   # Expect: simit 0.15.3
   simit projects scan
   # Expect: includes "N project(s) missing on disk..." line
   #         listing /data/nvme0/can/Projects/skillctl among others.
   simit projects show /data/nvme0/can/Projects/skillctl
   # Expect: attention header "missing" + cached features.
   ```

   In one of the affected repos (e.g. detritus), exercise the
   defensive installer (no rogue local config currently, but
   set one temporarily to prove the warning fires):

   ```sh
   cd /data/nvme0/can/Projects/detritus
   git config --local core.hooksPath .git/hooks
   simit hooks install 2>&1 | grep -i "rogue\|shadows\|dispatcher"
   # Expect: warning text from phase 01.
   simit hooks install --fix
   git config --local --get core.hooksPath || echo "(unset)"
   # Expect: (unset).
   ```

9. **Update the prior hooks-enforcement plan's calibration
   sidecar.** The release closes out the operational note from
   the verify pass; record that:

   ```sh
   cd /data/nvme0/can/Projects/simit/docs/src/planning/hooks-enforcement
   jq '.verify.surprises |= sub("note: installed simit profile.*Package the new simit release before declaring the fleet ready\\."; "note (resolved 2026-XX-XX): simit 0.15.3 released and activated on host; PATH simit reports 0.15.3 and parses the new feature states correctly.")' .calibration.json > .calibration.json.tmp \
     && mv .calibration.json.tmp .calibration.json
   ```

   (Adjust the regex and replacement to match the actual text
   in the surprises field. The intent is to mark the
   operational note as resolved without rewriting the original
   record.)

## Acceptance criteria

- [ ] `simit --version` on PATH reports `0.15.3`.
- [ ] `crates.io/crates/simit/versions` shows `0.15.3`
      published.
- [ ] `git -C /data/nvme0/can/Projects/canix log -1 flake.lock`
      shows the simit input bump commit.
- [ ] `canix activate` completed without errors (most recent
      generation in `home-manager generations` is from this
      session).
- [ ] `simit projects scan` (no flags) prints at least one
      "N project(s) missing on disk" line.
- [ ] `simit projects show /data/nvme0/can/Projects/skillctl`
      prints the attention header with `missing`.
- [ ] In detritus, with a deliberately-set local
      `core.hooksPath = .git/hooks`,
      `simit hooks install` warns about the rogue value and
      `simit hooks install --fix` unsets it.
- [ ] After the activation, no repo has a stale local
      `core.hooksPath` (the user already unset all six earlier;
      this is a re-check, not a re-fix).
- [ ] simit `CHANGELOG.md` `[0.15.3]` section dated today
      includes both phase-01 and phase-02 entries.

## Files likely touched

- `/data/nvme0/can/Projects/simit/CHANGELOG.md` — release entry.
- `/data/nvme0/can/Projects/simit/Cargo.toml` — only if the
  version needs further bumping (likely no; `0.15.3` is
  already there).
- `/data/nvme0/can/Projects/canix/flake.lock` — simit input
  bumped.
- `/data/nvme0/can/Projects/simit/docs/src/planning/hooks-enforcement/.calibration.json`
  — mark operational note as resolved.

No source-code changes in this phase. Source changes belong to
phases 01 and 02.

## Pitfalls

**P1. Tag already exists.** Symptom: `git tag v0.15.3` exits
non-zero. Cause: a prior release attempt left a tag behind.
Recovery: confirm with `git show v0.15.3` that the tag points
at the right commit. If yes, just `git push origin v0.15.3`.
If the tag is wrong, `git tag -d v0.15.3` (local) and re-tag;
do NOT delete the remote tag if it was already pushed and a
publish workflow has run against it.

**P2. Publish workflow fails on tag re-push.** Symptom:
publish CI reports "version already exists on crates.io".
Cause: simit `0.15.3` was published in a prior aborted attempt.
Recovery: bump to `0.15.4` (no source change required, just
CHANGELOG header + Cargo.toml). Document the dead `0.15.3` as a
note in the new entry.

**P3. `nix flake update simit` produces no diff.** Symptom: the
input was already on a flake-rev that includes our commits.
Cause: the input URL specifies a branch that the user has
already manually bumped. Recovery: confirm via
`grep -A 5 '"simit"' flake.lock` that the `rev` matches the
new tag's commit. If yes, the update is a no-op and step 6's
commit is skipped — proceed to activation.

**P4. Activation activates an older simit derivation.**
Symptom: `simit --version` still says `0.15.2` after
activation. Cause: a binary cache hit served an older
derivation, or the activation didn't re-eval the simit input.
Recovery: `nix store gc-roots | grep simit` to inspect; force
a rebuild with `nix build .#homeConfigurations.<host>.activationPackage --rebuild`
or invalidate the flake input cache with
`nix flake metadata --refresh`.

**P5. Codeberg publish workflow not yet hooked up for simit.**
Symptom: `berg run list` shows no recent runs for
`publish-crate-simit.yaml`. Cause: the workflow may not be
present in simit's `.forgejo/workflows/` (verify with `ls`).
Recovery: this should have been set up by the prior
`publish-version-extractor-fix` plan — re-read that plan's
phase 01/02 to confirm the workflow file exists. If missing,
this phase is blocked on adding it; file a stop-phase and
surface in the chat report.

**P6. CHANGELOG promote loses unreleased entries.** Symptom:
`[0.15.3]` section is missing one of the two unreleased lines.
Cause: rebase mistake in step 2. Recovery: open the file, copy
the missing line in by hand from the most-recent commit that
added it, recommit before tagging.

## Reference

- Triggering operational note:
  [`../hooks-enforcement/.calibration.json`](../hooks-enforcement/.calibration.json),
  field `verify.surprises`, line beginning `note: installed simit profile`.
- Prior release plan:
  [`../publish-version-extractor-fix/`](../publish-version-extractor-fix/).
- `update-canix` skill (canonical flow for bumping a flake
  input that canix consumes).
- `berg-codeberg-ci` skill (for watching the publish workflow).
- `canix-cli` skill (for `canix activate`).
- Simit version source: `Cargo.toml`.

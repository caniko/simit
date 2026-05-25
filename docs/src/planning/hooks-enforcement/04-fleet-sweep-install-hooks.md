# Phase 04 — Fleet sweep: install hooks in all affected projects and prove end-to-end enforcement

> **Recommended Codex model: GPT 5.5 low**
>
> Leaf-node mechanical work: run `simit hooks install` against six
> known project paths, run `simit projects show` to confirm the new
> detector flips them to `installed`, and trigger one deliberate
> clippy violation in detritus to prove `git commit` blocks. No
> design decisions, no novel code, no log interpretation beyond
> matching expected error strings. `medium` is wasted on this; the
> only judgment call is "did the detector report flip from
> `configured` to `installed` for project N", which is a yes/no
> check.

## Working tree

Multi-repo. The agent running this phase will `cd` between project
directories. Primary working tree for status tracking:
`/data/nvme0/can/Projects/detritus` (where the final acceptance
proof runs).

Per-project working trees:

- `/data/nvme0/can/Projects/detritus`
- `/data/nvme0/can/Projects/open-data-license`
- `/data/nvme0/can/Projects/rs-memory-admission`
- `/data/nvme0/can/Projects/simit`
- `/data/nvme0/can/Projects/skillnet`
- `/data/nvme0/can/Projects/sorrel`

## Goal

Every project that the research dossier flagged as mislabeled
`hooks: installed` (configured but with no actual hooks wired)
ends this phase with:

- `.git/hooks/pre-commit` and `.git/hooks/pre-push` written by
  `simit hooks install`, executable, with the pre-commit framework
  wrapper content.
- `simit projects show <path>` reports `hooks: installed`.
- A deliberate clippy violation in detritus is rejected at
  `git commit`, demonstrating the whole chain works end-to-end.

## Why this matters now

This is the verification phase for the whole plan set. Phases 01
and 02 ship a working installer; phase 03 unblocks installation at
the host level. Until phase 04 runs, no project actually has
working hooks — the chain is theoretical.

Originating symptom that started this work:

```
error: the function has a cognitive complexity of (49/25)
   --> crates/detritus-server/src/logs.rs:230:10
    |
230 | async fn writer_loop(
    |          ^^^^^^^^^^^
```

caught in CI, missed locally. Phase 04 acceptance proves it is now
caught locally.

## Out of scope

- Fixing the cognitive_complexity lint itself in detritus. The
  acceptance check intentionally reintroduces a small violation,
  asserts hooks block it, and reverts. Cleaning up the actual
  long-standing lint is a follow-up.
- Installing hooks in scratch `/tmp/*` projects from the simit
  registry. The dossier explicitly scopes those out.
- Running `simit hooks install` against projects with
  `nix/pre-commit.nix` absent (`rs-modde`, `skillctl` — the
  detector correctly reports them as `absent` or correctly skips
  them; this phase doesn't add config to them).
- Re-running `simit projects scan` as the trigger; that runs as
  step 1 here but is incidental, not the deliverable.

## Plan

1. **Refresh the registry.**

   ```sh
   simit projects scan
   simit projects list --json > /tmp/projects-pre.json
   ```

2. **Per project, install and verify.** For each path in the list
   above (the six affected projects):

   ```sh
   cd <project-path>
   simit hooks install
   simit hooks install --check  # expect exit 0
   simit projects show .        # expect hooks: installed
   ls -la .git/hooks/pre-commit .git/hooks/pre-push
   ```

   If `simit hooks install` reports the warning about an unfriendly
   `core.hooksPath`, phase 03 has not been activated yet — stop
   and resolve before continuing. The warning means the install
   wrote files git won't run.

   Track outcomes in a simple table; a one-line summary per project
   in the final commit message is enough.

3. **End-to-end acceptance in detritus.**

   ```sh
   cd /data/nvme0/can/Projects/detritus

   # Save a known-clean state.
   git status --short  # expect clean
   git stash push -u -m "hooks-enforcement phase-04 acceptance" || true

   # Introduce a deliberate cognitive_complexity violation in a
   # throwaway file (do NOT modify logs.rs:230 which is what we're
   # trying to fix separately).
   cat > /tmp/cog-test.rs <<'EOF'
   pub fn cog_violation(x: i32) -> i32 {
       let mut y = x;
       if x > 0 { if x > 1 { if x > 2 { if x > 3 { if x > 4 {
           if x > 5 { if x > 6 { if x > 7 { if x > 8 { if x > 9 {
               if x > 10 { if x > 11 { if x > 12 { if x > 13 {
                   y += 1;
               }}}}
           }}}}}}}}}}
       y
   }
   EOF
   cp /tmp/cog-test.rs crates/detritus-server/src/cog_test_temp.rs
   # Wire the module so clippy actually lints it.
   echo 'mod cog_test_temp;' >> crates/detritus-server/src/lib.rs
   git add crates/detritus-server/src/cog_test_temp.rs \
           crates/detritus-server/src/lib.rs

   # Now attempt to commit. EXPECT this to fail at pre-commit.
   git commit -m "acceptance: deliberate cog violation (should fail)"
   echo "commit exit code: $?"
   # Expected: non-zero. stderr contains the cognitive_complexity
   # error pointing at cog_test_temp.rs.

   # Clean up.
   git reset HEAD
   rm crates/detritus-server/src/cog_test_temp.rs
   git checkout -- crates/detritus-server/src/lib.rs
   git stash pop || true
   git status --short  # expect clean
   ```

4. **Verify AI-strip system step still works.**

   ```sh
   cd /data/nvme0/can/Projects/detritus
   git commit --allow-empty -m "$(cat <<'EOF'
   acceptance: AI strip smoke test

   Co-Authored-By: Test User <noreply@anthropic.com>
   EOF
   )"
   git log -1 --format=%B
   # Expect: no "Co-Authored-By:" line present.
   git reset --hard HEAD~1  # discard the empty test commit
   ```

5. **Snapshot the post-state.**

   ```sh
   simit projects scan
   simit projects list --json > /tmp/projects-post.json
   diff <(jq '.[] | {path, hooks: .features.hooks}' /tmp/projects-pre.json) \
        <(jq '.[] | {path, hooks: .features.hooks}' /tmp/projects-post.json)
   ```

   Expected diff: the six affected projects flip from `configured`
   (the new post-phase-01 default) → `installed`. `rs-modde` stays
   `absent`. `skillctl`'s stale entry resolves to its true state.

6. **Report.** Concise summary in chat:
   - Per-project verdict (6 lines).
   - Acceptance proof transcript (step 3 commit failure, step 4
     AI-strip success).
   - Any project where install didn't take and why.

## Acceptance criteria

- [ ] For each of the six affected projects (detritus,
      open-data-license, rs-memory-admission, simit, skillnet,
      sorrel): `simit projects show <path>` reports
      `hooks: installed` AND `.git/hooks/pre-commit` exists and is
      executable.
- [ ] `simit hooks install --check` exits 0 in every affected
      project after install (idempotency).
- [ ] In detritus, attempting to commit the deliberate
      cognitive_complexity violation (plan step 3) results in a
      non-zero exit and a stderr message naming
      `cog_test_temp.rs` and `cognitive_complexity`.
- [ ] In detritus, a commit message containing
      `Co-Authored-By: …<noreply@anthropic.com>` is committed with
      that line stripped (plan step 4).
- [ ] No file in any project is left modified after the acceptance
      run (`git status` is clean in every touched repo at end of
      phase).
- [ ] `simit projects list --json` post-state shows zero projects
      with `hooks: conflicted` AND `nix/pre-commit.nix` present.

## Files likely touched

- None permanently. This phase modifies `.git/hooks/` content per
  project (not tracked by git) and runs through a transient
  staging cycle in detritus that is reverted before phase end.

## Pitfalls

**P1. Phase 03 not yet activated on the host.** Symptom: install
runs but the deliberate clippy violation commits successfully.
Cause: git is still routing to the old single-hook
`core.hooksPath`. Recovery: check
`git config --global --get core.hooksPath` resolves to a path
containing both `pre-commit` and `commit-msg`. If only
`commit-msg`, run `home-manager switch` (or canix activate).

**P2. Project hooks fire but lint the wrong target.** Symptom:
commit succeeds despite the violation. Cause: the deliberate
violation is in a file the project's clippy invocation excludes
(workspace globs, `[lints]` table overrides). Recovery: read the
project's `cargo clippy` invocation in `nix/pre-commit.nix` and
make sure the synthetic file is in scope. For detritus, the entry
in the generated `.pre-commit-config.yaml` runs
`cargo clippy --all-targets --all-features -- --deny warnings`,
which lints everything.

**P3. `git stash pop` conflicts.** Symptom: the post-acceptance
cleanup leaves merge conflict markers. Cause: another change
landed in detritus during the phase. Recovery: resolve manually,
do not rerun. The phase is otherwise complete.

**P4. `simit hooks install` overwrites a hand-edited
`.git/hooks/pre-commit`.** Symptom: a developer's bespoke hook
disappears. Cause: pre-commit `install --overwrite` is destructive
by design. Recovery: this phase assumes none of the six target
projects have bespoke hooks (none currently do — the dossier
audit confirmed `.git/hooks/` contained only `.sample` files in
every project). Spot-check before each install.

**P5. The `lib.rs` edit in step 3 conflicts with detritus's
actual lib.rs.** Symptom: `echo 'mod ... ;' >> lib.rs` produces
invalid Rust because lib.rs is `#![no_implicit_prelude]` or has
`#![deny(missing_docs)]`. Cause: the synthetic violation is too
naive. Recovery: instead of appending a module, modify an
_existing_ function in a leaf file to add deep nesting. Revert
via `git checkout --` after. Adjust step 3 if the appended-module
form doesn't compile.

## Reference

- Research dossier: [hooks-enforcement-research.md](./hooks-enforcement-research.md)
- Phase 02 (installer): [02-simit-hooks-install-subcommand.md](./02-simit-hooks-install-subcommand.md)
- Phase 03 (dispatcher): [03-canix-dispatcher-hooks.md](./03-canix-dispatcher-hooks.md)
- Originating CI failure context:
  `crates/detritus-server/src/logs.rs:230` (in detritus).

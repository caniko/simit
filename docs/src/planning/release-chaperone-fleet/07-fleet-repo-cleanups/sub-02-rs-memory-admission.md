# Phase 07.02 — `rs-memory-admission` cleanup for chaperone

> **Recommended Codex model: GPT 5.4 / medium**
>
> Already-published crate; sub-layer is mostly
> infrastructure-reconciliation. One open decision (cargo-audit
> pre-commit hook: keep or drop) the sub-layer flags. Sub-agent
> role on moderate work — 5.4 at `medium`.

## Working tree

[/data/nvme0/can/Projects/rs-memory-admission](file:///data/nvme0/can/Projects/rs-memory-admission)
— feature branch off `trunk`. Remote:
`ssh://git@codeberg.org/caniko/rs-memory-admission.git`.

## Goal

`rs-memory-admission` is chaperone-ready:

- `simit` flake input tracks current `trunk`;
- adoption-branch content is on `origin/trunk`;
- local worktree is clean (`.forgejo/workflows/pages.yaml` and
  `docs/src/development/nix.md` either committed or reverted; the
  `.claude/` directory either committed, ignored via
  `.gitignore`, or removed);
- `simit init flake --check --diff` is clean; if the generator
  wants to remove the `cargo-audit` pre-commit hook, the choice
  is recorded in the PR description (keep via opt-out, or accept
  the swap to `cargo-msrv`).

Crate `memory-admission 0.1.7` is already live on crates.io; the
chaperone path here is "release infrastructure reconciliation",
not a new publish, unless the user explicitly wants to bump to
`0.1.8` to ship something user-visible.

## Why this matters now

The dossier flags rs-memory-admission as the "already-published"
case: most of the chaperone bar is about whether the next release
can ship cleanly, not about retroactively republishing `0.1.7`.
But the local worktree is dirty and the flake-hook drift will keep
flagging the repo until reconciled.

## Out of scope

- Republishing `0.1.7`.
- Bumping to `0.1.8` without a user-visible change motivating it.
- Changing the `cargo-audit` policy unilaterally (decision is
  flagged for the user).

## Plan

1. **Re-verify the snapshot.** From the working tree:

   ```sh
   git fetch origin
   git status --short
   git diff
   ls -la .claude 2>/dev/null
   git log --oneline --left-right origin/trunk...origin/simit-ci-adoption-20260525
   ```

2. **Clean the worktree.** For each dirty file:
   - `.forgejo/workflows/pages.yaml`: decide whether the change
     is a real fix or accidental; commit or revert with a
     one-line note in the PR.
   - `docs/src/development/nix.md`: same triage.
   - `.claude/`: untracked agent state directory. Add to
     `.gitignore` if it should not be tracked; otherwise commit
     or remove.

3. **Bump the simit flake input** to current `trunk` (same
   pattern as sub-01 step 2). `nix flake update simit`.

4. **Merge the adoption branch.** `git merge --ff-only
origin/simit-ci-adoption-20260525` onto a working branch.

5. **Regenerate CI** with the appropriate flag set (or bare if
   phase 01 has landed and the persisted config is in place):

   ```sh
   nix develop -c simit init ci \
     --platform forgejo \
     --runtime cargo \
     --runner atlas \
     --with-audit --with-deny --with-docs --with-msrv
   ```

6. **Run `simit init flake --check --diff`.** If it proposes to
   remove the `cargo-audit` pre-commit hook:
   - decide with the user whether to accept the removal (move
     audit to CI only, where it already runs) or opt out via a
     repo-local override;
   - if phase 03 has landed with the `[flake].scope` schema, set
     `[flake].scope = "full"` and either accept the hook swap or
     patch the generator (out of scope here — the right fix is in
     simit, not in the repo).
   - if neither phase 03 nor an opt-out mechanism is available
     yet, record the decision in the PR description and let the
     drift persist as a known-state until the simit-side fix
     lands.

7. **Run the chaperone bar checks:**

   ```sh
   nix develop -c simit init flake --check --diff
   nix develop -c simit release trust check
   nix develop -c simit init ci --platform forgejo --check --diff
   nix develop -c cargo package --list
   nix develop -c cargo publish --dry-run
   ```

   Or `simit release verify` if phase 05 has landed.

8. **PR.** Title: `Reconcile simit-managed infrastructure for
chaperone`. Body covers: worktree cleanup decisions, simit
   input bump, adoption-branch merge, CI regen results, the
   cargo-audit pre-commit decision, chaperone-bar check results.

## Acceptance criteria

- [ ] `git status --short` is empty.
- [ ] `nix develop -c simit --version` matches current `trunk`.
- [ ] Adoption-branch content is on the working branch.
- [ ] `simit init ci --check --diff` is clean (bare or with the
      documented flag set).
- [ ] `simit init flake --check --diff` is clean OR the
      remaining hook drift is explicitly documented as awaiting
      a simit-side fix (phase 03 + the cargo-audit policy decision).
- [ ] `simit release trust check` passes.
- [ ] `cargo package --list` and `cargo publish --dry-run` succeed.
- [ ] PR description records the cargo-audit policy decision.

## Files likely touched

- `flake.nix`, `flake.lock`
- `.forgejo/workflows/ci.yaml`
- `.forgejo/workflows/publish-crate.yaml`
- `.forgejo/workflows/pages.yaml` (committed or reverted)
- `docs/src/development/nix.md` (committed or reverted)
- `.gitignore` (if `.claude/` is added)
- `nix/pre-commit.nix` (potentially, depending on the audit decision)

## Pitfalls

- **Do not publish `0.1.7` again.** It is already live; the
  generated publish workflow's "already-published" gate
  (introduced in `0.14.1`) should handle this gracefully, but
  do not trigger it deliberately.
- **Do not silently delete the `.claude/` directory** if the user
  did not ask for it. If unsure, add to `.gitignore` and leave
  the files in place.

## Reference

- Research dossier: per-repo cleanups (rs-memory-admission).
- Phase 07 README.
- Phase 11 sub-02 in
  `/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/sub-02-rs-memory-admission.md`
  (predecessor).

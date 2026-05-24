# Phase 04 — Sweep latent dependents

> **Recommended Codex model: GPT 5.5 low**
>
> Trivial mechanical work, leaf role: run `simit init ci` (or apply
> the one-line patch directly) in four repositories, commit, push.
> Each repo is independent and the change per repo is a single line.
> The only judgement call is "the working tree is dirty; should I
> interleave with the user's in-flight work?" — that lands as a
> per-repo "defer if dirty" rule, not as design content. `medium`
> would over-spend on what is essentially four scripted steps.

## Working tree

Per sub-task (run each from its own repo):

- `/data/nvme0/can/Projects/open-data-license`
- `/data/nvme0/can/Projects/rs-memory-admission`
- `/data/nvme0/can/Projects/simit` (simit's own
  `.forgejo/workflows/publish-crate.yaml`)
- `/data/nvme0/can/Projects/skillnet`

## Goal

Every latent-affected dependent that the registry sweep identified
either has its `publish-crate*.yaml` regenerated with the fixed
extractor and committed, or carries an explicit deferred note in its
working tree (e.g. a `TODO` referencing the dossier) with a reason the
user accepted.

## Why this matters now

All four affected repos are single-crate today, so the bug is
**latent** — `grep -m1 -o` over single-line JSON returns one match
because there's only one package. The moment any of them adopts a
workspace layout the bug becomes a hard CI failure (detritus's exact
story). Cleaning up while the fix is fresh and the dossier is live
avoids re-doing the discovery later.

Registry evidence from the prior `simit-dependent-fixes` sweep:

```
open-data-license   .forgejo/workflows/publish-crate.yaml:44  (nix runtime)
rs-memory-admission .forgejo/workflows/publish-crate.yaml:50
simit               .forgejo/workflows/publish-crate.yaml:48
skillnet            .forgejo/workflows/publish-crate.yaml:48
```

## Out of scope

- Touching any repo not on the list (`rs-modde` and `sorrel` are
  clean; `/tmp/nix-shell.*` entries are ephemeral scratch).
- Pushing to `trunk`/`main` on any repo without the user's review of
  the diff.
- Bundling the regen with unrelated work-in-progress changes in the
  same commit.

## Plan

For each repo in the list, run this sub-procedure (in any order, all
four are independent):

1. Check git status:
   ```sh
   git -C <repo> status --short
   ```
2. If `.forgejo/workflows/publish-crate.yaml` is **already modified**
   for unrelated reasons (this is the case for
   `open-data-license` per the earlier sweep), STOP and either:
   - Coordinate with the user to reconcile, or
   - Apply the one-line patch directly with a separate commit that
     does not touch any other files.
3. If the working tree is clean (or only contains unrelated files):
   ```sh
   cd <repo>
   simit --version   # expect 0.15.1+ once Phase 01 ships
   simit init ci --platform forgejo --check --diff
   ```
   Confirm the only diff is the one-line extractor change. If the
   diff includes anything else, investigate before regenerating —
   another simit change may be pending unrelated work.
4. Apply:
   ```sh
   simit init ci --platform forgejo
   git -C <repo> diff .forgejo/workflows/publish-crate.yaml
   ```
5. Commit:
   ```sh
   git -C <repo> add .forgejo/workflows/publish-crate.yaml
   git -C <repo> commit -m 'ci: fix publish workflow version extractor (simit 0.15.1)'
   ```
6. Push when the user authorises (don't auto-push these — each repo
   may have a different release / branch protection convention).

### simit's own publish-crate.yaml

Phase 01 publishes simit 0.15.1 *with* its own `publish-crate.yaml`
still on the old extractor. That's safe because simit is single-crate.
Regenerate simit's own workflow as part of this phase to prevent a
future workspace-conversion landmine, and commit the change as a
follow-up to the 0.15.1 release (no new release required just for
this — it's a generated-file refresh).

## Acceptance criteria

- [ ] `grep -rn 'grep -m1 -o' /data/nvme0/can/Projects/{open-data-license,rs-memory-admission,simit,skillnet}/.forgejo`
      returns no hits.
- [ ] Each repo's `publish-crate.yaml` contains the new
      `grep -o '"version":"[^"]*"' | head -n1` form.
- [ ] Every regen is committed in a focused, single-purpose commit
      (no entanglement with unrelated working-tree changes).
- [ ] For any repo where the user declined to land the change now, an
      explicit deferred-work note exists (issue, comment, or
      `simit projects` annotation) referencing the dossier.

## Files likely touched

Per repo:

- `<repo>/.forgejo/workflows/publish-crate.yaml`

That's it. If `simit init ci` proposes any other diff, treat the extra
change as out of scope for this phase.

## Pitfalls

- **Symptom:** `simit init ci --check --diff` reports drift in
  `.forgejo/workflows/ci.yaml` too. **Cause:** unrelated simit
  improvements landed between when the repo was last regenerated and
  now. **Recovery:** in scope only if the user explicitly wants the
  refresh; otherwise narrow the regen by hand-patching just
  `publish-crate.yaml`.
- **Symptom:** repo uses `nix develop -c cargo metadata` (e.g.,
  `open-data-license`). **Cause:** the project's runtime is `nix`.
  **Recovery:** the simit generator handles both runtimes; the fix
  applies to both. Confirm the regenerated line still has the
  `nix develop -c` prefix.
- **Symptom:** committing the regen sweeps in a stray file from a
  dirty working tree. **Cause:** `git add .` instead of an explicit
  path. **Recovery:** always `git add` the exact workflow file path,
  never the directory.

## Reference

- Research dossier:
  [`../publish-workflow-version-extraction-research.md`](../publish-workflow-version-extraction-research.md)
- Prior fleet-sweep report (per-project verdicts) — see the chat
  transcript that produced this plan.
- Related phase: [`01-simit-commit-and-release.md`](./01-simit-commit-and-release.md)
  — must land before this so that `simit --version` reports 0.15.1+
  and downstream regen is reproducible from a released CLI.

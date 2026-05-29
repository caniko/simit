# Phase 07.01 — `open-data-license` cleanup for chaperone

> **Recommended Codex model: GPT 5.4 / medium**
>
> Single-crate repo, well-understood adoption pattern. One open
> decision (sync-up vs. bump for the existing `0.2.0` tag) the
> sub-layer flags for the user but does not make. Sub-agent role
> on moderate work — 5.4 at `medium`.

## Working tree

[/data/nvme0/can/Projects/open-data-license](file:///data/nvme0/can/Projects/open-data-license)
— feature branch off the default branch (`trunk`). Remote:
`ssh://git@codeberg.org/caniko/open-data-license.git`.

## Goal

`open-data-license` is chaperone-ready:

- `simit` is available deterministically (either added as a flake
  input pinned to current `trunk`, or the PATH expectation is
  documented in the repo);
- `origin/simit-ci-adoption-20260525` (or the equivalent up-to-date
  adoption branch at execution time) is merged into `origin/trunk`;
- `simit init ci --platform forgejo --check --diff` is clean (bare
  if phase 01 has landed; full flag set otherwise);
- the existing `0.2.0` remote tag is either confirmed as the
  intended release (in which case the chaperone will use sync-up)
  or replaced by a `0.2.1` bump (decided by the user, not by this
  sub-layer);
- the `v0.1.0` legacy tag is audited and removed if abandoned.

## Why this matters now

`open-data-license` is the simplest sub-layer and a useful
warm-up before the heavier ones. Crate `open-data-license` `0.2.0`
is not on crates.io as of the dossier snapshot; the existing remote
tag plus the empty crates.io state is the exact "sync-up vs. bump"
case the chaperone needs a clean answer to.

## Out of scope

- Publishing the crate. Sub-layer leaves it chaperone-ready; the
  user invokes the chaperone separately.
- Touching `pages.yaml` content. Preserve as the supplementary
  `managed+extra` file.
- Bulk-deleting legacy tags before user confirmation.

## Plan

1. **Re-verify the snapshot.** From the working tree, run:

   ```sh
   git fetch origin
   git status --short
   git branch -a
   git log --oneline --left-right origin/trunk...HEAD
   git log --oneline --left-right origin/trunk...origin/simit-ci-adoption-20260525
   git tag --list
   ```

   Confirm the divergence and the tag set before acting. If the
   adoption branch no longer exists or has been retitled, find its
   equivalent.

2. **Add (or bump) the `simit` flake input.** If the flake has no
   `simit` input, add it pointing at the current simit `trunk`:

   ```nix
   inputs.simit = {
     url = "git+https://codeberg.org/caniko/simit.git?ref=refs/heads/trunk";
     inputs.nixpkgs.follows = "nixpkgs";
   };
   ```

   Wire it into `outputs` and expose it via the devShell so
   `nix develop -c simit --version` returns the current `trunk`
   build, not a PATH-leaked binary. Run `nix flake update simit`.

3. **Merge the adoption branch.** `git merge --ff-only
origin/simit-ci-adoption-20260525` onto a working branch off
   `origin/trunk`. If fast-forward is not possible, rebase the
   adoption commits and explain in the PR description.

4. **Regenerate CI against current simit.** Run:

   ```sh
   nix develop -c simit init ci \
     --platform forgejo \
     --runtime nix \
     --runner atlas-nix-trusted \
     --with-om-ci \
     --with-msrv --with-audit --with-deny --with-docs
   ```

   Commit any resulting diff. If phase 01 has landed, run
   `simit init ci --platform forgejo` (bare) and let the persisted
   config carry the options.

5. **Run the chaperone bar checks locally:**

   ```sh
   nix develop -c simit init flake --check --diff
   nix develop -c simit release trust check
   nix develop -c simit init ci --platform forgejo --check --diff
   nix develop -c cargo package --list
   nix develop -c cargo publish --dry-run
   ```

   If phase 05 has landed, prefer `nix develop -c simit release
verify`.

6. **Audit tags.** `git tag --list` + `git ls-remote --tags
origin`. Identify the `v0.1.0` legacy tag and document in the
   PR whether it should be removed. Do not delete without user
   confirmation.

7. **Flag the `0.2.0` decision.** In the PR description:
   - state whether the current `Cargo.toml` version (`0.2.0`)
     matches the remote tag's target commit;
   - state whether `git diff 0.2.0..HEAD` is release-repair only;
   - present two options to the user: `simit release sync-up` (if
     no meaningful changes since the tag) or bump to `0.2.1` (if
     in-tree commits add user-visible behavior). **Do not pick
     one for the user.**

8. **Push the working branch and open the PR.** Title:
   `Adopt simit-managed CI and prepare for chaperone`. Body
   covers: simit input addition/bump, adoption-branch merge, CI
   regen results, chaperone-bar check results, tag-audit findings,
   open `0.2.0` decision.

## Acceptance criteria

- [ ] `nix develop -c simit --version` in the working tree reports
      a simit version matching current `trunk` and the CLI has
      `init ci` / `init flake` nested subcommands.
- [ ] Adoption-branch content is merged onto the working branch.
- [ ] `nix develop -c simit init ci --platform forgejo --check
    --diff` is clean.
- [ ] `nix develop -c simit init flake --check --diff` is clean
      (or, if phase 03 is not landed, the hooks-file scope is
      explicitly opted into).
- [ ] `nix develop -c simit release trust check` passes.
- [ ] `cargo package --list` and `cargo publish --dry-run` succeed.
- [ ] PR description flags the `0.2.0` sync-up-vs-bump decision
      with the evidence the user needs to decide.
- [ ] PR description documents the `v0.1.0` legacy tag finding.
- [ ] `simit projects show /data/nvme0/can/Projects/open-data-license`
      reports `ci: managed` (or `managed+extra` if `pages.yaml`
      remains).

## Files likely touched

- `flake.nix` (add simit input)
- `flake.lock`
- `.forgejo/workflows/ci.yaml`
- `.forgejo/workflows/publish-crate.yaml`

## Pitfalls

- **Do not delete `v0.1.0` in this PR.** Tag deletion is the
  user's call; flag it, do not act on it.
- **Do not touch `pages.yaml` content.** It is the supplementary
  `managed+extra` file by design.
- **Do not silently choose between sync-up and bump for `0.2.0`.**
  The dossier explicitly calls this out as user-owned.
- **Beware PATH-leaked simit.** Before adding the flake input, the
  repo may have been using whatever `simit` is in PATH. After
  adding the input, verify `nix develop -c which simit` resolves
  inside the Nix closure, not to the host.

## Reference

- Research dossier: per-repo cleanups (open-data-license).
- Phase 07 README.
- Phase 11 sub-01 in
  `/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/sub-01-open-data-license.md`
  (predecessor).
- Chaperone skill.

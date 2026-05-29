# Phase 07.03 — `rs-modde` cleanup for chaperone

> **Recommended Codex model: GPT 5.5 / high**
>
> Hardest sub-layer in the phase: stale simit pin pre-dating the
> CLI restructure (blocks every `simit init ci` invocation until
> bumped), multi-crate workspace (5 publishable + 1 non-publishable),
> overlapping release endpoints (legacy `release.yml` + simit-generated
> `release-artifacts-*.yaml` both write Homebrew), branch hygiene
> (many `worktree-agent-*` branches), and an open architectural
> decision the user must make. Orchestrator role on complex,
> high-stakes work — 5.5 at `high` matches the routing matrix.

## Working tree

[/data/nvme0/can/Projects/rs-modde](file:///data/nvme0/can/Projects/rs-modde)
— feature branch off `trunk`. Remote:
`ssh://git@codeberg.org/caniko/rs-modde.git`.

## Goal

`rs-modde` is chaperone-ready:

- `simit` flake input is bumped to current `trunk` so that
  `nix develop -c simit init ci` works (today it fails with
  `unrecognized subcommand 'init'`);
- `origin/simit-ci-adoption-20260525` is merged onto `origin/trunk`;
- release-endpoint architecture is decided and recorded
  (legacy `release.yml` vs. simit-generated
  `release-artifacts-modde-*.yaml`; no double-write to Homebrew);
- `worktree-agent-*` branches are pruned (after user confirmation);
- `simit init ci --workspace --check --diff` is clean;
- `simit init flake --check --diff` is clean (hooks-only scope is
  fine if phase 03 has landed; otherwise documented opt-out);
- workspace publish order is documented (or `simit release plan`
  agrees on it, if phase 06 has landed).

Crates: `modde-core`, `modde-sources`, `modde-games`, `modde-ui`,
`modde-cli` (all publishable at `0.2.0`); `modde-xtask` at `0.2.0`
with `publish = false`. None at `0.2.0` are live on crates.io
(`0.1.0` is).

## Why this matters now

The dossier names rs-modde as the hardest repo. Until the simit
pin bump lands, the entire validation suite for this repo cannot
even run inside `nix develop`. After the pin bump, the legacy
`release.yml` vs. simit-generated artifacts question must be
resolved before the chaperone touches the publish tag, because
they overlap on Homebrew and would race on the same tag-push
trigger.

## Out of scope

- Publishing `0.2.0` (chaperone does that after this sub-layer).
- Implementing serialized publish-job triggers in the generated
  workflows. That is a future simit-side workflow-generator
  change.
- Migrating away from `release.yml` if the maintainer wants to
  keep it — sub-layer flags the decision, does not force it.

## Plan

1. **Re-verify the snapshot.** From the working tree:

   ```sh
   git fetch origin
   git status --short
   git branch -a | head -40
   git log --oneline --left-right origin/trunk...origin/simit-ci-adoption-20260525
   grep -A2 simit flake.nix | head -10
   ```

2. **Bump the simit flake input** to current `trunk`. Edit
   `flake.nix`, replace the pinned rev under
   `inputs.simit.url`, then:

   ```sh
   nix flake update simit
   nix develop -c simit --version
   nix develop -c simit init --help
   ```

   The last command must list `ci` and `flake` as subcommands.

3. **Merge the adoption branch.** `git merge --ff-only
origin/simit-ci-adoption-20260525` onto a working branch.
   Resolve conflicts with `release.yml` (legacy) carefully — do
   not delete it; the decision in step 5 may keep it.

4. **Regenerate workspace CI** against current simit:

   ```sh
   nix develop -c simit init ci \
     --platform forgejo \
     --runtime nix \
     --runner atlas-nix-trusted \
     --workspace \
     --with-deny --with-docs \
     --with-artifacts \
     --with-homebrew \
     --homebrew-name modde \
     --homebrew-tap https://codeberg.org/caniko/homebrew-modde.git \
     --homebrew-binary modde --homebrew-binary modde-ui \
     --homebrew-description 'Cross-platform game mod manager' \
     --homebrew-homepage https://modde.tartanoglu.com \
     --homebrew-license GPL-3.0-only \
     --homebrew-download-repo caniko/rs-modde \
     --homebrew-archive-pattern 'modde-{version}-{arch}-{os}.tar.gz'
   ```

   (Adjust flags if phase 01 has landed and the persisted config
   captures them.)

5. **Decide release-endpoint architecture.** The repo currently has
   both `release.yml` (1k-line legacy: artifacts + Homebrew + Scoop
   - Flathub + AUR + COPR + Mastodon + Matrix + WINGET) AND
     simit-generated `release-artifacts-modde-*.yaml` (per-crate
     artifacts + Homebrew). They overlap on Homebrew and would
     double-write on the same tag.

   Present the decision to the user as two options in the PR
   description:
   - **A. Retire `release.yml`.** Use simit-generated artifacts;
     lose the Scoop/Flathub/AUR/COPR/Mastodon/Matrix/WINGET
     endpoints. Use only if those endpoints are no longer
     desired.
   - **B. Keep `release.yml`.** Generate simit CI _without_
     `--with-artifacts` / `--with-homebrew` so simit does not
     emit `release-artifacts-*.yaml`. `release.yml` remains the
     single source of artifact publication.

   Do not pick for the user. Mark the PR as
   `chaperone-ready when this is resolved`.

6. **Prune `worktree-agent-*` branches** after user confirmation.
   Generate the list:

   ```sh
   git branch -a | grep worktree-agent
   ```

   PR description names the branches; user authorizes the deletion;
   sub-layer executes:

   ```sh
   git for-each-ref --format='%(refname:short)' 'refs/heads/worktree-agent-*' \
     | xargs -r -n1 git branch -D
   ```

   (Do not push deletes to a remote you do not own.)

7. **Verify cargo-deny policy.** The repo has its own
   `deny.toml`. After the simit `Treat cargo-deny policy as
project-owned` change, the project deny.toml should not be
   regenerated. Confirm `cargo deny check bans licenses sources`
   passes:

   ```sh
   nix develop -c cargo deny check bans licenses sources
   ```

8. **Workspace publish dry-run.** Run `cargo package` per crate
   (in dependency order if known; if phase 06 has landed, use
   `simit release plan`):

   ```sh
   nix develop -c cargo package -p modde-core --allow-dirty
   nix develop -c cargo package -p modde-sources --allow-dirty
   nix develop -c cargo package -p modde-games --allow-dirty
   nix develop -c cargo package -p modde-ui --allow-dirty
   nix develop -c cargo package -p modde-cli --allow-dirty
   ```

   Also `cargo publish -p <name> --dry-run` for each.

9. **Run chaperone bar checks.** Same shape as other sub-layers;
   prefer `simit release verify` if phase 05 is landed.

10. **PR.** Title:
    `Adopt current simit, regen workspace CI, resolve release endpoints`.
    Body covers every decision above with the evidence the user
    needs.

## Acceptance criteria

- [ ] `nix develop -c simit init --help` lists `ci` and `flake`.
- [ ] `nix develop -c simit --version` matches current `trunk`.
- [ ] Adoption-branch content is on the working branch.
- [ ] `simit init ci --workspace --platform forgejo --check
    --diff` is clean (bare or with documented flag set).
- [ ] `simit init flake --check --diff` is clean.
- [ ] `simit release trust check` passes.
- [ ] `cargo deny check bans licenses sources` passes.
- [ ] `cargo package -p <crate>` succeeds for each of the 5
      publishable crates.
- [ ] PR description records the release-endpoint architecture
      decision (option A or B from step 5).
- [ ] PR description lists `worktree-agent-*` branches and the
      user's decision on each.
- [ ] If both `release.yml` and simit `release-artifacts-*.yaml`
      remain, the PR explicitly explains why and how the
      Homebrew double-write is avoided.

## Files likely touched

- `flake.nix`, `flake.lock` (simit input bump)
- `.forgejo/workflows/ci-modde-*.yaml`
- `.forgejo/workflows/publish-crate-modde-*.yaml`
- `.forgejo/workflows/release-artifacts-modde-*.yaml` (created or
  removed depending on step 5)
- `.forgejo/workflows/release.yml` (preserved or removed depending
  on step 5)
- `nix/` (potentially, depending on flake hook reconciliation)
- `deny.toml` (verify only; do not regenerate)

## Pitfalls

- **Do not pick the release-endpoint architecture for the user.**
  This is the central architectural decision in this sub-layer and
  must be the maintainer's call.
- **Do not delete `release.yml` without user authorization.** It
  publishes endpoints simit does not.
- **Do not push branch deletions across remotes.** Local branch
  cleanup is safe; remote-branch deletions need an explicit user
  go.
- **Do not regenerate `deny.toml`.** Per simit `0.15.x`, deny
  policy is project-owned. If the generator tries to regenerate,
  it is a simit bug — file it, do not paper over.
- **Beware path-dep packaging order.** `modde-cli` depends on
  `modde-core` / `modde-sources` / `modde-games` / `modde-ui`;
  package the leaf crates first. Phase 06 (`simit release plan`)
  encodes this if landed.

## Reference

- Research dossier: per-repo cleanups (rs-modde).
- Phase 07 README.
- Phase 11 sub-03 in
  `/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/sub-03-rs-modde.md`
  (predecessor).
- Phase 06 (`simit release plan` — workspace order).

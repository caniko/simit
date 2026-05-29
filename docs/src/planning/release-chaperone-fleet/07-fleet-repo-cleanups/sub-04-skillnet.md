# Phase 07.04 — `skillnet` cleanup for chaperone

> **Recommended Codex model: GPT 5.4 / medium**
>
> Single-crate repo on a current default branch. Most of the
> "drift" is the option-persistence gap (phase 01 dependency).
> Flake-drift is real but covered by phase 03's hooks-only mode.
> Sub-agent role on moderate work — 5.4 at `medium`.

## Working tree

[/data/nvme0/can/Projects/skillnet](file:///data/nvme0/can/Projects/skillnet)
— feature branch off the default branch (`main`). Remote:
`ssh://git@codeberg.org/caniko/skillnet.git`.

## Goal

`skillnet` is chaperone-ready:

- `simit` flake input tracks current `trunk`;
- `simit init ci --check --diff` is clean — bare if phase 01 has
  landed (preferred), otherwise via a repo-local
  `simit.toml [ci]` forward-compatible with phase 01's schema;
- `simit init flake --check --diff` does not propose a destructive
  rewrite — either phase 03 has landed and the repo opts into
  hooks-only, or the current full-scope drift is documented as
  awaiting phase 03;
- `simit release trust check` passes;
- crate `skillnet` (max live version `0.4.0`) can be published as
  `0.5.1` (or whatever the maintainer's intended bump) — verified
  by `cargo package --list` and `cargo publish --dry-run`.

## Why this matters now

The dossier documents skillnet as the canonical "spurious drift"
case: the `ci=drift` flag in `simit projects list` clears the
moment the full flag set is passed to `--check`. The flake-drift
is more substantive because skillnet consumes the shared
`rs-harbor` flake plus `home-manager` and `advisory-db` inputs;
the default `simit init flake` wants to remove them.

## Out of scope

- Publishing `0.5.1`.
- Changing skillnet's choice to consume the `rs-harbor` flake.
- Re-routing skillnet to a different runner.

## Plan

1. **Re-verify the snapshot.**

   ```sh
   git fetch origin
   git status --short
   git log --oneline --left-right origin/main...HEAD
   git log --oneline --left-right origin/main...origin/simit-ci-adoption-20260525
   git tag --list
   ```

2. **Bump the simit flake input** to current `trunk` (same pattern
   as sub-01 step 2). `nix flake update simit`.

3. **Handle adoption-branch state.** At dossier snapshot time
   `origin/simit-ci-adoption-20260525` was behind `main` by one
   commit (post-adoption work landed). Verify the current state:
   - if the adoption branch is behind `main`, no merge needed;
   - if it has new commits, merge them forward.

4. **Resolve the CI "drift".** Two paths:
   - **Phase 01 landed**: run `simit init ci --platform forgejo
--runtime cargo --runner atlas --with-audit --with-deny
--with-docs --with-msrv` once to populate `simit.toml [ci]`;
     thereafter bare `simit init ci --check --diff` is clean.
   - **Phase 01 not landed**: add a `simit.toml [ci]` skeleton
     by hand using the forward-compatible schema documented in
     phase 01. The next time phase 01 lands and someone runs
     `simit init ci`, the schema is honored.

5. **Resolve the flake "drift".** Two paths:
   - **Phase 03 landed**: set `[flake].scope = "hooks-only"` in
     `simit.toml`. Run `simit init flake --check --diff`; only
     `nix/pre-commit.nix` is compared.
   - **Phase 03 not landed**: document the flake drift as a
     known-state in the PR description, awaiting phase 03.
     `simit init flake --check --diff` will remain non-clean
     until then; do not "fix" by accepting the destructive
     rewrite.

6. **Run chaperone bar checks:**

   ```sh
   nix develop -c simit init flake --check --diff
   nix develop -c simit release trust check
   nix develop -c simit init ci --platform forgejo --check --diff
   nix develop -c cargo package --list
   nix develop -c cargo publish --dry-run
   ```

   Or `simit release verify` if phase 05 has landed.

7. **Confirm changelog alignment.** The dossier notes
   `CHANGELOG.md` entries for `0.5.0` and `0.5.1`. Verify both
   are intentional, decide with the user whether to publish only
   `0.5.1` (and either remove the unreleased `0.5.0` entry or
   leave it as historical), or both.

8. **PR.** Title: `Track current simit; clear spurious CI drift`.
   Body covers: simit input bump, CI drift resolution path used,
   flake drift status, changelog decisions, chaperone-bar check
   results.

## Acceptance criteria

- [ ] `nix develop -c simit --version` matches current `trunk`.
- [ ] `simit init ci --check --diff` is clean (bare or with
      documented config workaround).
- [ ] `simit init flake --check --diff` is clean OR the remaining
      drift is explicitly documented as awaiting phase 03.
- [ ] `simit release trust check` passes.
- [ ] `cargo package --list` and `cargo publish --dry-run` succeed
      for the intended next version.
- [ ] PR description records the changelog decision for `0.5.0`
      vs. `0.5.1`.

## Files likely touched

- `flake.nix`, `flake.lock`
- `.forgejo/workflows/ci.yaml`
- `.forgejo/workflows/publish-crate.yaml`
- `simit.toml` (new, with `[ci]` and possibly `[flake].scope`)
- `CHANGELOG.md` (if changelog decisions modify entries)

## Pitfalls

- **Do not accept the wholesale `flake.nix` rewrite.** It would
  delete the `rs-harbor` / `home-manager` / `advisory-db` inputs
  and break the project's shared-toolchain story.
- **Do not silently publish `0.5.0` instead of `0.5.1`** because
  the chaperone happened to encounter that entry first.

## Reference

- Research dossier: per-repo cleanups (skillnet).
- Phase 01 (preferred dependency for clean `--check`).
- Phase 03 (preferred dependency for clean flake `--check`).
- Phase 07 README.
- Phase 11 sub-04 in
  `/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/sub-04-skillnet.md`
  (predecessor).

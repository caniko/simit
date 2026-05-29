# Phase 03 — `simit init flake` hooks-only mode

> **Recommended Codex model: GPT 5.5 / high**
>
> Destructive scope change with cross-repo blast radius. Today
> `simit init flake` owns the entire `flake.nix`; this phase
> downscopes its canonical ownership to `nix/pre-commit.nix` and
> adjacent simit-owned files. Wrong design choice here perpetuates
> the wholesale-rewrite problem or breaks repos that depend on the
> current ownership. Orchestrator role, complex work — 5.5 at
> `high`.

## Working tree

[/data/nvme0/can/Projects/simit](file:///data/nvme0/can/Projects/simit)
— branch off `trunk`. Independent of phases 01 and 02; can land in
either order.

## Goal

Adopting repos with project-specific `flake.nix` content
(shared-toolchain inputs like `rs-harbor`, custom outputs, custom
devShells, project-specific descriptions) can run
`simit init flake --check --diff` without simit proposing a wholesale
rewrite of their flake. simit's canonical ownership shrinks to
`nix/pre-commit.nix` and adjacent simit-owned files; the project
owns `flake.nix`.

`simit init flake` gains a `--scope hooks-only|full` flag (default
`hooks-only`) that selects which slice is generated and which is
verified by `--check`.

## Why this matters now

Three of the five fleet repos (`skillnet`, `rs-modde`, `sorrel`)
have heavily customized `flake.nix` files that consume shared
toolchain flakes or declare project-specific outputs. The current
`simit init flake --check --diff` proposes to replace the whole
thing. That makes `flake(drift)` a permanent state for those repos
and pushes them off the simit-managed dashboard.

The dossier names this as the highest blast-radius simit change
because it touches the public flake-input surface of consumer
projects. The fix is to ship the smallest viable slice first
(hooks-only as default) and leave full ownership behind an explicit
opt-in for the projects that want it.

## Out of scope

- Adding a "managed section" fence inside `flake.nix` (alternative
  approach considered in the dossier; defer to a later phase if
  hooks-only proves insufficient).
- Migrating the simit repo itself off full-scope flake ownership.
  simit's own `flake.nix` keeps the canonical shape; this phase
  changes only the default behavior on adopting repos.
- Auto-bumping the simit input in adopting repos (that is research
  dossier improvement 11, scheduled separately).

## Plan

1. **Inventory simit-owned flake files.** Read
   [src/render/flake.rs](../../../src/render/flake.rs) and
   [src/commands/init_flake.rs](../../../src/commands/init_flake.rs).
   List every file the current generator emits (`flake.nix`,
   `nix/pre-commit.nix`, `nix/devshell.nix` if present, etc.). Mark
   each as `hooks-only` or `full-scope-only`. `nix/pre-commit.nix`
   is the canonical hooks file; `flake.nix` is full-scope-only.

2. **Introduce `FlakeScope` enum and `--scope` flag.** In
   [src/cli.rs](../../../src/cli.rs) under `InitFlakeCommand`, add
   `--scope hooks-only|full` with default `hooks-only` for new
   adopters and `full` only when an existing `simit.toml`
   `[flake].scope = "full"` is set (default `hooks-only` otherwise).

3. **Add `[flake].scope` to `FlakeConfig`** in
   [src/config.rs:41-69](../../../src/config.rs#L41).
   Persist the resolved scope via the same write path phase 01
   establishes for `[ci]`. Migration safety: if a project already
   has `simit init flake`-managed `flake.nix` content (detected by
   the generated marker), default `[flake].scope = "full"` for
   that project on the first run so behavior does not change for
   existing adopters.

4. **Refactor the file emission** in
   [src/commands/init_flake.rs](../../../src/commands/init_flake.rs)
   to honor `FlakeScope::HooksOnly`: emit only `nix/pre-commit.nix`
   (and any other purely-hooks files), skip `flake.nix`. The
   `--check` path checks only the emitted set; project-owned
   `flake.nix` content is ignored.

5. **Update drift detection.** In
   [src/registry.rs](../../../src/registry.rs) (search for
   `detect_flake_status` and related), respect the scope: a
   `hooks-only` project's `flake.nix` shape does not count toward
   `flake=drift`. Only the hooks-file content matters.

6. **Generator removal-note hook (cross-link with phase 04 sub-02).**
   When `init flake --check --diff` would remove a pre-commit hook
   (e.g. `cargo-audit` → `cargo-msrv` swap), emit a one-line note
   above the diff: `note: pre-commit hook 'cargo-audit' will be
removed; the equivalent CI check now lives in
.forgejo/workflows/ci.yaml::Audit dependencies`. Implementation
   may share code with phase 04 sub-02; coordinate via the simit
   trunk.

7. **Tests.** Add coverage:
   - New project with no `simit.toml`: `simit init flake` emits only
     `nix/pre-commit.nix`; bare `simit init flake --check --diff`
     is clean regardless of what `flake.nix` content the test
     fixture has.
   - Existing project with simit-generated `flake.nix`: scope
     defaults to `full`, behavior is unchanged.
   - Explicit `--scope full` on a hooks-only project re-emits the
     full flake.

8. **Docs.** Add a section to
   [docs/src/getting-started/installation.md](../../getting-started/installation.md)
   or a new `flake-ownership.md` explaining the two scopes and how
   to migrate between them.

## Acceptance criteria

- [ ] `simit init flake` on `/data/nvme0/can/Projects/skillnet`
      (after merging this change into its simit input) does not
      propose to rewrite `flake.nix`.
- [ ] The simit repo itself continues to generate the full flake
      (its `[flake].scope = "full"`).
- [ ] `simit init flake --check --diff` on a hooks-only project
      compares only `nix/pre-commit.nix` (and any adjacent
      simit-owned hook files).
- [ ] `simit projects show` reports `flake=managed` for hooks-only
      projects whose hook file matches the generator output.
- [ ] A pre-commit hook removal (e.g. `cargo-audit`) prints an
      inline note explaining where the equivalent check moved.
- [ ] `cargo test` covers the three scope scenarios above.
- [ ] CHANGELOG entry calls out the default scope change and the
      migration path for existing adopters.

## Files likely touched

- `src/cli.rs` (`InitFlakeCommand` flag)
- `src/config.rs` (`FlakeConfig::scope`)
- `src/commands/init_flake.rs` (emission and check logic)
- `src/render/flake.rs` (file partitioning)
- `src/registry.rs` (drift detection respecting scope)
- `tests/`
- `docs/src/getting-started/installation.md` or new
  `docs/src/getting-started/flake-ownership.md`
- `CHANGELOG.md`

## Pitfalls

- **Do not silently demote existing adopters.** Any repo that
  already has simit-managed `flake.nix` content (marker present)
  must keep `scope = "full"` until the maintainer opts in. The
  _default_ change only applies to fresh adopters or to projects
  that explicitly set `[flake].scope = "hooks-only"`.
- **Do not break the simit repo's own self-check.** The simit repo
  uses full-scope today; verify nothing regresses on it.
- **Beware the marker collision.** If `flake.nix` lacks the
  generated marker but `nix/pre-commit.nix` has it, the project is
  effectively `hooks-only` already even without the config flag.
  Detect that state and default `[flake].scope = "hooks-only"` for
  it on the next `init flake` run.
- **Do not delete user-owned files.** If a project transitions from
  `full` → `hooks-only`, `simit init flake` must not remove the
  `flake.nix` it previously generated. Print a one-line notice
  pointing the user at the file so they can take ownership.

## Reference

- Research dossier: improvement (4) and (5).
- [src/commands/init_flake.rs](../../../src/commands/init_flake.rs)
- [src/render/flake.rs](../../../src/render/flake.rs)
- [src/config.rs:41](../../../src/config.rs#L41) — `FlakeConfig`.

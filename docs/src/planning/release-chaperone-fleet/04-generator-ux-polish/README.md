# Phase 04 — Generator UX polish (multi-sub-layer)

> **Recommended Codex model for the phase-level merge: GPT 5.4 / medium**
>
> Coordinating three small disjoint sub-layers, each independently
> verifiable. The merge is just confirming each PR landed and the
> simit repo's self-check stays clean. Sub-agent role on moderate
> work; 5.4 at `medium` matches the routing matrix.

## Sub-layers

| #   | Slug                           | Model                 | Touches                                               | Sub-layer file                                                                         |
| --- | ------------------------------ | --------------------- | ----------------------------------------------------- | -------------------------------------------------------------------------------------- |
| 01  | projects-show-regen-command    | GPT 5.4 / medium      | `src/commands/projects.rs`, `src/commands/init_ci.rs` | [sub-01-projects-show-regen-command.md](./sub-01-projects-show-regen-command.md)       |
| 02  | flake-removal-notes            | GPT 5.4 / medium      | `src/render/flake.rs`, `src/commands/init_flake.rs`   | [sub-02-flake-removal-notes.md](./sub-02-flake-removal-notes.md)                       |
| 03  | filter-ephemeral-from-listings | GPT 5.4-mini / medium | `src/commands/projects.rs`                            | [sub-03-filter-ephemeral-from-listings.md](./sub-03-filter-ephemeral-from-listings.md) |

## Goal (phase-level)

Three small simit UX fixes that collectively make the chaperone's
read-only inspection phase ("what state is this repo in?") less
noisy:

- `simit projects show` prints the exact `simit init ci` command
  that would regenerate the current CI shape (sub-01).
- `simit init flake --check --diff` warns when a hook is being
  silently removed (sub-02).
- `simit projects list` filters ephemeral `/tmp/*` test scratch
  projects from the main listing by default (sub-03).

## Why this matters now

The research dossier ranks these as low-cost, high-leverage polish
items. None of them blocks the chaperone alone, but together they
remove three sources of "what does this even mean?" friction that
the chaperone hits on every repo.

Sub-01 depends on phase 01 (it consumes the persisted `[ci]`
config). Sub-02 and sub-03 are independent and can run any time.

## Out of scope

- Restructuring `simit projects show` output beyond adding the regen
  command (sub-01).
- Implementing the broader "managed sections" model in `flake.nix`
  (that is deferred from phase 03).
- Adding new ephemeral-path heuristics; reuse
  `is_ephemeral_project_path` from `src/commands/projects.rs`.

## Merge plan

Each sub-layer ships as its own PR / commit on simit `trunk`. The
phase merges when all three are on `trunk` and `cargo test` /
`cargo run -- init ci --check` on the simit repo are clean.

Conflict surface is small:

- Sub-01 and sub-03 both touch `src/commands/projects.rs`. Land
  sub-03 first (smaller, leaves less surface), then sub-01.
- Sub-02 is fully disjoint.

## Phase-level acceptance criteria

- [ ] `simit projects show /data/nvme0/can/Projects/simit` prints
      the `simit init ci` regeneration command in its output.
- [ ] `simit init flake --check --diff` on a project that would
      lose a pre-commit hook prints an inline `note: removing
<hook>` line.
- [ ] `simit projects list` does not show `/tmp/*` ephemeral
      projects in its default output; `--include-ephemeral`
      restores them.
- [ ] `cargo test` passes for all three sub-layers.
- [ ] simit's own self-check (`cargo run -- init ci --check`,
      `cargo run -- init flake --check`) stays clean.

## Reference

- Research dossier: improvements (3), (5), (13).
- Phase 01 (sub-01 dependency).
- Phase 03 (sub-02 may share emission code).

# Plan: hooks-enforcement

> **Recommended Codex model for plan-set orchestration: GPT 5.5 high**
>
> Coordinating across two repos (simit, canix) plus a downstream verification
> repo (detritus), with one phase that touches the user's system-wide git
> configuration. Orchestration needs to weigh sequencing carefully:
> phase 03 changes how _every_ git commit on the host behaves, and phase 04
> proves the fix end-to-end against a real CI failure. A `medium` model can
> mis-sequence phase 03 vs phase 02 and brick commits during the window in
> between. Worth `high` for the orchestrator; individual phases self-route.

## Scope

Repair the broken local-hook enforcement chain across the user's Rust
project fleet. Source of truth: the
[research dossier](./hooks-enforcement-research.md) — read it before
dispatching any phase.

User selected **Option B** for the canix layer (replace system-wide
`core.hooksPath` with a dispatcher hook directory that forwards to the
project's resolved `.git/hooks/<name>` after running the canix system
step), so phase 03 follows that direction.

## Current state

- simit's `detect_hooks_status` returns `installed` based on the presence
  of `nix/pre-commit.nix` alone — currently labels 7 projects `installed`
  where none have working hooks.
- canix sets a system-wide `core.hooksPath` pointing at a Nix store dir
  containing only `commit-msg` (AI co-author trailer strip).
- `pre-commit install` (invoked from `nix develop` shellHook) refuses to
  write to `.git/hooks/` while `core.hooksPath` is set — silent no-op.
- Detritus surfaced the gap when a `cognitive_complexity` clippy
  violation in `crates/detritus-server/src/logs.rs:230` reached CI
  unblocked by any local hook.

## Phase table

| Phase | File                                                                           | Depends on | Touches                                                             | Blocking?          | Parallel with |
| ----- | ------------------------------------------------------------------------------ | ---------- | ------------------------------------------------------------------- | ------------------ | ------------- |
| 01    | [01-simit-detect-hooks-status.md](./01-simit-detect-hooks-status.md)           | —          | simit repo (`src/registry.rs`, registry schema, tests)              | yes (gates 02, 05) | 03            |
| 02    | [02-simit-hooks-install-subcommand.md](./02-simit-hooks-install-subcommand.md) | 01         | simit repo (`src/cli.rs`, new `src/commands/hooks.rs`)              | yes (gates 04)     | 03, 05        |
| 03    | [03-canix-dispatcher-hooks.md](./03-canix-dispatcher-hooks.md)                 | —          | canix repo (`home/modules/development/vcs/git.nix`, new dispatcher) | yes (gates 04)     | 01, 02, 05    |
| 04    | [04-fleet-sweep-install-hooks.md](./04-fleet-sweep-install-hooks.md)           | 02, 03     | 6 project `.git/hooks/` dirs (not tracked)                          | no (terminal)      | —             |
| 05    | [05-simit-surface-conflicted-state.md](./05-simit-surface-conflicted-state.md) | 01         | simit repo (`src/cli.rs`, `src/commands/projects.rs`)               | no                 | 02, 03        |

Per-phase model recommendations (also surfaced in each phase file's
callout block): 01=`5.5 medium`, 02=`5.5 medium`, 03=`5.5 high`,
04=`5.5 low`, 05=`5.5 medium`.

Phase 04 follow-up work (defensive installer, scan-surfaces-missing,
release propagation) lives in the sibling plan set
[`../hooks-enforcement-followups/`](../hooks-enforcement-followups/README.md).
See [`p04-followup-research.md`](./p04-followup-research.md) for the
diagnosis dossier that informed it.

## Parallelism layer

**Wave 0** (start from current tree):

- **01** (simit detect) and **03** (canix dispatcher) run in parallel — different repos, zero file overlap. 01 is a leaf in the simit registry layer; 03 is the highest-risk phase in the set and benefits from being underway early so its blast-radius windows are visible.

**Wave 1** (unlocked after 01 lands):

- **02** (simit hooks install) — depends on the new feature states defined in 01.
- **05** (simit conflicted-state surfacing) — also depends on 01. Can run concurrently with 02; both touch `src/cli.rs` so coordinate the order of merges. 05 is independent of 02's CLI changes (different subcommand and different formatter path) but rebases may be needed.
- 03 may still be running or completed; it does not gate this wave.

**Wave 2** (unlocked after 02 and 03 both land):

- **04** (fleet sweep) — pure execution. Runs `simit hooks install` on the 6 affected projects (detritus, open-data-license, rs-memory-admission, simit, skillnet, sorrel). Verifies acceptance end-to-end against detritus.

**Plan exhausted** after wave 2.

## External repo coordination

Three repos:

- `/data/nvme0/can/Projects/simit` — phases 01, 02, 05.
- `/data/nvme0/can/Projects/canix` — phase 03.
- `/data/nvme0/can/Projects/detritus` — verification only (phase 04 acceptance).

Phase 03 changes the user's home-manager git config. Activation is via
`canix` CLI (likely `canix activate` or `nixos-rebuild switch --flake`).
Until phase 03 is _activated on the host_, phases 02 and 04 cannot
prove "install actually wires git hooks that fire" — the simit installer
can still write the files, but `core.hooksPath` will still bypass them
on this host. Either:

- Activate canix immediately after phase 03 lands, or
- Run phase 02 development behind a one-off local override
  (`git -c core.hooksPath=.git/hooks pre-commit install`).

## Shared-file lockstep

- 02 and 05 both touch `src/cli.rs` (new subcommand registration vs.
  new `--show-conflicts` flag or equivalent). Whichever lands first
  forces a small rebase on the other — flagged in both phase docs.

## Infrastructure SPOF

Phase 03 is infra-SPOF: it changes how _every_ `git commit` on the host
behaves. A botched dispatcher script can block all commits system-wide
(the AI-strip `commit-msg` would still need to run, and the dispatcher
is what runs it). Phase 03 includes a rollback drill (revert to the
prior generation via `nixos-rebuild --rollback` or `home-manager
generations`).

Downstream phase 04 inherits a smoke-invalid note: if 04 reports
"hooks installed but not firing", check phase 03's activation status
before re-running the simit installer.

## Whole-set acceptance criteria

- [ ] In detritus on this host, after wave 2: `git commit` rejects a
      deliberate cognitive_complexity-violating change in
      `crates/detritus-server/src/logs.rs` (or equivalent) before the
      commit is created — same lint that surfaced in CI.
- [ ] `git config --get core.hooksPath` resolves to a dispatcher
      directory whose `pre-commit` script execs the project's
      `.git/hooks/pre-commit` when one exists, and is a no-op otherwise.
- [ ] The canix AI-co-author-strip `commit-msg` still removes
      `Co-Authored-By: …<noreply@anthropic.com>` lines on every commit
      across every repo.
- [ ] `simit projects list --json` reports `hooks: installed` only for
      projects whose `.git/hooks/pre-commit` (resolved via
      `git rev-parse --git-path hooks`) exists AND would be invoked by
      the resolved `core.hooksPath`. Projects with
      `nix/pre-commit.nix` but no installed hook report
      `hooks: configured`. Projects blocked by an unresolvable
      `core.hooksPath` setup report `hooks: conflicted`.
- [ ] `simit hooks install` and `simit hooks install --check` work on
      all 6 affected projects (detritus, open-data-license,
      rs-memory-admission, simit, skillnet, sorrel) without manual
      `git config --unset`.
- [ ] `pre-push` hooks correctly receive `<local-ref> <local-sha>
<remote-ref> <remote-sha>` lines on stdin after passing through
      the dispatcher (validated in phase 03 acceptance).

## Global constraints

- Do not bypass pre-commit framework's "cowardly refusing" check by
  modifying user config silently. The phase 02 installer should override
  `core.hooksPath` for the `pre-commit install` invocation only, never
  rewrite the user's git config. (The follow-up plan
  [`../hooks-enforcement-followups/`](../hooks-enforcement-followups/README.md)
  revisits this constraint for the narrow case where the local
  override is a hijack bypassing the canix dispatcher.)
- Do not delete or modify the existing canix `commit-msg` AI-strip
  behavior. It is a user requirement.
- Do not introduce a tool dependency outside what is already in the
  simit/canix flakes. The dispatcher is plain bash; the simit installer
  shells out to `pre-commit` (already vendored via `git-hooks.nix`).

## Reference

- [hooks-enforcement-research.md](./hooks-enforcement-research.md) —
  full evidence, root cause chain, design tradeoffs, alternatives
  considered.
- simit registry detection: [`src/registry.rs:582-588`](../../../../src/registry.rs) (relative to simit root).
- canix global hooks: `home/modules/development/vcs/git.nix:34` and
  `init.templateDir` at `:36`.
- detritus failing lint: `crates/detritus-server/src/logs.rs:230`.
- pre-commit "cowardly refusing" check:
  <https://pre-commit.com/#pre-commit-during-install>.

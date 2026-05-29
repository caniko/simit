# Phase 05 — `simit release verify` (chaperone bar bundler)

> **Recommended Codex model: GPT 5.5 / medium**
>
> New command that ties together existing checks (git status, init
> ci --check, init flake --check, release trust check, changelog
> alignment, crates.io reachability) into one structured report.
> Bundling work is moderate; the design call is which checks belong
> in `verify` vs. left out. Orchestrator role on complex work — 5.5
> at `medium` matches the routing matrix.

## Working tree

[/data/nvme0/can/Projects/simit](file:///data/nvme0/can/Projects/simit)
— branch off `trunk`. Independent of phases 01–04, but benefits
from phase 02's unified `--check` story (verify will call the same
check path). If phase 02 has not landed yet, fall back to invoking
`init ci --check` with the full flag set inferred from the project.

## Goal

`simit release verify` runs the entire release-bar checklist and
prints a structured pass/fail/blocked report. The chaperone can
invoke this once per repo and act on a single answer instead of
running six separate commands.

Output shape (one line per check, plus a summary footer):

```
simit release verify
  [pass]    worktree clean
  [pass]    simit ci managed (no drift)
  [pass]    simit flake managed (no drift)
  [pass]    release trust root present (keys/maintainers.gpg)
  [pass]    CHANGELOG entry exists for 0.5.1
  [fail]    crates.io: 0.5.1 not yet published; tag 0.5.1 not on
             origin
  [blocked] remote secrets: CRATES_IO_API_TOKEN presence not
             verifiable locally (run `simit release secrets`
             once available)
summary: 1 fail, 1 blocked
```

## Why this matters now

The research dossier names this as the single highest-leverage
ergonomic fix. Today the chaperone runs `git status`,
`simit init ci --check`, `simit init flake --check`,
`simit release trust check`, a manual changelog check, and a manual
crates.io lookup — six tool calls, six output formats, no
structured comparison. `simit release verify` collapses this into
one call and lets the chaperone gate on a single exit code.

## Out of scope

- Actually performing the release. `verify` is read-only.
- Fixing what it finds. Each failure is reported with the producer
  and the suggested remediation; remediation belongs to other
  commands.
- Remote secret verification. That is improvement (10), deferred
  to a future `simit release secrets` command. Verify reports
  secrets as `blocked` with a pointer.
- Multi-crate workspace publish-order checking — that is phase 06.

## Plan

1. **Add `ReleaseAction::Verify`** to
   [src/cli.rs](../../../src/cli.rs) (alongside `Trust`, `SyncUp`,
   `Patch`, etc.) and a corresponding `simit release verify`
   subcommand.

2. **Implement `verify` in
   [src/commands/release.rs](../../../src/commands/release.rs)**:
   - read the workspace metadata;
   - run each check in order, capturing a `CheckResult` with `pass
| fail | blocked`, a one-line description, and an optional
     remediation hint;
   - on `fail` or `blocked`, do not bail — keep running so the user
     sees the full picture;
   - exit `0` on all-pass, `1` on any fail, `2` on no-fail-but-blocked.

3. **Checks to include (in order):**
   - **worktree clean**: `git status --porcelain` empty.
   - **simit ci managed**: invoke the same check path as
     `simit init ci --check` against the resolved options (use
     phase 02 unified path if available).
   - **simit flake managed**: same for `simit init flake --check`.
   - **release trust**: invoke the same path as
     `simit release trust check`.
   - **CHANGELOG alignment**: parse `CHANGELOG.md`; assert the
     current `Cargo.toml` version (or the version named by
     `--version`) has a non-`[Unreleased]` entry.
   - **crates.io reachability**: `GET
https://crates.io/api/v1/crates/<name>` (no auth needed; HEAD
     would be ideal). Report whether the current version is
     already live. Skip publishable=false members.
   - **tag presence**: `git tag --list <version>` locally; if
     `--push-target <remote>` provided, `git ls-remote <remote>
refs/tags/<version>`.
   - **remote secrets**: emit `blocked` with a remediation note
     pointing at improvement (10).

4. **Output formatting.** Use a deterministic table (machine-
   parseable). Add `--json` flag for the chaperone to consume the
   results structurally.

5. **Tests.** Per-check unit tests with fixture projects:
   - dirty worktree → `worktree clean` fails.
   - drifted CI → `simit ci managed` fails.
   - missing CHANGELOG entry → that check fails.
   - already-published version → `crates.io reachability` passes
     with a "already live" annotation.
   - Integration test that the full `verify` exits with the right
     code per scenario.

6. **Docs.** Add
   `docs/src/getting-started/release-verify.md` (or extend
   `release-integrity.md`) covering the command, its exit codes,
   and the chaperone integration model.

## Acceptance criteria

- [ ] `simit release verify --help` documents the command and its
      flags (including `--json`, `--version`, `--push-target`).
- [ ] Running it on the simit repo at a release-clean commit exits
      `0` with all checks passing.
- [ ] Running it on a repo with mixed state exits `1` or `2`
      appropriately and prints one line per check.
- [ ] `--json` output validates against a documented schema.
- [ ] `cargo test` covers each check independently and the
      end-to-end exit code logic.

## Files likely touched

- `src/cli.rs` (`ReleaseAction::Verify`, args)
- `src/commands/release.rs` (new `verify` fn)
- New module if the check set grows large:
  `src/commands/release_verify.rs`
- `tests/release.rs`
- `docs/src/getting-started/release-verify.md` (new) or
  `release-integrity.md` (extension)

## Pitfalls

- **Do not let `verify` mutate state.** Every check must be
  read-only — including the CI and flake `--check` paths, which
  are already read-only but worth verifying with the test that
  asserts no files change.
- **Do not block on network for an unreasonable amount of time.**
  The crates.io check should have a small timeout (~5s) and degrade
  to `blocked` (with reason "network unreachable") rather than
  failing.
- **Beware non-publishable workspace members.** `cargo metadata`
  tells you which crates have `publish = false`; skip them in the
  crates.io check (research dossier improvement context, already
  encoded in recent simit changes).
- **Do not gate exit code on `blocked`.** The chaperone needs to
  distinguish "real failure" from "needs out-of-band evidence";
  the exit code triad (0/1/2) makes this explicit.

## Reference

- Research dossier: improvement (6) and (10).
- Phase 02 (preferred dependency, optional).
- Chaperone skill:
  `/home/can/.claude/skills/rust-crate-release-chaperone/SKILL.md`
- [src/commands/release.rs](../../../src/commands/release.rs)
  — existing release subcommands.

# Phase 04.01 — `simit projects show` prints the regen command

> **Recommended Codex model: GPT 5.4 / medium**
>
> Small, well-scoped feature on a single command. Reuses the
> existing `render_regeneration_command` helper. Moderate sub-agent
> work; 5.4 at `medium` matches the routing matrix.

## Working tree

[/data/nvme0/can/Projects/simit](file:///data/nvme0/can/Projects/simit)
— branch off `trunk` _after_ phase 01 has landed.

## Goal

`simit projects show <path>` includes an explicit
"regeneration command" line in its output that, when copied and run,
reproduces the project's current generated CI shape. The command
uses bare `simit init ci --platform <p>` when the project's
`simit.toml [ci]` already captures the option set, or the
flag-explicit form when it does not.

## Why this matters now

Today, when the chaperone inspects a repo, it has to read the
generated workflow files and reverse-engineer the flag set the
maintainer used. That is the implicit task behind every
"Validation command set" in the original readiness report. Sub-01
collapses that reverse-engineering into one line.

This sub-layer depends on phase 01 because the cleanest output is
"bare command + populated `[ci]`"; without phase 01, sub-01 would
print the full flag set every time and provide no marginal benefit
over what the chaperone could compute itself.

## Out of scope

- Restructuring `simit projects show` output.
- Auto-running the suggested command.
- Showing equivalent commands for `init flake`, `init homebrew-tap`,
  etc. (those can follow if the pattern works.)

## Plan

1. **Reuse `render_regeneration_command`** from
   [src/commands/init_ci.rs:193-300](../../../src/commands/init_ci.rs#L193).
   Expose it (or factor a thin wrapper) from `init_ci.rs` for use
   in `projects.rs`.

2. **Resolve the project's effective `InitCiCommand`** by reading
   its `simit.toml` `[ci]` and the inferred values from existing
   workflows (use the unified `ResolvedCiInputs` from phase 02 if
   that has landed; otherwise replicate the resolution locally and
   note the TODO).

3. **Add a "regeneration command" section** to
   [src/commands/projects.rs::show](../../../src/commands/projects.rs)
   that prints the rendered command on its own line, prefixed with
   `regen: ` so it is easy to copy-paste.

4. **Tests.** Add a test under `tests/projects.rs` that runs
   `simit projects show` against a fixture project with a populated
   `simit.toml [ci]` and asserts the output contains a bare
   `regen: simit init ci --platform forgejo` (no `--with-*` noise).

5. **Docs.** Mention in
   [docs/src/getting-started/project-registry.md](../../../getting-started/project-registry.md).

## Acceptance criteria

- [ ] `simit projects show <path>` output includes a `regen:` line.
- [ ] On a repo with populated `simit.toml [ci]`, the command is
      bare (`simit init ci --platform <p>` only).
- [ ] On a legacy repo without `simit.toml [ci]`, the command
      includes the inferred flag set.
- [ ] `cargo test` covers both cases.

## Files likely touched

- `src/commands/projects.rs`
- `src/commands/init_ci.rs` (export `render_regeneration_command`)
- `tests/projects.rs`
- `docs/src/getting-started/project-registry.md`

## Pitfalls

- **Do not print a command that would change the workflow content.**
  The rendered command must be a no-op when run with `--check`.
  Verify by snapshot-testing against simit's own repo.
- **Inference can be incomplete.** If `simit.toml` is absent and
  inference cannot determine the runner (e.g. on a self-hosted
  label simit does not recognize), print a `regen: <best guess>
  # verify --runner` comment rather than a confidently wrong
  command.

## Reference

- Research dossier: improvement (3).
- Phase 01 (required dependency).
- [src/commands/init_ci.rs:193](../../../src/commands/init_ci.rs#L193)
  — `render_regeneration_command`.
- [src/commands/projects.rs::show](../../../src/commands/projects.rs)
  — current output shape.

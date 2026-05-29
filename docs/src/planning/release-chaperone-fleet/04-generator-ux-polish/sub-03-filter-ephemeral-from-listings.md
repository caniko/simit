# Phase 04.03 — Filter ephemeral `/tmp/*` projects from `simit projects list`

> **Recommended Codex model: GPT 5.4-mini / medium**
>
> Trivial mechanical change: extend an existing filter from the
> attention footer to the main listing. Leaf node role on trivial
> work; 5.4-mini at `medium` matches the routing matrix.

## Working tree

[/data/nvme0/can/Projects/simit](file:///data/nvme0/can/Projects/simit)
— branch off `trunk`. Independent of every other phase. Can land
first if desired.

## Goal

`simit projects list` does not show ephemeral test-scratch projects
(typically rooted at `/tmp/...`) in its default output. A new
`--include-ephemeral` flag restores the old behavior. The attention
footer already filters these; this sub-layer extends the same
behavior to the main listing.

## Why this matters now

Today the dashboard is cluttered with `/tmp/nix-shell.*/...`,
`/tmp/simit-selfcheck.*`, and `/tmp/tmp.*/project` entries left
from test runs. The chaperone has to mentally skip them every time
it inspects the fleet state.

## Out of scope

- Changing the registry storage format.
- Changing what counts as ephemeral. Reuse the existing
  `is_ephemeral_project_path` helper.
- Auto-pruning ephemeral entries from the registry (separate
  decision; some test workflows depend on them sticking around).

## Plan

1. **Locate `is_ephemeral_project_path`** in
   [src/commands/projects.rs](../../../src/commands/projects.rs)
   (already used by the attention footer per
   [src/commands/projects.rs:200-220](../../../src/commands/projects.rs#L200)
   neighborhood — verify exact line).

2. **Add `--include-ephemeral`** to `ProjectsListArgs` in
   [src/cli.rs](../../../src/cli.rs). Default `false`.

3. **Filter the listing** in
   [src/commands/projects.rs::list](../../../src/commands/projects.rs)
   so ephemeral projects are skipped unless `--include-ephemeral`
   is set.

4. **Tests.** Add a test that seeds the registry with a `/tmp/*`
   project and asserts:
   - bare `simit projects list` does not show it;
   - `simit projects list --include-ephemeral` does.

5. **Docs.** Mention the flag in
   [docs/src/getting-started/project-registry.md](../../../getting-started/project-registry.md).

## Acceptance criteria

- [ ] `simit projects list` (bare) does not show ephemeral
      projects.
- [ ] `simit projects list --include-ephemeral` shows them.
- [ ] `cargo test` covers both modes.
- [ ] Attention footer behavior unchanged.

## Files likely touched

- `src/cli.rs`
- `src/commands/projects.rs`
- `tests/projects.rs`
- `docs/src/getting-started/project-registry.md`

## Pitfalls

- **Coordinate edit position with sub-01.** If sub-01 lands first,
  the `list` function may have moved. Re-read before editing.
- **Do not filter from `simit projects scan`.** Scan should still
  see ephemeral entries (some are intentional test fixtures); only
  `list` output is filtered.

## Reference

- Research dossier: improvement (13).
- `is_ephemeral_project_path` in `src/commands/projects.rs`.

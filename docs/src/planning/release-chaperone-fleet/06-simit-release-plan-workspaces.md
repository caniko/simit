# Phase 06 — `simit release plan` for workspaces

> **Recommended Codex model: GPT 5.4 / medium**
>
> `cargo metadata` parsing + topological sort + dry-run packaging
> dispatch. Moderate, well-defined sub-agent work; no novel design
> calls. 5.4 at `medium` matches the routing matrix.

## Working tree

[/data/nvme0/can/Projects/simit](file:///data/nvme0/can/Projects/simit)
— branch off `trunk`. Independent of phases 01–05; sequencing with
phase 05 only matters if they edit `src/commands/release.rs`
concurrently (land 05 first to minimize conflict surface).

## Goal

`simit release plan` reads the workspace's publishable crate graph,
emits the dependency-ordered publish sequence, and (optionally)
runs `cargo package --dry-run -p <crate> --allow-dirty` in that
order. The chaperone can use this to confirm a workspace will
publish cleanly before pushing a tag.

Output example for sorrel:

```
$ simit release plan
publish order (8 crates):
  1. sorrel-io        0.1.0
  2. sorrel-cache     0.1.0
  3. sorrel-compute   0.1.0
  4. sorrel-gpu       0.1.0
  5. sorrel-data      0.1.0
  6. sorrel-render    0.1.0
  7. sorrel-ui        0.1.0
  8. sorrel           0.1.0
non-publishable members skipped: (none)
$ simit release plan --dry-run-package
... runs cargo package per crate in order, fails fast on first
    error ...
```

## Why this matters now

Two of the five fleet repos are multi-crate workspaces (rs-modde,
sorrel). The tag-triggered publish workflows fire concurrently per
crate, but crates.io rejects `publish` until all path dependencies
are live. Without a documented or computed order, the chaperone is
guessing.

The dossier names this as the workspace-specific blocker for sorrel
(8 crates, all at 0.1.0, first publish). rs-modde has the same
shape with 5 publishable crates.

## Out of scope

- Actually publishing the crates. `release plan` is a planning /
  dry-run tool only.
- Generating per-crate CI workflows (already handled by
  `simit init ci --workspace`).
- Implementing publish-job serialization in the generated workflows.
  That is a separate, larger workflow-generator change; this phase
  just documents the order.

## Plan

1. **Add `ReleaseAction::Plan`** in
   [src/cli.rs](../../../src/cli.rs) and a corresponding
   `simit release plan` subcommand. Flags:
   - `--dry-run-package` (run `cargo package -p` per crate).
   - `--package <name>` (subset of the workspace; default = all
     publishable members).
   - `--json` (machine-readable output for the chaperone).

2. **Compute the order** in
   [src/commands/release.rs](../../../src/commands/release.rs)
   (or a new sibling module):
   - Read `cargo metadata --no-deps` (already available via
     `src/cargo.rs`).
   - Build a graph: edge from crate A to crate B iff A has a
     local path dependency on B. Use `cargo_metadata::Package`'s
     `dependencies[].path` for detection (consistent with
     [src/render/ci.rs::has_local_path_dependencies](../../../src/render/ci.rs)).
   - Topological sort (e.g. Kahn's algorithm). On cycle, bail with
     a clear error listing the cycle members — workspaces with
     cyclic local deps cannot publish.
   - Filter to `is_publishable() == true` (already implemented in
     [src/cargo.rs:48](../../../src/cargo.rs#L48)).

3. **Emit the plan** in the documented format. Default human-
   readable; `--json` emits an ordered array of `{ name, version,
manifest_path, publish, depends_on: [name...] }`.

4. **Optional dry-run packaging.** When `--dry-run-package` is set,
   for each crate in order:
   - `cargo package -p <name> --allow-dirty --no-verify`
   - Capture exit code; print `ok` / `fail` per crate; bail on
     first failure (the chaperone wants fail-fast).

5. **Tests.** Add fixture workspace with three crates `a` → `b` →
   `c` and a non-publishable `xtask`:
   - Plan prints `a, b, c` in order, skipping `xtask`.
   - Cyclic-dep fixture produces a clear cycle error.
   - `--dry-run-package` runs in order and stops on first failure.

6. **Docs.** Add
   `docs/src/getting-started/release-plan.md` (or extend
   `release-integrity.md`) describing the command, its output
   shape, and how the chaperone consumes it.

## Acceptance criteria

- [ ] `simit release plan` on the simit repo prints the simit
      single-crate plan (just `simit 0.x.y`).
- [ ] `simit release plan` on a multi-crate workspace prints the
      topologically-ordered publish list.
- [ ] Non-publishable members are excluded from the order.
- [ ] `--dry-run-package` runs `cargo package -p <name>` per crate
      in order, fail-fast.
- [ ] `--json` output validates against a documented schema.
- [ ] `cargo test` covers the three fixture scenarios in step 5.

## Files likely touched

- `src/cli.rs`
- `src/commands/release.rs`
- Optional new module: `src/commands/release_plan.rs`
- `src/cargo.rs` (if extracting the path-dep traversal)
- `tests/release.rs` (or new `tests/release_plan.rs`)
- `docs/src/getting-started/release-plan.md`

## Pitfalls

- **`cargo metadata` returns all workspace members, including
  non-published ones.** Always filter through `is_publishable()`
  before emitting the plan.
- **Topological sort is non-deterministic in tie-breaking.** Pick
  a stable secondary key (alphabetical by crate name) so the
  output is reproducible across runs and between machines.
- **Dependency edges are by manifest path, not by package name.**
  A crate may depend on `foo` via both a `path = "../foo"` (local)
  and a `version = "1.0"` (registry) entry. Treat the local edge
  as binding for publish-order purposes; ignore the registry edge.
- **Do not run `cargo publish --dry-run` instead of `cargo
package`.** `cargo publish --dry-run` requires registry
  credentials in some configurations; `cargo package` does not.

## Reference

- Research dossier: improvement (7).
- [src/cargo.rs:48](../../../src/cargo.rs#L48) — `is_publishable`.
- [src/render/ci.rs::has_local_path_dependencies](../../../src/render/ci.rs)
  — existing path-dep detection.
- Phase 05 (`simit release verify`) — may eventually delegate
  workspace-order checks here; coordinate via the simit trunk if
  both phases are in flight.

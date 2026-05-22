# Phase 2 — Render templates and bootstrap commands (parallel sub-layers)

This phase has two independent sub-layers. They can run in parallel after phase 1 lands.

- **2a** — Chocolatey: `render/chocolatey_nuspec.rs`, `simit chocolatey {render,bump}`, `simit init-chocolatey`.
- **2b** — Scoop: `render/scoop_manifest.rs`, `simit scoop {render,bump}`, `simit init-scoop-bucket`.

Both consume the resolved structs from phase 1; neither modifies CI YAML (phase 3) or extends test fixtures beyond the per-renderer unit tests (phase 4 covers integration tests).

## Parallelism notes

- No shared mutable files except `src/commands/mod.rs`, `src/render/mod.rs`, `src/cli.rs::Commands` enum, and `src/main.rs` dispatch. Resolve those merge conflicts by hand or stage the sub-layers serially through `git`.
- Test files are disjoint: `tests/chocolatey.rs` vs `tests/scoop.rs`.

## Sub-layer files

- [`a-chocolatey.md`](a-chocolatey.md)
- [`b-scoop.md`](b-scoop.md)

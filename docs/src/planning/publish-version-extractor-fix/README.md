# Plan: Publish Version Extractor Fix

> **Recommended Codex model for orchestration: GPT 5.5 medium**
>
> Plan-set is small (four phases, two repos, well-scoped). Coordination is
> mechanical — commit-and-release, refactor, downstream regen — with one
> non-trivial design decision (extractor approach in Phase 02). Routing the
> top-level orchestrator at 5.5 medium matches the moderate complexity /
> orchestrator coordinates from the routing matrix; 5.5 high would be
> over-spend for a four-phase coordination layer.

## Scope and current state

simit 0.15.0 (and the installed 0.14.1 CLI) emits a publish-workflow tag/version
check that breaks on Cargo workspaces:

```
version="$(cargo metadata --no-deps --format-version 1 | grep -m1 -o '"version":"[^"]*"' | cut -d '"' -f4)"
```

`grep -m NUM` bounds matching *lines*, not match *occurrences*. Cargo metadata
emits compact single-line JSON, so on a workspace every member's
`"version":"…"` reaches `$version`, and the equality check against `$tag`
always fails. Root cause and full repro live in
[`publish-workflow-version-extraction-research.md`](../publish-workflow-version-extraction-research.md).

The minimal fix — `grep -o … | head -n1` — is already applied in
[`src/render/ci.rs:1652`](../../../../src/render/ci.rs#L1652) with a
regression test in
[`tests/init_ci.rs`](../../../../tests/init_ci.rs); the full simit test
suite is green. The fix is not committed yet and not released. Detritus
workflows have been regenerated locally but not committed. Four single-crate
dependents (`open-data-license`, `rs-memory-admission`, `simit` itself,
`skillnet`) still carry the old line latently.

## Phases

| Phase | File | Depends on | Touches | Blocking? | Parallel with |
|-------|------|-----------|---------|-----------|---------------|
| 01 | [01-simit-commit-and-release.md](./01-simit-commit-and-release.md) | — | simit repo | yes (gates 04) | 03 |
| 02 | [02-simit-package-scope-extractor.md](./02-simit-package-scope-extractor.md) | 01 | simit repo | no | 03 |
| 03 | [03-detritus-commit-and-retry-publish.md](./03-detritus-commit-and-retry-publish.md) | 01 (logical) | detritus repo | no | 01, 02 |
| 04 | [04-sweep-latent-dependents.md](./04-sweep-latent-dependents.md) | 01 | 4 dependent repos | no | — |

## Parallelism layer

- **Wave 0:** 01 and 03 can run concurrently — they touch different repos
  (simit vs detritus) and detritus's regenerated workflows are already on
  disk. 03 only depends on 01 *logically* (so users tracing the fix find a
  released simit), not by file conflict.
- **Wave 1:** 02 starts once 01 is committed (it builds on the same
  `validate_release_tag_step` function and would conflict with an
  uncommitted 01). 04 starts once 01 is released (or once a simit checkout
  is explicitly used).
- **Wave 2:** Plan exhausted after 02 and 04 land. If 02 ships, a follow-up
  micro-phase to regenerate dependents again (picking up package-scoping)
  is implied but trivial and not pre-scheduled here.

## Whole-set acceptance criteria

- [ ] simit `src/render/ci.rs` no longer emits `grep -m1 -o` against cargo
      metadata; regression test in `tests/init_ci.rs` exercises both the
      negative and positive form.
- [ ] simit 0.15.1 (or a patch release with the fix) is tagged and
      published, or the user explicitly defers release to bundle with 02.
- [ ] Detritus's `publish-crate-*.yaml` regen is committed on
      `fix/remove-rust-cache` and the publish workflow for at least one
      detritus crate succeeds end-to-end against tag `0.1.0` (re-tagged or
      bumped to `0.1.1` as the user prefers).
- [ ] All four latent-affected dependents either (a) have their
      `publish-crate*.yaml` regenerated and committed, or (b) carry an
      explicit deferred note in their working tree with the rationale.
- [ ] Either 02 lands or it is explicitly deferred with a captured issue
      reference — the package-scoping defect must not silently linger.

## Global constraints

- Do not push force-update existing tags on remotes without confirming the
  retag strategy with the user (Phase 03).
- Do not regenerate workflows in repos with unrelated dirty working trees
  without first reading the existing diff and confirming the user's
  rebase/sequence preference (Phase 04 sub-checklists).
- All simit edits must round-trip through `cargo test`, `cargo fmt --check`,
  and `cargo clippy --all-targets --all-features -- --deny warnings`.

## Reference

- Research dossier:
  [`publish-workflow-version-extraction-research.md`](../publish-workflow-version-extraction-research.md).
- Generator: [`src/render/ci.rs:1648-1699`](../../../../src/render/ci.rs#L1648-L1699).
- Regression test:
  [`tests/init_ci.rs:613-620`](../../../../tests/init_ci.rs#L613-L620).

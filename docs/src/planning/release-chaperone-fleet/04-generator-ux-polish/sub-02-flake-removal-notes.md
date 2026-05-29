# Phase 04.02 — `simit init flake --check --diff` surfaces hook removals

> **Recommended Codex model: GPT 5.4 / medium**
>
> Small generator-side message addition. Reads the existing
> hook-file content, diffs against the generator output, emits a
> one-line note per removed hook. Moderate sub-agent role; 5.4 at
> `medium`.

## Working tree

[/data/nvme0/can/Projects/simit](file:///data/nvme0/can/Projects/simit)
— branch off `trunk`. Independent of phases 01–03; can land any
time, but coordinate with phase 03 if they ship together (both
touch the same removal-detection path).

## Goal

When `simit init flake --check --diff` would silently drop a
pre-commit hook (e.g. `cargo-audit`) in favor of a new one (e.g.
`cargo-msrv`), the output includes an inline note naming the
removed hook and pointing at where the equivalent check moved
(typically the generated CI workflow). Today the diff is the only
signal and reads as accidental churn.

## Why this matters now

The dossier flags this as the cause of the rs-memory-admission
"why is simit deleting my cargo-audit hook?" confusion. A one-line
note converts a destructive-looking diff into a documented
migration.

## Out of scope

- Refactoring hook detection logic.
- Letting the user opt back in to a removed hook (that is improvement
  9 territory, deferred).
- Adding equivalent notes for `init ci` (deferred; the CI removal
  story is different).

## Plan

1. **Identify the hook-comparison site** in
   [src/render/flake.rs](../../../src/render/flake.rs) or
   [src/commands/init_flake.rs](../../../src/commands/init_flake.rs).
   The site already knows which hooks the generator would emit;
   add a parallel read of the current `nix/pre-commit.nix` to
   extract the existing hook names.

2. **Compute the removal set** as `existing_hooks -
generated_hooks`. For each removed hook, look up a static
   `HOOK_REMOVAL_NOTES` table mapping hook names to their CI-side
   replacement (e.g. `cargo-audit` →
   `.forgejo/workflows/ci.yaml::Audit dependencies`). If a hook
   has no entry, fall back to a generic
   `note: removing pre-commit hook '<name>'`.

3. **Emit the notes** above the diff in the `--check --diff`
   output. Use `note:` prefix to match the style of existing
   simit warnings.

4. **Tests.** Add a fixture project with a `nix/pre-commit.nix`
   that has `cargo-audit`; run `simit init flake --check --diff`;
   assert the output contains the removal note.

5. **Maintenance.** Add a comment in `HOOK_REMOVAL_NOTES` reminding
   future contributors that when a hook is removed from the
   generator, they must add an entry here.

## Acceptance criteria

- [ ] `simit init flake --check --diff` on a project whose
      pre-commit file has `cargo-audit` (and the generator would
      remove it) prints a `note: removing pre-commit hook
    'cargo-audit'; ...` line.
- [ ] Generic fallback note appears for hooks not in the static
      table.
- [ ] `cargo test` covers the rs-memory-admission case.
- [ ] No false positives: a project whose hooks match the generator
      gets no removal notes.

## Files likely touched

- `src/render/flake.rs` (or `src/commands/init_flake.rs`,
  depending on where check rendering lives)
- New static table (could live in the same module)
- `tests/`

## Pitfalls

- **Do not panic on unparseable hook files.** If the existing
  `nix/pre-commit.nix` is malformed or non-standard, skip the
  removal-note step rather than failing the whole `--check`.
- **Do not duplicate the actual diff.** The note is a header
  above the diff, not a replacement for it.

## Reference

- Research dossier: improvement (5).
- [src/render/flake.rs:296](../../../src/render/flake.rs#L296)
  — current hook content detection.
- [src/render/flake.rs:587](../../../src/render/flake.rs#L587)
  — current hook emission.

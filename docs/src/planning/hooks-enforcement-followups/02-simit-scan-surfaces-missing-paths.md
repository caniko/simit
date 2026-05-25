# Phase 02 — Surface registry entries whose paths no longer exist

> **Recommended Codex model: GPT 5.5 medium**
>
> Sub-agent work touching scan logic, list formatter, and the
> attention classifier from the prior plan's phase 05. The design
> call is "prune by default, or just surface?" with a
> backwards-compatibility consideration (auto-pruning is silent
> data loss; surfacing is loud but harmless). `low` would likely
> ship `--prune` as a default and surprise the user; `high` is
> overkill for a behavioral tweak in two functions.

## Working tree

`/data/nvme0/can/Projects/simit`.

## Goal

`simit projects scan` (no flags) **reports** every registry
entry whose path no longer exists on disk, instead of silently
skipping them. `simit projects list` flags the same entries in
its attention footer (added by hooks-enforcement phase 05) with
a `missing` marker. Existing `--prune` behavior is unchanged —
pruning remains opt-in.

## Why this matters now

After hooks-enforcement, `simit projects show
/data/nvme0/can/Projects/skillctl` still reported
`hooks: installed` even though the directory had been deleted
months ago. Cause: `scan` correctly skips non-existent paths
([src/commands/projects.rs:150-170](/data/nvme0/can/Projects/simit/src/commands/projects.rs#L150-L170)),
but doesn't report which ones it skipped, so the registry
silently accumulates ghost entries that report whatever feature
state they had at their last successful scan.

Originating surprise:
[`../hooks-enforcement/.calibration.json`](../hooks-enforcement/.calibration.json)
`missed-signal: projects-scan-skips-entries`.

## Out of scope

- Auto-pruning. The user explicitly designed `--prune` to be
  opt-in (per the existing `ProjectsScanArgs` flag); this phase
  does not change that.
- Re-evaluating features for non-existent paths (nothing to
  evaluate against).
- New `simit projects` subcommands (e.g. `prune`, `health`).
  Existing flags are sufficient.
- Changing the `FeatureStatus` enum or adding a `Missing`
  variant per-feature. The missing-ness is a property of the
  registry _entry_, not of any individual feature.

## Plan

1. **Extend `simit projects scan` to count and report missing
   paths.** In `src/commands/projects.rs` around line 150
   (`fn scan`), track a `missing: Vec<Utf8PathBuf>` alongside
   the existing `pruned` vec. Move the `continue` branch to
   push into `missing` first, then conditionally also push into
   `pruned` when `args.prune` is set.

   After the loop, print missing paths regardless of `--prune`:

   ```text
   scanned 20 projects
   3 project(s) missing on disk (use --prune to remove):
     /data/nvme0/can/Projects/skillctl
     /tmp/skillnet-phase07
     /tmp/tmp.KhfZNdxdl9
   ```

   When `--prune` is passed, append the existing "pruned N"
   line.

   When `--dry-run` is passed, print "would prune N" and skip
   the warning about `--prune`.

2. **Extend `simit projects list`'s attention footer** (added by
   hooks-enforcement phase 05). The current attention classifier
   in `src/commands/projects.rs` flags `drift` / `conflicted`
   features. Extend it: also flag entries whose path doesn't
   exist on disk. New attention item shape:

   ```text
   3 project(s) need attention:
     /data/nvme0/can/Projects/detritus     hooks=conflicted
     /data/nvme0/can/Projects/skillctl     missing
     /tmp/skillnet-phase07                 missing
   ```

   The `missing` marker is its own AttentionItem variant (or a
   `("missing", None)` tuple in the existing structure —
   conform to whatever phase 05 implemented).

   The per-row glyph (`!`) also fires for missing-path entries.

3. **Skip `/tmp/*` and `/tmp/nix-shell.*` from the attention
   surfacing.** Per the existing
   `simit-dependent-fixes` convention, ephemeral paths are
   noise. Apply the same filter to the missing-path classifier.
   Continue to _report_ them in `scan`'s output (which has no
   filter), so the user sees them once and can `--prune` them
   if they want.

4. **Refactor: extract the missing-path check** into a helper
   in `src/commands/projects.rs` (or a new
   `src/commands/projects/attention.rs` if the file is getting
   long). The helper is reused by `scan`, `list`, and `show`.
   Signature roughly:

   ```rust
   pub fn entry_is_missing(path: &Utf8Path) -> bool {
       !path.as_std_path().exists()
   }
   ```

5. **Extend `simit projects show <path>`.** If the inspected
   path is missing, print the attention header from
   hooks-enforcement phase 05 with the `missing` marker, then
   print the stored last-known feature table with a header note
   like "(features as of last successful scan)".

6. **Tests.** Add to `tests/projects.rs`:
   - Scan with a registry that includes one missing path:
     assert stdout includes the missing path and the "use
     --prune to remove" hint.
   - Scan + `--prune` removes the missing entry from the saved
     registry.
   - List with a missing entry: assert glyph + attention footer
     include the entry with `missing`.
   - List with a missing `/tmp/*` entry: assert it does NOT
     appear in the attention footer (ephemeral filter).
   - Show on a missing path: assert the attention header
     appears, table is still printed, and the stored features
     are unchanged.

7. **Run gates:**

   ```sh
   cargo fmt --all -- --check
   cargo test --all-features
   cargo clippy --all-targets --all-features -- --deny warnings
   ```

8. **CHANGELOG.** Add a `[Unreleased]` entry:

   ```
   - `simit projects scan` now reports registry entries whose
     paths no longer exist on disk (use `--prune` to remove).
   - `simit projects list` flags missing-path entries in its
     attention footer alongside `drift` / `conflicted` features.
   ```

## Acceptance criteria

- [ ] `simit projects scan` (no flags) on the user's current
      registry prints at least one line naming a missing path
      (e.g. `/data/nvme0/can/Projects/skillctl`) and the
      "use --prune to remove" hint.
- [ ] `simit projects scan --prune` removes the missing entry
      from the saved registry; subsequent
      `simit projects show <removed-path>` errors with "no such
      project".
- [ ] `simit projects list` flags missing-path entries with the
      `!` glyph AND lists them in the attention footer with
      a `missing` marker.
- [ ] `/tmp/*` and `/tmp/nix-shell.*` entries do NOT trigger
      the attention footer for `list` (but DO appear in `scan`'s
      output).
- [ ] `simit projects show <missing-path>` prints the attention
      header and the cached feature table.
- [ ] All new test scenarios in `tests/projects.rs` pass.
- [ ] `cargo clippy --all-targets --all-features -- --deny warnings`
      is clean.
- [ ] CHANGELOG `[Unreleased]` mentions the new behavior.

## Files likely touched

- `src/commands/projects.rs` — scan output, list/show
  attention classifier extension, missing-path helper.
- `tests/projects.rs` — five new test scenarios.
- `CHANGELOG.md`.

No changes to `src/registry.rs` (the registry data shape is
unchanged) and no changes to `src/cli.rs` (no new flags).

## Pitfalls

**P1. `Utf8Path::exists()` returns false for broken symlinks.**
Symptom: a symlinked project that the user expected to keep
gets flagged as missing. Cause: `exists` follows symlinks.
Recovery: probably correct behavior — a project whose symlink
target is gone is effectively missing. Document in the
CHANGELOG so users aren't surprised.

**P2. NFS / network drive eval cost.** Symptom: `scan` becomes
noticeably slower on a 50-entry registry with paths on a slow
filesystem. Cause: per-entry `exists()` stat. Recovery: only an
issue at scale; current registries are ~25 entries. Skip
mitigation for now.

**P3. `--prune` interaction with the new missing report.**
Symptom: `scan --prune` prints both the "missing" report and
the "pruned" report, double-counting the same entries. Cause:
the loop populates both vecs. Recovery: when `--prune` is set,
suppress the standalone "missing" line and rely on the
"pruned N" line only — they refer to the same entries.

**P4. Attention footer formatting drift between hooks-enforcement
phase 05 and this phase.** Symptom: the new `missing` entries
render with different column widths than `hooks=conflicted`.
Cause: reusing the formatter without extending the column-width
calculation. Recovery: use the same formatter; pad the marker
column to max(longest marker, "missing".len()).

**P5. Ephemeral `/tmp/*` filter shadows a legitimate use case.**
Symptom: a user with `/tmp/my-real-project` (unusual but
possible) sees their project filtered from the attention list.
Cause: hard-coded prefix filter. Recovery: keep the filter
because it matches existing
`simit-dependent-fixes` convention, but mention in CHANGELOG
that ephemeral-pattern paths are filtered from the attention
list.

## Reference

- Triggering surprise:
  [`../hooks-enforcement/.calibration.json`](../hooks-enforcement/.calibration.json),
  `missed-signal: projects-scan-skips-entries`.
- Existing scan impl:
  [`src/commands/projects.rs:150`](/data/nvme0/can/Projects/simit/src/commands/projects.rs#L150).
- Existing attention classifier (from hooks-enforcement
  phase 05):
  `src/commands/projects.rs:100+, 421+, 487+`.
- Phase 01 (sibling): [01-simit-defensive-installer.md](./01-simit-defensive-installer.md).

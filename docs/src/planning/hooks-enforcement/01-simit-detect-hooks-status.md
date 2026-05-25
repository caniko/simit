# Phase 01 — Replace presence-based hook detection with reality-based detection

> **Recommended Codex model: GPT 5.5 medium**
>
> Moderate complexity sub-agent work: rewrite one detector function,
> introduce two new enum variants, migrate stored registry data, and
> update tests and the projects-list formatter. The design space is
> narrow (the dossier already names the three target states) but the
> registry schema migration and `core.hooksPath` resolution both have
> non-obvious failure modes (worktrees, bare repos, dangling symlinks
> in the resolved hooks dir). A `low` tier risks shipping a detector
> that still mislabels conflicted setups; `high` is overkill for one
> file of well-bounded logic plus tests.

## Working tree

`/data/nvme0/can/Projects/simit` — the simit Rust workspace. Single
repo, no submodules.

## Goal

`simit projects list --json` reports `hooks` as one of three states
that accurately describe whether project-level hooks would fire on
`git commit` / `git push`:

- `installed` — config present AND a hook script is wired in the
  resolved hooks directory AND that directory is what git will execute
  for this repo.
- `configured` — config file (`nix/pre-commit.nix`) present but no
  installed hook in the resolved hooks directory.
- `conflicted` — config present and `core.hooksPath` (effective for
  this repo) points somewhere that does not contain the project's
  pre-commit / pre-push scripts and is not under this project's git
  directory.
- `absent` — no config file (unchanged from today).

## Why this matters now

`simit projects show /data/nvme0/can/Projects/detritus` currently
reports `hooks: installed` while `.git/hooks/pre-commit` does not
exist and `core.hooksPath` is globally set to a path that contains
only `commit-msg`. Result: routine `simit projects` inspection
silently lies, and the user has no way to discover the broken state
without running CI. See [the research dossier](./hooks-enforcement-research.md)
for the seven-project audit table.

Originating code: [`src/registry.rs:582-588`](../../../../src/registry.rs)

```rust
fn detect_hooks_status(workspace_root: &Path) -> FeatureStatus {
    if workspace_root.join("nix/pre-commit.nix").exists() {
        FeatureStatus::Installed
    } else {
        FeatureStatus::Absent
    }
}
```

## Out of scope

- Installing hooks (that's phase 02's `simit hooks install`).
- Changing canix or modifying `core.hooksPath` on the host (phase 03).
- Adding a `doctor` subcommand or special `--show-conflicts` formatting
  (phase 05).
- Touching any non-hooks feature detector in `detect_feature_status`.

## Plan

1. **Extend `FeatureStatus`.** In `src/registry.rs`, add
   `Configured` and `Conflicted` variants alongside the existing
   `Installed`, `Managed`, `Drift`, `HandRolled`, `Absent` (or
   whichever enum surface currently exists — confirm by reading the
   enum definition before editing). Update the `Display` /
   `serde::Serialize` impl so the JSON output uses
   `"configured"` / `"conflicted"` (lower-case, matches existing
   pattern).

2. **Rewrite `detect_hooks_status`.** Algorithm:

   ```text
   if !workspace_root.join("nix/pre-commit.nix").exists():
       return Absent

   hooks_dir = `git -C workspace_root rev-parse --git-path hooks`
              (canonicalized)
   effective_hooks_path = `git -C workspace_root config --get core.hooksPath`
                          OR hooks_dir if unset
                          (canonicalized; expand ~)

   pre_commit_present = effective_hooks_path.join("pre-commit").is_file()

   if effective_hooks_path == hooks_dir AND pre_commit_present:
       return Installed
   if effective_hooks_path != hooks_dir AND pre_commit_present:
       return Installed   # user's chosen hooksPath has a hook; honor it
   if effective_hooks_path != hooks_dir AND !pre_commit_present:
       # core.hooksPath redirects to a dir that has no pre-commit
       return Conflicted
   # hooksPath unset (or == hooks_dir) and no pre-commit
   return Configured
   ```

   Shell out to `git` via `std::process::Command` — do NOT take a
   `git2` dependency for this. Match the existing dependency posture
   in `src/registry.rs`.

3. **Handle git errors gracefully.** A workspace_root that is not in
   a git repository (uncommon for registered simit projects but
   possible for `--include-empty` discovery) should fall back to
   inspecting `<workspace_root>/.git/hooks/pre-commit` directly.
   Wrap `Command::new("git")` failures in a debug log and return
   `Configured` (the safe, non-claiming state).

4. **Migrate stored registry entries.** Registry entries currently
   serialized as `"hooks": "installed"` may be stale. The simplest
   migration is: on `load`, treat any existing `"installed"` value as
   advisory and re-run `detect_hooks_status` at scan time anyway
   (which already happens in `touch_loaded`). Verify by reading
   `touch_loaded` — `features.extend(detected)` will overwrite. No
   schema-version bump needed; new states deserialize fine into the
   enum.

   If the enum is currently `#[serde(rename_all = "snake_case")]` or
   similar, that posture stays; new variants get the same treatment.

5. **Tests.** Add unit tests in `src/registry.rs` (or its test module)
   covering:
   - `Absent` — no `nix/pre-commit.nix`.
   - `Configured` — config present, no installed hook, no
     `core.hooksPath`.
   - `Installed` — config present, `.git/hooks/pre-commit` exists,
     no `core.hooksPath`.
   - `Installed (custom hooksPath)` — config present,
     `core.hooksPath` set to a dir that contains `pre-commit`.
   - `Conflicted` — config present, `core.hooksPath` set to a dir
     that does NOT contain `pre-commit`.

   Use `tempfile` or the existing test scaffold to create a real git
   repo per test (`git init` in a `tempdir`, set `core.hooksPath`
   with `git config --local`). If the existing test suite already has
   a "git scratch repo" helper, use it.

6. **Run gates.**

   ```sh
   cargo fmt --all -- --check
   cargo test --all-features -p simit registry
   cargo clippy --all-targets --all-features -- --deny warnings
   ```

7. **Sanity check end-to-end.** From the simit repo:

   ```sh
   cargo run -- projects scan
   cargo run -- projects show /data/nvme0/can/Projects/detritus
   ```

   Expect `hooks: conflicted` (because the host still has
   `core.hooksPath` set to a directory without a `pre-commit`). After
   phase 03 lands and the dispatcher carries `pre-commit`, the state
   should flip to `installed` after phase 04. Document this in the
   PR description so it's not mistaken for a regression.

## Acceptance criteria

- [ ] `FeatureStatus` enum exports `Configured` and `Conflicted`
      variants that round-trip through serde to `"configured"` and
      `"conflicted"`.
- [ ] `detect_hooks_status` resolves `core.hooksPath` via `git config
    --get` (or returns the project's `.git/hooks` when unset) and
      uses canonicalized path comparison.
- [ ] Unit tests cover all five scenarios in step 5; they pass under
      `cargo test --all-features`.
- [ ] `cargo clippy --all-targets --all-features -- --deny warnings`
      is clean.
- [ ] `cargo run -- projects show /data/nvme0/can/Projects/detritus`
      reports `hooks: conflicted` on this host (pre-phase 03).
- [ ] `cargo run -- projects show /data/nvme0/can/Projects/rs-modde`
      continues to report `hooks: absent` (regression guard for the
      one project that already reported correctly).
- [ ] No new external crate dependencies introduced.

## Files likely touched

- `/data/nvme0/can/Projects/simit/src/registry.rs` — enum, detector,
  tests.
- `/data/nvme0/can/Projects/simit/src/registry/` — if the tests module
  is split out, the test file there.
- `/data/nvme0/can/Projects/simit/Cargo.toml` — only if `tempfile`
  is not already a dev-dependency (likely already present).

No changes to `src/cli.rs` in this phase — the projects-list formatter
already prints whatever `FeatureStatus` displays as. Cosmetic
highlighting of `conflicted` is phase 05.

## Pitfalls

**P1. `git rev-parse --git-path hooks` returns a relative path.**
Symptom: path comparison fails because one side is relative and the
other absolute. Cause: `--git-path` resolves relative to the cwd of
the `git` invocation. Recovery: canonicalize both sides via
`fs::canonicalize`, or run git with `-C workspace_root` and prepend
`workspace_root` to relative results.

**P2. `core.hooksPath` may contain `~`.** Symptom: the resolved path
doesn't exist on disk. Cause: `git config` returns the literal value;
shell expansion is git's job at hook-execution time. Recovery:
expand `~` to `$HOME` before canonicalizing.

**P3. Worktrees.** Symptom: detection passes for the main checkout
but reports `Configured` for a `git worktree add`-ed directory.
Cause: `--git-path hooks` resolves to the common dir's hooks/, which
both worktrees share — that's actually correct. Just make sure the
path comparison uses the resolved common dir, not the worktree-local
`.git` file. `git rev-parse --git-path hooks` handles this; do not
hand-construct `workspace_root/.git/hooks`.

**P4. `pre-commit` framework's wrapper script.** Symptom: `pre-commit`
in `.git/hooks/` exists but is a tiny shim that re-execs `pre-commit
run`. The detector should treat this as `Installed` regardless of
script contents — presence is sufficient.

**P5. Stale registry serialization.** Symptom: `simit projects list`
still shows `"installed"` for projects that should be `"configured"`
after the rebuild. Cause: `touch_loaded` is only called on
`scan`/`discover`/etc. Recovery: run `simit projects scan` once.
Document this in the phase commit message.

## Reference

- Research dossier: [hooks-enforcement-research.md](./hooks-enforcement-research.md)
- Originating detector: [`src/registry.rs:582-588`](../../../../src/registry.rs)
- Phase 02 (consumer): [02-simit-hooks-install-subcommand.md](./02-simit-hooks-install-subcommand.md)
- Phase 05 (consumer): [05-simit-surface-conflicted-state.md](./05-simit-surface-conflicted-state.md)

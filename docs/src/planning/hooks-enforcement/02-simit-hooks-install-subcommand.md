# Phase 02 — Add `simit hooks install` (and `--check`) subcommand

> **Recommended Codex model: GPT 5.5 medium**
>
> Sub-agent CLI work: register a new top-level subcommand, shell out
> to `pre-commit install` with a one-shot `core.hooksPath` override,
> emit actionable diagnostics when the host's effective hooksPath
> won't honor the install. The design questions are bounded
> (subcommand name, hook stages to install, what `--check` returns)
> and well-precedented by the existing `simit init flake --check
--diff` shape. A `low` model is likely to forget the
> `--hook-type pre-push` stage or to invoke `pre-commit install`
> without the `core.hooksPath` override and ship a still-broken
> installer; `high` is overkill for a bounded CLI plus pre-commit
> invocation.

## Working tree

`/data/nvme0/can/Projects/simit`.

## Goal

`simit hooks install` writes `.git/hooks/pre-commit`,
`.git/hooks/pre-push`, and `.git/hooks/commit-msg` for projects that
have a `nix/pre-commit.nix` (or equivalent pre-commit framework
config), without requiring the user to manually `git config --unset
core.hooksPath`. `simit hooks install --check` reports whether
re-running `install` would change anything, exits non-zero on drift.

## Why this matters now

The current install path is the `git-hooks.nix` `shellHook` invoked
on `nix develop` entry, which calls `pre-commit install`. That
command aborts with `Cowardly refusing to install hooks with
core.hooksPath set.` whenever the user's git config has a
system-wide `core.hooksPath` — which on this host is always (see
phase 03 for why). Result: entering `nix develop` silently fails to
install hooks on every project. There must be a deliberate,
diagnosed install path that doesn't depend on luck.

## Out of scope

- Modifying the user's permanent git config in any way.
- Implementing a "hooks uninstall" command (file a follow-up if
  needed; not required for the acceptance chain).
- Detecting / supporting non-`pre-commit` hook frameworks (lefthook,
  husky). This phase targets `pre-commit` exclusively because that's
  what `nix/pre-commit.nix` produces.
- Calling into phase 03's dispatcher logic from simit. Simit installs
  per-project hooks; the dispatcher chains them at execution time —
  the two pieces never talk.

## Plan

1. **Add the subcommand to the CLI.** In `src/cli.rs`, register a new
   `Hooks` top-level command following the same pattern as the
   existing `Init`, `Release`, `Projects` commands. Subcommands:
   - `simit hooks install` — install hooks into the resolved hooks
     directory.
   - `simit hooks install --check` — exit 0 if installed hooks would
     not change, non-zero otherwise (mirror `simit init flake --check`).
   - `simit hooks install --diff` — print a per-file diff between
     current and desired hook content (mirror `simit init flake --diff`).
   - Both flags may combine: `--check --diff`.

2. **Create `src/commands/hooks.rs`.** Implements:

   ```rust
   pub fn run(workspace_root: &Path, mode: InstallMode) -> Result<()>;
   ```

   where `InstallMode` is `{ check: bool, diff: bool }`. The function:
   - Verifies `<workspace_root>/nix/pre-commit.nix` exists; bail with
     a clear message if not ("no pre-commit configuration found —
     run `simit init flake` first" or similar).
   - Resolves the target hooks directory via
     `git -C workspace_root rev-parse --git-path hooks`
     (canonicalized; reuse the helper from phase 01 if there is one).
   - Resolves the _effective_ hooks path via
     `git -C workspace_root config --get core.hooksPath` (unset
     means use the rev-parse'd hooks dir).
   - If `effective_hooks_path` resolves outside `<workspace_root>/.git/`
     AND the user is not already on a phase-03 dispatcher (heuristic:
     look for a sentinel file `dispatched-by-canix` in the directory),
     emit a _warning_ explaining the install will write to
     `<git-dir>/hooks/` but git will execute the system path instead
     — and link to the canix dispatcher direction. Do not refuse to
     install; the user may be running this in a CI sandbox where the
     warning is irrelevant.
   - Invokes pre-commit:

     ```sh
     pre-commit install \
       --install-hooks \
       --hook-type pre-commit \
       --hook-type pre-push \
       --hook-type commit-msg \
       --overwrite
     ```

     with `core.hooksPath` overridden for this single git invocation
     so pre-commit doesn't refuse. The cleanest way is to invoke
     pre-commit with `GIT_CONFIG_PARAMETERS` set, or to wrap with
     `git -c core.hooksPath=<resolved-git-hooks-dir>` — but
     pre-commit shells out to `git` itself, so the safer pattern is
     to set the env var:

     ```sh
     GIT_CONFIG_COUNT=1 \
     GIT_CONFIG_KEY_0=core.hooksPath \
     GIT_CONFIG_VALUE_0=<resolved-git-hooks-dir> \
     pre-commit install ...
     ```

     This temporarily overrides the user's config for the
     install-only subshell without writing anything to disk.

3. **`--check` mode.** Capture the would-be content of each hook
   wrapper script that `pre-commit install` would write
   (pre-commit's wrapper is deterministic given the config hash and
   pre-commit version). Compare to what's on disk. Exit 0 if equal,
   non-zero with a per-file summary if drifted. The simplest
   correct implementation: run `pre-commit install --dry-run` if
   the version supports it (pre-commit 3.7+); otherwise diff the
   resulting files after install-into-tempdir.

4. **`--diff` mode.** Print a unified diff per drifted file. Reuse
   any existing simit diff helper from `simit init flake --diff`.

5. **Update `detect_hooks_status` test scenario.** The phase 01
   `Installed` test is sufficient; this phase doesn't add a new
   detector test. But add an integration test in `tests/` that:
   - Creates a temp git repo with a minimal `nix/pre-commit.nix`.
   - Sets a foreign `core.hooksPath` via `git config --local`.
   - Runs `simit hooks install` against that repo.
   - Asserts `<repo>/.git/hooks/pre-commit` exists and is executable.
   - Asserts the user's `git config --get core.hooksPath` value is
     unchanged after the install (no config writes).

6. **Run gates.**

   ```sh
   cargo fmt --all -- --check
   cargo test --all-features
   cargo clippy --all-targets --all-features -- --deny warnings
   ```

7. **Manpage / help text.** If the project generates manpages via
   `simit man`, regenerate and commit the updated output.

8. **CHANGELOG.** Add an entry under `## [Unreleased]` describing
   the new subcommand.

## Acceptance criteria

- [ ] `simit hooks install --help` lists the subcommand under
      `Usage: simit hooks <COMMAND>`.
- [ ] `simit hooks install` exits 0 in a temp repo with
      `nix/pre-commit.nix` and a foreign `core.hooksPath` set;
      after the run, `<repo>/.git/hooks/pre-commit` exists and is
      executable.
- [ ] `simit hooks install --check` exits 0 immediately after a
      successful install; exits non-zero if `.git/hooks/pre-commit`
      is deleted between calls.
- [ ] `simit hooks install --diff` prints a unified diff when drift
      is present and nothing when clean.
- [ ] The integration test from step 5 passes.
- [ ] `cargo clippy --all-targets --all-features -- --deny warnings`
      is clean.
- [ ] No write to the user's `~/.gitconfig` or repo-local git config
      during install (verify by snapshotting `git config --list
    --show-scope` before and after in the integration test).
- [ ] CHANGELOG `[Unreleased]` mentions `simit hooks install`.

## Files likely touched

- `src/cli.rs` — register the `Hooks` enum variant. **Note:** phase
  05 also touches this file; rebase carefully.
- `src/commands/mod.rs` — `pub mod hooks;`.
- `src/commands/hooks.rs` — new file with the install logic.
- `tests/hooks_install.rs` — new integration test.
- `CHANGELOG.md`.
- (Possibly) `src/registry.rs` if helper functions for resolving
  `core.hooksPath` are extracted from phase 01's detector and shared
  here — make the call when reading 01's code.

## Pitfalls

**P1. `pre-commit install` writes nothing if `--overwrite` is missing
and a hook exists.** Symptom: re-running `install` after a manual
edit leaves the manual edit in place. Cause: pre-commit's default
behavior. Recovery: always pass `--overwrite`. Document that the
installer takes ownership of `pre-commit`, `pre-push`, `commit-msg`
files in the project's resolved hooks dir.

**P2. The `GIT_CONFIG_*` env-var override is per-invocation.**
Symptom: env-var overrides leak into subprocesses pre-commit spawns
(it shells out to git heavily). That's actually what we _want_ — the
override should propagate so pre-commit's internal git calls also
see the local hooksPath. Just make sure you don't pre-set these
vars in the simit process itself; scope them to the
`Command::env()` of the pre-commit invocation only.

**P3. `pre-commit install --dry-run` may not exist on older
versions.** Symptom: `--check` mode falls through to a tempdir
install and prints "drift" for cosmetic version-string differences
in the wrapper. Cause: pre-commit wrappers embed the pre-commit
version they were installed by. Recovery: normalize the version
line before comparison, or require `pre-commit >= 3.7` in the
`nix/pre-commit.nix` and document it.

**P4. Foreign hook in `.git/hooks/` from a previous hook framework.**
Symptom: install succeeds but pre-commit prints a warning about a
non-pre-commit-managed hook. Recovery: surface pre-commit's stderr
to the user; do not swallow it.

**P5. Reusing the `--diff` formatter from `simit init flake`.**
Symptom: subtle path-prefix differences (init renders relative-to-
workspace, hooks renders relative-to-hooks-dir). Cause: the helper
probably hardcodes the workspace_root assumption. Recovery: pass
the base path in explicitly or wrap the helper.

## Reference

- Research dossier: [hooks-enforcement-research.md](./hooks-enforcement-research.md)
- Phase 01 (prerequisite): [01-simit-detect-hooks-status.md](./01-simit-detect-hooks-status.md)
- Phase 03 (companion): [03-canix-dispatcher-hooks.md](./03-canix-dispatcher-hooks.md)
- Phase 04 (consumer): [04-fleet-sweep-install-hooks.md](./04-fleet-sweep-install-hooks.md)
- pre-commit install flags: <https://pre-commit.com/#pre-commit-install>
- `GIT_CONFIG_PARAMETERS` / `GIT_CONFIG_COUNT` env-var mechanism:
  <https://git-scm.com/docs/git-config#ENVIRONMENT>

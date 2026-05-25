# Phase 01 — Make `simit hooks install` defensive about rogue local `core.hooksPath`

> **Recommended Codex model: GPT 5.5 medium**
>
> Sub-agent CLI/diagnostic work. The hard part is not the code (a
> single git-config read + conditional cleanup + tests); it's the
> design call: warn, auto-fix, prompt, or require `--fix`. That
> call has user-experience implications (silent config mutation
> vs. broken AI-strip in the field). `low` invites a
> "just always unset it" implementation that violates the
> phase-02 acceptance criterion "no write to the user's git
> config during install" from the prior plan set. `high` is
> overkill — the design space is bounded.

## Working tree

`/data/nvme0/can/Projects/simit`.

## Goal

`simit hooks install` detects a local repo
`core.hooksPath = .git/hooks` (or any path inside
`<git-common-dir>/hooks`) when the effective system path is a
friendly canix-style dispatcher (sentinel file
`dispatched-by-canix` present), and either warns loudly or fixes
the value depending on a `--fix` flag (default: warn). Default
exit status is unchanged (install still succeeds); the warning is
emitted on stderr.

## Why this matters now

During `hooks-enforcement` phase 04 execution, all six affected
repos ended up with a local
`core.hooksPath = .git/hooks` written into `.git/config` by some
out-of-band action (verified that neither simit nor pre-commit
writes it). This silently bypassed the canix dispatcher: project
hooks fired (clippy/fmt), but the system `commit-msg` AI-strip
step never ran. The breakage was invisible until a verify pass
inspected commit messages.

The defensive installer closes the loop: future runs of
`simit hooks install` will either correct the rogue value (with
`--fix`) or shout about it in red.

Originating evidence:
[`../hooks-enforcement/.calibration.json`](../hooks-enforcement/.calibration.json)
`missed-signal: phase02-installer-not-defensive`.

## Out of scope

- Inspecting or modifying `~/.gitconfig` or `--global` config.
  Defensive checks operate on the local repo only.
- Handling non-canix dispatcher patterns (lefthook, husky). If
  the sentinel `dispatched-by-canix` is absent, the existing
  phase-02 warning ("git will execute that directory instead")
  is preserved and no defensive cleanup is attempted.
- Re-detecting `hooks` feature status mid-install. The detector
  from hooks-enforcement phase 01 is already correct; no changes
  there.

## Plan

1. **Extend the local-config inspection in `src/commands/hooks.rs`.**
   Read `git -C workspace_root config --local --get core.hooksPath`
   (note `--local`, not `--get` which inherits from global).
   Classify the local value:
   - **`None`** — nothing to defend against; proceed as today.
   - **`Some(path)` where `path` (resolved against
     `workspace_root`) is the project's own `<git-common-dir>/hooks`** —
     this is the rogue case. Continue to step 2.
   - **`Some(path)` pointing somewhere else** — user has an
     intentional non-standard layout. Print a single-line
     informational note and proceed; do not touch their value.

2. **Check the effective system path.** Read
   `git -C workspace_root config --system --get core.hooksPath`
   (or fall back to `--global` if `--system` is unset). If that
   path contains a `dispatched-by-canix` sentinel file, the
   "friendly dispatcher is in place" condition is met.

3. **Behavioral matrix:**

   | local                    | dispatcher? | `--fix` flag | action                                                                                                                 |
   | ------------------------ | ----------- | ------------ | ---------------------------------------------------------------------------------------------------------------------- |
   | rogue (own `.git/hooks`) | yes         | no           | print loud warning; install proceeds                                                                                   |
   | rogue (own `.git/hooks`) | yes         | yes          | `git config --local --unset core.hooksPath`; print one-line "unset rogue local core.hooksPath"; install proceeds       |
   | rogue (own `.git/hooks`) | no          | no           | print existing phase-02 warning + the new rogue-local warning; install proceeds                                        |
   | rogue (own `.git/hooks`) | no          | yes          | refuse to auto-fix (no dispatcher to fall back to); print warning explaining why `--fix` was skipped; install proceeds |
   | none / other             | —           | —            | unchanged from today                                                                                                   |

   Warning text (rogue + dispatcher + no `--fix`):

   ```
   warning: repo-local core.hooksPath = .git/hooks shadows the
   system dispatcher at <effective-system-path>. Project pre-commit
   hooks will fire, but the dispatcher's system step (e.g. AI-strip
   commit-msg) is bypassed. Run `simit hooks install --fix` to
   unset the local override and restore the dispatcher chain.
   ```

4. **Add the `--fix` flag** in `src/cli.rs` to the
   `HooksInstallCommand` struct. Document in `--help`:

   ```
   --fix  Unset rogue repo-local core.hooksPath values that
          shadow a friendly system dispatcher (no-op otherwise).
   ```

5. **Integration test.** Add a test in `tests/hooks_install.rs`:
   - Create a temp git repo with a minimal `nix/pre-commit.nix`.
   - Set up a fake "system" dispatcher dir in another tempdir
     containing a `dispatched-by-canix` sentinel.
     Inject it into the repo's config via
     `git config --local core.hooksPath <fake-dispatcher>` _(test
     uses local; production reads system but the warning logic
     is identical and easier to test against local)_. Actually,
     to keep test isolation, the test should override the system
     read path via a helper — see the existing test scaffold for
     how this is done if it exists, otherwise factor a
     `system_hooks_path()` function that the test can
     monkey-patch.
   - Set the rogue
     `git config --local core.hooksPath .git/hooks` value too.
     (Hint: this is contradictory in real life; for the test you
     may need to refactor — possibly mock both `--local` and
     `--system` reads, or assert against the
     `effective_hooks_path` resolver alone.)
   - Run `simit hooks install`; assert stderr contains the
     warning text.
   - Run `simit hooks install --fix`; assert
     `git config --local --get core.hooksPath` is now unset and
     install still succeeds.
   - Run `simit hooks install --fix` in a repo with no rogue
     local value; assert no config mutation occurred (snapshot
     `git config --list --show-scope` before/after).

6. **Update the existing "no config writes during install"
   assertion.** The phase-02 acceptance criterion in the prior
   plan was "No write to the user's `~/.gitconfig` or repo-local
   git config during install". That remains true _without
   `--fix`_. With `--fix`, the installer intentionally writes
   (an unset) to local config. Document the carveout in the
   integration test and the CHANGELOG entry; do not silently
   change behavior.

7. **Run gates:**

   ```sh
   cargo fmt --all -- --check
   cargo test --all-features
   cargo clippy --all-targets --all-features -- --deny warnings
   ```

8. **CHANGELOG.** Add a `[Unreleased]` entry:

   ```
   - `simit hooks install` warns when a repo-local
     `core.hooksPath` shadows a friendly system dispatcher.
     Pass `--fix` to unset the rogue value automatically.
   ```

## Acceptance criteria

- [ ] `simit hooks install --help` lists the `--fix` flag with
      the documented behavior.
- [ ] In a test repo with rogue
      `core.hooksPath = .git/hooks` (local) AND a friendly
      dispatcher (sentinel) as the effective system path, running
      `simit hooks install` (no `--fix`) exits 0, installs hooks,
      AND prints the warning text on stderr.
- [ ] Same setup with `--fix`: install exits 0, hooks installed,
      `git config --local --get core.hooksPath` is unset
      afterwards.
- [ ] In a test repo with no rogue local config, neither
      `--fix` nor the warning path triggers any config mutation.
- [ ] Integration test for all three scenarios (no rogue, rogue
      no-fix, rogue with-fix) passes under
      `cargo test --all-features --test hooks_install`.
- [ ] `cargo clippy --all-targets --all-features -- --deny warnings`
      is clean.
- [ ] CHANGELOG `[Unreleased]` mentions the new behavior.

## Files likely touched

- `src/commands/hooks.rs` — detection + warning + conditional
  unset.
- `src/cli.rs` — `--fix` flag on `HooksInstallCommand`. **Note:**
  if any of phases 02 (in this plan set) also lands a `cli.rs`
  edit, whichever lands first forces a small rebase. Phase 02 in
  this set only touches `src/commands/projects.rs`, so likely no
  conflict.
- `tests/hooks_install.rs` — three new scenarios.
- `CHANGELOG.md` — `[Unreleased]` entry.

## Pitfalls

**P1. `--local --get` returns non-zero when unset.** Symptom:
the installer aborts. Cause: git returns 1 for "key not set"
on `--get`. Recovery: handle non-zero exit as "unset" rather
than as error, the same way `effective_hooks_path` already does
in `src/commands/hooks.rs`.

**P2. Resolving `core.hooksPath = .git/hooks` relative.**
Symptom: comparison against `<git-common-dir>/hooks` (absolute)
fails. Cause: the rogue value is stored as a relative string.
Recovery: canonicalize both sides — `Path::new(".git/hooks")`
joined against `workspace_root` then canonicalized, vs.
`git_common_dir.join("hooks")` canonicalized.

**P3. The sentinel file may have a different name in the
future.** Symptom: dispatcher correctly chained but installer
doesn't recognize it. Cause: the sentinel path is hardcoded.
Recovery: keep the name as a `const SENTINEL: &str =
"dispatched-by-canix"` near the top of `hooks.rs` so a future
rename is a single-line change.

**P4. `git config --system` may not work in non-NixOS
environments where system config is not writeable or readable.**
Symptom: test fails in CI. Cause: `--system` requires
`/etc/gitconfig` permissions on Linux; some sandboxes restrict.
Recovery: fall back to `--global` when `--system` exits non-zero
or returns empty. The warning text already says "system
dispatcher" in a friendly-fuzzy way.

**P5. User has set a _local_ override to point at the canix
dispatcher path explicitly.** Symptom: the rogue-detector
flags it as benign (it's not rogue), but skips the
`dispatched-by-canix` sentinel check. Cause: in step 1, we
classify by "local resolves to own `.git/hooks`" — pointing at
the dispatcher path is a different case. Behavior: do nothing
(this user intentionally pinned a local path). Already covered
by the "Some(path) pointing somewhere else" branch.

## Reference

- Triggering surprise:
  [`../hooks-enforcement/.calibration.json`](../hooks-enforcement/.calibration.json),
  field `verify.surprises`.
- Existing installer:
  `/data/nvme0/can/Projects/simit/src/commands/hooks.rs`.
- Existing warn function: `warn_if_git_will_not_execute_installed_hooks`
  at `src/commands/hooks.rs:99`.
- Phase 02 (sibling): [02-simit-scan-surfaces-missing-paths.md](./02-simit-scan-surfaces-missing-paths.md).

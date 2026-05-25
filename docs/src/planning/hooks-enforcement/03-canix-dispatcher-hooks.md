# Phase 03 — Replace canix global hooks directory with a forwarding dispatcher

> **Recommended Codex model: GPT 5.5 high**
>
> Complex sub-agent work with infrastructure-SPOF blast radius. The
> phase rewrites the user's system-wide git hook dispatch in a
> Nix-managed home-manager module, with three non-obvious traps:
> (1) `pre-push` carries refs on stdin that a naive `exec` would
> consume once and lose, (2) `core.hooksPath` resolution interacts
> with worktrees, bare clones, and per-repo overrides, (3) a botched
> dispatcher script can block every commit on the host until rolled
> back. The design also requires deciding which subset of git's many
> hook names to symlink in the dispatcher (every hook git can fire,
> or only the ones the canix system step + pre-commit framework
> actually use). `medium` invites a half-correct dispatcher that
> works for `commit-msg` but silently drops `pre-push` stdin. `max`
> is overkill — the solution space is bounded once the design
> questions are settled.

## Working tree

`/data/nvme0/can/Projects/canix` — caniko's NixOS multi-host
flake repo. Single repo.

## Goal

The home-manager git module installs a _dispatcher_ hook directory at
`core.hooksPath` such that for every git hook event:

1. The existing canix system step (today: `commit-msg` AI co-author
   strip) runs first.
2. After the system step exits 0, the dispatcher invokes the project's
   resolved per-repo hook at `$(git rev-parse --git-path hooks)/<hook-name>`
   if it exists, forwarding the original argv and stdin.
3. If either step exits non-zero, the dispatcher exits non-zero with
   the same status — git aborts the operation as expected.

`init.templateDir` continues to seed `commit-msg` into `.git/hooks/`
on `git init`/`git clone` for repos where the user has set a local
`core.hooksPath` override (lefthook, husky, etc.) and bypassed the
system dispatcher — same comment as the existing line 11–13 in the
current module, preserved.

## Why this matters now

The current setup blocks every project-level pre-commit hook on the
host (see [research dossier](./hooks-enforcement-research.md)
section "Current Reality"). Pre-commit framework refuses to
install while `core.hooksPath` is set, and even if it did install,
git wouldn't run the project hooks because the system path takes
priority. Adding a dispatcher restores per-project enforcement
without losing the AI-strip behavior.

User chose Option B from the dossier's "Open Decisions" section
specifically to keep the "new clones automatically get AI-strip"
property, which is what the dispatcher delivers.

## Out of scope

- Adding new system hooks. The dispatcher is purely structural; if a
  future canix change wants a new system `pre-commit` step, the
  hooks/ source dir grows a `pre-commit` file and the dispatcher
  picks it up automatically. Not a goal of this phase.
- Reworking `init.templateDir`. Keep the existing seed behavior; the
  dispatcher coexists with it cleanly.
- Modifying simit or detritus.
- Cross-host deployment. Activating the new generation on this host
  is part of acceptance; activating on other canix hosts is a
  follow-up (and is naturally covered by `canix activate <host>`
  cycles).

## Plan

1. **Read the existing module.**
   `home/modules/development/vcs/git.nix` currently defines two Nix
   derivations: `gitHooksDir` (the `core.hooksPath` target) and
   `gitTemplateDir` (the `init.templateDir` target). Lines 1–39.
   Preserve the `gitTemplateDir` derivation and its hookup at
   line 36 unchanged.

2. **Design the dispatcher script.** Create
   `home/modules/development/vcs/hooks/dispatcher.sh` (new file).
   Single script, symlinked under every dispatched hook name in the
   `gitHooksDir` derivation. Script behavior:

   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   hook_name="$(basename "$0")"

   # Identify the source-of-truth system-step file (read-only,
   # baked into the Nix store dir).
   system_step="${SYSTEM_HOOKS_DIR}/${hook_name}"

   # Capture stdin once if this hook receives ref data (pre-push,
   # post-rewrite, etc.) so we can re-feed both steps.
   stdin_file=""
   case "$hook_name" in
     pre-push|post-rewrite|pre-receive|update|post-receive|post-update|push-to-checkout|reference-transaction)
       stdin_file="$(mktemp)"
       trap 'rm -f "$stdin_file"' EXIT
       cat > "$stdin_file"
       ;;
   esac

   # Step 1: system step (if present).
   if [[ -x "$system_step" ]]; then
     if [[ -n "$stdin_file" ]]; then
       "$system_step" "$@" < "$stdin_file"
     else
       "$system_step" "$@"
     fi
   fi

   # Step 2: project hook (if present).
   # Resolve via git so worktrees, gitdir files, and bare repos all
   # work. `--git-path hooks` yields the common-dir hooks/.
   project_hooks_dir="$(git rev-parse --git-path hooks 2>/dev/null || true)"
   if [[ -n "$project_hooks_dir" ]]; then
     project_hook="${project_hooks_dir}/${hook_name}"
     if [[ -x "$project_hook" ]]; then
       if [[ -n "$stdin_file" ]]; then
         exec "$project_hook" "$@" < "$stdin_file"
       else
         exec "$project_hook" "$@"
       fi
     fi
   fi
   ```

   Notes on the design:
   - `set -e` ensures the dispatcher fails fast if the system step
     fails; git aborts the operation. Matches existing behavior.
   - `exec` for the project step means the dispatcher's process is
     replaced — the project hook gets git's expected stdin handling
     for output, the original exit code propagates cleanly. We can
     only `exec` after the system step because `exec` would not
     return to run step 2.
   - The stdin-bearing hooks list is from git's
     [hooks documentation](https://git-scm.com/docs/githooks).
     `pre-push` is the one we critically care about; the others are
     included to avoid future surprise.
   - `SYSTEM_HOOKS_DIR` is substituted at Nix build time (see step 3).

3. **Build the dispatcher derivation in `git.nix`.** Replace the
   `gitHooksDir` derivation with one that:
   - Creates `$out/_system/` and copies the existing
     `./hooks/commit-msg` (and any future system hooks) into it.
   - Writes a copy of `dispatcher.sh` into `$out/_dispatcher.sh`
     with `SYSTEM_HOOKS_DIR=$out/_system` substituted.
   - For each hook name in the dispatched set, creates a symlink
     `$out/<hook-name> -> _dispatcher.sh`.

   ```nix
   gitHooksDir = pkgs.runCommand "git-global-hooks" {
     nativeBuildInputs = [ pkgs.bash ];
   } ''
     mkdir -p $out/_system
     cp ${./hooks/commit-msg} $out/_system/commit-msg
     chmod +x $out/_system/commit-msg

     substitute ${./hooks/dispatcher.sh} $out/_dispatcher.sh \
       --subst-var-by SYSTEM_HOOKS_DIR "$out/_system"
     chmod +x $out/_dispatcher.sh

     for hook in \
       applypatch-msg pre-applypatch post-applypatch \
       pre-commit pre-merge-commit prepare-commit-msg commit-msg \
       post-commit pre-rebase post-checkout post-merge \
       pre-push pre-receive update proc-receive post-receive \
       post-update reference-transaction push-to-checkout \
       pre-auto-gc post-rewrite sendemail-validate \
       fsmonitor-watchman p4-changelist p4-prepare-changelist \
       p4-post-changelist p4-pre-submit post-index-change
     do
       ln -s _dispatcher.sh "$out/$hook"
     done

     # Sentinel so simit's `simit hooks install` can detect that the
     # foreign core.hooksPath is a friendly dispatcher, not a user
     # mistake. See phase 02 plan step 2.
     touch $out/dispatched-by-canix
   '';
   ```

   The full hook list comes from
   <https://git-scm.com/docs/githooks>. Symlinking all of them is
   cheap and means the dispatcher is forward-compatible if a future
   project hook framework uses an obscure hook name.

4. **Keep `init.templateDir` untouched.** The existing
   `gitTemplateDir` derivation at line 14–18 stays. New clones still
   get `.git/hooks/commit-msg` seeded directly. With the new
   dispatcher in place, if a project also sets a local
   `core.hooksPath` (lefthook), the template-seeded commit-msg
   remains the last-resort fallback for that project (lefthook will
   chain to it if configured, or git will run only lefthook's hooks
   — that's the user's choice in that repo).

5. **Sanity-test the dispatcher locally before activation.**

   ```sh
   # Build the new generation without activating.
   nix build .#homeConfigurations.<this-host>.activationPackage \
     --out-link /tmp/canix-hooks-test
   ls -la /tmp/canix-hooks-test/home-files/.config  # spot-check

   # Sanity-run the dispatcher against a scratch repo.
   tmp=$(mktemp -d)
   ( cd "$tmp" && git init && cat > .git/hooks/pre-commit <<'EOF'
   #!/usr/bin/env bash
   echo "project pre-commit fired" >&2
   exit 0
   EOF
   chmod +x .git/hooks/pre-commit
   git commit --allow-empty -m "test" -c \
     core.hooksPath=$(realpath /nix/store/*-git-global-hooks)
   )

   # Sanity-run pre-push stdin handling.
   ( cd "$tmp" && cat > .git/hooks/pre-push <<'EOF'
   #!/usr/bin/env bash
   echo "pre-push got: $*" >&2
   cat >&2
   exit 0
   EOF
   chmod +x .git/hooks/pre-push
   # Drive pre-push directly (no remote needed):
   echo "refs/heads/main 0000000 refs/heads/main 0000000" | \
     env GIT_DIR=.git \
     /nix/store/*-git-global-hooks/pre-push origin git@example.com:x
   # Expect the system step's commit-msg behavior is irrelevant
   # here; expect "pre-push got: origin git@example.com:x" plus
   # the ref line echoed.
   )
   ```

6. **Activate on this host.**

   ```sh
   # Whatever the canix activation command is. Likely:
   canix activate
   # or
   nh home switch .  # if using nh
   # or
   home-manager switch --flake .
   ```

   Verify activation:

   ```sh
   git config --global --get core.hooksPath
   # Expect a /nix/store/...-git-global-hooks path that contains
   # both `commit-msg` and `pre-commit` (symlinks).
   ls -la "$(git config --global --get core.hooksPath)"
   ```

7. **End-to-end sanity in detritus** (full acceptance is phase 04):

   ```sh
   cd /data/nvme0/can/Projects/detritus
   # Write a fake project pre-commit to prove dispatch works,
   # without yet running phase 04's real installer.
   cat > .git/hooks/pre-commit <<'EOF'
   #!/usr/bin/env bash
   echo "DETRITUS PRE-COMMIT FIRED" >&2
   exit 0
   EOF
   chmod +x .git/hooks/pre-commit
   git commit --allow-empty -m "dispatch smoke test"
   # Expect: stderr contains "DETRITUS PRE-COMMIT FIRED"
   # Expect: commit message has no AI co-author line (system step worked).
   git log -1 --format=%B
   # Cleanup so phase 04 starts from a clean .git/hooks/.
   rm .git/hooks/pre-commit
   ```

## Risk profile

- **R1.** Dispatcher script bug blocks every commit on the host
  until rolled back. Includes: wrong shebang, `set -e` triggering
  unexpectedly, `git rev-parse` failure outside a repo (the canix
  `commit-msg` hook may be invoked by `git commit` even in places
  the user doesn't think of as repos, e.g. `git stash`).
- **R2.** `pre-push` stdin forwarding subtly wrong. Symptom: pushes
  succeed but the project's pre-push hook doesn't see ref lines, so
  guards meant to gate pushes silently pass. This is the
  highest-impact "looks fine but isn't" failure mode.
- **R3.** `git rev-parse --git-path hooks` inside a non-git
  directory exits non-zero, aborting the dispatcher under `set -e`.
  This breaks `commit-msg` invocation for `git commit` run inside
  weird states (during rebase abort, in submodules, etc.).
- **R4.** Project hook on PATH but not executable. The dispatcher
  skips silently. Probably correct (matches git's own behavior
  before `core.hooksPath` was set), but worth confirming.
- **R5.** Symlinking the dispatcher under all 25+ hook names enables
  hooks the user previously had no system step for, which could
  cause surprise interactions with other tools (e.g.,
  `post-checkout` firing in CI runners or scripted clones). Risk is
  low because the system step is a no-op when the source file
  doesn't exist, but worth documenting in the canix changelog.

## Strategy

Single commit on a canix branch. Verify locally with the scratch
repo from plan step 5 before `canix activate`. The build is in the
Nix store with a different hash from the prior generation, so
`home-manager generations` retains the previous generation for
rollback.

Commit ladder:

- Commit 1: add `dispatcher.sh` source and rewrite `git.nix`.
  Includes a regression test if canix has a test harness; if not,
  the plan-step-5 scratch-repo sanity check serves.
- Activate via `canix activate` (or equivalent) — not part of the
  commit but the gate before declaring the phase done.

## Rollback drill

Practice before activating:

```sh
# Identify the current generation.
home-manager generations | head -5

# Activate the previous generation.
/nix/store/...-home-manager-generation/activate
# (path comes from `home-manager generations` output)

# Verify rollback.
git config --global --get core.hooksPath
# Expect the prior hash, with the prior `commit-msg`-only behavior.
```

SLA: 30 seconds from "commits are broken" → "previous generation
active". If the SLA is at risk because home-manager activation is
slow on this host, also know the in-place override:

```sh
# Emergency: bypass the broken dispatcher without rolling back canix.
git config --global --unset core.hooksPath
# Restore after fix:
git config --global core.hooksPath "$(home-manager option \
  programs.git.settings.core.hooksPath)"
```

## Failure modes and recoveries

**F1 — pre-push hook gets empty stdin.** Symptom: `git push` works
but project pre-push hook reports zero refs. Cause: `cat > tmp` in
the dispatcher exited early because of `set -o pipefail` interaction
with an upstream `tee` or because the case-statement missed the hook
name. Recovery: confirm `pre-push` is in the stdin-capture case
statement; test with the plan-step-5 stdin sanity invocation.

**F2 — `git commit` aborts with "rev-parse: not a git repository".**
Symptom: every commit fails after activation. Cause: `git rev-parse
--git-path hooks` returns non-zero in some contexts and `set -e`
aborts the dispatcher. Recovery: the script already uses `|| true`
on that call; if the failure surfaces, instrument the failing case
and tighten the trap. Rollback if not fixable in < 5 min.

**F3 — System step runs twice.** Symptom: duplicate AI-co-author
strip (no-op visually but a sign of a logic bug). Cause: `exec`
placement wrong, system step accidentally invoked after the project
step. Recovery: re-read the dispatcher; ensure `exec` is the
last statement on the project-hook branch and the system step
appears once before it.

**F4 — Project hook does not fire even though `.git/hooks/<name>`
exists.** Symptom: detritus commits succeed without running clippy.
Cause: `git rev-parse --git-path hooks` returned a path that doesn't
match the project's actual hooks dir (worktree edge case, gitdir
file pointing elsewhere). Recovery: add a debug echo to the
dispatcher temporarily and diagnose. The fix is usually to honor
the literal `rev-parse` output rather than re-deriving the path.

**F5 — pre-commit framework still refuses to install on this host.**
Symptom: after phase 03 activation, phase 04 sees the same
"Cowardly refusing" error from `pre-commit install`. Cause: that's
expected — phase 04 uses the simit installer from phase 02, which
overrides `core.hooksPath` for the install subshell. If phase 04
falls back to plain `nix develop`, that path will still fail.
Recovery: this is why the simit `hooks install` subcommand exists.
Do not regress to invoking `pre-commit install` from the shellHook.

## Acceptance criteria

- [ ] `home/modules/development/vcs/git.nix` builds a `gitHooksDir`
      derivation whose output contains symlinks for every hook name
      in the list, all pointing at `_dispatcher.sh`, plus a
      `_system/` directory with the canix `commit-msg`, plus a
      `dispatched-by-canix` sentinel file.
- [ ] `home/modules/development/vcs/hooks/dispatcher.sh` exists,
      handles `pre-push` stdin via temp file, and execs the project
      hook last.
- [ ] After `canix activate`,
      `git config --global --get core.hooksPath` points at the new
      dispatcher derivation.
- [ ] In any repo on the host, `git commit -m "test
    Co-Authored-By: Foo <noreply@anthropic.com>"` produces a
      commit whose message no longer contains the
      `Co-Authored-By:` line (system step preserved).
- [ ] In the detritus smoke test from plan step 7, the synthetic
      `.git/hooks/pre-commit` fires and its stderr appears.
- [ ] `pre-push` smoke test from plan step 5 shows the project hook
      receives the ref line on stdin.
- [ ] Previous generation is available via `home-manager
    generations` and the rollback command in the rollback drill
      executes in under 30 seconds.

## Files likely touched

- `/data/nvme0/can/Projects/canix/home/modules/development/vcs/git.nix`
  — rewrite `gitHooksDir`, keep `gitTemplateDir` and config block.
- `/data/nvme0/can/Projects/canix/home/modules/development/vcs/hooks/dispatcher.sh`
  — new file.
- `/data/nvme0/can/Projects/canix/home/modules/development/vcs/hooks/commit-msg`
  — existing; unchanged.
- (Possibly) canix `CHANGELOG.md` or release notes if the repo
  tracks them.

No simit, no detritus, no per-project files change here.

## Pitfalls

(See "Failure modes and recoveries" above for the high-impact ones.)

**P1. `substitute` is a `pkgs.stdenv`-provided helper.** Symptom:
`runCommand` build fails with `substitute: command not found`.
Cause: `runCommand` doesn't always pull in `stdenv` setup. Recovery:
use `runCommandLocal` or explicitly source `$stdenv/setup`, or write
the dispatcher with an inline `sed`:

```nix
sed "s|@SYSTEM_HOOKS_DIR@|$out/_system|g" \
    ${./hooks/dispatcher.sh} > $out/_dispatcher.sh
chmod +x $out/_dispatcher.sh
```

and use `@SYSTEM_HOOKS_DIR@` as the placeholder in the script.

**P2. Shell glob across `/nix/store/*-git-global-hooks`** in test
commands may match multiple generations. Symptom: scripts use the
wrong path. Recovery: in tests, resolve via `git config` rather
than globbing the store.

**P3. The `dispatched-by-canix` sentinel filename collides with a
hook name.** Won't happen with the chosen name, but worth a sanity
check against git's hook list before merging.

**P4. Activation order.** Symptom: after `canix activate`, the
shell still sees the old `core.hooksPath` value. Cause: home-manager
writes a new `.gitconfig` but the user's current shell may have
cached the env. Recovery: `git config --global --get
core.hooksPath` re-reads the file — no shell restart needed. If a
test driver caches the value, restart it.

## Reference

- Research dossier: [hooks-enforcement-research.md](./hooks-enforcement-research.md)
- Existing module:
  `/data/nvme0/can/Projects/canix/home/modules/development/vcs/git.nix:1-39`.
- Git hooks reference (canonical hook name list):
  <https://git-scm.com/docs/githooks>.
- `git rev-parse --git-path`:
  <https://git-scm.com/docs/git-rev-parse#Documentation/git-rev-parse.txt---git-pathltpathgt>.
- Phase 02 sentinel detection logic:
  [02-simit-hooks-install-subcommand.md](./02-simit-hooks-install-subcommand.md) plan step 2.
- Phase 04 (terminal verification):
  [04-fleet-sweep-install-hooks.md](./04-fleet-sweep-install-hooks.md).

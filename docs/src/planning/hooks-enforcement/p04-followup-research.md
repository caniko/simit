---
name: Hooks-enforcement Phase 04 follow-up research
description: Diagnose why Phase 04's AI-strip proof failed despite hooks landing in six repos, audit simit's hook-state detection against the sorrel anomaly, and stage the remaining work needed to close the hooks-enforcement plan.
---

# Hooks-enforcement Phase 04 Follow-up Research

## Goal And Trigger

Phase 04 of the
[hooks-enforcement plan](./README.md) reported partial acceptance:

- **Per-project install** passed in all six target repos (`detritus`,
  `open-data-license`, `rs-memory-admission`, `simit`, `skillnet`,
  `sorrel`) — `simit hooks install --check` exits 0 and each repo has
  `pre-commit/pre-push/commit-msg` written into its `.git/hooks/`.
- **End-to-end clippy proof** passed in detritus (commit rejected on a
  deliberate `cognitive_complexity` violation).
- **AI-strip proof failed**: an empty commit carrying
  `Co-Authored-By: Test User <noreply@anthropic.com>` retained the
  trailer in `git log -1 --format=%B` rather than being stripped by
  the canix system commit-msg hook.

Stated root cause from the Phase 04 report: _"all six repos now have a
local core.hooksPath pointing at .git/hooks, which overrides the global
canix dispatcher path."_

The user wants research to settle (a) what actually happened in the
proof, (b) what is durably broken in either simit or canix, and (c)
what residual phases close out the plan set.

## Current Reality

### Effective `core.hooksPath` per repo (re-measured 2026-05-25)

```
detritus            → /nix/store/r04cyg001f5ka0v5wqgqlx21xmcrsnc8-git-global-hooks  (global)
open-data-license   → /nix/store/…-git-global-hooks                                  (global)
rs-memory-admission → /nix/store/…-git-global-hooks                                  (global)
simit               → /nix/store/…-git-global-hooks                                  (global)
skillnet            → /nix/store/…-git-global-hooks                                  (global)
sorrel              → .git/hooks                                                     (LOCAL — bypasses dispatcher)
```

Source: `git -C <repo> config --show-origin --get core.hooksPath` on
each path. Only **sorrel** currently has a local override; the other
five route through the canix dispatcher. The Phase 04 report's claim
that all six had the local override does not match the current state —
either the override was a transient mid-install state and a later
cleanup reverted five of six (sorrel undone), or the report
generalised from one repo. Either reading leaves sorrel as a real,
reproducible broken case worth solving.

### Sorrel-specific anomalies

```
sorrel/.git/config:
[core]
    hooksPath = .git/hooks

sorrel/.git/hooks/  (excluding *.sample):
(empty)
```

Sorrel has a local `core.hooksPath` redirect to `.git/hooks` _and_ no
non-sample hook files in that directory. Net effect: no hook runs at
all on `git commit`, including the canix AI-strip system hook. This
fully explains the Phase 04 AI-strip proof failure if the test ran in
sorrel (or any repo in the same state at the time).

Meanwhile, [simit src/commands/projects.rs] still reports
`hooks: installed` for sorrel — the Phase 01 detector is not
recognising either gap.

### The canix dispatcher contract

`/nix/store/r04cyg001f5ka0v5wqgqlx21xmcrsnc8-git-global-hooks/_dispatcher.sh`
does exactly what the hooks-enforcement plan intended:

1. Run `_system/${hook_name}` if present (e.g.,
   `_system/commit-msg` strips `Co-Authored-By: …<noreply@anthropic.com>`).
2. `exec` `$(git rev-parse --git-common-dir)/hooks/${hook_name}` if
   present, forwarding stdin and args.

This is the "Option B" dispatcher Phase 03 was meant to deliver, and
it works correctly for every repo whose effective hooksPath is the
dispatcher. The `_system/commit-msg` content is a two-line sed script
matching exactly the trailer the failed proof used, so the strip is
not a regex-mismatch issue — it simply did not run.

### What simit's hooks installer actually does

[`src/commands/hooks.rs`](../../../../src/commands/hooks.rs):

- `install_in_workspace` calls `pre-commit install --overwrite
--hook-type pre-commit --hook-type pre-push --hook-type commit-msg`
  with `GIT_CONFIG_COUNT=1`, `GIT_CONFIG_KEY_0=core.hooksPath`,
  `GIT_CONFIG_VALUE_0=""`. The `GIT_CONFIG_*` triple forces git's
  _read_ view of `core.hooksPath` to empty for the duration of the
  `pre-commit install` child, so pre-commit thinks no override exists
  and writes its wrapper into `.git/hooks/`.
- `warn_if_git_will_not_execute_installed_hooks` already whitelists
  three "the chain still works" cases, including detecting the canix
  dispatcher via the `dispatched-by-canix` sentinel file. Outside
  those cases it prints a warning but does not refuse to install.

The `GIT_CONFIG_*` mechanism prevents simit's installer from _reading_
a non-empty `core.hooksPath`, but it does not prevent something else
(a different `pre-commit install` invocation outside simit's wrapper,
a stray editor plugin, a manual `git config core.hooksPath .git/hooks`)
from writing the local override. Sorrel's state is consistent with one
such out-of-band write at some point in its history — the simit
installer would not have produced it.

### What simit's hooks detector misses

[`src/registry.rs`](../../../../src/registry.rs) `detect_hooks_status`
(post Phase 01) classifies sorrel as `installed`. It does not check:

- Whether local `core.hooksPath` differs from the canix dispatcher
  AND differs from `.git/hooks` of the same repo's `git_common_dir`
  AND breaks the dispatcher chain (sorrel: yes).
- Whether the hook files referenced by the effective hooksPath
  actually exist (sorrel: missing).
- Whether the canix dispatcher is in use and the project's
  `.git/hooks/<name>` files exist as the chain expects.

Either gap alone is enough to leave a project silently un-enforced.
Together they are sorrel.

### Toolchain staleness

The installed simit profile is `0.15.2`, which predates the
hooks-enforcement plan landing. It cannot parse
`features.hooks = "conflicted"` and lacks the `hooks` subcommand
entirely. Phase 04 was run with
`/data/nvme0/can/Projects/simit/target/release/simit` built from the
local checkout. Until a release ships these features, every downstream
user (including future-self picking this work back up after a fresh
shell) is on the old behaviour.

## Evidence Inventory

| Source                                                                         | Proves                                                                                                                       |
| ------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| [`.git/config` of sorrel](/data/nvme0/can/Projects/sorrel/.git/config)         | `[core] hooksPath = .git/hooks` set locally; bypasses dispatcher.                                                            |
| `ls /data/nvme0/can/Projects/sorrel/.git/hooks` (no non-sample files)          | The redirected directory is empty; no hook runs.                                                                             |
| `git -C <repo> config --show-origin --get core.hooksPath` for each repo        | Five of six route to dispatcher; sorrel routes to local empty dir.                                                           |
| `/nix/store/…-git-global-hooks/_dispatcher.sh`                                 | Dispatcher correctly chains `_system/<hook>` then `exec $git_common_dir/hooks/<hook>`.                                       |
| `/nix/store/…-git-global-hooks/_system/commit-msg`                             | AI-strip sed regex matches the failed-proof trailer exactly — content is fine.                                               |
| `simit projects list --json` (built from checkout)                             | Sorrel reports `hooks: installed`, masking the broken state.                                                                 |
| [`src/commands/hooks.rs:99-115`](../../../../src/commands/hooks.rs#L99-L115)   | Installer warns but does not refuse on hooksPath mismatch; only detects canix via sentinel.                                  |
| [`src/commands/hooks.rs:122-132`](../../../../src/commands/hooks.rs#L122-L132) | `GIT_CONFIG_VALUE_0=""` trick prevents simit's wrapper from mutating local hooksPath, but cannot prevent out-of-band writes. |
| Phase 04 transcript (user-provided)                                            | Per-repo install succeeded; clippy proof succeeded in detritus; AI-strip proof failed.                                       |
| `git log --oneline -10` on simit                                               | Latest published release is 0.15.2; hooks subcommand exists only in working tree.                                            |

## Existing Plan Status

| Phase | File                                                                           | Verdict                                                                                                                                                                                          |
| ----- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 01    | [01-simit-detect-hooks-status.md](./01-simit-detect-hooks-status.md)           | **partial** — `FeatureStatus::Conflicted` exists in the type but the classifier does not flag sorrel-style conflicts (local hooksPath override + missing files).                                 |
| 02    | [02-simit-hooks-install-subcommand.md](./02-simit-hooks-install-subcommand.md) | **done** for the happy path; missing post-install verification that the chain actually runs end-to-end.                                                                                          |
| 03    | [03-canix-dispatcher-hooks.md](./03-canix-dispatcher-hooks.md)                 | **done** — dispatcher present and chaining correctly.                                                                                                                                            |
| 04    | [04-fleet-sweep-install-hooks.md](./04-fleet-sweep-install-hooks.md)           | **partial** — six installs succeeded, clippy proof passed, AI-strip proof failed. Acceptance criterion "AI co-author trailer is stripped by the canix system hook on a real commit" not yet met. |
| 05    | [05-simit-surface-conflicted-state.md](./05-simit-surface-conflicted-state.md) | **not started**, and now blocked by Phase 01's gap above — surfacing a state the classifier never produces would be a no-op.                                                                     |

Carry-forward from this audit:

- Phase 01 needs a tightening pass for the two missed conflict cases.
- Phase 04's AI-strip acceptance needs to be re-proven _after_ Phase 01
  reclassifies sorrel as conflicted (or after sorrel is repaired —
  whichever the user prefers as the proof environment).
- Phase 05 only becomes worth running once Phase 01's classifier
  reliably emits `conflicted` for known-broken states.

## Work That Should Survive Into The Long-Term Plan

1. **Diagnose sorrel's local hooksPath origin.** Decide whether simit's
   installer needs a defensive cleanup pass (post-install: if local
   `core.hooksPath` was _added by this process_ and points anywhere
   other than the canix dispatcher, unset it) or whether the rule
   should be "detect-and-refuse without touching user config". User
   has historically preferred non-destructive behaviour for git config
   — confirm before mutating.
2. **Tighten `detect_hooks_status`.** Promote `installed` → `conflicted`
   when any of:
   - Effective `core.hooksPath` neither equals `.git/hooks` of the
     repo _nor_ contains the `dispatched-by-canix` sentinel.
   - Effective hooks directory exists but the `pre-commit`,
     `pre-push`, or `commit-msg` files this project declares are
     absent.
   - The repo has `nix/pre-commit.nix` (or `.pre-commit-config.yaml`)
     declaring hooks that are missing from the effective directory.
     Add focused unit tests using a temp git repo for each branch.
3. **Add a `simit hooks install` post-condition check.** After invoking
   pre-commit, re-read local `core.hooksPath`. If it was mutated and
   the new value bypasses the canix dispatcher (sentinel check), emit
   a hard error explaining the conflict and pointing at remediation.
   This catches the failure mode even if its origin remains the
   "something else wrote local hooksPath" path.
4. **Repair sorrel.** `git -C sorrel config --local --unset
core.hooksPath` then `simit hooks install` (or just `simit hooks
install` after the post-condition check in item 3 lands and treats
   this as a known case). Re-run AI-strip proof in sorrel.
5. **Release simit with the hooks subcommand and the tightened
   classifier.** The version bump can roll Phase 01 tightening, the
   Phase 02 installer, the post-condition guard, and Phase 05 into a
   single release; downstream users currently cannot exercise any of
   this without a checkout build.
6. **Phase 05 (surface conflicted state).** Now becomes meaningful
   once item 2 is in. Keep the design content already in the existing
   Phase 05 doc; only the precondition changes.
7. **Re-prove Phase 04 acceptance criteria end-to-end** after items
   2–6 land. The AI-strip proof must pass on a repo that is supposed
   to chain through the dispatcher (e.g. detritus, where the current
   snapshot already routes correctly), not just on sorrel.

## Blockers And Missing Artifacts

- **Sorrel origin trace.** The git reflog / shell history is not in
  scope of this dossier to reconstruct, but if the user can recall
  whether they ran a bare `pre-commit install` outside simit's
  wrapper in sorrel, that closes the diagnostic question without
  needing a deeper test. If recall is unavailable, write the
  post-install guard regardless — it is defensive against any cause.
- **A canonical AI-strip proof script.** Phase 04 used an ad-hoc
  empty commit with a fake co-author. A small repeatable script
  (e.g., `tests/integration/ai_strip_smoke.sh` in the canix repo, or
  a simit acceptance fixture) would let any future regression sweep
  run the proof in seconds instead of from memory.

## Risks And Constraints

- **Mutating user `core.hooksPath`** silently is the most invasive
  remediation. Prefer detection over mutation unless the user
  explicitly opts in. The post-install guard in item 3 surfaces the
  problem; an opt-in `--repair` flag could perform the unset.
- **canix dispatcher is in a `/nix/store` path** — any change to its
  contents requires a canix rebuild and propagation, gating its
  iteration speed against simit's. Avoid putting fix logic on the
  canix side that simit could equally well own.
- **pre-commit's "Cowardly refusing" behaviour** is precisely the
  thing Phase 02's `GIT_CONFIG_VALUE_0` trick works around. If
  upstream pre-commit ever changes its install-time behaviour, simit
  must re-validate the trick still works. Cover this with at least
  one integration test that asserts `simit hooks install` does NOT
  leave a local `core.hooksPath` behind on a repo where one wasn't
  present before.
- **Stale installed simit (`0.15.2`)** means any user who pulls a
  fresh shell loses access to the in-progress hooks work. Release
  cadence should keep up with the plan — the
  `publish-version-extractor-fix` plan set already established the
  pattern of "commit + release + sweep dependents".

## Candidate Phase Boundaries

Proposed follow-up phase set (consolidating with the existing
hooks-enforcement plan rather than starting a fresh one):

1. **F1 — Tighten Phase 01 classifier.** Promote `installed` →
   `conflicted` for (a) effective hooksPath that bypasses the
   dispatcher, (b) files-missing-in-effective-dir. Unit tests per
   branch. Moderate complexity, leaf role.
2. **F2 — Add `simit hooks install` post-condition auto-repair.**
   After invoking pre-commit, check whether local `core.hooksPath`
   was added/changed and would break the canix dispatcher chain; if
   so, unset the local override, log the mutation to stderr, and
   re-verify the chain. Moderate complexity, leaf role.
3. **F3 — Verify F1+F2 against sorrel without repairing it; re-prove
   Phase 04 acceptance on detritus.** Sub-steps: (a) confirm F1
   reports `hooks: conflicted` for sorrel in `simit projects list`;
   (b) run `simit hooks install` on a _copy_ of sorrel's broken
   state (or sorrel itself in a worktree) and confirm F2's repair
   pass unsets the override and leaves working hooks; (c) run the
   AI-strip proof on detritus (chain already healthy) and assert
   the trailer is stripped. Sorrel proper stays in its broken state
   as the living regression fixture. Trivial mechanical +
   verification.
4. **F4 — Release simit with hooks features.** Bump, CHANGELOG, tag,
   publish — mirrors the recipe in the earlier
   `publish-version-extractor-fix` Phase 01. Moderate.
5. **F5 — Phase 05 (surface conflicted state).** Resume the existing
   Phase 05 doc; precondition now satisfied by F1. Moderate.
6. **F6 — Codify the AI-strip smoke test in simit.** Add a simit
   integration test (or `cargo test`-invokable harness) that
   initialises a temp repo, lets the canix dispatcher chain run
   over an empty commit carrying a fake co-author trailer, and
   asserts the trailer is stripped from the resulting commit
   message. Acts as a continuous-regression guard for the
   dispatcher contract. Trivial, leaf.

Dependencies and parallelism:

- F1 and F2 are independent (different files, same crate) and can
  land in parallel, but both gate F3 (which validates them) and F4
  (which releases them).
- F5 depends on F1 only.
- F6 can run any time; it is a documentation/test addition.

## Resolved Decisions

User decisions taken on 2026-05-25:

- **simit auto-repairs.** `simit hooks install` is allowed to mutate
  local `core.hooksPath`: if the effective hooksPath bypasses the
  canix dispatcher and the project is supposed to chain through it,
  the installer unsets the local override as part of the install.
  This sharpens F2 from "post-install guard that refuses" into
  "post-install repair that unsets and re-verifies". Continue to log
  the mutation so a user reading the install output sees what
  changed; do not silently rewrite without a stderr breadcrumb.
- **AI-strip smoke proof lives in simit.** F6 lands as a simit
  acceptance fixture (integration test or a small harness invokable
  from `cargo test`), not as a canix-side test. This keeps the
  proof close to the install flow that gates it and keeps canix
  iteration speed unaffected.
- **Sorrel stays broken on purpose.** It is the live test case for
  F1's tightened classifier and F2's auto-repair. F3 drops the
  "repair sorrel" step and becomes "verify F1 reports `conflicted`
  on sorrel, then verify F2 repairs it idempotently on a _separate_
  invocation, then run the AI-strip proof on detritus (chain
  already healthy) to confirm the system path is intact." Do not
  unset sorrel's local hooksPath out of band — that destroys the
  reproduction.

## Planner Handoff

- **Downstream skill:** `multi-phase-plan-codex` (default; matches
  the existing hooks-enforcement plan set's flavour).
- **Dossier path:**
  [`docs/src/planning/hooks-enforcement/p04-followup-research.md`](./p04-followup-research.md)
  in the simit repo.
- **Current-state summary:** Phase 04 left the plan one acceptance
  criterion short (AI-strip proof). Root cause is a combination of
  Phase 01's classifier missing two real conflict patterns and one
  repo (sorrel) being in a state simit cannot currently express.
  The canix dispatcher and the system AI-strip hook are correct in
  isolation.
- **Phases to produce:** F1–F6 above. Keep them as additions to
  `docs/src/planning/hooks-enforcement/` (e.g., `06-…`, `07-…`)
  rather than spinning up a new plan directory — the work is a
  continuation, not a fresh initiative.
- **Blockers to preserve:** sorrel origin trace and AI-strip proof
  location are open decisions, not hard blockers; the planner can
  proceed with the defaults proposed above and surface them in
  whichever phase's "Open decisions" section is most appropriate.
- **Acceptance evidence the phase set should preserve:** (a)
  `detect_hooks_status` reports `conflicted` for sorrel before
  remediation; (b) `simit hooks install` post-condition test refuses
  to leave a freshly-set bypassing local `core.hooksPath`; (c)
  AI-strip proof passes on detritus end-to-end; (d) released simit
  binary (≥ 0.15.3 or whatever F4 lands as) exposes `hooks` and
  parses `hooks = "conflicted"` from `simit projects list --json`.

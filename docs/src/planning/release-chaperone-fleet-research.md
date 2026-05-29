# Release Chaperone Fleet Research Dossier

## Goal And Trigger

User goal: turn the existing readiness report at
`/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/release-chaperone-gap-report.md`
into a list of concrete improvements that can land in simit, and the
follow-up cleanups the five target repositories (`open-data-license`,
`rs-memory-admission`, `rs-modde`, `skillnet`, `sorrel`) need before
`rust-crate-release-chaperone` can be driven end-to-end without
manual triage at every step.

Trigger: the readiness report is "evidence-gathering only" — it lists
what the chaperone would block on, but does not propose generator or
workflow changes that would prevent the next round from regenerating
the same blockers.

This dossier collects:

- which report claims still hold against the current repo state
  (evidence snapshots taken 2026-05-25; treat exact SHAs and ahead/behind
  counts as point-in-time data, not durable conclusions);
- which blockers are repo-owned vs. simit-generator-owned;
- candidate simit changes that would shrink the chaperone bar across
  the fleet, not just unblock individual releases.

**Version policy note.** Where the original report and the snapshots below
mention specific pinned simit revisions, the durable rule is: every
adopting repo should track the latest simit commit from the default
branch (`trunk`) at the time the chaperone runs. Specific revs are noted
only as evidence that a given pin pre-dated a generator/CLI change; the
fix is always "bump to current `trunk`", not "match this exact rev".

## Current Reality

### simit baseline

- Working tree: `/data/nvme0/can/Projects/simit`, branch `trunk`.
  Recent generator changes that adopting repos need to absorb:
  absolute `CARGO_HOME` for Nix runtime, `cargo-audit` advisory-DB
  pre-fetch, pinned `cargo-deny 0.18.3` policy-checks,
  `Skip publish gates for non-publishable crates`,
  `Treat cargo-deny policy as project-owned`.
- CLI shape: current is `simit init ci`, `simit init flake` (nested
  subcommands). The older flat form (`simit init-ci`,
  `simit init-flake`) was used in earlier simit releases; any repo
  whose flake input pins a pre-restructure rev will fail
  `simit init ci ...` with `unrecognized subcommand 'init'`. The
  fix is always "bump the simit flake input to current `trunk`".

### Per-repo facts

Snapshots below are point-in-time. Re-run the checks before acting.

- [`open-data-license`](file:///data/nvme0/can/Projects/open-data-license):
  default branch `trunk`. `flake.nix` does not consume simit as a
  flake input; CI/dev shells fall back to whatever simit is in PATH.
  `simit init ci --platform forgejo --check --diff` (no extra flags)
  reports both `ci.yaml` and `publish-crate.yaml` as drifted because
  the actual file uses `--runtime nix --runner atlas-nix-trusted
--with-om-ci --with-msrv --with-audit --with-deny --with-docs`,
  none of which the bare check restores. An adoption branch
  (`simit-ci-adoption-20260525`) carries up-to-date generated CI
  plus recent `cargo-audit`/`cargo-deny` fix-ups and is ahead of
  `origin/trunk`; the fix is to merge it forward and then make
  `simit` available deterministically (either add it as a flake
  input or document the PATH expectation).

- [`rs-memory-admission`](file:///data/nvme0/can/Projects/rs-memory-admission):
  `simit init flake --check --diff` would replace the current
  `nix/pre-commit.nix` `cargo-audit` pre-commit hook with a
  `cargo-msrv` hook. That is a deliberate generator change (audit
  is fetched in CI now), but the diff is silent about removing a
  hook the maintainer relied on. Local worktree had uncommitted
  `.forgejo/workflows/pages.yaml` and `docs/src/development/nix.md`
  at snapshot time plus an untracked `.claude/` directory; re-verify
  before acting.

- [`rs-modde`](file:///data/nvme0/can/Projects/rs-modde):
  the `simit` flake input is pinned to a pre-CLI-restructure
  revision, so `simit init ci ...` invocations inside `nix develop`
  fail with `unrecognized subcommand 'init'`. The report's
  validation command set cannot run there until the flake input is
  bumped to current `trunk`. The repository also carries a legacy
  monolithic `release.yml` (~1k lines) in parallel with
  simit-generated `release-artifacts-modde-*.yaml`; both publish
  Homebrew artifacts, creating a duplicate-write risk on the same
  tag. Many stale `worktree-agent-*` branches still live in
  `git branch -a`.

- [`skillnet`](file:///data/nvme0/can/Projects/skillnet):
  default branch `main` is current; an adoption branch exists at
  snapshot time but appears behind `main`. `simit projects list`
  flags `ci=drift`, but with the full flag set
  (`--with-audit --with-deny --with-docs --with-msrv`)
  `simit init ci --check --diff` is clean — the "drift" is the
  option-persistence gap, not real divergence.
  `simit init flake --check --diff` wants to wholesale-replace the
  project-specific flake that consumes the shared `rs-harbor` flake
  plus `home-manager` and `advisory-db` inputs.

- [`sorrel`](file:///data/nvme0/can/Projects/sorrel):
  local `trunk` carries additional commits beyond `origin/trunk`
  and a substantial dirty worktree (modified `Cargo.lock`, multiple
  `crates/sorrel-*/` source files, modified docs, untracked
  workflows under `.forgejo/workflows/`). Eight publishable crates
  (`sorrel-io`, `sorrel-cache`, `sorrel-compute`, `sorrel-gpu`,
  `sorrel-data`, `sorrel-render`, `sorrel-ui`, `sorrel`) all at
  `0.1.0`. No `CHANGELOG.md`. Dependency ordering for crates.io
  publish is not encoded anywhere. **Verify before implementing:**
  the flake-drift claim (and whether `simit init flake` would
  destructively rewrite sorrel's flake) should be re-checked against
  the cleaned worktree, not the current dirty snapshot, before
  treating it as a hard blocker.

### simit-managed registry view

`nix develop -c simit projects list` (run from
`/data/nvme0/can/Projects/simit`) reports the same 5 fleet
projects as `ci(drift)`, plus `sorrel` as
`flake(drift) ci(drift) hooks(conflicted)`. The footer also lists
ephemeral `/tmp/*` entries left over from test runs; the recent
`0.15.4` work filters them out of the _attention_ footer but they
still appear in the main listing.

## Evidence Inventory

Durable claims (true regardless of snapshot date) are listed first;
snapshot-dependent observations are tagged `(snapshot)` and should be
re-verified before acting.

| Claim                                                                                                           | Evidence                                                                                                                                                                                                                                                                 |
| --------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| simit CLI shape is `init ci` / `init flake` (nested); older flat form (`init-ci`, `init-flake`) is incompatible | `simit init --help` from `nix develop` on current `trunk`                                                                                                                                                                                                                |
| `init ci --check --diff` does not infer options from existing files                                             | [src/commands/init_ci.rs:84-103](../../src/commands/init_ci.rs#L84) builds `CiOptions` from `command.with_*`; [src/registry.rs:754-789](../../src/registry.rs#L754) has a separate `infer_ci_options` for drift detection. The two paths are not unified                 |
| simit has no per-project storage for `--with-audit` etc.                                                        | [src/config.rs:106-121](../../src/config.rs#L106) — `CiConfig` has no `runtime`, `runner`, `with_*` fields                                                                                                                                                               |
| Drift detector tolerates non-marked supplementary workflows                                                     | [src/registry.rs:541-562](../../src/registry.rs#L541) — `managed+extra` requires at least one _marked_ workflow; the rest are tolerated                                                                                                                                  |
| `simit init flake` silently swaps `cargo-audit` for `cargo-msrv` in pre-commit                                  | Generator change: see [src/render/flake.rs:296-310](../../src/render/flake.rs#L296), [src/render/flake.rs:587-610](../../src/render/flake.rs#L587). Reproducible on any repo whose flake was generated before the swap                                                   |
| `simit init flake` wholesale-rewrites custom flakes                                                             | Reproducible: run `simit init flake --check --diff` on any repo whose `flake.nix` consumes a shared toolchain flake (e.g. `rs-harbor`) or declares custom outputs                                                                                                        |
| Generated publish workflows trigger on `*.*.*` tags only                                                        | Search `"*.*.*"` in [src/render/ci.rs](../../src/render/ci.rs); legacy `v0.1.0`-style tags are ignored                                                                                                                                                                   |
| (snapshot) rs-modde pins a pre-CLI-restructure simit rev                                                        | `grep -A2 simit rs-modde/flake.nix` showed a pre-restructure rev at snapshot time; `simit --version` from `nix develop` returned the corresponding older version; `simit init ci ...` failed with `unrecognized subcommand 'init'`. Re-check after any flake-input bump. |
| (snapshot) open-data-license has no in-flake simit pin                                                          | `cat open-data-license/flake.nix` showed no `simit` input                                                                                                                                                                                                                |
| (snapshot) skillnet "drift" is just missing flags                                                               | Bare `simit init ci --platform forgejo --check --diff` reported drift; the same with `--with-audit --with-deny --with-docs --with-msrv` returned clean                                                                                                                   |
| (snapshot) rs-modde has both `release.yml` and `release-artifacts-modde-*.yaml`                                 | `ls rs-modde/.forgejo/workflows/`; legacy `release.yml` is ~1k lines and publishes Homebrew + Scoop + Flathub + AUR + COPR + Mastodon + Matrix + WINGET artifacts                                                                                                        |
| (snapshot) sorrel has no CHANGELOG                                                                              | `ls sorrel/CHANGELOG.md` → not found                                                                                                                                                                                                                                     |
| sorrel has 8 publishable crates with cross-crate path dependencies                                              | `ls sorrel/crates/`; cross-crate `path = "..."` dependencies in each `Cargo.toml`                                                                                                                                                                                        |
| Adoption branches share the `simit-ci-adoption-<date>` convention but the convention is not codified in simit   | All five repos used `simit-ci-adoption-20260525` at snapshot time; no simit command creates or recognizes the convention                                                                                                                                                 |

## Existing Plan Status

Source: `/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/`
sub-layer plan set, plus the report itself.

| Item                                         | Status       | Notes                                                                                                                                                                                             |
| -------------------------------------------- | ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| sub-01 `open-data-license` adoption          | partial      | Adoption branch is up to date and 8 commits ahead of `origin/trunk`; not merged. Local `trunk` carries the same lead. Default branch still drifts.                                                |
| sub-02 `rs-memory-admission` adoption        | partial      | Crate `memory-admission 0.1.7` already on crates.io; adoption branch carries cargo-deny / nextest pin commits but flake hook drift remains, and local worktree has uncommitted Pages/docs edits.  |
| sub-03 `rs-modde` adoption                   | blocked      | Pinned simit is 0.9.0; report's validation commands fail before they begin. Workspace-level decisions (legacy `release.yml` vs. per-crate artifacts; Homebrew duplication) still open.            |
| sub-04 `skillnet` adoption                   | partial      | Default branch `main` carries the adoption. `simit projects list` still flags `ci=drift` because of the option-persistence gap, not because of real drift. Flake hook integration remains custom. |
| sub-05 `sorrel` adoption                     | blocked      | Local worktree is dirty; no CHANGELOG; eight first-publish crates; first-time CI adoption scope; dependency ordering not encoded.                                                                 |
| Phase 11 "no `ci(hand-rolled)` remains" goal | not met      | 5/5 repos in `ci(drift)` per `simit projects list`.                                                                                                                                               |
| Phase 8 `managed+extra` recognition          | done in code | `src/registry.rs:541-562` honors the `managed+extra` state; `pages.yaml`/`pages.yml` as supplementary works in principle.                                                                         |

The Phase 11 sub-layers were scoped as "regenerate CI, push, merge."
They did not anticipate (a) the simit pin in `rs-modde` predating the
CLI restructure, (b) the option-persistence gap causing
post-adoption "drift" to recur, or (c) the flake integration cost in
repos that consume `rs-harbor`/shared toolchain flakes.

## Work That Should Survive

### simit (generator/CLI changes)

1. **Persist CI options in `simit.toml`** so that bare
   `simit init ci --check --diff` reproduces the same workflow as the
   last `simit init ci`. New `[ci]` fields: `runtime`, `runner`,
   `windows_runner`, `workspace`, `packages`, `with_msrv`,
   `with_audit`, `with_deny`, `with_docs`, `with_nextest`,
   `with_artifacts`, `with_om_ci`. CLI flags continue to override.
   Reference: [src/config.rs:106](../../src/config.rs#L106),
   [src/commands/init_ci.rs:84](../../src/commands/init_ci.rs#L84).

2. **Unify drift-detection and `--check` rendering** so that
   `simit projects list` and `simit init ci --check --diff` agree on
   what "drift" means. Either route the registry's `infer_ci_options`
   into `--check` as a fallback when flags are absent, or remove
   inference in favor of the persisted config from (1). The current
   double codepath (one inferring from file content, one demanding
   CLI flags) is the root cause of skillnet's spurious
   `ci(drift)` report.
   Reference: [src/registry.rs:754](../../src/registry.rs#L754).

3. **`simit projects show` should print the regeneration command**
   (the same string `render_regeneration_command` builds in
   [src/commands/init_ci.rs:193](../../src/commands/init_ci.rs#L193)),
   based on inferred options. Today the chaperone has to reverse-engineer
   the flag set from the existing files; the report's per-repo
   "Validation command set" is hand-written for this reason.

4. **`simit init flake` needs a "merge" or "managed-section" mode**
   so that projects with custom flake structure (e.g. consuming
   `rs-harbor`, custom outputs, custom devShells) can still benefit
   from generated hook wiring without losing project-owned
   customizations. Today the diff is wholesale-replace, which
   triggers `flake(drift)` permanently on
   [skillnet](file:///data/nvme0/can/Projects/skillnet/flake.nix),
   [rs-modde](file:///data/nvme0/can/Projects/rs-modde/flake.nix),
   and [sorrel](file:///data/nvme0/can/Projects/sorrel/flake.nix).
   Options:
   - Split the generator into "hook file" (`nix/pre-commit.nix`)
     and "flake skeleton" (`flake.nix`); only the hook file needs
     to be canonical.
   - Add a fenced "managed section" marker in `flake.nix` and only
     compare/regenerate inside the fences.
   - Document the supported customizations and add a `[flake]
extra_inputs` config so simit can render them inline.

5. **Generator changes should surface intentional removals.**
   `simit init flake` silently drops `cargo-audit` from pre-commit
   in favor of `cargo-msrv`. Generator should emit a one-line
   migration note in `--check --diff` output naming the removed hook
   and pointing at CI-side replacement. Today the diff is the only
   signal and it reads as accidental churn.
   Reference: [src/render/flake.rs:296-310](../../src/render/flake.rs#L296),
   [src/render/flake.rs:587-610](../../src/render/flake.rs#L587).

6. **`simit release verify` (new command) for the chaperone bar.**
   The report's release bar (worktree clean, simit drift checks,
   changelog alignment, signed-tag presence, publish workflow
   shape, crates.io live state) is currently six separate manual
   invocations. A bundled `simit release verify` that prints a
   structured punch list (one line per check, pass/fail/blocked,
   producer + regeneration command) would compress the chaperone's
   evidence-gathering phase and let `rust-crate-release-chaperone`
   take a single action on a single answer.

7. **`simit release plan` for workspaces** that prints the
   dependency-ordered publish order (`sorrel-io` → `sorrel-data`
   → ... → `sorrel`) and runs `cargo package -p <pkg>
--allow-dirty` in that order. Today the report lists each
   `cargo package` call manually; the chaperone has to discover
   the order from `Cargo.toml`s.

8. **Tag-convention helper.** open-data-license has both `0.2.0`
   (matches generated `*.*.*` trigger) and `v0.1.0` (does not).
   simit has no command to audit or repair this. Suggest a
   `simit release tags audit` that lists tags not matching the
   generated publish workflow trigger and offers to mirror them
   under the supported convention. Reference: the publish workflow
   trigger pattern is `*.*.*` per
   [src/render/ci.rs](../../src/render/ci.rs) (search
   `"      - \"*.*.*\""`).

9. **Detect conflicting publish endpoints.** rs-modde's
   `release.yml` and `release-artifacts-modde-*.yaml` both publish
   to the Homebrew tap. simit could detect overlapping
   `Publish Homebrew tap` steps across all workflow files (both
   marked and unmarked) and warn during `simit projects show` /
   `simit init ci --check`.

10. **Codeberg/Forgejo secret pre-flight (optional).** Every
    publish workflow encodes `# Project-required secrets:` in a
    header (see
    [src/render/ci.rs:1187-1197](../../src/render/ci.rs#L1187)).
    Verifying that those secrets actually exist on the remote is
    not yet covered by simit or `berg` automation; today the
    chaperone confirms manually. A `simit release secrets` command
    that hits the Forgejo API (directly or via `berg`) and confirms
    presence — without fetching values — would convert that manual
    step into a concrete chaperone gate.

11. **Bumping the simit flake input is currently the maintainer's
    job.** Every adopting repo should track the latest simit commit
    from `trunk`; today this is enforced by convention, not by
    tooling. Suggest `simit projects upgrade` or
    `simit init flake --bump-simit-input` that detects an outdated
    `simit` flake input and rewrites the rev/url to current `trunk`.
    Repos that don't pin simit in their flake at all (so they pick
    up whatever is in PATH) should be offered a one-shot
    `simit init flake --add-simit-input` to make the dependency
    explicit and deterministic.

12. **Adoption-branch convention.** All five repos use
    `simit-ci-adoption-20260525`. Suggest `simit init ci
--adopt-branch` that creates the branch (with today's date),
    generates, commits, and reports the suggested PR title/body.
    Currently the convention exists only as discipline.

13. **`simit projects list` should not list ephemeral `/tmp/*`
    test scratch entries** in the main table by default. Today they
    appear (e.g.
    `/tmp/simit-ci-retry/sorrel`,
    `/tmp/nix-shell.*/...`), and only the attention footer filters
    them. Add a default-on filter mirroring
    `is_ephemeral_project_path` to the listing itself, with
    `--include-ephemeral` to restore the old behavior.

### Per-repo cleanups that fall out

These are concrete fixes that survive the simit improvements above and
do not depend on them:

- `rs-modde`: bump the `simit` flake input to current `trunk`
  before any chaperone validation. Without this, every
  `simit init ...` invocation through `nix develop` fails
  immediately on the CLI restructure.
- `rs-modde`: decide release-endpoint architecture explicitly —
  either retire `release.yml` in favor of simit's per-crate
  `release-artifacts-*.yaml` (and disable duplicate Homebrew
  publish), or keep `release.yml` and gate simit's artifacts off
  via `--with-artifacts` absence. The duplicate Homebrew write
  risk is real because both workflows trigger on the same tag.
- `rs-modde`: prune the `worktree-agent-*` branches before the
  chaperone runs — they confuse `simit projects scan` and
  inflate the registry attention footer.
- `sorrel`: commit or stash the local worktree; introduce a
  `CHANGELOG.md` (or document the equivalent per-crate changelogs)
  before any release; declare the publish dependency order
  somewhere repo-owned (top-level README or `RELEASE.md`).
- `open-data-license`: merge the adoption branch into `trunk`;
  audit and remove the `v0.1.0` legacy tag if no longer needed.
  **User decision required** for the existing `0.2.0` remote tag
  (crates.io is empty for that version): if the tag is considered
  authoritative for what was meant to ship at `0.2.0`, use
  `simit release sync-up` for a no-meaningful-change repair path;
  if the tag is stale or the in-tree commits add user-visible
  change, bump to `0.2.1`. Neither path is forced by repo
  evidence alone.
- `rs-memory-admission`: commit `.forgejo/workflows/pages.yaml`
  and `docs/src/development/nix.md` (or revert) before adoption;
  decide explicitly whether `cargo-audit` should remain a
  pre-commit hook (and override the generator) or move to CI only.
- `skillnet`: the spurious `ci(drift)` will clear automatically
  if simit (1) lands. In the meantime, document the full flag set
  in a `simit.toml` skeleton so contributors can re-run
  `simit init ci --check --diff` without flag archaeology.

## Blockers And Missing Artifacts

| Blocker                                                                                | Producer                                               | Regeneration command                                                                                                                                                                                                         | Validation                                                                                                                                  |
| -------------------------------------------------------------------------------------- | ------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| Adopting repos must track latest simit `trunk`; pre-restructure pins break the new CLI | repo maintainer                                        | Bump the `simit` flake input to current `trunk` (or add it if absent); `nix flake update simit`                                                                                                                              | `nix develop -c simit init --help` lists `ci` and `flake` as subcommands                                                                    |
| `simit init ci --check` does not infer options from existing files                     | simit maintainer                                       | Implement simit improvement (1) or (2) above                                                                                                                                                                                 | `simit init ci --platform forgejo --check --diff` (no flags) is clean on a repo whose workflows were generated with extra flags             |
| `simit init flake` cannot preserve custom flake structure                              | simit maintainer                                       | Implement simit improvement (4). **Verify before scoping for sorrel:** re-run `simit init flake --check --diff` on a clean sorrel worktree to confirm the wholesale-rewrite claim holds there, not just on skillnet/rs-modde | `simit init flake --check --diff` is clean on `skillnet`, `rs-modde`, and (post-verification) `sorrel` without losing project-owned outputs |
| Codeberg/Forgejo secrets not yet covered by simit/berg automation                      | repo owner today; simit improvement (10) later         | Manual Forgejo settings inspection now; future `simit release secrets --check` once shipped                                                                                                                                  | Future `simit release secrets --check` succeeds, or maintainer confirms via Codeberg UI                                                     |
| sorrel has no CHANGELOG                                                                | sorrel maintainer                                      | Add `CHANGELOG.md` with the target version entry, or document per-crate equivalents                                                                                                                                          | `simit changelog show <version>` returns the entry                                                                                          |
| sorrel local worktree dirty with mixed concerns                                        | sorrel maintainer                                      | `git status` triage; commit or stash                                                                                                                                                                                         | `git status --short` empty                                                                                                                  |
| rs-modde release-endpoint policy undecided                                             | rs-modde maintainer                                    | Architecture decision — see per-repo cleanups                                                                                                                                                                                | Single workflow path publishes each artifact type                                                                                           |
| Workspace publish ordering not encoded                                                 | sorrel + rs-modde maintainers (or simit improvement 7) | Document order in repo, or wait for `simit release plan`                                                                                                                                                                     | `cargo publish --dry-run -p <each in order>` all succeed                                                                                    |

## Risks And Constraints

- **CLI-restructure pain repeats elsewhere.** rs-modde is the loudest
  current case, but any repo pinning a pre-restructure simit rev
  will hit the same `unrecognized subcommand 'init'` failure. The
  generator changes proposed above (1, 2, 4) raise the cost of
  running stale simit even higher. The durable rule is "every
  adopting repo tracks current simit `trunk`"; the supporting
  tooling change is improvement (11). In the interim, document a
  one-shot `nix flake update simit` as a pre-adoption step.

- **Wholesale flake replacement is destructive.** Improvement (4) is
  the highest-risk change because it touches the public flake-input
  surface of consumer projects. A staged rollout — start with
  hook-file-only canonicalization (lowest blast radius), then add
  managed sections — is safer than a single big change.

- **Adoption branches as moving targets.** All five repos share the
  `simit-ci-adoption-20260525` branch name, but the branch _content_
  diverges with each simit-generator commit. The branch is not
  reproducible — re-running `simit init ci` today on the same source
  HEAD would emit different content than it did on 2026-05-23. A
  branch-name convention without content reproducibility is fragile.
  Improvement (12) should pin the simit revision at the moment of
  branch creation.

- **`managed+extra` honors non-marked workflows of any shape.** This
  was intentional (Phase 8), but the same liberality means a stale
  `release.yml` next to `publish-crate-*.yaml` (rs-modde) is not
  flagged as a hazard. Improvement (9) needs to surface only
  _conflicting_ endpoints, not all extra files.

- **Publishing eight first-time crates simultaneously (sorrel).**
  crates.io rejects publish for a crate whose path dependencies are
  not yet live. The tag-triggered concurrent publish workflows will
  race. Either improvement (7) plus serialized triggers, or a
  documented manual ordering, is required before the first sorrel
  release.

- **`simit release sync-up` requires the local tag to already point
  somewhere meaningful.** For `open-data-license`, the remote tag
  `0.2.0` exists but crates.io is empty. sync-up will retarget the
  tag but does not validate that the destination commit is
  publishable. Improvement (6) (`simit release verify`) is the
  cleanest fix.

## Candidate Next Steps

Sequencing suggestion. Each step is a discrete simit-side or repo-side
change; they can run in parallel across the two domains.

### simit (parallel-safe)

1. **Persist CI options in `simit.toml`** (improvement 1). Smallest
   viable cut: serialize `runtime`, `runner`, `with_audit`,
   `with_deny`, `with_docs`, `with_msrv`, `with_nextest`, `workspace`.
   Migrate `infer_ci_options` to read this config first, fall back to
   inference, and have `simit init ci` write the resolved options
   back. Unblocks improvements (2), (3).
2. **`simit init ci --check --diff` falls back to inference**
   (improvement 2) when no flags + no `simit.toml` are present. Or
   simply re-render using the resolved config from (1). Eliminates
   skillnet-style spurious drift across the fleet.
3. **`simit projects show` prints the regeneration command**
   (improvement 3). Trivial after (1).
4. **`simit init flake` adds a `hooks-only` mode** (the smallest
   slice of improvement 4). Stops generating `flake.nix` and limits
   the canonical scope to `nix/pre-commit.nix` and adjacent
   simit-owned files. Unblocks skillnet, rs-modde, sorrel.
5. **`simit init flake --check --diff` surfaces a removal note**
   (improvement 5) listing hooks being deleted. One-line, no diff
   reformat required.
6. **`simit release verify`** (improvement 6) as a thin wrapper over
   existing checks. Bundles `git status`, `simit init ci --check`,
   `simit init flake --check`, `simit release trust check`, changelog
   alignment check, crates.io reachability check.
7. **`simit release plan` for workspaces** (improvement 7). Reads
   the publishable subgraph from `cargo metadata`, topologically
   orders it, runs `cargo package --dry-run` per crate.

Improvements (8)–(13) can land later; each is independently scoped.

### Repository-side (parallel-safe, repo-local)

1. **`rs-modde`:** bump pinned simit input to current `trunk`. PR
   must include the regenerated CI to keep `simit projects show`
   clean.
2. **`open-data-license`:** ff-merge the adoption branch into
   `trunk`, drop legacy `v0.1.0` tag if abandoned. **User decision**
   on the `0.2.0` tag (see per-repo cleanups): if authoritative,
   run chaperone in sync-up mode; if the in-tree commits change
   user-visible behavior, bump.
3. **`rs-memory-admission`:** commit or revert the dirty worktree,
   merge adoption branch, decide on `cargo-audit` pre-commit hook.
4. **`skillnet`:** wait for simit improvement (1) so `ci(drift)`
   clears; otherwise add a `simit.toml` skeleton manually.
5. **`sorrel`:** triage the dirty worktree, introduce a CHANGELOG,
   write down the publish order, defer first publish until simit
   improvement (7) or a documented manual sequencing.
6. **`rs-modde`:** post-pin-bump, decide release-endpoint policy
   (legacy `release.yml` vs. per-crate artifacts) and prune
   `worktree-agent-*` branches.

Repo-side step 1 (rs-modde simit bump) is the highest-leverage
single fix: it unblocks the report's entire validation command set
for that repo in a single PR.

simit-side steps 1 and 2 are the highest-leverage simit fixes
because they collectively turn the report's per-repo flag archaeology
into a no-op for the next round.

## Open Decisions For The User

These decisions are not derivable from repo state alone; they will
shape which simit improvements ship first.

1. **Should `simit init flake` continue to own the entire `flake.nix`,
   or downscope to hooks-only by default?** Hooks-only is much less
   invasive; full ownership keeps the canonical Rust crate flake
   value proposition. Pick one before improvement (4) lands —
   they are not compatible.

2. **Should `simit release verify` (improvement 6) include a remote
   secrets check?** Doing so requires a Forgejo API token in the
   chaperone's environment and changes the threat model for the
   verify command. Alternative: keep secrets out of `simit release
verify`, ship them as a separate optional `simit release secrets`
   (improvement 10).

3. **Workspace publish ordering: simit-enforced or repo-declared?**
   Improvement (7) can compute it from `cargo metadata`, but the
   author may want manual ordering for non-dependency reasons
   (release notes, smoke-test gating). Decide whether
   `simit release plan` is authoritative or advisory.

4. **`rs-modde` release-endpoint architecture.** This is a
   project-level decision (single `release.yml` vs. per-crate
   `release-artifacts-*.yaml`) that simit improvements cannot make
   for the maintainer. Resolve before the next chaperone attempt on
   that repo.

5. **Cargo-audit pre-commit hook: keep or drop?** Generator currently
   drops it for `cargo-msrv` in pre-commit. If audit should stay
   client-side too, that needs to be an opt-in flag rather than the
   silent removal the diff currently shows.

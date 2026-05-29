# Phase 07 — Fleet repo cleanups (multi-sub-layer)

> **Recommended Codex model for phase-level merge: GPT 5.4 / medium**
>
> Phase merge is just verifying each sub-layer's PR landed and that
> `simit release verify` (or the workaround flag-set check) returns
> clean for each repo. Routing per repo happens in each sub-layer
> file. Sub-agent role on moderate coordination — 5.4 at `medium`.

## Sub-layers

| #   | Slug                | Model            | Touches                                         | Sub-layer file                                                   |
| --- | ------------------- | ---------------- | ----------------------------------------------- | ---------------------------------------------------------------- |
| 01  | open-data-license   | GPT 5.4 / medium | `/data/nvme0/can/Projects/open-data-license/`   | [sub-01-open-data-license.md](./sub-01-open-data-license.md)     |
| 02  | rs-memory-admission | GPT 5.4 / medium | `/data/nvme0/can/Projects/rs-memory-admission/` | [sub-02-rs-memory-admission.md](./sub-02-rs-memory-admission.md) |
| 03  | rs-modde            | GPT 5.5 / high   | `/data/nvme0/can/Projects/rs-modde/`            | [sub-03-rs-modde.md](./sub-03-rs-modde.md)                       |
| 04  | skillnet            | GPT 5.4 / medium | `/data/nvme0/can/Projects/skillnet/`            | [sub-04-skillnet.md](./sub-04-skillnet.md)                       |
| 05  | sorrel              | GPT 5.5 / medium | `/data/nvme0/can/Projects/sorrel/`              | [sub-05-sorrel.md](./sub-05-sorrel.md)                           |

Sub-03 (`rs-modde`) gets the high tier because it carries the most
open decisions (legacy `release.yml` vs. simit-generated artifacts,
Homebrew duplication, multi-crate publish order, branch hygiene).
Sub-05 (`sorrel`) gets the medium tier (one step up from the
other repos) because it bundles worktree triage with a first-time
multi-crate publish prep and the flake-drift claim still needs
re-verification.

## Goal (phase-level)

Each fleet repo is in a state where
`rust-crate-release-chaperone` can be invoked without per-repo
manual pre-triage. Concretely:

- `simit` flake input tracks current `trunk` (or simit is added as
  an explicit flake input where it was implicit).
- adoption branch (where it exists) is merged into the default
  branch, or its commits are otherwise on the default branch.
- local worktree is clean; uncommitted release-relevant edits are
  either committed or stashed with a note.
- a `CHANGELOG.md` (or documented per-crate equivalent) exists.
- any repo-local architecture decisions (release-endpoint policy
  for rs-modde, tag-vs-bump for open-data-license, audit pre-commit
  policy for rs-memory-admission, publish order for sorrel) are
  recorded in the repo, not in the agent's head.

## Why this matters now

The Phase 11 sub-layer set in
`/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/`
was scoped as "regenerate CI, push, merge." It did not anticipate
the simit pin pre-dating the CLI restructure, the option-persistence
gap causing post-adoption "drift" to recur, or the flake integration
cost in repos that consume shared toolchain flakes. This phase
finishes the adoption from the repo side, with workarounds in place
for any Track-A simit work that has not landed yet.

## Out of scope (phase-level)

- Actually running `rust-crate-release-chaperone` to publish a new
  version. Each sub-layer leaves the repo _ready_; the chaperone
  invocation is the user's call.
- Bulk-editing across repos. Each sub-layer is repo-local; do not
  share branches or commits across sub-layers.
- Making the open architectural decisions for the maintainer (the
  sub-layer reports them; the user resolves).

## Merge plan

Each sub-layer ships as a PR (or merge) on its respective repo's
default branch. The phase merges when all five sub-layers report
"chaperone-ready" and `simit projects list` on the fleet machine
shows no `ci=drift` (or `flake=drift`) for any of the five repos.

If a sub-layer cannot reach chaperone-ready because the maintainer
has not made the required architectural decision, the sub-layer
reports `blocked` with the decision flagged. The phase still
merges; the decision becomes a separate user-owned follow-up.

## Phase-level acceptance criteria

- [ ] `nix develop -c simit init --help` works in every fleet repo
      (proves the simit input is current).
- [ ] `nix develop -c simit init ci --platform forgejo --check
    --diff` is clean in every fleet repo. If phase 01 has not
      landed yet, sub-layers may add a per-repo `simit.toml [ci]`
      manually as a forward-compatible workaround.
- [ ] `nix develop -c simit init flake --check --diff` is clean in
      every fleet repo. If phase 03 has not landed, sub-layers may
      opt out of full-scope ownership via repo-local
      `simit.toml [flake].scope = "hooks-only"` (forward-compatible
      with the phase 03 schema).
- [ ] `git status --short` is empty in every fleet repo (or the
      remaining dirty files are explicitly authorized).
- [ ] Each repo's default branch is at or beyond its adoption
      branch.
- [ ] `simit release verify` (if phase 05 has landed) returns 0 or
      2 (no real failures; remote-secret blocked is acceptable) for
      every fleet repo. Otherwise the equivalent manual checks pass.
- [ ] Each sub-layer's individual acceptance criteria are met.

## Pitfalls

- **Do not run sub-layers from the simit working tree.** Each
  sub-layer's working tree is its own repo. The simit working tree
  is not touched in phase 07.
- **Do not bulk-merge adoption branches without re-verifying the
  generated CI is clean.** Some adoption branches are weeks old at
  the snapshot time; the generator has changed since. After
  merging, re-run `simit init ci --check --diff` and regenerate if
  needed.
- **Do not delete user-owned files mistaken for stale.** rs-modde
  in particular has a legacy `release.yml` that publishes binary
  artifacts; that is not stale generator output and must not be
  removed without the maintainer's explicit go.

## Reference

- Research dossier: per-repo cleanups section.
- Original readiness report:
  `/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/release-chaperone-gap-report.md`
- Predecessor sub-layer set:
  `/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/`
- Chaperone skill:
  `/home/can/.claude/skills/rust-crate-release-chaperone/SKILL.md`

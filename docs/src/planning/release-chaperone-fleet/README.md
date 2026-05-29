# Release Chaperone Fleet — Plan Set

> **Recommended Codex model for plan-set orchestration / merge: GPT 5.5 high**
>
> Coordinating two parallel work tracks (simit-side generator changes
> and per-repo cleanups), with sequencing constraints between
> foundational simit changes and the cleanups that depend on them.
> Top-level planner role on moderately complex, multi-repo work; 5.5
> at `high` matches the routing matrix for this coordinates.

## Scope

Convert the findings in
[release-chaperone-fleet-research.md](../release-chaperone-fleet-research.md)
into a phase doc set that gets the five fleet repos
(`open-data-license`, `rs-memory-admission`, `rs-modde`, `skillnet`,
`sorrel`) into a state where `rust-crate-release-chaperone` can run
end-to-end without per-repo flag archaeology.

Two work tracks:

- **Track A — simit generator / CLI changes** (phases 01–06):
  remove the structural reasons the chaperone keeps tripping over the
  same triage. Highest leverage is improvements 1, 2, 4 from the
  research dossier; these unblock the rest.
- **Track B — Per-repo cleanups** (phase 07): bump simit input to
  current `trunk`, merge adoption branches, decide repo-local policy
  (release endpoints, tag policy, audit hook), and prep release
  hygiene (worktree, changelog, publish ordering) so each repo can
  be handed to the chaperone.

Out of scope for this plan set:

- Running `rust-crate-release-chaperone` itself. That is a separate,
  per-repo invocation the user owns. This plan delivers
  _chaperone-readiness_, not releases.
- Optional research-dossier improvements 8–10, 12 (tag-convention
  helper, conflicting-endpoint detector, secrets pre-flight,
  adoption-branch tooling). They are deliberately deferred — none
  of the five fleet repos block on them.
- Any change that requires the user to make the open architectural
  decisions enumerated in the dossier's _Open Decisions_ section.
  Where a decision is needed, the relevant phase pauses and reports
  it; this plan does not make those calls.

## Phases

| #   | Slug                          | Layout | Sub-layers | Model                    | Blocking?                                                                     |
| --- | ----------------------------- | ------ | ---------: | ------------------------ | ----------------------------------------------------------------------------- |
| 01  | persist-ci-options            | flat   |          — | 5.4 / medium             | foundational; blocks 02, 04 sub-01                                            |
| 02  | unify-drift-and-check         | flat   |          — | 5.5 / high               | depends on 01                                                                 |
| 03  | init-flake-hooks-only-mode    | flat   |          — | 5.5 / high               | independent; high blast radius                                                |
| 04  | generator-ux-polish           | dir    |          3 | mixed (see phase README) | sub-01 depends on 01                                                          |
| 05  | simit-release-verify          | flat   |          — | 5.5 / medium             | independent; pulls in existing checks                                         |
| 06  | simit-release-plan-workspaces | flat   |          — | 5.4 / medium             | independent                                                                   |
| 07  | fleet-repo-cleanups           | dir    |          5 | mixed (see phase README) | each sub-layer benefits from 01/02 landing but does not strictly require them |

[Phase 01 — Persist CI options](./01-persist-ci-options.md)
[Phase 02 — Unify drift detection and --check rendering](./02-unify-drift-and-check.md)
[Phase 03 — `simit init flake` hooks-only mode](./03-init-flake-hooks-only-mode.md)
[Phase 04 — Generator UX polish (multi-sub-layer)](./04-generator-ux-polish/README.md)
[Phase 05 — `simit release verify` (chaperone bar bundler)](./05-simit-release-verify.md)
[Phase 06 — `simit release plan` for workspaces](./06-simit-release-plan-workspaces.md)
[Phase 07 — Fleet repo cleanups (multi-sub-layer)](./07-fleet-repo-cleanups/README.md)

## Parallelism Layer

Execution waves, from start to plan exhaustion:

**Wave 1 — foundational and independent simit changes (parallel).**

- 01 (`persist-ci-options`) — sequential on simit `trunk`, but does
  not touch the same files as 03 or 05.
- 03 (`init-flake-hooks-only-mode`) — touches `src/render/flake.rs`
  and `src/commands/init_flake.rs`; disjoint from 01/05.
- 05 (`simit-release-verify`) — adds a new command under
  `src/commands/release.rs` and a new `Verify` action; disjoint
  from 01/03.
- 06 (`simit-release-plan-workspaces`) — also under
  `src/commands/release.rs`; coordinate ordering with 05 to avoid
  edit conflicts on `release.rs`, but the implementations are
  disjoint.

Wave 1 may go five-wide if the user is comfortable resolving small
merge conflicts in `src/commands/release.rs` between 05 and 06. The
safe sequencing is 05 → 06 sequentially within their slot, with 01
and 03 running in parallel beside them.

**Wave 2 — derived simit changes (parallel after Wave 1 lands).**

- 02 (`unify-drift-and-check`) — depends on 01 because it expects
  the `simit.toml` schema.
- 04 (`generator-ux-polish`) sub-01 — depends on 01 for the same
  reason. Sub-02 and sub-03 are independent and could ride Wave 1.

**Wave 3 — fleet repo cleanups (parallel by repo).**

- 07 sub-01..05 — one fresh agent session per repo. Disjoint
  working trees, no shared state. Order does not matter; retries
  are repo-local. Benefits from 01/02 having landed (clearer
  `--check --diff` story), but a session can adopt the workaround
  (pass full flag set explicitly) and proceed without them.

**Serialization points.**

- The boundary between Wave 1 and Wave 2 is a real gate: do not
  start phase 02 until phase 01 has merged to `trunk`.
- Within phase 04, sub-01 must wait for 01.
- Each phase 07 sub-layer must run inside its own repo's working
  tree (the sub-layer README has the absolute paths).

## Whole-set acceptance criteria

- [ ] `simit projects list` on the fleet machine reports no `ci=drift`
      for the five fleet repos under the option-persistence flow.
- [ ] `nix develop -c simit init ci --platform forgejo --check --diff`
      (bare, no extra flags) is clean for each of the five repos.
- [ ] `nix develop -c simit init flake --check --diff` is clean for
      each of the five repos, with no destructive rewrite of
      project-owned flake content.
- [ ] `nix develop -c simit --version` in each fleet repo reports a
      version matching current simit `trunk` (exact rev not
      important; CLI surface must include `init ci` and `init flake`
      nested subcommands).
- [ ] `simit release verify` (new command from phase 05) runs to
      completion on each fleet repo and prints a structured
      pass/fail/blocked report — even where the result is "blocked".
- [ ] Phase 07 sub-layers each leave their repo in a state where
      `rust-crate-release-chaperone` can be invoked without manual
      pre-triage; remaining open decisions are flagged in the
      sub-layer report, not silently deferred.

## Global constraints (apply to every phase)

- **Track simit `trunk`, not specific revs.** Whenever a repo needs
  its `simit` flake input bumped, target current simit `trunk` at
  the time of execution. Do not pin to a specific rev "for
  reproducibility"; the durable contract is "latest `trunk`".
- **No `--no-verify`, no `--no-gpg-sign`.** Hooks and signing are
  part of the chaperone bar; respect them everywhere.
- **No destructive operations on shared state without explicit
  confirmation in the phase.** Branch deletions, tag deletions, and
  force-pushes only when the phase explicitly authorizes them and
  the user confirms.
- **Snapshot-sensitive claims in the research dossier are evidence,
  not durable conclusions.** Phases that act on per-repo state must
  re-verify the relevant snapshot themselves before acting (each
  phase's _Plan_ spells out the re-verification).
- **Phase 07 sub-layers must not gate on simit-side improvements
  they do not strictly need.** If a sub-layer can run today with a
  documented workaround (e.g. passing the full `--with-*` flag set),
  it should — do not wait for phase 01 unless the repo cleanup
  literally cannot proceed without it.

## Reference

- Research dossier:
  [release-chaperone-fleet-research.md](../release-chaperone-fleet-research.md)
- Upstream readiness report:
  `/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/release-chaperone-gap-report.md`
- Phase 11 sub-layer set (predecessor):
  `/home/can/.claude/plans/detritus-rust-cache/11-fleet-adoption/`
- Chaperone skill:
  `/home/can/.claude/skills/rust-crate-release-chaperone/SKILL.md`

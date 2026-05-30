# Plan: multichannel packaging release

> **Recommended Codex model for orchestrating this plan-set: GPT 5.5 high**
>
> Cross-repo release coordination (simit → crates.io/codeberg, nixpkgs fork,
> canix runner secrets, rs-modde adoption, live CI release) with sequencing
> constraints and one frontier-risk terminal phase. An orchestrator that loses
> the dependency order or the "local-verify before CI" discipline ships a broken
> release pipeline. Holding the whole graph warrants the high tier; individual
> phases route cheaper.

## Scope and current state

simit gained generic generators for **all** of rs-modde's distribution channels
— `simit init aur|copr|apt`, `simit dist aur|copr|apt render`, and `simit init
release` (one comprehensive `.forgejo/workflows/release.yml` covering Codeberg
release upload + apt + aur + copr + homebrew + scoop + chocolatey + flathub +
winget + announce, with minisign + cosign SLSA signing). Config lives in
`simit/src/config.rs`; renderers in `simit/src/render/{pkgbuild,rpm_spec,copr_makefile,apt_conf,release_workflow}.rs`;
commands in `simit/src/commands/{aur,copr,apt,init_aur,init_copr,init_apt,init_release}.rs`.
Most channel secret names are configurable; chocolatey's api-key wiring and
`nix_tool` are configurable. rs-modde's `flake.nix` `simitConfig` is populated
for every channel, its dist artifacts are regenerated, and `release.yml` is
simit-generated (overlapping `release-artifacts-*.yaml` deleted).

**All of this is uncommitted in working trees.** This plan commits it, closes the
last configurability gap (homebrew/scoop secret names), releases simit, wires the
runner secrets, adopts the released simit in rs-modde, and validates end-to-end
on CI before cutting a real release.

Current versions: simit `0.15.4` (Cargo.toml); rs-modde pins simit at the
`0.15.3` flake-input rev — a deliberate skew this plan resolves in Phase 06.

## Phase table

| Phase | File                                                                         | Repo         | Depends on           | Touches                                                                | Can parallel with | Blocking?            |
| ----- | ---------------------------------------------------------------------------- | ------------ | -------------------- | ---------------------------------------------------------------------- | ----------------- | -------------------- |
| 01    | [01-homebrew-scoop-secret-config.md](./01-homebrew-scoop-secret-config.md)   | simit        | —                    | `simit/src/{config.rs,render/release_workflow.rs,render/ci.rs}`, tests | 04, 05            | blocks 02            |
| 02    | [02-simit-release-readiness.md](./02-simit-release-readiness.md)             | simit        | 01                   | `simit/{tests,README.md,docs,CHANGELOG.md}`                            | 04, 05            | blocks 03            |
| 03    | [03-release-simit.md](./03-release-simit.md)                                 | simit        | 02                   | `simit` Cargo.toml/CHANGELOG + tag                                     | 04, 05            | blocks 06            |
| 04    | [04-upstream-nixpkgs-choco-scoop.md](./04-upstream-nixpkgs-choco-scoop.md)   | nixpkgs fork | —                    | `pkgs/by-name/{ch/chocolatey,sc/scoop}`                                | all               | optional; relaxes 07 |
| 05    | [05-canix-expose-runner-secrets.md](./05-canix-expose-runner-secrets.md)     | canix        | —                    | `canix/root/hosts/atlas/server/forgejo-runners.nix` + secret modules   | 01–04, 06         | blocks 07            |
| 06    | [06-rs-modde-adopt-simit-release.md](./06-rs-modde-adopt-simit-release.md)   | rs-modde     | 03                   | `rs-modde/flake.{nix,lock}`, `.forgejo/workflows/*`, dist artifacts    | 04, 05            | blocks 07            |
| 07    | [07-rs-modde-validate-and-release.md](./07-rs-modde-validate-and-release.md) | rs-modde     | 05, 06 (04 optional) | tags on rs-modde                                                       | —                 | terminal             |

## Parallelism layer

- **Wave 0 (start now, three repos, fully parallel):** Phase 01 (simit code),
  Phase 04 (nixpkgs upstream PR — optional, long review latency, start early),
  Phase 05 (canix runner secrets). Disjoint repos, no shared files.
- **Wave 1:** Phase 02 (simit tests/docs) — after 01; same repo, overlapping
  `simit/tests/` and `src/`, so it must serialize after 01.
- **Wave 2:** Phase 03 (release simit) — after 02. Publishes to crates.io
  (irreversible) and Codeberg.
- **Wave 3:** Phase 06 (rs-modde adopts the released simit) — after 03, because
  it bumps the `simit` flake input to the new version.
- **Wave 4 (plan exhaustion):** Phase 07 (rs-modde live-CI validation + real
  release) — after 06 (rs-modde on new simit) and 05 (secrets reachable by
  jobs). Uses the nixpkgs fork ref for chocolatey unless Phase 04 has merged, in
  which case chocolatey's `nix_tool` simplifies to `nixpkgs#chocolatey`.

Serialization points: 01→02→03 share the simit repo/files; 03→06 is a
version-dependency; 07 is gated on both 05 and 06 green.

## Whole-set acceptance criteria

- [ ] simit: every distribution channel's secret/env names are configurable
      (no hardcoded `homebrew_tap_token` / `SCOOP_BUCKET_TOKEN` remain in
      `render/release_workflow.rs`); `cargo test` + `cargo clippy --all-targets
    -- -D warnings` clean.
- [ ] simit: a new release is published to crates.io and Codeberg via simit's own
      tooling, with the new `init`/`dist`/`init release` commands documented.
- [ ] canix: every secret the rs-modde release workflow references is reachable
      by the atlas Forgejo job (as an Actions secret or a runner-provided env),
      or explicitly deferred with a soft-skip confirmed.
- [ ] rs-modde: `simit` flake input points at the new release; `simit init
    release --check` and `simit init ci --check` are clean; all working-tree
      changes committed in coherent groups.
- [ ] rs-modde: a throwaway prerelease tag drives a green Codeberg release run
      where every configured channel either publishes or soft-skips cleanly;
      then a real release is cut and the artifacts/release verified.

## Global constraints

- This maintainer **releases via `simit`** (`simit commit` / `simit release` /
  `simit changelog`) — do not hand-roll cargo+git+sed.
- CI runs on the **self-hosted `atlas` Forgejo runner** against **Codeberg**.
- **Local verification before CI**: every simit change is `cargo
build/test/clippy` green and diffed against rs-modde's committed artifacts
  before anything is tagged. Live CI is the last resort, used only in Phase 07.
- Nothing is force-pushed; releases are tag-driven; prereleases (`-rc.N`) are
  used for validation so real version numbers aren't burned.

## Reference

- Originating work + design rationale: `~/.claude/plans/dreamy-hatching-charm.md`
  (this session's implementation plan) and the project memory
  `project_simit_multichannel_packaging`.
- simit release tooling: `simit release --help`, `simit changelog --help`.
- canix runner secrets: `canix/root/modules/server/forgejo-runner-secrets/`.

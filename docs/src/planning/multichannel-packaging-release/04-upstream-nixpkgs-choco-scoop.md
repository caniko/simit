# Phase 04 — Upstream chocolatey + scoop to nixpkgs (optional)

> **Recommended Codex model: GPT 5.5 high**
>
> nixpkgs contribution is a complex, review-latency-bound workflow: live PR
> template, package-quality conventions, `nixpkgs-review`, maintainer back-and-
> forth. The package code largely exists on the fork; the difficulty is the
> process and getting the packages to upstream-acceptable quality. Complex ×
> orchestrator → `high`. Optional and parallel, so it never blocks the release.

## Working tree

`/data/nvme0/can/Projects/nixpkgs-add-chocolatey-scoop` (fork of
`NixOS/nixpkgs`, branch `add-chocolatey-scoop`, origin `caniko/nixpkgs`). Fully
independent repo — runs in parallel with all other phases.

## Goal

`chocolatey` (and ideally `scoop`) are submitted as upstream nixpkgs PRs and, on
merge, become available as `nixpkgs#chocolatey` / `nixpkgs#scoop`, so rs-modde's
`[chocolatey].nix_tool` can drop the fork ref and use plain `nixpkgs#chocolatey`.

## Why this matters now

It's the only thing standing between "chocolatey publishes from a fork ref" and
"chocolatey publishes from stock nixpkgs." It is **optional for the release** —
Phase 07 works today via the fork ref — but it removes a long-lived fork
dependency. Start early because nixpkgs review latency is measured in weeks.

## Out of scope

- Do **not** block Phases 01–03, 05–07 on this. If unmerged at Phase 07,
  rs-modde keeps the fork `nix_tool` ref.
- Do **not** bundle unrelated package changes into the PR(s).

## Plan

1. Use the `nixpkgs-init-pr` skill (and `nixpkgs-pr-common`) for decorum, the
   live PR template, and the `caniko/nixpkgs-review-gha` review flow.
2. Bring `pkgs/by-name/ch/chocolatey/package.nix` to upstream quality: meta
   (license, maintainers, platforms, mainProgram), passthru tests, structured
   attrs already present — verify against nixpkgs conventions. Do the same for
   `pkgs/by-name/sc/scoop/` (confirm/complete the package; the dir existed but
   may be incomplete).
3. `nix-build`/`nixpkgs-review` the packages; ensure `choco --version` /
   `scoop` smoke tests pass.
4. Open one PR per package (or a single PR if conventions allow), fill the live
   template, and shepherd review.
5. On merge: open a follow-up note for Phase 07 / rs-modde to flip
   `[chocolatey].nix_tool` to `nixpkgs#chocolatey` once a nixpkgs bump includes
   the merge.

## Acceptance criteria

- [ ] A nixpkgs PR exists for `chocolatey` (and `scoop` if pursued) using the
      live template, passing CI / `nixpkgs-review`.
- [ ] `nix build <fork-ref>#chocolatey` and `#scoop` succeed and their smoke
      tests pass locally.
- [ ] On merge, a documented one-line change for rs-modde to simplify
      `[chocolatey].nix_tool` is recorded (in this file or the rs-modde config
      comment).

## Files likely touched

- `/data/nvme0/can/Projects/nixpkgs-add-chocolatey-scoop/pkgs/by-name/ch/chocolatey/package.nix`
- `/data/nvme0/can/Projects/nixpkgs-add-chocolatey-scoop/pkgs/by-name/sc/scoop/package.nix`

## Pitfalls

- **scoop dir may be incomplete.** Earlier inspection showed
  `pkgs/by-name/sc/scoop/` without a confirmed `package.nix`. Symptom: eval
  failure. Recovery: complete the package before PRing, or scope the PR to
  chocolatey only and keep scoop on the fork (rs-modde's scoop step needs no
  nixpkgs tool anyway — it's git-clone-bucket + sed).
- **Review latency.** Do not let this gate the release; it's explicitly optional.

## Reference

- Skills: `nixpkgs-init-pr`, `nixpkgs-pr-common`.
- Consumer of the merge: [07-rs-modde-validate-and-release.md](./07-rs-modde-validate-and-release.md)
  (chocolatey `nix_tool`).

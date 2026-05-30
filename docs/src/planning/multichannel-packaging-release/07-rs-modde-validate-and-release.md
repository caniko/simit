# Phase 07 — rs-modde end-to-end CI validation, then real release

> **Recommended Codex model: GPT 5.5 max**
>
> Terminal, frontier-risk phase: a live multi-job Codeberg release that fans out
> to ~10 external publish targets (crates? no — Codeberg release, APT repo push,
> AUR SSH push, COPR build, Homebrew/Scoop bucket pushes, Chocolatey push,
> Flathub/winget PRs, announcements), several **irreversible**. Debugging a red
> run means reading CI logs across many steps and distinguishing a clean
> soft-skip from a real failure, then deciding whether to fix-forward or abort.
> Mediocre judgment here either burns a real version number or pushes broken
> packages to public registries. Frontier × orchestrator → `max`, with the full
> pre-mortem below.

## Working tree

`/data/nvme0/can/Projects/rs-modde`. Depends on Phase 06 (rs-modde on the
released simit, workflows regenerated, committed) and Phase 05 (secrets reach
the atlas job). Uses the fork `nix_tool` for chocolatey unless Phase 04 merged.
This is the last phase; on success the plan is exhausted.

## Goal

A throwaway **prerelease** tag drives a green Codeberg release run in which every
configured channel either publishes correctly or soft-skips cleanly (no hard
failures); any issues are fixed; then a **real** release tag is pushed, the
release run is green, and the published artifacts/release are verified.

## Why this matters now

Everything to date is locally verified but **no channel has run on CI**. The
generated `release.yml` re-implements the proven hand-rolled pipeline with
simit's idioms; only a live run proves the bash, the secret wiring (Phase 05),
and the fork `nix_tool` (chocolatey) actually work. Prereleases exist precisely
so this proof doesn't cost a real version.

## Out of scope

- Do **not** validate by pushing a real `X.Y.Z` tag first — always a `-rc.N`
  prerelease first.
- Do **not** "fix" a channel by disabling it silently; a channel that should
  publish must publish, or be explicitly deferred with the user's agreement.
- Do **not** edit generated workflows by hand to patch a CI bug — fix the simit
  generator (bounce to Phase 01-scope) and regenerate, so the fix is durable.

## Plan

1. Pre-flight: confirm Phase 06 acceptance (flake input on the release, both
   `--check`s clean) and Phase 05 acceptance (secret mapping recorded). Ensure
   `CHANGELOG.md` has a section for the prerelease version (the workflow's
   validate-tag step requires it) and the tag will be signed.
2. Cut a prerelease with simit, e.g. `simit release prerelease --pre rc.1`
   (version like `0.0.0-rc.1` or the next real version's `-rc.1`), pushing the
   signed tag to Codeberg. The `release.yml` `[0-9]*` trigger fires.
3. Watch the run (`berg` CLI / Codeberg Actions). Walk every step:
   - **Must publish** on a stable tag only — most downstream channels gate on
     `IS_PRERELEASE=false`, so on an `-rc.N` tag they will **intentionally
     skip** ("Prerelease …; skipping …"). Confirm the _build + sign + Codeberg
     release upload_ steps run (those are not stable-gated) and the downstream
     channel steps skip cleanly.
   - This means a prerelease validates the build/sign/upload spine + the gating
     logic, but NOT the actual downstream pushes. To exercise a downstream
     channel without a real release, temporarily flip that channel's
     `stable_only`/gate via config on a throwaway branch+tag, or accept that
     downstream pushes are first exercised on the real release and watched
     closely.
4. Fix any red step by correcting the **simit generator** + regenerating in
   rs-modde (re-run Phase 06 regen), re-tag a fresh `-rc.N`, re-run. Iterate to
   green.
5. Cut the **real** release: `simit release <patch|minor|major>` per the
   versioning policy, push the signed tag. Watch the full run; confirm each
   stable-gated channel publishes (or soft-skips because its secret is
   intentionally absent — verify which).
6. Post-release verification per channel: Codeberg release has the expected
   assets + `SHA256SUMS.txt(.minisig)` + cosign bundles; APT repo branch updated;
   AUR packages updated (`.SRCINFO` present); COPR build submitted; Homebrew/
   Scoop buckets bumped; Chocolatey package pushed (if fork `nix_tool` +
   credential present); Flathub/winget PRs opened (if tokens present).

## Acceptance criteria

- [ ] A `-rc.N` prerelease run on Codeberg is **green**, with build + sign +
      Codeberg-release-upload steps executed and every downstream channel step
      either skipped-by-gate (logged reason) or succeeded — zero hard failures.
- [ ] Any CI failures were fixed in the **simit generator** and regenerated (no
      hand-edited `release.yml`); `simit init release --check` remains clean.
- [ ] A real release tag run is green; the Codeberg release exists with signed
      checksums + cosign attestations and all expected platform/deb/srpm/AppImage
      assets.
- [ ] Each downstream channel is confirmed published **or** documented as
      intentionally soft-skipped (missing secret), with no channel in a silent-
      failure state.

## Files likely touched

- `/data/nvme0/can/Projects/rs-modde` — tags only (and, if a generator bug is
  found, a bounce to simit + regenerated workflows committed via Phase 06 steps).

## Risk profile

- **Irreversible publishes**: a real release pushes to public registries
  (Chocolatey push, AUR, COPR, Homebrew/Scoop buckets) and opens public PRs
  (Flathub/winget) — hard or impossible to fully retract.
- **Version burn**: a real `X.Y.Z` consumed by a failed run can't be reused;
  prerelease-first mitigates this.
- **Partial publish**: some channels succeed and some fail mid-run, leaving an
  inconsistent public state (e.g. Codeberg release up, AUR not).
- **Secret exposure**: a misconfigured step could echo a token to logs.
- **Cross-channel ordering**: downstream channels download from the Codeberg
  release; if upload partially failed, downloads 404.

## Strategy

Commit ladder / escalation, each a separate validated rung with low revert cost:

1. `-rc.1` prerelease → validates build + sign + Codeberg upload + gate logic.
   Revert: delete the prerelease tag + its Codeberg release (cheap).
2. (Optional) targeted downstream exercise on a throwaway branch with one
   channel's gate flipped → validates that one channel's push mechanics. Revert:
   delete branch/tag; undo the test push in that channel's repo if it landed.
3. Real release → full fan-out. Revert: see Rollback drill; mostly forward-fix.

Never skip rung 1. Only proceed to rung 3 after rung 1 is green and the secret
mapping (Phase 05) is confirmed.

## Rollback drill

Practice before the real tag (SLA: be able to do this in < 5 min):

- Delete a bad tag locally + remote:
  `git tag -d <tag> && git push origin :refs/tags/<tag>`.
- Delete a Codeberg release created by a run: via the Codeberg API
  `DELETE /repos/caniko/rs-modde/releases/{id}` (token), or the UI.
- Cancel an in-flight run: Codeberg Actions UI / `berg` (stop the run) before it
  reaches downstream-publish steps.
- Yank a mistaken crates.io publish (N/A here — rs-modde crates publish via the
  separate `publish-crate-*` workflow; if a wrong crate version ships, `cargo
yank`, do not overwrite).

Rehearse tag-delete + release-delete on the `-rc.1` artifacts first so the moves
are muscle memory before the real release.

## Failure modes and recoveries

- **F1 — validate-tag step fails (CHANGELOG/sig/version).** Symptom: run dies at
  "Validate tag". Cause: missing `## [<tag>]` changelog section, unsigned tag, or
  `nix eval .#modde.version` ≠ tag. Recovery: add the changelog section / sign
  the tag / align the version; re-tag a fresh `-rc.N`.
- **F2 — a downstream channel hard-fails instead of soft-skipping.** Symptom: red
  step with an auth/tool error rather than "skipping". Cause: secret present but
  wrong (Phase 05 mismatch) or tool missing (chocolatey fork `nix_tool`
  unresolvable). Recovery: fix the secret mapping (Phase 05) or the `nix_tool`
  ref; if the bug is in generated bash, fix the simit generator and regenerate.
- **F3 — Codeberg release upload partial (some assets missing/404 downstream).**
  Symptom: Homebrew/Scoop/AUR steps 404 fetching release assets. Cause: upload
  loop failed for some files. Recovery: re-run the release step (it deletes +
  re-uploads idempotently); confirm all assets present before downstream steps.
- **F4 — chocolatey `nix shell <fork-ref>` fetch is slow/fails.** Symptom: long
  hang or eval error on the fork flakeref. Cause: fork branch moved / network.
  Recovery: confirm `caniko/nixpkgs#add-chocolatey-scoop` resolves; if Phase 04
  merged, switch to `nixpkgs#chocolatey`.
- **F5 — partial public publish across channels.** Symptom: some registries
  updated, run later fails. Cause: a mid-run failure after some pushes. Recovery:
  the steps are individually idempotent (git pushes skip if unchanged; Codeberg
  upload re-uploads; COPR/AUR re-push) — fix the failing step and re-run the
  _same_ tag; do not bump the version just to retry.
- **F6 — secret echoed to logs.** Symptom: a token visible in run output. Cause:
  a step printing an env var. Recovery: rotate the exposed secret immediately
  (Phase 05 / Codeberg), fix the generator to stop printing it.

## Reference

- Codeberg CI inspection + run triage: the `berg-codeberg-ci` skill.
- Release mechanics + gating live in the generated `release.yml` (each downstream
  channel's `IS_PRERELEASE` guard).
- Prior phases: [05-canix-expose-runner-secrets.md](./05-canix-expose-runner-secrets.md),
  [06-rs-modde-adopt-simit-release.md](./06-rs-modde-adopt-simit-release.md).
- Originating implementation context: `~/.claude/plans/dreamy-hatching-charm.md`.

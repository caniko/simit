# Phase 01 — Make homebrew & scoop secret names configurable

> **Recommended Codex model: GPT 5.5 medium**
>
> Mechanical Rust threading across two render paths (`render/release_workflow.rs`
> and the older `render/ci.rs`), the `ResolvedHomebrew`/`ResolvedScoop` structs,
> their `resolve_*` constructors, and every struct literal in tests. No design
> frontier, but the ripple across multiple literals plus a second render path is
> exactly where a `low` tier drops a field and produces a non-compiling or
> silently-stale generator. Moderate-complexity leaf work; `medium` holds it.

## Working tree

`/data/nvme0/can/Projects/simit` — same repo as the simit tool. This is the
first simit phase; Phase 02 also touches `simit/src` and `simit/tests`, so land
this before starting 02.

## Goal

Every distribution channel in simit sources its publish secret(s) through a
configurable name. Specifically, `[homebrew].tap_token_secret` (default
`homebrew_tap_token`) and `[scoop].bucket_token_secret` (default
`SCOOP_BUCKET_TOKEN`) replace the hardcoded `${{ secrets.homebrew_tap_token }}`
and `${{ secrets.SCOOP_BUCKET_TOKEN }}` in the generated release workflow, so a
project can point them at whatever secret name its forge uses — uniform with
aur/copr/apt/codeberg/flatpak/winget/chocolatey, which are already configurable.

## Why this matters now

The user asked for uniformity: "make those [homebrew/scoop] the same treatment
so every channel is uniform." Today `render/release_workflow.rs` hardcodes:

```
HOMEBREW_TAP_TOKEN: ${{ secrets.homebrew_tap_token }}
SCOOP_BUCKET_TOKEN: ${{ secrets.SCOOP_BUCKET_TOKEN }}
```

A project whose forge names the secret differently (e.g. a Forgejo runner
credential, as in canix — see Phase 05) cannot retarget these without editing
generated YAML, which `simit init release --check` would then flag as drift.
This is the last configurability gap before simit can be released (Phase 03).

## Out of scope

- Do **not** change the chocolatey api-key wiring — it was already made
  configurable (`api_key_env`/`api_key_secret`/`api_key_from_runner`).
- Do **not** add a "from runner" boolean for homebrew/scoop unless trivially
  symmetric; the homebrew/scoop steps `git clone` with a credential helper, which
  always needs the token as an env value, so a secret-name knob is sufficient.
- Do **not** touch rs-modde or any other repo. Config defaults must keep
  rs-modde's generated `release.yml` byte-identical when the defaults are used.
- Do **not** release simit (Phase 03) or write broad new integration tests
  (Phase 02).

## Plan

1. `config.rs`: add `tap_token_secret: String` (serde default
   `default_homebrew_tap_token_secret` → `"homebrew_tap_token"`) to
   `HomebrewConfig` and a matching field to `ResolvedHomebrew`; add
   `bucket_token_secret: String` (default `"SCOOP_BUCKET_TOKEN"`) to
   `ScoopConfig` and `ResolvedScoop`. Set them in `resolve_homebrew`/
   `resolve_scoop` via `cfg.map_or_else(default_…, |c| c.<field>.clone())`.
2. `render/release_workflow.rs`: in `push_publish_homebrew`, emit
   `HOMEBREW_TAP_TOKEN: ${{ secrets.<tap_token_secret> }}` and reference the same
   token var in the credential helper / gate; in `push_publish_scoop`, do the
   same with `<bucket_token_secret>`. Keep the env var _names_ (`HOMEBREW_TAP_TOKEN`,
   `SCOOP_BUCKET_TOKEN`) as-is — only the `${{ secrets.X }}` name is configurable.
   Update the secrets-header comment lines to use the configured names.
3. `render/ci.rs`: the legacy `push_homebrew_publish_step` /
   `push_scoop_publish_step` also hardcode these. Thread the new fields through
   `HomebrewOptions`/`ScoopOptions` in `ci.rs` and the
   `homebrew_options_from_resolved`/`scoop_options_from_resolved` mappers in
   `commands/init_ci.rs`. If threading into `ci.rs` balloons scope, instead make
   `ci.rs` read the field off the resolved options it already carries — but do
   not leave two divergent defaults.
4. Update every `ResolvedHomebrew`/`ResolvedScoop`/`HomebrewConfig`/`ScoopConfig`
   struct literal (tests in `render/release_workflow.rs`, `tests/homebrew.rs`,
   `tests/scoop.rs`, `src/ci_resolution.rs` if any) with the new field. Add a
   focused assertion in the `release_workflow` full-pipeline test that a
   non-default `tap_token_secret`/`bucket_token_secret` shows up in the output.
5. `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`.
6. Regenerate rs-modde's `release.yml` with the freshly built binary and confirm
   **no diff** when defaults are used:
   `cd /data/nvme0/can/Projects/rs-modde && /data/nvme0/can/Projects/simit/target/debug/simit init release --check` → CLEAN.
7. Commit the entire simit multichannel feature (this phase's change **plus** the
   already-uncommitted session work) in coherent groups using the
   `grouped-git-commits` approach: e.g. (a) config schema, (b) render modules,
   (c) commands + CLI + main + registry, (d) homebrew/scoop secret config, (e)
   tests. Do not `git push`.

## Acceptance criteria

- [ ] `rg 'secrets.homebrew_tap_token|secrets.SCOOP_BUCKET_TOKEN' simit/src/render/release_workflow.rs` returns nothing (both now interpolate the configured name).
- [ ] `HomebrewConfig`/`ScoopConfig` expose `tap_token_secret`/`bucket_token_secret` with the documented defaults; `cargo test` passes including a new assertion proving a custom secret name reaches the generated workflow.
- [ ] `cargo clippy --all-targets -- -D warnings` reports zero warnings.
- [ ] From rs-modde, `simit init release --check` is CLEAN (defaults preserve byte-identical output) — i.e. rs-modde's committed `release.yml` is unchanged by this phase.
- [ ] `git -C /data/nvme0/can/Projects/simit status --porcelain` shows a clean tree after commits (all session work committed in coherent groups, nothing pushed).

## Files likely touched

- `/data/nvme0/can/Projects/simit/src/config.rs`
- `/data/nvme0/can/Projects/simit/src/render/release_workflow.rs`
- `/data/nvme0/can/Projects/simit/src/render/ci.rs`
- `/data/nvme0/can/Projects/simit/src/commands/init_ci.rs`
- `/data/nvme0/can/Projects/simit/tests/{homebrew.rs,scoop.rs}` (struct-literal updates)

## Pitfalls

- **Forgetting the legacy `ci.rs` path.** `HomebrewOptions`/`ScoopOptions` in
  `ci.rs` are a _separate_ struct family from `ResolvedHomebrew`/`ResolvedScoop`.
  Symptom: `release_workflow.rs` is configurable but `init ci --with-homebrew`
  still hardcodes. Recovery: thread the field through the options mappers in
  `init_ci.rs`, or scope `ci.rs` out explicitly and note it (the new release path
  is what rs-modde uses).
- **Default drift breaks rs-modde idempotency.** If the default string differs by
  one character from the current hardcoded value, `simit init release --check`
  against rs-modde fails. Symptom: unexpected diff in step 6. Recovery: defaults
  must be exactly `homebrew_tap_token` and `SCOOP_BUCKET_TOKEN`.
- **rustfmt PostToolUse hook reorders struct fields.** Expect the formatter to
  rewrite literals after each edit; re-read before the next edit rather than
  assuming offsets.

## Reference

- Prior session implementation + the chocolatey-wiring precedent to mirror:
  `~/.claude/plans/dreamy-hatching-charm.md`.
- The chocolatey configurable-wiring commit pattern in
  `simit/src/config.rs` (`ChocolateyConfig.api_key_secret`) is the template.
- Next phase: [02-simit-release-readiness.md](./02-simit-release-readiness.md).

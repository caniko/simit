# Phase 01 — Persist CI options in `simit.toml`

> **Recommended Codex model: GPT 5.4 / medium**
>
> Schema add + read/write + tests on a well-understood config module.
> Moderate complexity, sub-agent role. 5.4 at `medium` matches the
> routing matrix; 5.5 buys nothing here because there is no
> ambiguous design call — the dossier already fixed the schema.

## Working tree

[/data/nvme0/can/Projects/simit](file:///data/nvme0/can/Projects/simit)
— same repo as the parent project. Branch off current `trunk`.

## Goal

`simit init ci` writes the resolved CI option set (runtime, runner,
windows runner, workspace, packages, `with_*` flags, om-ci mode and
ref) into the project's `simit.toml` under `[ci]`, and on subsequent
runs `simit init ci --check --diff` reproduces the same workflows
without re-passing those flags. Pre-existing user config takes
precedence over CLI flags only when no CLI flag is present; explicit
CLI flags continue to override.

## Why this matters now

The research dossier names this as the foundational fix: the report's
per-repo "Validation command set" is hand-written precisely because
`simit init ci --check --diff` requires the user to re-pass every
option. Without persistence, every repo that opts into `--with-audit
--with-deny --with-docs --with-msrv` (or any non-default runtime /
runner) is permanently reported as `ci=drift` on a bare check, and
the chaperone has to reconstruct the flag set from the file content.

Phases 02 and 04 sub-01 read from this config; without it they have
nothing to read.

## Out of scope

- Touching the drift detector in `src/registry.rs` (phase 02 owns
  that unification).
- Changing the on-disk shape of `[homebrew]`, `[chocolatey]`, or
  `[scoop]` sections — they are already config-aware.
- Migrating existing project repos to start using this config
  (phase 07 sub-layers handle per-repo adoption).
- Inferring options from existing workflow files (that is a fallback
  the unification work in phase 02 may consume).

## Plan

1. **Extend `CiConfig`** in
   [src/config.rs:106-121](../../../src/config.rs#L106) with
   serde-default fields covering the option set the CLI accepts:
   - `runtime`: `Option<Runtime>` (default `None`, falls back to
     auto-detect).
   - `runner`: `Option<String>` (validated via
     `user_config::validate_runner_label`).
   - `windows_runner`: `Option<String>`.
   - `workspace`: `bool` (default `false`).
   - `packages`: `Vec<String>` (default empty).
   - `with_nextest`, `with_msrv`, `with_audit`, `with_deny`,
     `with_docs`, `with_artifacts`: each `bool` (default `false`).
   - om-ci fields (`om_ci`, `om_ci_augment`, `omnix_ref`) already
     exist; do not duplicate.

   Keep `#[serde(default)]` on every new field and
   `#[serde(deny_unknown_fields)]` on the struct so legacy
   `simit.toml` files keep loading.

2. **Resolve CLI flags + config in `init_ci.rs`.** Refactor
   [src/commands/init_ci.rs:84-160](../../../src/commands/init_ci.rs#L84)
   so the `CiOptions` builder consumes a single `ResolvedCiInputs`
   that merges CLI flags (highest precedence) over `simit.toml`
   `[ci]` over defaults. A boolean flag like `--with-msrv` with
   default `false` cannot itself signal "absent" — adopt one of:
   - clap `Option<bool>` (preferred): `--with-msrv` / `--no-msrv` /
     unset. Maintain backward compatibility by treating bare
     `--with-msrv` as `Some(true)`.
   - tracked via a separate "was-set" mask if clap version makes
     `Option<bool>` awkward.
     Pick the simpler form for the codebase; document the choice in
     the commit message.

3. **Write the resolved config back** after a successful (non-check)
   `simit init ci` run. Add a `ProjectConfig::write` (or
   `merge_ci_into_simit_toml`) that:
   - creates `simit.toml` if absent;
   - merges the new `[ci]` section in without clobbering unrelated
     sections (`[release]`, `[homebrew]`, etc.);
   - preserves user comments and ordering as best a TOML
     round-tripper allows. Prefer `toml_edit` over `toml` for this.

   Skip the write when `--check` is set or when the resolved config
   exactly equals what is already on disk.

4. **Update the regeneration-command renderer** in
   [src/commands/init_ci.rs:193-300](../../../src/commands/init_ci.rs#L193)
   so the "run this to regenerate" message printed by `--check` is
   the _bare_ `simit init ci --platform <platform>` once
   `simit.toml` carries the options. Until then, keep emitting the
   full flag set so the migration path is obvious.

5. **Tests.** Add coverage under `tests/`:
   - `simit init ci --with-audit --with-deny` writes `[ci]` with
     `with_audit = true`, `with_deny = true`.
   - Bare `simit init ci --check --diff` after the above is clean.
   - CLI `--with-audit` on a project whose `simit.toml` has
     `with_audit = false` wins (CLI overrides config).
   - Existing `[homebrew]` / `[chocolatey]` sections are preserved
     after a write.

6. **Docs.** Update
   [docs/src/getting-started/ci-adoption.md](../../getting-started/ci-adoption.md)
   with a short section showing the persisted `[ci]` shape and
   noting that bare `simit init ci --check --diff` is now the
   canonical drift check.

## Acceptance criteria

- [ ] `[ci]` schema in `src/config.rs` covers runtime, runner,
      windows runner, workspace, packages, and the six `with_*`
      booleans, with `deny_unknown_fields` preserved.
- [ ] `simit init ci --with-audit --with-deny --with-docs --with-msrv`
      writes those settings into `simit.toml` `[ci]`.
- [ ] `simit init ci --platform forgejo --check --diff` (no other
      flags) is clean immediately after the previous step.
- [ ] CLI flags continue to override `simit.toml` when both specify
      a value.
- [ ] Existing `[release]`, `[homebrew]`, `[chocolatey]`, `[scoop]`
      sections survive a `simit init ci` run unchanged.
- [ ] `cargo test` passes the new test cases.
- [ ] `cargo run -- init ci --check` on the simit repo itself remains
      clean.

## Files likely touched

- `src/config.rs`
- `src/commands/init_ci.rs`
- `src/cli.rs` (if switching `--with-*` flags to `Option<bool>`)
- `tests/` (new test file or extension of existing)
- `docs/src/getting-started/ci-adoption.md`
- `Cargo.toml` (add `toml_edit` if not already present)

## Pitfalls

- **Do not break legacy `simit.toml`.** `deny_unknown_fields` plus
  serde defaults is the contract; verify by hand on a checked-in
  repo (e.g. simit itself) before merging.
- **Do not let `--check` mutate `simit.toml`.** `--check` must be
  read-only end-to-end; the write step belongs only to the
  non-check path.
- **Beware boolean flag semantics.** With clap's default `bool`
  type, you cannot tell "unset" from "false". Either move to
  `Option<bool>` or use a paired `--no-*` flag, but document the
  choice clearly in `--help` and the CHANGELOG.
- **Do not silently lose user comments in `simit.toml`.** If
  `toml_edit` is awkward, at minimum warn the user on a write
  rather than overwriting.

## Reference

- Research dossier: improvement (1).
  [`docs/src/planning/release-chaperone-fleet-research.md`](../release-chaperone-fleet-research.md)
- [src/config.rs:106](../../../src/config.rs#L106) — current `CiConfig`.
- [src/commands/init_ci.rs:84](../../../src/commands/init_ci.rs#L84)
  — current option build site.
- [src/commands/init_ci.rs:193](../../../src/commands/init_ci.rs#L193)
  — regeneration-command renderer.
